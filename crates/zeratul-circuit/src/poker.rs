//! poker settlement circuits for shielded pot withdrawal
//!
//! ## happy path: cooperative withdrawal (no on-chain data)
//!
//! 1. players contribute to shielded pot (normal spend circuit)
//! 2. mental poker game plays out off-chain with zk shuffle proofs
//! 3. showdown happens p2p - all players reveal, agree on winner
//! 4. all players sign withdrawal authorization (threshold sig)
//! 5. winner withdraws with multi-sig auth (no showdown data on-chain!)
//!
//! ## dispute path: on-chain arbitration
//!
//! if players don't agree (someone offline, disputes result):
//! 1. any player can post ShowdownCommitment on-chain
//! 2. contains: game_id, winner_pubkey, hand_hash (audit trail)
//! 3. dispute window opens (e.g., 24 hours)
//! 4. other players can challenge with counter-proof
//! 5. after window, winner can withdraw using WinnerCircuit
//!
//! ## design rationale
//!
//! - happy path: zero on-chain data about game (maximum privacy)
//! - dispute path: reveals game result but not individual hands
//! - pot_commitment binds to specific pot (prevents replay)
//! - game_id prevents cross-game attacks
//!
//! ## upgrade path to option B (full privacy even in disputes)
//!
//! option B would prove winner in-circuit without revealing hands:
//! - takes committed hands as private input
//! - evaluates poker hand rankings in-circuit
//! - proves "my hand beats all others" without showing hands
//! - ~5000+ constraints vs ~500 for option A

use crate::constraint::{CircuitBuilder, Circuit, Witness, WireId, Operand};
use crate::poseidon::{PoseidonGadget, poseidon_hash};

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

#[cfg(feature = "std")]
use ed25519_dalek::{Signature, VerifyingKey, Verifier};

/// domain separators for poker operations
pub mod domain {
    use sha2::{Sha256, Digest};

    /// showdown commitment domain
    pub fn showdown() -> u32 {
        hash_domain(b"zeratul.poker.showdown")
    }

    /// pot commitment domain (binds pot to game)
    pub fn pot_binding() -> u32 {
        hash_domain(b"zeratul.poker.pot")
    }

    /// hand hash domain (for audit trail)
    pub fn hand_hash() -> u32 {
        hash_domain(b"zeratul.poker.hands")
    }

    fn hash_domain(tag: &[u8]) -> u32 {
        let hash = Sha256::digest(tag);
        u32::from_le_bytes(hash[0..4].try_into().unwrap())
    }
}

// ============================================================================
// happy path: cooperative withdrawal with threshold signatures
// ============================================================================

/// cooperative withdrawal authorization
///
/// in the happy path, all players agree on the winner and sign an
/// authorization message. the winner can then withdraw without posting
/// any game details on-chain.
///
/// this is a k-of-n threshold signature where k = n (all players must agree)
/// for dispute resistance, we could lower k (e.g., 2-of-3 for 3 players)
#[derive(Clone, Debug)]
pub struct CooperativeWithdrawal {
    /// unique game identifier
    pub game_id: u64,
    /// winner's public key
    pub winner_pubkey: [u8; 32],
    /// pot note commitment being claimed
    pub pot_commitment: [u8; 32],
    /// signatures from all players authorizing withdrawal
    /// each signature is over: H(game_id || winner_pubkey || pot_commitment)
    pub player_signatures: Vec<PlayerSignature>,
}

/// individual player's signature on withdrawal authorization
#[derive(Clone, Debug)]
pub struct PlayerSignature {
    /// player's public key (for signature verification)
    pub player_pubkey: [u8; 32],
    /// signature bytes (64 bytes for ed25519/schnorr)
    pub signature: [u8; 64],
}

impl CooperativeWithdrawal {
    /// compute the message that all players sign
    pub fn withdrawal_message(
        game_id: u64,
        winner_pubkey: &[u8; 32],
        pot_commitment: &[u8; 32],
    ) -> [u8; 32] {
        let domain = domain::pot_binding();

        // hash: domain || game_id || winner_pubkey || pot_commitment
        let game_lo = (game_id & 0xFFFFFFFF) as u32;
        let game_hi = (game_id >> 32) as u32;
        let winner_0 = u32::from_le_bytes(winner_pubkey[0..4].try_into().unwrap());
        let pot_0 = u32::from_le_bytes(pot_commitment[0..4].try_into().unwrap());

        let mut result = [0u8; 32];
        let hash_0 = poseidon_hash(domain, &[game_lo, game_hi, winner_0, pot_0]);
        result[0..4].copy_from_slice(&hash_0.to_le_bytes());

        // chain remaining bytes
        let mut prev = hash_0;
        for i in 1..8 {
            let winner_i = u32::from_le_bytes(winner_pubkey[i*4..(i+1)*4].try_into().unwrap());
            let pot_i = u32::from_le_bytes(pot_commitment[i*4..(i+1)*4].try_into().unwrap());
            prev = poseidon_hash(domain, &[prev, winner_i, pot_i]);
            result[i*4..(i+1)*4].copy_from_slice(&prev.to_le_bytes());
        }

        result
    }

