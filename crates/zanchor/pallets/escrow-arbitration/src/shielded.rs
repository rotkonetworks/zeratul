//! Shielded Escrow Design
//!
//! The goal: an observer of the chain learns NOTHING about:
//! - Who is trading with whom
//! - How much is being traded
//! - What currencies are involved
//! - Whether a trade succeeded or failed
//! - Links between parachain identities and external chain addresses
//!
//! Threat model:
//! - Chain state is fully public (assume adversary runs all nodes)
//! - Adversary can correlate timing across chains
//! - Adversary may control some arbitrators
//! - We CANNOT hide that *something* happened (tx exists)
//! - We CAN hide *what* happened
//!
//! # Design: Commitment-based Shielded Escrow
//!
//! ## On-chain state (what adversary sees):
//!
//! ```text
//! ShieldedEscrow {
//!     // Pedersen commitment to escrow parameters
//!     // C = g^amount * h^blinding * j^escrow_id
//!     commitment: [u8; 32],
//!
//!     // Encrypted blob (ChaCha20Poly1305)
//!     // Only buyer+seller can decrypt
//!     // Contains: amounts, addresses, terms
//!     encrypted_params: Vec<u8>,
//!
//!     // State commitment (hides actual state)
//!     // H(state || nonce)
//!     state_commitment: [u8; 32],
//!
//!     // Nullifier (prevents double-spend without revealing escrow)
//!     nullifier: [u8; 32],
//!
//!     // Timing (unavoidable - but use random delays)
//!     created_slot: u32, // coarse-grained, not block
//! }
//! ```
//!
//! ## Key insight: NO AccountIds on chain
//!
//! Instead of `buyer: AccountId`, use:
//! - Anonymous authentication via ring signatures
//! - Or: nullifier-based spend authorization
//! - Or: threshold decryption where parties prove knowledge
//!
//! ## Trade flow (shielded):
//!
//! ```text
//! 1. SETUP (off-chain):
//!    - Buyer & Seller meet via anonymous channel (Tor/I2P)
//!    - Exchange ephemeral DH keys
//!    - Agree on terms: amount, rate, payment method
//!    - Generate shared secrets for escrow
//!
//! 2. CREATE (on-chain):
//!    - Seller submits: commitment, encrypted_params, nullifier
//!    - NO identities, NO amounts in cleartext
//!    - Chain stores opaque blob
//!
//! 3. FUND (external chain):
//!    - Seller funds escrow address on Zcash/Penumbra
//!    - Uses shielded tx - chain can't see amount
//!    - Submits ZK proof of funding (not the tx itself)
//!
//! 4. PAYMENT (off-chain):
//!    - Buyer sends fiat via agreed method
//!    - Communication via encrypted messages only
//!
//! 5. RELEASE (happy path):
//!    - Seller reveals release preimage
//!    - Buyer can sweep with preimage + their key
//!    - Chain only sees: nullifier consumed
//!
//! 6. DISPUTE (unhappy path):
//!    - Either party submits dispute proof
//!    - Proof reveals ONLY what's needed for resolution
//!    - Arbitrators see encrypted evidence
//!    - Chain signs release without knowing recipient
//! ```
//!
//! ## The hard part: disputes
//!
//! In disputes, someone must judge. Options:
//!
//! A) **Threshold decryption** - arbitrators collectively decrypt
//!    Problem: now arbitrators know everything
//!
//! B) **Commit-reveal with ZK** - prove claim without revealing
//!    "I prove payment happened" without showing bank details
//!    Hard but possible with recursive SNARKs
//!
//! C) **Secure MPC** - arbitrators compute on encrypted data
//!    Nobody learns inputs, only output (who wins)
//!    Complex but theoretically sound
//!
//! D) **Accept some leakage in disputes only**
//!    99% of trades = happy path = zero leakage
//!    1% disputes = reveal to small arbitrator set
//!    Arbitrators are bonded, slashed if they leak
//!
//! For v1, option D is pragmatic. Design for happy path privacy.
//!
//! ## Nullifier scheme
//!
//! ```text
//! nullifier = H(escrow_secret || "nullifier")
//! commitment = Commit(amount, blinding, escrow_id)
//!
//! To consume escrow:
//! - Reveal nullifier (marks as spent)
//! - Provide ZK proof: "I know secret such that H(secret||"nullifier") = this nullifier"
//! - No link to original commitment revealed
//! ```
//!
//! ## What about timing correlation?
//!
//! Adversary watches:
//! - Escrow created at T1
//! - Zcash shielded tx at T2
//! - Escrow consumed at T3
//!
//! Mitigation:
//! - Random delays before actions
//! - Batch submissions (many escrows per block)
//! - Decoy escrows (cost: storage)
//! - Use Zcash's shielded pool timing
//!
//! ## External chain address privacy
//!
//! Current leak: `buyer_escrow_pubkey` stored on chain
//!
//! Fix: NEVER store external chain keys on parachain
//!
//! Instead:
//! - Derive escrow address from shared secret
//! - `escrow_addr = DeriveAddress(H(buyer_secret || seller_secret || chain_secret))`
//! - Only parties who know secrets can compute address
//! - Chain stores: `H(escrow_addr)` for verification, not addr itself
//!
//! ## Implementation sketch

