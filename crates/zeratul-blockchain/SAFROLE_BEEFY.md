# Safrole Block Production + BEEFY Bridge

**Date**: 2025-11-12

## Overview

Combining JAM's **Safrole** (simplified SASSAFRAS) for block production with **BEEFY** for trustless Polkadot bridging.

### Key Goals

1. **Safrole**: Anonymous block production with tickets
2. **BEEFY**: BLS-signed finality proofs for Polkadot bridge
3. **Pooled liquidity**: Bridge Polkadot assets into Zeratul

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Safrole Block Production (JAM-style)                        │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Epoch N-1: Tickets submitted (Ring VRF)                    │
│    ├─ Validators submit anonymous tickets                    │
│    ├─ Best tickets win slot assignments                      │
│    └─ Sequence of sealing keys for Epoch N                  │
│                                                               │
│  Epoch N: Block production                                   │
│    ├─ Each slot has assigned sealing key                     │
│    ├─ Block sealed with Bandersnatch signature               │
│    ├─ VRF output → entropy accumulator                       │
│    └─ Fallback to deterministic if no tickets                │
│                                                               │
└─────────────────────────────────────────────────────────────┘

                             ⬇

┌─────────────────────────────────────────────────────────────┐
│  BEEFY Finality Gadget                                       │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  For each finalized block:                                   │
│    ├─ Validators sign MMR root with BLS (BLS12-381)         │
│    ├─ Aggregate signatures (t/n threshold)                   │
│    ├─ Publish to Polkadot relay chain                        │
│    └─ Enables trustless light client on Polkadot            │
│                                                               │
│  Polkadot → Zeratul:                                         │
│    ├─ Polkadot validators sign asset transfers               │
│    ├─ Zeratul verifies BEEFY proofs                          │
│    ├─ Mint wrapped assets (wDOT, wUSDT, etc.)              │
│    └─ Use in lending/staking/DeFi                            │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

## Safrole vs SASSAFRAS

**SASSAFRAS** (Polkadot):
- Full protocol with complex ticket accumulation
- Ring VRF with complete anonymity
- Designed for Polkadot's validator set size

**Safrole** (JAM - simplified):
- Simplified ticket system
- Same Ring VRF primitives (Bandersnatch)
- Optimized for smaller validator sets (15 in our case)
- Outside-in sequencer for tickets

**Key differences**:
```
SASSAFRAS:
├─ Tickets accumulated throughout epoch
├─ Complex threshold calculations
└─ Balancing iterations

Safrole (JAM):
├─ Tickets submitted in first 2/3 of epoch
├─ Simple: best N tickets win
├─ Outside-in sequencer: best, worst, 2nd-best, 2nd-worst...
└─ Fallback: deterministic key selection if tickets fail
```

## Safrole State

```rust
/// Safrole consensus state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafroleState {
    /// Pending validator set (next epoch)
    pub pending_set: ValidatorSet,

    /// Epoch's ring root (Bandersnatch)
    pub epoch_root: RingRoot,

    /// Seal tickets for current epoch
    /// Either tickets (normal) or keys (fallback)
    pub seal_tickets: SealTickets,

    /// Ticket accumulator (for next epoch)
    pub ticket_accumulator: Vec<SafroleTicket>,
}

/// Seal tickets (normal or fallback mode)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SealTickets {
    /// Normal: tickets from previous epoch
    Tickets(Vec<SafroleTicket>),

    /// Fallback: deterministic key selection
    Keys(Vec<BandersnatchKey>),
}

/// Safrole ticket (ticket ID + entry index)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafroleTicket {
    /// Ticket ID (VRF output, used for scoring)
    pub id: TicketId,  // [u8; 32]

    /// Entry index (which validator slot)
    pub entry_index: u32,
}
```

## Entropy Accumulation

Safrole generates high-quality randomness for the protocol:

```rust
/// Entropy accumulator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropyAccumulator {
    /// Current accumulator value
    pub current: Hash,

    /// Last 3 epoch boundaries
    pub epoch_1: Hash,
    pub epoch_2: Hash,
    pub epoch_3: Hash,
}

impl EntropyAccumulator {
    /// Accumulate VRF output
    pub fn accumulate(&mut self, vrf_output: &Hash) {
        self.current = blake3::hash(&[
            self.current.as_bytes(),
            vrf_output.as_bytes(),
        ].concat());
    }

    /// Rotate on epoch change
    pub fn rotate_epoch(&mut self) {
        self.epoch_3 = self.epoch_2;
        self.epoch_2 = self.epoch_1;
        self.epoch_1 = self.current;
    }
}
```

**Uses**:
- Seed ticket verification (prevents bias)
- Fallback key selection (deterministic randomness)
- General protocol randomness (accessible to services)

## Block Sealing

```rust
/// Block header (JAM-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafroleHeader {
    /// Parent block hash
    pub parent: Hash,

    /// Slot index (timeslot)
    pub timeslot: u64,

    /// Block author's Bandersnatch key
    pub author_bs_key: BandersnatchKey,

    /// Seal signature (proves authorship)
    pub seal_sig: BandersnatchSignature,

    /// VRF signature (entropy source)
    pub vrf_sig: BandersnatchSignature,

    /// Epoch marker (if first block of epoch)
    pub epoch_mark: Option<EpochMarker>,

    /// Winners marker (if ticket submission closed)
    pub winners_mark: Option<Vec<TicketId>>,

    /// Is this block sealed with ticket? (vs fallback)
    pub is_ticketed: bool,
}

impl SafroleHeader {
    /// Verify seal signature
    pub fn verify_seal(&self, slot_seal_key: &BandersnatchKey) -> Result<bool> {
        // Message depends on mode
        let message = if self.is_ticketed {
            // Normal: $jam_ticket_seal || entropy_3 || entry_index
            [b"$jam_ticket_seal", /* ... */].concat()
        } else {
            // Fallback: $jam_fallback_seal || entropy_3
            [b"$jam_fallback_seal", /* ... */].concat()
        };

        verify_bandersnatch_signature(
            &self.seal_sig,
            slot_seal_key,
            &message,
            &self.encode_unsigned(),
        )
    }

    /// Verify VRF signature (entropy)
    pub fn verify_vrf(&self) -> Result<bool> {
        // VRF signs the seal's output
        let message = [
            b"$jam_entropy",
            &bandersnatch_output(&self.seal_sig),
        ].concat();

        verify_bandersnatch_signature(
            &self.vrf_sig,
            &self.author_bs_key,
            &message,
            b"",
        )
    }
}
```

## Ticket Submission & Selection

```rust
/// Ticket extrinsic (submitted in blocks)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketExtrinsic {
    /// Entry index (0..MAX_ENTRIES)
    pub entry_index: u32,

    /// Ring VRF proof (anonymous)
    pub proof: BandersnatchRingProof,
}

impl SafroleState {
    /// Process ticket extrinsic
    pub fn submit_tickets(
        &mut self,
        tickets: Vec<TicketExtrinsic>,
        slot_phase: u64,
        epoch_length: u64,
    ) -> Result<()> {
        // Check submission period
        let tail_start = (epoch_length * 2) / 3;
        if slot_phase >= tail_start {
            bail!("Ticket submission period closed");
        }

        // Extract ticket IDs from Ring VRF proofs
        let new_tickets: Vec<SafroleTicket> = tickets
            .iter()
            .map(|t| SafroleTicket {
                id: bandersnatch_ring_output(&t.proof),
                entry_index: t.entry_index,
            })
            .collect();

        // Merge into accumulator (sorted by ID, keep best)
        self.ticket_accumulator = merge_tickets(
            &self.ticket_accumulator,
            &new_tickets,
            epoch_length,
        );

        Ok(())
    }

    /// On epoch change: apply outside-in sequencer
    pub fn seal_tickets_for_new_epoch(&mut self) -> Result<()> {
        if self.ticket_accumulator.len() == EPOCH_LENGTH {
            // Normal mode: use tickets with outside-in sequencer
            self.seal_tickets = SealTickets::Tickets(
                outside_in_sequence(&self.ticket_accumulator)
            );
        } else {
            // Fallback mode: deterministic key selection
            self.seal_tickets = SealTickets::Keys(
                fallback_key_sequence(
                    &self.entropy.epoch_2,
                    &self.pending_set.validators,
                )
            );
        }

        // Clear accumulator for next epoch
        self.ticket_accumulator.clear();

        Ok(())
    }
}

/// Outside-in sequencer (JAM-style)
///
/// Takes sorted tickets and reorders: best, worst, 2nd-best, 2nd-worst...
fn outside_in_sequence(tickets: &[SafroleTicket]) -> Vec<SafroleTicket> {
    let mut result = Vec::with_capacity(tickets.len());
    let n = tickets.len();

    for i in 0..n {
        if i % 2 == 0 {
            // Even: take from start
            result.push(tickets[i / 2].clone());
        } else {
            // Odd: take from end
            result.push(tickets[n - 1 - i / 2].clone());
        }
    }

    result
}

/// Fallback key sequence (deterministic from entropy)
fn fallback_key_sequence(
    entropy: &Hash,
    validators: &[ValidatorInfo],
) -> Vec<BandersnatchKey> {
    (0..EPOCH_LENGTH)
        .map(|i| {
            // Hash entropy || slot_index
            let hash = blake3::hash(&[
                entropy.as_bytes(),
                &i.to_le_bytes(),
            ].concat());

            // Select validator deterministically
            let index = u32::from_le_bytes(hash[0..4].try_into().unwrap());
            let validator_idx = (index as usize) % validators.len();

            validators[validator_idx].bandersnatch_key
        })
        .collect()
}
```