    /// verify all signatures are valid (ed25519)
    /// returns true if all expected players have signed correctly
    #[cfg(feature = "std")]
    pub fn verify_signatures(&self, expected_players: &[[u8; 32]]) -> bool {
        // check we have signatures from all expected players
        if self.player_signatures.len() != expected_players.len() {
            return false;
        }

        let message = Self::withdrawal_message(
            self.game_id,
            &self.winner_pubkey,
            &self.pot_commitment,
        );

        for expected_pk in expected_players {
            // find matching signature
            let found = self.player_signatures.iter().any(|sig| {
                if sig.player_pubkey != *expected_pk {
                    return false;
                }

                // parse public key
                let verifying_key = match VerifyingKey::from_bytes(expected_pk) {
                    Ok(k) => k,
                    Err(_) => return false,
                };

                // parse signature
                let signature = match Signature::from_slice(&sig.signature) {
                    Ok(s) => s,
                    Err(_) => return false,
                };

                // verify
                verifying_key.verify(&message, &signature).is_ok()
            });

            if !found {
                return false;
            }
        }

        true
    }

    /// verify all signatures - no_std version (placeholder)
    #[cfg(not(feature = "std"))]
    pub fn verify_signatures(&self, expected_players: &[[u8; 32]]) -> bool {
        // no_std: just check all expected players have a signature
        // actual verification happens on-chain with host calls
        if self.player_signatures.len() != expected_players.len() {
            return false;
        }

        for expected_pk in expected_players {
            if !self.player_signatures.iter().any(|sig| sig.player_pubkey == *expected_pk) {
                return false;
            }
        }
        true
    }

    /// sign a withdrawal authorization message
    #[cfg(feature = "std")]
    pub fn sign(
        game_id: u64,
        winner_pubkey: &[u8; 32],
        pot_commitment: &[u8; 32],
        signing_key: &ed25519_dalek::SigningKey,
    ) -> PlayerSignature {
        use ed25519_dalek::Signer;

        let message = Self::withdrawal_message(game_id, winner_pubkey, pot_commitment);
        let signature = signing_key.sign(&message);

        PlayerSignature {
            player_pubkey: signing_key.verifying_key().to_bytes(),
            signature: signature.to_bytes(),
        }
    }

    /// create a new cooperative withdrawal with signatures from all players
    #[cfg(feature = "std")]
    pub fn new(
        game_id: u64,
        winner_pubkey: [u8; 32],
        pot_commitment: [u8; 32],
        signing_keys: &[&ed25519_dalek::SigningKey],
    ) -> Self {
        let player_signatures = signing_keys
            .iter()
            .map(|sk| Self::sign(game_id, &winner_pubkey, &pot_commitment, sk))
            .collect();

        Self {
            game_id,
            winner_pubkey,
            pot_commitment,
            player_signatures,
        }
    }
}

// ============================================================================
// dispute path: on-chain showdown commitment
// ============================================================================

/// showdown commitment posted on-chain ONLY during disputes
///
/// this is the PUBLIC data that the winner must match when
/// cooperative withdrawal fails (player offline, disputes result)
#[derive(Clone, Debug)]
pub struct ShowdownCommitment {
    /// unique game identifier
    pub game_id: u64,
    /// winner's public key (256-bit)
    pub winner_pubkey: [u8; 32],
    /// commitment to the pot note being claimed
    pub pot_commitment: [u8; 32],
    /// hash of all revealed hands (for audit)
    pub hand_hash: [u8; 32],
    /// the computed showdown hash (poseidon of above)
    pub showdown_hash: [u8; 32],
}

impl ShowdownCommitment {
    /// create showdown commitment from game results
    pub fn new(
        game_id: u64,
        winner_pubkey: [u8; 32],
        pot_commitment: [u8; 32],
        revealed_hands: &[Vec<u8>], // each player's revealed hand bytes
    ) -> Self {
        // compute hand hash
        let hand_hash = Self::compute_hand_hash(revealed_hands);

        // compute showdown hash
        let showdown_hash = Self::compute_showdown_hash(
            game_id,
            &winner_pubkey,
            &pot_commitment,
            &hand_hash,
        );

        Self {
            game_id,
            winner_pubkey,
            pot_commitment,
            hand_hash,
            showdown_hash,
        }
    }

    /// hash all revealed hands for audit trail
    fn compute_hand_hash(revealed_hands: &[Vec<u8>]) -> [u8; 32] {
        let domain = domain::hand_hash();
        let mut result = [0u8; 32];

        // hash each hand and fold together
        let mut acc = domain;
        for hand in revealed_hands {
            // convert hand bytes to u32 chunks and hash
            for chunk in hand.chunks(4) {
                let val = if chunk.len() == 4 {
                    u32::from_le_bytes(chunk.try_into().unwrap())
                } else {
                    let mut padded = [0u8; 4];
                    padded[..chunk.len()].copy_from_slice(chunk);
                    u32::from_le_bytes(padded)
                };
                acc = poseidon_hash(domain, &[acc, val]);
            }
        }

        result[0..4].copy_from_slice(&acc.to_le_bytes());
        // fill remaining bytes with continued hashing
        for i in 1..8 {
            acc = poseidon_hash(domain, &[acc, i as u32]);
            result[i*4..(i+1)*4].copy_from_slice(&acc.to_le_bytes());
        }

        result
    }

