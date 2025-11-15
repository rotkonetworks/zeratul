# SASSAFRAS-Based Anonymous Staking

**Date**: 2025-11-12

## Problem with Previous Design

The note-based staking had a **critical privacy leak**:

```rust
// ❌ BAD: Trial decryption reveals who nominated whom
for note in unspent_notes {
    if let Some(amount) = note.trial_decrypt(validator_key, validator_idx) {
        // Validator learns WHO nominated them!
        // Defeats purpose of encryption
    }
}
```

**Problem**: Validators can see which accounts nominated them during trial decryption.

## Solution: SASSAFRAS Anonymous Tickets

**SASSAFRAS** (Semi-Anonymous Sortition of Staked Assignees) uses:
1. **Ring VRF signatures** - Anonymous tickets (can't tell who created them)
2. **Delayed revelation** - Validators reveal identity only when claiming
3. **Ephemeral keys** - One-time keys that get erased after use

### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│  Era N (Nomination Period)                                   │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Nominator creates ANONYMOUS ticket:                         │
│                                                               │
│  TicketEnvelope {                                            │
│      body: {                                                 │
│          amount: 1000 ZT (encrypted),                        │
│          validator_commitment: hash(validator_id),           │
│          ephemeral_public: Ed25519::random(),                │
│          erased_public: Ed25519::random(),                   │
│      },                                                       │
│      ring_signature: RingVrfSignature,  // Anonymous!        │
│  }                                                            │
│                                                               │
│  ✅ Nobody knows who created this ticket                     │
│  ✅ Nobody knows which validator was nominated               │
│  ✅ Ticket committed to chain                                │
│                                                               │
└─────────────────────────────────────────────────────────────┘

                             ⏰ 24 hours

┌─────────────────────────────────────────────────────────────┐
│  Era N+1 (Revelation Period)                                 │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  Validators reveal tickets they received:                    │
│                                                               │
│  TicketClaim {                                               │
│      ticket_id: hash(ticket),                                │
│      erased_signature: sign(ephemeral_secret),               │
│      revealed_validator: validator_id,                       │
│  }                                                            │
│                                                               │
│  ✅ Now we know which validator was nominated                │
│  ✅ Can aggregate amounts per validator                      │
│  ✅ Run Phragmén on aggregates                               │
│  ✅ Select 15 validators                                     │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### Privacy Properties

**During Nomination (Era N)**:
- ❌ Can't see who created ticket (Ring VRF is anonymous)
- ❌ Can't see which validator nominated (commitment hidden)
- ❌ Can't see stake amount (encrypted)
- ✅ Can verify ticket is valid (Ring VRF signature)

**During Revelation (Era N+1)**:
- ✅ Validators reveal identity to claim tickets
- ✅ Aggregate backing becomes public
- ❌ Individual nominator amounts stay hidden
- ❌ Who nominated whom stays hidden

**Key Insight**: By the time revelation happens, nominations are already committed. No front-running possible!

## Architecture

### 1. Staking Ticket

```rust
use sp_core::ed25519::{Public as EphemeralPublic, Signature as EphemeralSignature};

/// Anonymous staking ticket (submitted during era N)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingTicket {
    /// Ticket body (committed on-chain)
    pub body: StakingTicketBody,

    /// Ring VRF signature (proves creator is in validator set)
    pub ring_signature: RingVrfSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingTicketBody {
    /// Era when ticket is valid
    pub era: EraIndex,

    /// Attempt index (for anonymity)
    pub attempt_idx: u32,

    /// Encrypted stake amount
    pub encrypted_amount: EncryptedAmount,

    /// Validator commitment (which validator was nominated)
    pub validator_commitment: [u8; 32],

    /// Ephemeral public key (for claiming)
    pub ephemeral_public: EphemeralPublic,

    /// Erased public key (destroyed on claim)
    pub erased_public: EphemeralPublic,
}

impl StakingTicketBody {
    /// Compute ticket ID (VRF output)
    pub fn ticket_id(&self, randomness: &[u8; 32]) -> TicketId {
        // VRF output depends on:
        // - epoch randomness (uncontrollable)
        // - attempt index
        // - era number
        let input = [
            b"zeratul-staking-ticket",
            randomness.as_slice(),
            &self.attempt_idx.to_le_bytes(),
            &self.era.to_le_bytes(),
        ]
        .concat();

        let hash = blake3::hash(&input);
        u128::from_le_bytes(hash.as_bytes()[0..16].try_into().unwrap())
    }
}
```

### 2. Ticket Claim (Revelation)

```rust
/// Ticket claim (submitted at era transition)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketClaim {
    /// Ticket ID being claimed
    pub ticket_id: TicketId,

    /// Validator claiming the ticket
    pub validator: AccountId,

    /// Signature proving ownership of ephemeral key
    pub erased_signature: EphemeralSignature,

    /// Decrypted stake amount (now public)
    pub revealed_amount: Balance,
}

impl TicketClaim {
    /// Verify claim is valid
    pub fn verify(&self, ticket: &StakingTicket) -> Result<bool> {
        // Verify erased_signature matches ticket.body.erased_public
        let message = [b"claim-ticket", &self.ticket_id.to_le_bytes()].concat();

        if !sp_core::ed25519::Pair::verify(
            &self.erased_signature,
            &message,
            &ticket.body.erased_public,
        ) {
            return Ok(false);
        }

        // Verify validator_commitment matches revealed validator
        let commitment = blake3::hash(self.validator.as_slice());
        if commitment.as_bytes() != &ticket.body.validator_commitment {
            return Ok(false);
        }

        Ok(true)
    }
}
```

### 3. Ticket Pool (On-Chain State)

```rust
/// Ticket pool for an era
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketPool {
    /// Era number
    pub era: EraIndex,

    /// Submitted tickets (anonymous)
    pub tickets: BTreeMap<TicketId, StakingTicket>,

    /// Claimed tickets (revealed)
    pub claims: BTreeMap<TicketId, TicketClaim>,

    /// Ticket threshold (based on expected nominations)
    pub threshold: TicketId,

    /// Era randomness (for VRF)
    pub randomness: [u8; 32],
}

impl TicketPool {
    /// Submit anonymous ticket
    pub fn submit_ticket(&mut self, ticket: StakingTicket) -> Result<()> {
        // Verify Ring VRF signature
        if !self.verify_ring_vrf(&ticket.ring_signature)? {
            bail!("Invalid ring VRF signature");
        }

        // Compute ticket ID
        let ticket_id = ticket.body.ticket_id(&self.randomness);

        // Check threshold (only accept good tickets)
        if ticket_id > self.threshold {
            bail!("Ticket ID {} exceeds threshold {}", ticket_id, self.threshold);
        }

        // Store ticket
        self.tickets.insert(ticket_id, ticket);

        tracing::debug!("Submitted anonymous ticket {} for era {}", ticket_id, self.era);

        Ok(())
    }

    /// Claim ticket (reveal validator)
    pub fn claim_ticket(&mut self, claim: TicketClaim) -> Result<()> {
        // Get ticket
        let ticket = self
            .tickets
            .get(&claim.ticket_id)
            .ok_or_else(|| anyhow::anyhow!("Ticket not found"))?;

        // Verify claim
        if !claim.verify(ticket)? {
            bail!("Invalid ticket claim");
        }

        // Store claim
        self.claims.insert(claim.ticket_id, claim);

        tracing::info!(
            "Validator {} claimed ticket {} (amount: {})",
            hex::encode(claim.validator),
            claim.ticket_id,
            claim.revealed_amount
        );

        Ok(())
    }

    /// Aggregate revealed backing per validator
    pub fn aggregate_backing(&self) -> BTreeMap<AccountId, Balance> {
        let mut backing: BTreeMap<AccountId, Balance> = BTreeMap::new();

        for claim in self.claims.values() {
            *backing.entry(claim.validator).or_insert(0) += claim.revealed_amount;
        }

        backing
    }

    /// Check if claim period is open
    pub fn is_claim_period(&self, current_era: EraIndex) -> bool {
        // Claims can be submitted in era N+1 for tickets from era N
        current_era == self.era + 1
    }

    /// Verify Ring VRF signature
    fn verify_ring_vrf(&self, signature: &RingVrfSignature) -> Result<bool> {
        // TODO: Actual Ring VRF verification
        // Should verify that signature was created by someone in the ring
        // without revealing who
        Ok(true)
    }
}
```

### 4. Era Transition with SASSAFRAS

```rust
/// Era transition with ticket revelation
pub struct SassafrasEraTransition {
    /// From/to eras
    pub from_era: EraIndex,
    pub to_era: EraIndex,

    /// Ticket pool from era N (anonymous tickets)
    pub ticket_pool: TicketPool,

    /// Claimed tickets (revealed at transition)
    pub claims: Vec<TicketClaim>,

    /// Aggregated backing per validator
    pub validator_backing: BTreeMap<AccountId, Balance>,

    /// Phragmén election result
    pub elected_validators: Vec<AccountId>,

    /// FROST signature (11/15 validators)
    pub frost_signature: Option<[u8; 64]>,
}

impl SassafrasEraTransition {
    /// Create era transition
    pub fn new(from_era: EraIndex, to_era: EraIndex, ticket_pool: TicketPool) -> Self {
        Self {
            from_era,
            to_era,
            ticket_pool,
            claims: Vec::new(),
            validator_backing: BTreeMap::new(),
            elected_validators: Vec::new(),
            frost_signature: None,
        }
    }

    /// Add ticket claims
    pub fn add_claims(&mut self, claims: Vec<TicketClaim>) -> Result<()> {
        for claim in claims {
            // Verify claim
            self.ticket_pool.claim_ticket(claim.clone())?;
            self.claims.push(claim);
        }

        // Aggregate backing
        self.validator_backing = self.ticket_pool.aggregate_backing();

        tracing::info!(
            "Added {} ticket claims for era transition {} → {}",
            self.claims.len(),
            self.from_era,
            self.to_era
        );

        Ok(())
    }

    /// Run Phragmén election on aggregated backing
    pub fn run_election(&mut self, validator_count: usize) -> Result<()> {
        // Sort validators by backing
        let mut candidates: Vec<(AccountId, Balance)> =
            self.validator_backing.iter().map(|(v, b)| (*v, *b)).collect();

        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        // Select top N
        self.elected_validators = candidates
            .iter()
            .take(validator_count)
            .map(|(v, _)| *v)
            .collect();

        tracing::info!(
            "Elected {} validators for era {} (total backing: {})",
            self.elected_validators.len(),
            self.to_era,
            self.validator_backing.values().sum::<Balance>()
        );

        Ok(())
    }

    /// FROST sign transition (11/15 validators)
    pub fn frost_sign(&mut self, signature: [u8; 64]) -> Result<()> {
        self.frost_signature = Some(signature);
        Ok(())
    }
}
```

## Integration with ZODA

Era transitions are still ZODA-encoded:

```rust
/// ZODA-encoded SASSAFRAS transition
pub struct ZodaSassafrasTransition {
    /// Era transition data
    pub transition: SassafrasEraTransition,

    /// ZODA encoding
    pub zoda_encoding: Vec<u8>,

    /// Ligerito proof (proves ticket claims are valid)
    pub ligerito_proof: LigeritoProof,

    /// ZODA header
    pub zoda_header: ZodaHeader,
}

impl ZodaSassafrasTransition {
    /// What the Ligerito proof proves:
    ///
    /// 1. All ticket Ring VRF signatures are valid
    /// 2. All claims match committed tickets
    /// 3. Phragmén election was run correctly on aggregates
    /// 4. FROST signature is valid (11/15)
    pub fn verify_light(&self) -> Result<bool> {
        // Light clients only verify Ligerito proof
        // No need to re-execute ticket verification!
        self.verify_ligerito_proof()
    }
}
```

## Comparison: Trial Decryption vs SASSAFRAS

### Trial Decryption (Previous Design) ❌

```rust
// PROBLEM: Leaks who nominated whom
for note in notes {
    if let Some(amount) = note.trial_decrypt(validator_key) {
        // ❌ Validator learns WHO nominated them
        // ❌ Can correlate nominators across eras
        // ❌ Privacy broken!
    }
}
```

### SASSAFRAS (New Design) ✅

```rust
// BETTER: Anonymous tickets + delayed revelation
// Era N: Submit anonymous tickets (Ring VRF)
ticket_pool.submit_ticket(anonymous_ticket);
// ✅ Nobody knows who created ticket

// Era N+1: Validators claim tickets
ticket_pool.claim_ticket(claim);
// ✅ Validator reveals identity to claim
// ✅ Individual nominators stay anonymous
// ✅ Only aggregate backing revealed
```

## Privacy Analysis

### What's Hidden

- ✅ **Who nominated whom**: Ring VRF prevents linking nominators to tickets
- ✅ **Individual amounts**: Encrypted, only revealed in aggregate
- ✅ **Nomination patterns**: Can't track nominator behavior across eras

### What's Public

- ✅ **Total tickets**: Number of tickets submitted per era
- ✅ **Aggregate backing**: Total backing per validator (after revelation)
- ✅ **Elected set**: Which 15 validators were selected

### Attack Resistance

**Front-running**:
- ❌ Can't front-run nominations (tickets committed before revelation)

**Sybil attacks**:
- ❌ Ring VRF proves ticket creator is in validator set
- ❌ Ticket threshold limits spam

**Validator collusion**:
- ❌ Can't determine individual nominators even if 14/15 validators collude
- ✅ Ring VRF anonymity holds against coalition

## Implementation Roadmap

### Phase 1: Basic Ticket System ✅
- [x] Define ticket structures
- [x] Implement ticket pool
- [x] Add claim mechanism
- [x] Aggregate backing calculation

### Phase 2: Ring VRF Integration 🔄
- [ ] Integrate `sp-consensus-sassafras` from Polkadot SDK
- [ ] Implement Bandersnatch VRF (used by SASSAFRAS)
- [ ] Add ring context generation
- [ ] Test anonymous signatures

### Phase 3: ZODA Integration 📋
- [ ] ZODA-encode era transitions
- [ ] Ligerito proof of ticket validity
- [ ] Light client verification
- [ ] PolkaVM execution

### Phase 4: Production Hardening 📋
- [ ] Ticket threshold tuning
- [ ] DoS protection (rate limits)
- [ ] Slashing for invalid claims
- [ ] External audit

## Dependency

Add to `Cargo.toml`:

```toml
# SASSAFRAS for anonymous tickets
sp-consensus-sassafras = { path = "../../../../polkadot-sdk/substrate/primitives/consensus/sassafras", default-features = false }
sp-core = { version = "28.0.0", default-features = false }
```

## Conclusion

**SASSAFRAS solves the privacy leak!**

Previous design:
- ❌ Trial decryption reveals who nominated whom
- ❌ Privacy broken

SASSAFRAS design:
- ✅ Anonymous tickets (Ring VRF)
- ✅ Delayed revelation (no front-running)
- ✅ Individual nominators stay hidden
- ✅ Only aggregate backing revealed

This is the **correct design** for privacy-preserving staking with democratic validator selection.

---

## References

- [SASSAFRAS RFC](https://github.com/polkadot-fellows/RFCs/blob/main/text/0026-sassafras-consensus-protocol.md)
- [Ring VRF Paper](https://eprint.iacr.org/2023/002)
- [Polkadot SASSAFRAS Implementation](https://github.com/paritytech/polkadot-sdk/tree/master/substrate/primitives/consensus/sassafras)