use codec::{Decode, Encode};
use scale_info::TypeInfo;

/// Shielded escrow - adversary learns nothing from on-chain state
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub struct ShieldedEscrow {
    /// Pedersen commitment to escrow parameters
    /// Hides: amount, asset, parties, terms
    pub commitment: [u8; 32],

    /// Encrypted parameters (only parties can decrypt)
    /// Contains everything needed to execute trade
    /// Encrypted with: ChaCha20Poly1305(shared_key)
    /// shared_key = X25519(buyer_eph, seller_eph)
    pub encrypted_params: [u8; 256], // fixed size to hide content length

    /// Current state commitment
    /// H(state_enum || random_nonce)
    /// Adversary can't distinguish: pending vs funded vs complete
    pub state_commitment: [u8; 32],

    /// Nullifier - revealed when escrow is consumed
    /// Computed: H(escrow_secret || "nullifier")
    /// Prevents double-spend without linking to commitment
    pub nullifier_hash: [u8; 32], // H(nullifier) - actual nullifier revealed on spend

    /// Coarse timestamp (slot, not block)
    /// Reduces timing precision
    pub created_slot: u32,

    /// Timeout slot (when escrow can be reclaimed)
    pub timeout_slot: u32,
}

/// Nullifier - revealed when consuming escrow
/// Links to nothing except "this escrow is spent"
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub struct Nullifier {
    pub value: [u8; 32],
}

/// Shielded action - proves something without revealing what
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub enum ShieldedAction {
    /// Consume escrow with valid nullifier + ZK proof
    Consume {
        nullifier: Nullifier,
        /// ZK proof: "I know preimage of nullifier_hash in escrow X"
        /// Without revealing which escrow or the preimage
        proof: [u8; 192], // Groth16 proof size
    },

    /// Update state (e.g., mark funded) with proof
    UpdateState {
        /// New state commitment
        new_state_commitment: [u8; 32],
        /// Proof of valid transition
        proof: [u8; 192],
    },

    /// Dispute - reveals encrypted evidence to arbitrators only
    Dispute {
        /// Escrow commitment (proves which escrow, not contents)
        escrow_commitment: [u8; 32],
        /// Evidence encrypted to arbitrator threshold key
        encrypted_evidence: [u8; 1024],
        /// Proof: evidence is valid for this escrow
        proof: [u8; 192],
    },
}

/// What arbitrators see during dispute (threshold decrypted)
/// This is the ONLY leakage, and only in dispute cases
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub struct DisputeEvidence {
    /// Claimed payment proof (e.g., bank confirmation)
    pub payment_proof: [u8; 512],
    /// Who should receive funds (encrypted external addr)
    pub recipient_encrypted: [u8; 64],
    /// Amount (needed to construct release tx)
    pub amount: u128,
}

/// Resolution - arbitrators sign without seeing final recipient
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub struct ShieldedResolution {
    /// Signature over: H(escrow_commitment || resolution_type)
    /// Not over actual addresses or amounts
    pub signature: [u8; 64],
    /// Encrypted release info - only winner can decrypt
    pub encrypted_release: [u8; 128],
}

// ============ STORAGE (what's on chain) ============

// Active shielded escrows
// Key: commitment (not escrow_id - that would be linkable)
// Value: ShieldedEscrow blob
//
// Adversary sees: N blobs of identical structure
// Cannot distinguish: trade size, parties, state

// Nullifier set (spent escrows)
// Key: nullifier
// Value: () (just existence)
//
// Adversary sees: some escrows were consumed
// Cannot link: which commitment was consumed

// ============ ZK CIRCUITS NEEDED ============

// 1. CreateEscrow circuit:
//    Public: commitment, encrypted_params, nullifier_hash
//    Private: amount, blinding, escrow_secret, parties
//    Proves: commitment = Commit(amount, blinding, ...)
//            nullifier_hash = H(H(escrow_secret || "nullifier"))
//
// 2. ConsumeEscrow circuit:
//    Public: nullifier
//    Private: escrow_secret, commitment
//    Proves: nullifier = H(escrow_secret || "nullifier")
//            commitment exists in escrow set (Merkle proof)
//
// 3. StateTransition circuit:
//    Public: old_state_commitment, new_state_commitment
//    Private: old_state, new_state, transition_witness
//    Proves: valid state machine transition occurred

// ============ WHAT'S STILL LEAKED ============

// Unavoidable leakage:
// - Number of active escrows (but not which are real vs decoy)
// - Timing of transactions (mitigate with delays)
// - That the system is being used at all
//
// Dispute-only leakage (to arbitrators):
// - Trade amount
// - Payment evidence
// - But NOT: party identities on parachain
//
// NOT leaked:
// - Who trades with whom
// - Trade amounts (in happy path)
// - External chain addresses
// - Success/failure of trades
// - Exchange rates
// - Trading patterns