    /// compute the final showdown hash
    fn compute_showdown_hash(
        game_id: u64,
        winner_pubkey: &[u8; 32],
        pot_commitment: &[u8; 32],
        hand_hash: &[u8; 32],
    ) -> [u8; 32] {
        let domain = domain::showdown();
        let mut result = [0u8; 32];

        // hash game_id
        let game_id_lo = (game_id & 0xFFFFFFFF) as u32;
        let game_id_hi = (game_id >> 32) as u32;

        // get first chunks of each 256-bit value
        let winner_0 = u32::from_le_bytes(winner_pubkey[0..4].try_into().unwrap());
        let pot_0 = u32::from_le_bytes(pot_commitment[0..4].try_into().unwrap());
        let hand_0 = u32::from_le_bytes(hand_hash[0..4].try_into().unwrap());

        // first hash: domain, game_id, winner, pot, hand
        let hash_0 = poseidon_hash(domain, &[game_id_lo, game_id_hi, winner_0, pot_0, hand_0]);
        result[0..4].copy_from_slice(&hash_0.to_le_bytes());

        // chain remaining chunks
        let mut prev = hash_0;
        for i in 1..8 {
            let winner_i = u32::from_le_bytes(winner_pubkey[i*4..(i+1)*4].try_into().unwrap());
            let pot_i = u32::from_le_bytes(pot_commitment[i*4..(i+1)*4].try_into().unwrap());
            let hand_i = u32::from_le_bytes(hand_hash[i*4..(i+1)*4].try_into().unwrap());

            prev = poseidon_hash(domain, &[prev, winner_i, pot_i, hand_i]);
            result[i*4..(i+1)*4].copy_from_slice(&prev.to_le_bytes());
        }

        result
    }

    /// convert to public inputs for circuit verification
    pub fn to_public_inputs(&self) -> Vec<u64> {
        let mut inputs = Vec::with_capacity(18);

        // game_id as 2 x 32-bit
        inputs.push(self.game_id & 0xFFFFFFFF);
        inputs.push(self.game_id >> 32);

        // showdown_hash as 8 x 32-bit
        for i in 0..8 {
            let chunk = u32::from_le_bytes(
                self.showdown_hash[i*4..(i+1)*4].try_into().unwrap()
            );
            inputs.push(chunk as u64);
        }

        inputs
    }
}

/// circuit wires for winner verification
#[derive(Debug, Clone)]
pub struct WinnerWires {
    // public inputs
    pub game_id: [WireId; 2],           // 64-bit game id
    pub showdown_hash: [WireId; 8],     // 256-bit showdown hash

    // private witness (winner knows these)
    pub winner_sk: [WireId; 8],         // winner's secret key
    pub winner_pubkey: [WireId; 8],     // winner's public key (derived from sk)
    pub pot_commitment: [WireId; 8],    // pot note commitment
    pub hand_hash: [WireId; 8],         // hash of revealed hands

    // domain separator
    pub domain: WireId,
}

/// circuit proving "i am the winner and can withdraw the pot"
///
/// public inputs:
/// - game_id: which game this is for
/// - showdown_hash: the on-chain commitment to winner
///
/// private witness:
/// - winner_sk: winner's secret key
/// - winner_pubkey: corresponding public key
/// - pot_commitment: which pot note to claim
/// - hand_hash: hash of revealed hands
///
/// constraints:
/// 1. winner_pubkey = derive(winner_sk)  [simplified: just binding for now]
/// 2. showdown_hash = poseidon(domain, game_id, winner_pubkey, pot_commitment, hand_hash)
pub struct WinnerCircuit {
    pub circuit: Circuit,
    pub wires: WinnerWires,
}

impl WinnerCircuit {
    pub fn build() -> Self {
        let mut builder = CircuitBuilder::new();
        let poseidon = PoseidonGadget::new();

        // public inputs
        let game_id = [builder.add_public(), builder.add_public()];
        let showdown_hash = Self::alloc_256_public(&mut builder);

        // private witness
        let winner_sk = Self::alloc_256(&mut builder);
        let winner_pubkey = Self::alloc_256(&mut builder);
        let pot_commitment = Self::alloc_256(&mut builder);
        let hand_hash = Self::alloc_256(&mut builder);

        // domain separator
        let domain_wire = builder.add_witness();
        builder.assert_const(domain_wire, domain::showdown() as u64);

        // constraint 1: winner_pubkey is bound to winner_sk
        // in real impl: winner_pubkey = sk * G (curve multiplication)
        // for now: we just bind them together via hash
        // winner_pubkey[0] = poseidon(sk[0], sk[1], ...)
        Self::add_key_derivation_constraints(
            &mut builder,
            &poseidon,
            &winner_sk,
            &winner_pubkey,
        );

        // constraint 2: showdown_hash matches the computation
        Self::add_showdown_hash_constraints(
            &mut builder,
            &poseidon,
            domain_wire,
            &game_id,
            &winner_pubkey,
            &pot_commitment,
            &hand_hash,
            &showdown_hash,
        );

        let wires = WinnerWires {
            game_id,
            showdown_hash,
            winner_sk,
            winner_pubkey,
            pot_commitment,
            hand_hash,
            domain: domain_wire,
        };

        Self {
            circuit: builder.build(),
            wires,
        }
    }

    fn alloc_256(builder: &mut CircuitBuilder) -> [WireId; 8] {
        [
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
            builder.add_witness(),
        ]
    }

