# Dynamic Validator Set via Epochs

## Overview

Zeratul should support dynamic validator sets where a higher-level service (running on Zeratul itself) can dictate who the validators are. This follows the JAM model where the validator registry is just another service.

## Design

### Epoch-Based Transitions

```
Epoch 0 (genesis)          Epoch 1                    Epoch 2
[validators: A,B,C]        [validators: A,B,C,D]      [validators: B,C,D,E]
├─block 1─┼─block 2─┼...┼─block N─┤─block N+1─┼...┼─block 2N─┤
                              ^                           ^
                    read pending changes         read pending changes
                    from validator service       from validator service
```

- **Epoch length**: Configurable, e.g., 100 blocks
- **Transition point**: Last finalized block of epoch
- **Safety**: All validators have synchronized view at epoch boundary

### Validator Service (ServiceId = 0)

Reserved service that manages validator set. Its accumulated state contains:

```rust
struct ValidatorServiceState {
    /// Current active validators (applied this epoch)
    active: Vec<ValidatorInfo>,
    /// Pending changes (apply next epoch)
    pending_joins: Vec<ValidatorInfo>,
    pending_exits: Vec<PublicKey>,
    /// Epoch when changes were last applied
    last_epoch: u64,
}

struct ValidatorInfo {
    pubkey: [u8; 32],
    stake: u64,        // For weighted voting (optional)
    endpoint: String,  // P2P address
}
```

### WorkPackage for Validator Changes

Browser clients (or validators themselves) submit WorkPackages to service 0:

```rust
// Payload variants for validator service
enum ValidatorAction {
    Join {
        pubkey: [u8; 32],
        stake_proof: Vec<u8>,  // Proof of stake from external source
        endpoint: String,
    },
    Exit {
        pubkey: [u8; 32],
        signature: [u8; 64],   // Validator signs their exit
    },
    Slash {
        pubkey: [u8; 32],
        evidence: Vec<u8>,     // Proof of misbehavior
    },
}
```

### State Changes

```rust
// state.rs additions

pub struct State {
    // ... existing fields ...

    /// Current epoch number
    epoch: u64,

    /// Blocks per epoch
    epoch_length: u64,

    /// Validators for current epoch (immutable within epoch)
    validators: Vec<Validator>,

    /// Pending validator changes (from validator service)
    pending_validator_changes: ValidatorChanges,
}

impl State {
    /// Check if we're at epoch boundary
    pub fn is_epoch_boundary(&self) -> bool {
        self.height > 0 && self.height % self.epoch_length == 0
    }

    /// Apply pending validator changes at epoch boundary
    pub fn transition_epoch(&mut self) {
        if !self.is_epoch_boundary() {
            return;
        }

        // Read validator service state
        if let Some(service_state) = self.service_states.get(&0) {
            // Deserialize and apply changes
            let changes = self.pending_validator_changes.take();

            // Add new validators
            for join in changes.joins {
                if !self.validators.iter().any(|v| v.pubkey == join.pubkey) {
                    self.validators.push(Validator {
                        pubkey: join.pubkey,
                        stake: join.stake,
                        active: true,
                    });
                }
            }

            // Remove exiting validators
            for exit_pubkey in changes.exits {
                self.validators.retain(|v| v.pubkey != exit_pubkey);
            }
        }

        self.epoch += 1;
    }
}
```

### Consensus Changes

```rust
// consensus.rs additions

impl InstantBFT {
    /// Verify vote is from valid validator FOR THIS EPOCH
    pub fn verify_vote(&self, vote: &Vote, epoch_validators: &[Validator]) -> bool {
        // Vote must be from a validator in the current epoch's set
        epoch_validators.iter().any(|v| {
            v.pubkey == self.validator_pubkey(vote.validator) && v.active
        })
    }

    /// At epoch boundary, update our view of valid validators
    pub fn on_epoch_transition(&mut self, new_validators: &[Validator]) {
        // Clear pending votes from old epoch
        self.pending_votes.clear();

        // Update validator set for new epoch
        self.epoch_validators = new_validators.to_vec();
    }
}
```

### Block Header Changes

```rust
pub struct Header {
    // ... existing fields ...

    /// Epoch number
    pub epoch: u64,

    /// Validator set hash for this epoch (for light clients)
    pub validators_hash: Hash,
}
```

### Safety Guarantees

1. **No mid-epoch changes**: Validator set is frozen for entire epoch
2. **Finalized boundary**: Changes only read from finalized state
3. **2/3+1 continuity**: At least 2/3+1 of old validators must finalize boundary
4. **Slashing window**: Keep old validators accountable for `N` epochs after exit

### Implementation Steps

1. [ ] Add `epoch` and `epoch_length` to `State`
2. [ ] Add `validators_hash` to `Header`
3. [ ] Create validator service (ServiceId = 0) with special handling
4. [ ] Implement `is_epoch_boundary()` and `transition_epoch()`
5. [ ] Update consensus to track epoch validators separately
6. [ ] Add epoch info to `BlockProductionResult`
7. [ ] Handle vote validation with epoch awareness
8. [ ] Add genesis config for initial validator set
9. [ ] Tests for epoch transitions
10. [ ] Tests for validator joins/exits

### Future: Threshold Key Resharing

When using BLS threshold signatures for finality certificates, changing validators requires resharing the threshold key (as per Commonware's approach):

1. Dealers (current validators) generate shares for new validators
2. New validators acknowledge receipt
3. At epoch boundary, derive new group polynomial
4. Old validators' shares become invalid

This is complex and can be deferred. For MVP, use individual ED25519 signatures aggregated into finality certificates.

### Configuration

```rust
pub struct ChainConfig {
    /// Blocks per epoch (0 = no epochs, fixed validators)
    pub epoch_length: u64,

    /// Minimum validators required
    pub min_validators: usize,

    /// Maximum validators allowed
    pub max_validators: usize,

    /// Epochs before exited validator can withdraw
    pub exit_delay_epochs: u64,
}
```

## Open Questions

1. **Stake source**: Where does stake come from? External bridge? Another service?
2. **Slashing**: How to handle equivocation evidence across epochs?
3. **Light clients**: How do they track validator set changes efficiently?
4. **Bootstrapping**: First validators from genesis or DKG ceremony?