## BEEFY Integration

**BEEFY** (Bridge Efficiency Enabling Finality Yielder) provides succinct finality proofs for light clients.

```rust
/// BEEFY commitment (signed by validators)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeefyCommitment {
    /// Block number
    pub block_number: u64,

    /// MMR root (Merkle Mountain Range)
    pub mmr_root: Hash,

    /// Validator set ID
    pub validator_set_id: u64,
}

impl BeefyCommitment {
    /// Hash for signing (Keccak-256)
    pub fn signing_hash(&self) -> Hash {
        keccak256(&[
            b"$jam_beefy",
            &self.block_number.to_le_bytes(),
            self.mmr_root.as_bytes(),
            &self.validator_set_id.to_le_bytes(),
        ].concat())
    }
}

/// BEEFY signed commitment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeefySignedCommitment {
    /// Commitment
    pub commitment: BeefyCommitment,

    /// Validator signatures (BLS12-381)
    pub signatures: Vec<Option<BlsSignature>>,

    /// Validator set
    pub validator_set: ValidatorSet,
}

impl BeefySignedCommitment {
    /// Create commitment
    pub fn new(
        commitment: BeefyCommitment,
        validator_set: ValidatorSet,
    ) -> Self {
        Self {
            commitment,
            signatures: vec![None; validator_set.validators.len()],
            validator_set,
        }
    }

    /// Sign with validator's BLS key
    pub fn sign(&mut self, validator_idx: usize, bls_key: &BlsSecretKey) -> Result<()> {
        let hash = self.commitment.signing_hash();
        let signature = bls_sign(bls_key, hash.as_bytes());

        self.signatures[validator_idx] = Some(signature);

        Ok(())
    }

    /// Aggregate signatures
    pub fn aggregate(&self) -> Result<BlsSignature> {
        let valid_sigs: Vec<&BlsSignature> = self
            .signatures
            .iter()
            .filter_map(|s| s.as_ref())
            .collect();

        if valid_sigs.is_empty() {
            bail!("No signatures to aggregate");
        }

        aggregate_bls_signatures(&valid_sigs)
    }

    /// Verify aggregated signature
    pub fn verify_aggregate(&self, aggregate: &BlsSignature) -> Result<bool> {
        // Collect public keys of signers
        let pubkeys: Vec<&BlsPublicKey> = self
            .signatures
            .iter()
            .enumerate()
            .filter_map(|(i, sig)| {
                if sig.is_some() {
                    Some(&self.validator_set.validators[i].bls_key)
                } else {
                    None
                }
            })
            .collect();

        let hash = self.commitment.signing_hash();

        verify_aggregate_bls_signature(aggregate, &pubkeys, hash.as_bytes())
    }

    /// Check if commitment has enough signatures (2/3+ threshold)
    pub fn has_supermajority(&self) -> bool {
        let signed_count = self.signatures.iter().filter(|s| s.is_some()).count();
        let threshold = (self.validator_set.validators.len() * 2 + 2) / 3;

        signed_count >= threshold
    }
}
```