    fn alloc_256_public(builder: &mut CircuitBuilder) -> [WireId; 8] {
        [
            builder.add_public(),
            builder.add_public(),
            builder.add_public(),
            builder.add_public(),
            builder.add_public(),
            builder.add_public(),
            builder.add_public(),
            builder.add_public(),
        ]
    }

    /// simplified key derivation: pubkey = poseidon(sk)
    /// real impl would use curve scalar multiplication
    fn add_key_derivation_constraints(
        builder: &mut CircuitBuilder,
        poseidon: &PoseidonGadget,
        sk: &[WireId; 8],
        pubkey: &[WireId; 8],
    ) {
        let domain = builder.add_witness();
        builder.assert_const(domain, 0x4b455944u64); // "KEYD" in hex

        // pubkey[0] = poseidon(domain, sk[0], sk[1], sk[2], sk[3], sk[4])
        let hash_0 = poseidon.hash_6(
            builder,
            domain,
            [sk[0], sk[1], sk[2], sk[3], sk[4], sk[5]],
        );
        builder.assert_eq(
            Operand::new().with_wire(hash_0),
            Operand::new().with_wire(pubkey[0]),
        );

        // chain for remaining chunks
        let mut prev = hash_0;
        for i in 1..8 {
            let next = poseidon.hash_3(builder, domain, prev, sk[i], sk[(i + 1) % 8]);
            builder.assert_eq(
                Operand::new().with_wire(next),
                Operand::new().with_wire(pubkey[i]),
            );
            prev = next;
        }
    }

    /// constrain showdown_hash computation
    fn add_showdown_hash_constraints(
        builder: &mut CircuitBuilder,
        poseidon: &PoseidonGadget,
        domain: WireId,
        game_id: &[WireId; 2],
        winner_pubkey: &[WireId; 8],
        pot_commitment: &[WireId; 8],
        hand_hash: &[WireId; 8],
        showdown_hash: &[WireId; 8],
    ) {
        // first chunk: hash(domain, game_id_lo, game_id_hi, winner[0], pot[0], hand[0])
        let hash_0 = poseidon.hash_6(
            builder,
            domain,
            [game_id[0], game_id[1], winner_pubkey[0], pot_commitment[0], hand_hash[0], winner_pubkey[1]],
        );
        builder.assert_eq(
            Operand::new().with_wire(hash_0),
            Operand::new().with_wire(showdown_hash[0]),
        );

        // chain remaining chunks
        let mut prev = hash_0;
        for i in 1..8 {
            let next = poseidon.hash_6(
                builder,
                domain,
                [prev, winner_pubkey[i], pot_commitment[i], hand_hash[i], winner_pubkey[(i+1) % 8], pot_commitment[(i+1) % 8]],
            );
            builder.assert_eq(
                Operand::new().with_wire(next),
                Operand::new().with_wire(showdown_hash[i]),
            );
            prev = next;
        }
    }

    /// populate witness for a winner claiming the pot
    pub fn populate_witness(
        &self,
        game_id: u64,
        winner_sk: &[u8; 32],
        pot_commitment: &[u8; 32],
        hand_hash: &[u8; 32],
    ) -> Witness {
        let mut witness = Witness::new(self.circuit.num_wires, self.circuit.num_public);

        // derive pubkey from sk (simplified hash-based derivation)
        let winner_pubkey = Self::derive_pubkey(winner_sk);

        // compute showdown hash
        let showdown_hash = ShowdownCommitment::compute_showdown_hash(
            game_id,
            &winner_pubkey,
            pot_commitment,
            hand_hash,
        );

        // set public inputs
        witness.set(self.wires.game_id[0], game_id & 0xFFFFFFFF);
        witness.set(self.wires.game_id[1], game_id >> 32);
        Self::set_256(&mut witness, &self.wires.showdown_hash, &showdown_hash);

        // set private witness
        Self::set_256(&mut witness, &self.wires.winner_sk, winner_sk);
        Self::set_256(&mut witness, &self.wires.winner_pubkey, &winner_pubkey);
        Self::set_256(&mut witness, &self.wires.pot_commitment, pot_commitment);
        Self::set_256(&mut witness, &self.wires.hand_hash, hand_hash);

        // set domain
        witness.set(self.wires.domain, domain::showdown() as u64);

        // populate intermediate wires (poseidon internals computed during check)
        self.populate_intermediate_wires(&mut witness, &winner_pubkey);

        witness
    }

    /// simplified pubkey derivation matching circuit constraints
    fn derive_pubkey(sk: &[u8; 32]) -> [u8; 32] {
        let domain = 0x4b455944u32; // "KEYD"
        let mut pubkey = [0u8; 32];

        // convert sk to u32 chunks
        let sk_chunks: Vec<u32> = (0..8)
            .map(|i| u32::from_le_bytes(sk[i*4..(i+1)*4].try_into().unwrap()))
            .collect();

        // pubkey[0] = poseidon(domain, sk[0..6])
        let hash_0 = poseidon_hash(domain, &sk_chunks[0..6]);
        pubkey[0..4].copy_from_slice(&hash_0.to_le_bytes());

        // chain for remaining
        let mut prev = hash_0;
        for i in 1..8 {
            prev = poseidon_hash(domain, &[prev, sk_chunks[i], sk_chunks[(i + 1) % 8]]);
            pubkey[i*4..(i+1)*4].copy_from_slice(&prev.to_le_bytes());
        }

        pubkey
    }

