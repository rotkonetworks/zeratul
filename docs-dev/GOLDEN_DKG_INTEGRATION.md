# golden dkg integration for zeratul

integrated golden-rs (non-interactive distributed key generation) into zeratul blockchain for epoch-based validator threshold signatures

## architecture

### components added

1. **`dkg_coordinator`** - manages golden dkg protocol execution
   - `EpochDKG`: per-epoch dkg state tracking
   - `DKGCoordinator`: coordinates dkg across multiple epochs
   - broadcasts encrypted shares to validator set
   - accumulates shares to generate group public key
   - provides partial signing and signature recovery

2. **`dkg_scheme_provider`** - replaces staticscheme with epoch-aware signing
   - implements `SchemeProvider` trait for consensus integration
   - provides threshold signing schemes per epoch
   - caches schemes for performance
   - falls back to previous epoch if current dkg incomplete

3. **`governance/dkg_integration`** - bridges governance and dkg
   - `ValidatorRegistry`: maps AccountId <-> BLS PublicKey
   - `DKGGovernanceManager`: orchestrates validator selection + dkg
   - automatic slashing for non-participation
   - tracks slashing events per epoch

### integration flow

```
epoch boundary (every 4 hours)
    ↓
phragmén election selects 15 validators
    ↓
validators run golden dkg protocol
    ↓
each validator broadcasts encrypted shares
    ↓
validators decrypt and accumulate shares
    ↓
group public key + individual shares generated
    ↓
threshold signatures (t = 11/15 Byzantine fault tolerance)
    ↓
non-participating validators slashed
```

## golden dkg properties

- **non-interactive**: single round of broadcasts, no coordinator
- **efficient**: 223kb vs 27.8MB traditional dkg (50 participants)
- **threshold secure**: t = 2f + 1 Byzantine fault tolerance
- **publicly verifiable**: anyone can verify shares are valid
- **based on evrf**: exponent vrf creates one-time pads for encryption

## slashing rules

| violation | penalty |
|-----------|---------|
| missed dkg broadcast | 1% stake |
| invalid dkg broadcast | 5% stake |
| byzantine behavior | 100% stake (ejection) |

## usage example

```rust
use zeratul_blockchain::{DKGGovernanceManager, DKGSchemeProvider};
use commonware_cryptography::bls12381::primitives::group::Scalar;
use rand::thread_rng;

// initialize dkg manager
let mut rng = thread_rng();
let beta = Scalar::one();
let our_pubkey = /* our BLS public key */;

let mut manager = DKGGovernanceManager::new(
    &mut rng,
    our_pubkey,
    beta,
    60, // 60 second timeout
);

// register validator keys
manager.register_validator(account_id, bls_pubkey);

// start epoch with election results
let bmsg = manager.start_epoch(&mut rng, epoch, election_result)?;

// broadcast our shares (if we're a validator)
if let Some(msg) = bmsg {
    // send msg to all validators via p2p
}

// process incoming shares
manager.on_dkg_broadcast(epoch, sender, msg)?;

// check completion and slash non-participants
if manager.is_dkg_complete(epoch) {
    let group_key = manager.group_pubkey(epoch).unwrap();
    // use group_key for threshold signatures
}

// finalize and slash missing validators
let slashing_events = manager.finalize_dkg(epoch)?;
for event in slashing_events {
    // apply slashing to validator stake
}
```

## integration with consensus

the `DKGSchemeProvider` integrates seamlessly with commonware consensus:

```rust
use zeratul_blockchain::DKGSchemeProvider;

// create provider
let provider = DKGSchemeProvider::new(
    Arc::new(Mutex::new(manager)),
    participants_ed25519,
);

// use in consensus engine
let scheme = provider.scheme(epoch);
```

the consensus layer automatically uses the correct epoch's group key for threshold signatures.

## security considerations

1. **evrf randomness**: beta parameter must be shared securely among initial validators
2. **network timing**: dkg timeout must account for network latency
3. **stake requirements**: minimum stake prevents sybil attacks on validator selection
4. **slashing deterrence**: 1% penalty sufficient for honest-but-lazy validators

## future improvements

1. **zk proofs**: golden-rs is a mock implementation, needs zk proofs for share validity
2. **polynomial extraction**: properly extract polynomial commitments from dkg instead of mock
3. **key rotation**: support mid-epoch key rotation for emergency scenarios
4. **async dkg**: allow validators to join dkg late without blocking epoch progression

## differences from traditional dkg

traditional pedersen dkg:
- interactive: multiple rounds of communication
- slow: 27.8MB for 50 participants
- coordinator needed or synchronous broadcast
- vulnerable to timing attacks

golden dkg:
- non-interactive: one broadcast round
- fast: 223kb for 50 participants
- no coordinator needed
- timing independent (each validator broadcasts once)

## references

- golden dkg paper: https://eprint.iacr.org/2025/1924.pdf
- golden-rs implementation: https://github.com/fedemagnani/golden-rs
- commonware consensus: https://github.com/commonwarexyz/monorepo