## Polkadot Bridge Architecture

```
┌────────────────────────────────────────────────────────────┐
│  Polkadot Relay Chain                                      │
├────────────────────────────────────────────────────────────┤
│                                                             │
│  BEEFY Light Client (Zeratul finality)                    │
│    ├─ Tracks Zeratul validator set                         │
│    ├─ Verifies BLS aggregate signatures                    │
│    ├─ Trusts finalized Zeratul state                       │
│    └─ Enables trustless asset transfers                    │
│                                                             │
│  Bridge Pallet:                                            │
│    ├─ Lock DOT → emit event                                │
│    ├─ Verify Zeratul proofs                                │
│    └─ Unlock DOT ← burn wDOT                               │
│                                                             │
└────────────────────────────────────────────────────────────┘

                        ⬇ ⬆

┌────────────────────────────────────────────────────────────┐
│  Zeratul                                                    │
├────────────────────────────────────────────────────────────┤
│                                                             │
│  BEEFY Light Client (Polkadot finality)                   │
│    ├─ Tracks Polkadot validator set                        │
│    ├─ Verifies Polkadot BEEFY proofs                       │
│    ├─ Trusts finalized Polkadot state                      │
│    └─ Enables trustless asset transfers                    │
│                                                             │
│  Bridge Module:                                            │
│    ├─ Mint wDOT ← verify lock proof                        │
│    ├─ Use wDOT in lending/staking/DEX                      │
│    ├─ Burn wDOT → emit event                               │
│    └─ Unlock DOT on Polkadot                               │
│                                                             │
└────────────────────────────────────────────────────────────┘
```

### Asset Bridging Flow

**DOT → Zeratul**:
```rust
1. User locks DOT on Polkadot bridge pallet
2. Polkadot emits TransferInitiated event
3. Zeratul validators observe event (via light client)
4. Validators reach 11/15 FROST consensus
5. Zeratul mints wDOT to user
6. User can use wDOT in Zeratul DeFi
```

**wDOT → DOT** (return):
```rust
1. User burns wDOT on Zeratul
2. Zeratul emits BurnEvent with BEEFY proof
3. Polkadot bridge verifies BEEFY aggregate signature
4. Polkadot unlocks DOT to user
```

### Supported Assets

```rust
/// Bridged assets from Polkadot
pub enum PolkadotAsset {
    /// Wrapped DOT
    DOT,

    /// Wrapped USDT (Asset Hub)
    USDT,

    /// Wrapped USDC (Asset Hub)
    USDC,

    /// Other Asset Hub assets
    AssetHub(u32),

    /// Parachain tokens (via XCM)
    Parachain { para_id: u32, asset_id: u32 },
}

/// Bridge state
#[derive(Debug, Clone)]
pub struct PolkadotBridge {
    /// Polkadot light client (BEEFY)
    pub polkadot_client: BeefyLightClient,

    /// Wrapped assets
    pub wrapped_assets: BTreeMap<PolkadotAsset, Balance>,

    /// Pending transfers
    pub pending_transfers: Vec<PendingTransfer>,

    /// FROST 11/15 for transfer authorization
    pub frost_signer: FrostMultisig,
}
```

## Implementation Roadmap

### Phase 1: Safrole MVP 🔄
- [ ] Implement SafroleState structure
- [ ] Ticket submission and accumulation
- [ ] Outside-in sequencer
- [ ] Fallback key selection
- [ ] Block sealing with Bandersnatch
- [ ] VRF entropy accumulation

### Phase 2: Bandersnatch Integration 📋
- [ ] Integrate Bandersnatch from Polkadot SDK
- [ ] Ring VRF proof generation
- [ ] Ring VRF proof verification
- [ ] Ring root construction
- [ ] Test with real validator set