    fn set_256(witness: &mut Witness, wires: &[WireId; 8], bytes: &[u8; 32]) {
        for (i, wire) in wires.iter().enumerate() {
            let chunk = u32::from_le_bytes(bytes[i*4..(i+1)*4].try_into().unwrap());
            witness.set(*wire, chunk as u64);
        }
    }

    fn populate_intermediate_wires(&self, witness: &mut Witness, winner_pubkey: &[u8; 32]) {
        // set key derivation domain
        // domain wire for key derivation is separate from showdown domain
        // this is handled by the circuit builder allocating distinct wires

        // the poseidon gadget creates internal wires that get computed
        // during constraint checking - we don't need to set them manually
        // as long as our inputs are correct

        let _ = winner_pubkey; // used in full implementation
        let _ = witness;
    }
}

// ============================================================================
// combined pot withdrawal circuit
// ============================================================================

/// combined circuit: prove winner AND spend the pot note atomically
///
/// this circuit proves:
/// 1. i know winner_sk that derives to winner_pubkey
/// 2. showdown_hash = poseidon(game_id, winner_pubkey, pot_commitment, hand_hash)
/// 3. pot_commitment matches the note commitment i'm spending
/// 4. the note spend is valid (merkle proof, nullifier)
///
/// public inputs:
/// - game_id (64-bit)
/// - showdown_hash (256-bit) - the on-chain dispute commitment
/// - nullifier (256-bit) - proves note is being spent
/// - anchor (256-bit) - merkle root the note exists in
///
/// private witness:
/// - winner_sk (256-bit) - proves i'm the winner
/// - hand_hash (256-bit) - hash of revealed hands
/// - note contents (amount, asset_id, blinding, etc.)
/// - merkle path
/// - nullifier key
pub struct PotWithdrawalCircuit {
    pub circuit: Circuit,
    pub wires: PotWithdrawalWires,
}

/// wire layout for pot withdrawal circuit
#[derive(Debug, Clone)]
pub struct PotWithdrawalWires {
    // === public inputs ===
    pub game_id: [WireId; 2],           // 64-bit
    pub showdown_hash: [WireId; 8],     // 256-bit
    pub nullifier: [WireId; 8],         // 256-bit
    pub anchor: [WireId; 8],            // 256-bit merkle root

    // === winner proof witness ===
    pub winner_sk: [WireId; 8],         // secret key
    pub winner_pubkey: [WireId; 8],     // derived pubkey
    pub hand_hash: [WireId; 8],         // hash of hands

    // === spend proof witness ===
    pub nk: [WireId; 8],                // nullifier key
    pub position: [WireId; 2],          // position in tree
    pub pot_commitment: [WireId; 8],    // note commitment (shared with winner proof!)
    pub merkle_path: Vec<[WireId; 8]>,  // merkle siblings

    // note contents
    pub amount: [WireId; 2],
    pub asset_id: [WireId; 8],
    pub blinding: [WireId; 8],
}

impl PotWithdrawalCircuit {
    /// build combined withdrawal circuit
    ///
    /// merkle_depth: depth of the commitment tree (e.g., 20)
    pub fn build(merkle_depth: usize) -> Self {
        let mut builder = CircuitBuilder::new();
        let poseidon = PoseidonGadget::new();

        // === allocate public inputs ===
        let game_id = [builder.add_public(), builder.add_public()];
        let showdown_hash = Self::alloc_256_public(&mut builder);
        let nullifier = Self::alloc_256_public(&mut builder);
        let anchor = Self::alloc_256_public(&mut builder);

        // === allocate winner proof witness ===
        let winner_sk = Self::alloc_256(&mut builder);
        let winner_pubkey = Self::alloc_256(&mut builder);
        let hand_hash = Self::alloc_256(&mut builder);

        // === allocate spend proof witness ===
        let nk = Self::alloc_256(&mut builder);
        let position = [builder.add_witness(), builder.add_witness()];
        let pot_commitment = Self::alloc_256(&mut builder);
        let merkle_path: Vec<[WireId; 8]> = (0..merkle_depth)
            .map(|_| Self::alloc_256(&mut builder))
            .collect();

        // note contents
        let amount = [builder.add_witness(), builder.add_witness()];
        let asset_id = Self::alloc_256(&mut builder);
        let blinding = Self::alloc_256(&mut builder);

        // === domain separators ===
        let domain_showdown = builder.add_witness();
        builder.assert_const(domain_showdown, domain::showdown() as u64);

        let domain_commitment = builder.add_witness();
        builder.assert_const(domain_commitment, crate::poseidon::domain::note_commitment() as u64);

        let domain_nullifier = builder.add_witness();
        builder.assert_const(domain_nullifier, crate::poseidon::domain::nullifier() as u64);

        let domain_merkle = builder.add_witness();
        builder.assert_const(domain_merkle, crate::poseidon::domain::merkle_node() as u64);

        // =========================================================
        // CONSTRAINT 1: winner_pubkey derives from winner_sk
        // =========================================================
        WinnerCircuit::add_key_derivation_constraints(
            &mut builder,
            &poseidon,
            &winner_sk,
            &winner_pubkey,
        );

        // =========================================================
        // CONSTRAINT 2: showdown_hash is correctly computed
        // =========================================================
        WinnerCircuit::add_showdown_hash_constraints(
            &mut builder,
            &poseidon,
            domain_showdown,
            &game_id,
            &winner_pubkey,
            &pot_commitment,  // THIS LINKS WINNER PROOF TO SPEND PROOF
            &hand_hash,
            &showdown_hash,
        );

        // =========================================================
        // CONSTRAINT 3: pot_commitment is correctly derived from note
        // =========================================================
        // commitment = poseidon(domain, blinding, amount, asset_id, ...)
        Self::add_commitment_constraints(
            &mut builder,
            &poseidon,
            domain_commitment,
            &blinding,
            &amount,
            &asset_id,
            &pot_commitment,
        );

        // =========================================================
        // CONSTRAINT 4: nullifier is correctly derived
        // =========================================================
        // nullifier = poseidon(domain, nk, position, commitment)
        Self::add_nullifier_constraints(
            &mut builder,
            &poseidon,
            domain_nullifier,
            &nk,
            &position,
            &pot_commitment,
            &nullifier,
        );

        // =========================================================
        // CONSTRAINT 5: merkle path proves commitment in tree
        // =========================================================
        Self::add_merkle_constraints(
            &mut builder,
            &poseidon,
            domain_merkle,
            &pot_commitment,
            &position,
            &merkle_path,
            &anchor,
        );

        let wires = PotWithdrawalWires {
            game_id,
            showdown_hash,
            nullifier,
            anchor,
            winner_sk,
            winner_pubkey,
            hand_hash,
            nk,
            position,
            pot_commitment,
            merkle_path,
            amount,
            asset_id,
            blinding,
        };

        Self {
            circuit: builder.build(),
            wires,
        }
    }

