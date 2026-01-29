# osst

one-step schnorr threshold identification with proactive resharing.

implementation of the OSST protocol from ["One-Step Schnorr Threshold Identification"](https://eprint.iacr.org/2025/722) by Foteinos Mergoupis-Anagnou (GRNET).

## security warning

this crate has not been audited. use at your own risk.

## features

- **non-interactive**: provers generate proofs independently, no coordination needed
- **threshold**: requires t-of-n provers to verify
- **proactive resharing**: rotate custodian sets without changing the group public key
- **multi-curve**: ristretto255, pallas, secp256k1, decaf377
- **no_std**: works in constrained environments (wasm, polkavm)

## curves

| feature | curve | compatibility |
|---------|-------|---------------|
| `ristretto255` | curve25519 | polkadot, sr25519 |
| `pallas` | pallas | zcash orchard |
| `secp256k1` | secp256k1 | bitcoin, ethereum |
| `decaf377` | decaf377 | penumbra |

## usage

```rust
use osst::{SecretShare, Contribution, verify};

// after DKG, each custodian has a share
let share = SecretShare::new(index, scalar);

// generate contribution (schnorr proof)
let contribution = share.contribute(&mut rng, &payload);

// verifier collects t contributions and verifies
let valid = verify(&group_pubkey, &contributions, threshold, &payload)?;
```

## resharing

rotate custodian sets while preserving the group public key:

```rust
use osst::reshare::{Dealer, Aggregator};

// old custodians become dealers
let dealer = Dealer::new(index, current_share, new_threshold, &mut rng);
let commitment = dealer.commitment();
let subshare = dealer.generate_subshare(player_index);

// new custodians aggregate subshares
let mut aggregator = Aggregator::new(player_index);
aggregator.add_subshare(subshare, commitment)?;
let new_share = aggregator.finalize(old_threshold, &group_pubkey)?;
```

## modules

- `osst` - core OSST identification protocol
- `osst::reshare` - proactive secret sharing
- `osst::liveness` - checkpoint proofs for custodian participation
- `osst::curve` - curve backend traits

## license

MIT OR Apache-2.0