### Phase 3: BEEFY Finality 📋
- [ ] Implement BeefyCommitment structure
- [ ] BLS12-381 signature aggregation
- [ ] MMR (Merkle Mountain Range) construction
- [ ] Validator set tracking
- [ ] Light client sync protocol

### Phase 4: Polkadot Bridge 📋
- [ ] Deploy BEEFY light client on Polkadot
- [ ] Deploy Zeratul Polkadot light client
- [ ] Asset locking/unlocking logic
- [ ] Wrapped token minting (wDOT, wUSDT, etc.)
- [ ] IBC/XCM integration

### Phase 5: DeFi Integration 📋
- [ ] Use wDOT as collateral in lending
- [ ] wDOT staking pools
- [ ] wDOT/ZT liquidity pools
- [ ] Cross-chain yield strategies

## Security Considerations

### Safrole Security

**Ticket manipulation**:
- ❌ **Blocked**: Ring VRF proves validator membership without revealing identity
- ❌ **Blocked**: Ticket IDs are VRF outputs (unbiasable)

**Slot assignment prediction**:
- ❌ **Blocked**: Outside-in sequencer prevents gaming
- ❌ **Blocked**: Fallback mode ensures liveness

**Nothing-at-stake**:
- ✅ **Mitigated**: Each slot has single assigned validator
- ✅ **Mitigated**: BEEFY finality prevents long-range attacks

### BEEFY Security

**False finality**:
- ❌ **Blocked**: Requires 2/3+ validators to sign (11/15 in our case)
- ❌ **Blocked**: BLS aggregate signature is cryptographically secure

**Validator set changes**:
- ✅ **Handled**: Epoch markers track validator set updates
- ✅ **Handled**: Light clients sync validator set from headers

**Bridge exploits**:
- ❌ **Blocked**: FROST 11/15 required for minting wrapped assets
- ❌ **Blocked**: Polkadot bridge verifies BEEFY proofs before unlocking

## Comparison: Safrole vs Alternatives

| Feature | Safrole (JAM) | SASSAFRAS (Polkadot) | BABE (Polkadot) |
|---------|---------------|----------------------|-----------------|
| **Anonymity** | ✅ Ring VRF | ✅ Ring VRF | ❌ VRF (not anonymous) |
| **Complexity** | Low | High | Low |
| **Ticket system** | Simplified | Full | None |
| **Fallback** | ✅ Yes | ❌ No | N/A |
| **Forks** | Rare | Very rare | Common |
| **Finality** | BEEFY | GRANDPA | GRANDPA |

## Dependencies

```toml
# Bandersnatch VRF
bandersnatch-vrfs = { path = "../../../../polkadot-sdk/substrate/primitives/consensus/bandersnatch", default-features = false }

# BLS12-381 (for BEEFY)
bls12_381 = { version = "0.8", default-features = false }
w3f-bls = { version = "0.1", default-features = false }

# BEEFY
sp-consensus-beefy = { path = "../../../../polkadot-sdk/substrate/primitives/consensus/beefy", default-features = false }

# MMR
sp-mmr-primitives = { path = "../../../../polkadot-sdk/substrate/primitives/merkle-mountain-range", default-features = false }
```

## Conclusion

**Safrole + BEEFY gives us**:

1. ✅ **Anonymous block production** (Ring VRF tickets)
2. ✅ **Trustless Polkadot bridge** (BEEFY finality proofs)
3. ✅ **Pooled liquidity** (wDOT, wUSDT, etc. from Polkadot)
4. ✅ **Simplified design** (compared to full SASSAFRAS)
5. ✅ **Production-ready** (JAM specification is well-defined)

This enables **DeFi with Polkadot assets** while maintaining **privacy** and **security**!

## References

- [JAM Gray Paper](https://graypaper.com/) - Safrole specification
- [Bandersnatch VRF](https://eprint.iacr.org/2023/002) - Ring VRF primitives
- [BEEFY](https://spec.polkadot.network/sect-finality#sect-grandpa-beefy) - Finality gadget
- [BLS12-381](https://hackmd.io/@benjaminion/bls12-381) - BLS signature scheme