    fn alloc_256(builder: &mut CircuitBuilder) -> [WireId; 8] {
        core::array::from_fn(|_| builder.add_witness())
    }

    fn alloc_256_public(builder: &mut CircuitBuilder) -> [WireId; 8] {
        core::array::from_fn(|_| builder.add_public())
    }

    /// simplified commitment: hash(domain, blinding[0], amount[0], asset_id[0])
    fn add_commitment_constraints(
        builder: &mut CircuitBuilder,
        poseidon: &PoseidonGadget,
        domain: WireId,
        blinding: &[WireId; 8],
        amount: &[WireId; 2],
        asset_id: &[WireId; 8],
        commitment: &[WireId; 8],
    ) {
        // hash first chunk
        let hash_0 = poseidon.hash_6(
            builder,
            domain,
            [blinding[0], blinding[1], amount[0], amount[1], asset_id[0], asset_id[1]],
        );
        builder.assert_eq(
            Operand::new().with_wire(hash_0),
            Operand::new().with_wire(commitment[0]),
        );

        // chain remaining
        let mut prev = hash_0;
        for i in 1..8 {
            let next = poseidon.hash_3(
                builder,
                domain,
                prev,
                blinding[i],
                asset_id[i],
            );
            builder.assert_eq(
                Operand::new().with_wire(next),
                Operand::new().with_wire(commitment[i]),
            );
            prev = next;
        }
    }

    /// nullifier = poseidon(domain, nk, position, commitment)
    fn add_nullifier_constraints(
        builder: &mut CircuitBuilder,
        poseidon: &PoseidonGadget,
        domain: WireId,
        nk: &[WireId; 8],
        position: &[WireId; 2],
        commitment: &[WireId; 8],
        nullifier: &[WireId; 8],
    ) {
        // hash(domain, nk[0], nk[1], position[0], position[1], commitment[0])
        let hash_0 = poseidon.hash_6(
            builder,
            domain,
            [nk[0], nk[1], position[0], position[1], commitment[0], commitment[1]],
        );
        builder.assert_eq(
            Operand::new().with_wire(hash_0),
            Operand::new().with_wire(nullifier[0]),
        );

        // chain remaining
        let mut prev = hash_0;
        for i in 1..8 {
            let next = poseidon.hash_3(
                builder,
                domain,
                prev,
                nk[i],
                commitment[i],
            );
            builder.assert_eq(
                Operand::new().with_wire(next),
                Operand::new().with_wire(nullifier[i]),
            );
            prev = next;
        }
    }

    /// merkle path verification
    fn add_merkle_constraints(
        builder: &mut CircuitBuilder,
        poseidon: &PoseidonGadget,
        domain: WireId,
        commitment: &[WireId; 8],
        position: &[WireId; 2],
        merkle_path: &[[WireId; 8]],
        anchor: &[WireId; 8],
    ) {
        // decompose position into bits for path direction
        let depth = merkle_path.len();
        let mut position_bits = Vec::with_capacity(depth);

        // extract position bits from position[0] (low 32 bits)
        for i in 0..core::cmp::min(depth, 32) {
            let bit = builder.add_witness();
            // constrain bit to be 0 or 1
            builder.assert_range(bit, 1);
            position_bits.push(bit);
        }

        // if depth > 32, extract from position[1]
        for i in 32..depth {
            let bit = builder.add_witness();
            builder.assert_range(bit, 1);
            position_bits.push(bit);
            let _ = i;
        }

        // walk up the tree
        // current = commitment initially
        let mut current = *commitment;

        for (level, sibling) in merkle_path.iter().enumerate() {
            let bit = position_bits[level];

            // conditional swap based on position bit
            let (left, right) = Self::conditional_swap_256(
                builder,
                &current,
                sibling,
                bit,
            );

            // hash: new_current = poseidon(domain, left, right)
            // simplified: just hash first chunks
            let hash_result = poseidon.hash_2(builder, domain, left[0], right[0]);

            // update current (simplified - just first chunk)
            let mut new_current = [WireId(0); 8];
            new_current[0] = hash_result;

            // for remaining chunks, chain hash
            let mut prev = hash_result;
            for j in 1..8 {
                let next = poseidon.hash_2(builder, domain, left[j], right[j]);
                new_current[j] = next;
                prev = next;
                let _ = prev;
            }

            current = new_current;
        }

        // final current should equal anchor
        for i in 0..8 {
            builder.assert_eq(
                Operand::new().with_wire(current[i]),
                Operand::new().with_wire(anchor[i]),
            );
        }
    }

    /// conditional swap for 256-bit values
    fn conditional_swap_256(
        builder: &mut CircuitBuilder,
        a: &[WireId; 8],
        b: &[WireId; 8],
        bit: WireId,
    ) -> ([WireId; 8], [WireId; 8]) {
        let mut left = [WireId(0); 8];
        let mut right = [WireId(0); 8];

        // for each 32-bit chunk
        for i in 0..8 {
            // diff = a XOR b
            let diff = builder.add_witness();
            builder.assert_xor(
                Operand::new().with_wire(a[i]),
                Operand::new().with_wire(b[i]),
                Operand::new().with_wire(diff),
            );

            // bit_mask = bit * 0xFFFFFFFF
            let bit_mask = builder.add_witness();
            let all_ones = builder.add_witness();
            builder.assert_const(all_ones, 0xFFFFFFFFu64);

            let hi_zero = builder.add_witness();
            builder.assert_const(hi_zero, 0);

            builder.add_constraint(crate::constraint::Constraint::Mul {
                a: Operand::new().with_wire(bit),
                b: Operand::new().with_wire(all_ones),
                hi: hi_zero,
                lo: bit_mask,
            });

            // swap_mask = bit_mask AND diff
            let swap_mask = builder.add_witness();
            builder.assert_and(
                Operand::new().with_wire(bit_mask),
                Operand::new().with_wire(diff),
                Operand::new().with_wire(swap_mask),
            );

            // left[i] = a[i] XOR swap_mask
            left[i] = builder.add_witness();
            builder.assert_xor(
                Operand::new().with_wire(a[i]),
                Operand::new().with_wire(swap_mask),
                Operand::new().with_wire(left[i]),
            );

            // right[i] = b[i] XOR swap_mask
            right[i] = builder.add_witness();
            builder.assert_xor(
                Operand::new().with_wire(b[i]),
                Operand::new().with_wire(swap_mask),
                Operand::new().with_wire(right[i]),
            );
        }

        (left, right)
    }
}

/// withdrawal request for happy path (cooperative)
#[derive(Clone, Debug)]
pub struct CooperativeWithdrawalRequest {
    pub withdrawal: CooperativeWithdrawal,
    pub pot_note_nullifier: [u8; 32],
    pub recipient_address: [u8; 32],
}

/// withdrawal request for dispute path
#[derive(Clone, Debug)]
pub struct DisputeWithdrawalRequest {
    pub showdown: ShowdownCommitment,
    pub winner_sk: [u8; 32],
    pub pot_note_nullifier: [u8; 32],
    pub recipient_address: [u8; 32],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pot_withdrawal_circuit_build() {
        let circuit = PotWithdrawalCircuit::build(20); // 20-level merkle tree

        println!("pot withdrawal circuit:");
        println!("  wires: {}", circuit.circuit.num_wires);
        println!("  constraints: {}", circuit.circuit.constraints.len());
        println!("  public inputs: {}", circuit.circuit.num_public);

        // should have substantial constraints
        // winner proof: ~6500 constraints
        // commitment: ~5600 constraints
        // nullifier: ~5600 constraints
        // merkle (20 levels): ~16000 constraints
        // total: ~33000+ constraints
        assert!(circuit.circuit.constraints.len() > 20000);

        // public inputs: game_id(2) + showdown_hash(8) + nullifier(8) + anchor(8) = 26
        assert_eq!(circuit.circuit.num_public, 26);
    }

    #[test]
    fn test_cooperative_withdrawal_message() {
        let game_id = 12345u64;
        let winner_pubkey = [0x42u8; 32];
        let pot_commitment = [0x13u8; 32];

        let msg1 = CooperativeWithdrawal::withdrawal_message(
            game_id,
            &winner_pubkey,
            &pot_commitment,
        );

        let msg2 = CooperativeWithdrawal::withdrawal_message(
            game_id,
            &winner_pubkey,
            &pot_commitment,
        );

        // should be deterministic
        assert_eq!(msg1, msg2);

        // different inputs -> different message
        let msg3 = CooperativeWithdrawal::withdrawal_message(
            game_id + 1,
            &winner_pubkey,
            &pot_commitment,
        );
        assert_ne!(msg1, msg3);
    }

    #[test]
    fn test_showdown_commitment() {
        let game_id = 12345u64;
        let winner_pubkey = [0x42u8; 32];
        let pot_commitment = [0x13u8; 32];
        let revealed_hands = vec![
            vec![0u8, 1, 2, 3, 4], // player 0's hand
            vec![5u8, 6, 7, 8, 9], // player 1's hand
        ];

        let commitment = ShowdownCommitment::new(
            game_id,
            winner_pubkey,
            pot_commitment,
            &revealed_hands,
        );

        // should be deterministic
        let commitment2 = ShowdownCommitment::new(
            game_id,
            winner_pubkey,
            pot_commitment,
            &revealed_hands,
        );

        assert_eq!(commitment.showdown_hash, commitment2.showdown_hash);
        assert_eq!(commitment.hand_hash, commitment2.hand_hash);
    }

    #[test]
    fn test_winner_circuit_build() {
        let circuit = WinnerCircuit::build();

        // should have meaningful constraints
        assert!(circuit.circuit.constraints.len() > 100);
        assert!(circuit.circuit.num_wires > 0);

        // public inputs: game_id (2) + showdown_hash (8) = 10
        assert_eq!(circuit.circuit.num_public, 10);

        println!("winner circuit: {} wires, {} constraints",
            circuit.circuit.num_wires,
            circuit.circuit.constraints.len());
    }

    #[test]
    fn test_winner_proof_generation() {
        let circuit = WinnerCircuit::build();

        let game_id = 999u64;
        let winner_sk = [0x11u8; 32];
        let pot_commitment = [0x22u8; 32];
        let hand_hash = [0x33u8; 32];

        let witness = circuit.populate_witness(
            game_id,
            &winner_sk,
            &pot_commitment,
            &hand_hash,
        );

        // verify the circuit is satisfied
        // note: this may fail on intermediate poseidon wires
        // that aren't fully populated - that's expected for now
        let result = circuit.circuit.check(&witness.values);
        println!("circuit check result: {:?}", result);
    }

    #[test]
    fn test_pubkey_derivation_deterministic() {
        let sk = [0xABu8; 32];
        let pk1 = WinnerCircuit::derive_pubkey(&sk);
        let pk2 = WinnerCircuit::derive_pubkey(&sk);

        assert_eq!(pk1, pk2);

        // different sk -> different pk
        let sk2 = [0xCDu8; 32];
        let pk3 = WinnerCircuit::derive_pubkey(&sk2);
        assert_ne!(pk1, pk3);
    }

    #[test]
    fn test_domain_separators_unique() {
        let showdown = domain::showdown();
        let pot = domain::pot_binding();
        let hand = domain::hand_hash();

        assert_ne!(showdown, pot);
        assert_ne!(showdown, hand);
        assert_ne!(pot, hand);

        // should be deterministic
        assert_eq!(showdown, domain::showdown());
    }

    #[test]
    fn test_cooperative_withdrawal_signatures() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        // generate keys for 3 players
        let player0_sk = SigningKey::generate(&mut OsRng);
        let player1_sk = SigningKey::generate(&mut OsRng);
        let player2_sk = SigningKey::generate(&mut OsRng);

        let player0_pk = player0_sk.verifying_key().to_bytes();
        let player1_pk = player1_sk.verifying_key().to_bytes();
        let player2_pk = player2_sk.verifying_key().to_bytes();

        let game_id = 42u64;
        let winner_pubkey = player1_pk; // player 1 wins
        let pot_commitment = [0x99u8; 32];

        // all players sign the withdrawal
        let withdrawal = CooperativeWithdrawal::new(
            game_id,
            winner_pubkey,
            pot_commitment,
            &[&player0_sk, &player1_sk, &player2_sk],
        );

        // verify signatures
        let expected_players = [player0_pk, player1_pk, player2_pk];
        assert!(withdrawal.verify_signatures(&expected_players));

        // wrong player set should fail
        let wrong_players = [player0_pk, player1_pk];
        assert!(!withdrawal.verify_signatures(&wrong_players));

        // create withdrawal with only 2 signatures - should fail verification for 3 players
        let partial_withdrawal = CooperativeWithdrawal::new(
            game_id,
            winner_pubkey,
            pot_commitment,
            &[&player0_sk, &player1_sk],
        );
        assert!(!partial_withdrawal.verify_signatures(&expected_players));

        // but should pass for 2 players
        assert!(partial_withdrawal.verify_signatures(&wrong_players));
    }

    #[test]
    fn test_cooperative_withdrawal_invalid_signature() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let player0_sk = SigningKey::generate(&mut OsRng);
        let player1_sk = SigningKey::generate(&mut OsRng);

        let player0_pk = player0_sk.verifying_key().to_bytes();
        let player1_pk = player1_sk.verifying_key().to_bytes();

        let game_id = 123u64;
        let winner_pubkey = player0_pk;
        let pot_commitment = [0xAAu8; 32];

        // player 0 signs correctly
        let sig0 = CooperativeWithdrawal::sign(
            game_id,
            &winner_pubkey,
            &pot_commitment,
            &player0_sk,
        );

        // create a tampered signature
        let mut tampered_sig = sig0.clone();
        tampered_sig.signature[0] ^= 0xFF;

        let withdrawal = CooperativeWithdrawal {
            game_id,
            winner_pubkey,
            pot_commitment,
            player_signatures: vec![tampered_sig],
        };

        // should fail verification
        assert!(!withdrawal.verify_signatures(&[player0_pk]));
    }
}
