# osst

one-step schnorr threshold identification with proactive resharing.

implementation of the OSST protocol from ["One-Step Schnorr Threshold Identification"](https://eprint.iacr.org/2025/722) by Foteinos Mergoupis-Anagnou (GRNET).

## security warning

this crate has not been audited. use at your own risk.

## accountability and privacy tradeoff

**OSST threshold verification is NOT accountable.** if verification fails, you cannot identify which custodian provided a malformed contribution.

this is a double-edged property:

| perspective | implication |
|-------------|-------------|
| **security** | malicious custodian can cause DoS without identification |
| **privacy** | verifier cannot determine which specific custodians participated |

the same "share-free" design that prevents blame attribution also provides **signer privacy** - similar to ring signatures, the verifier only learns that *some* valid t-of-n subset signed, not *which* subset. this can be desirable for:

- **censorship resistance**: can't target specific signers for retaliation
- **plausible deniability**: any qualifying subset could have been the signers
- **reduced metadata leakage**: participation patterns not revealed
- **private rollups**: threshold-signed state roots without revealing sequencer set

this property makes OSST well-suited for **private syndicate** designs inspired by [narsil](https://www.youtube.com/watch?v=VWdHaKGrjq0&t=16m) - where a group collectively holds assets via threshold custody with internal bft consensus for governance. the syndicate maintains its own replicated state machine; only commitments, nullifiers, and proofs are posted to L1.

the privacy property is key for collective custody: when a syndicate signs a state transition, L1 verifiers cannot determine which members participated. internal voting patterns, dissent, and power dynamics remain hidden - the outside world only learns that *some* valid t-of-n subset authorized the action.

use cases:
- **investment syndicates**: pooled capital with private voting on trades
- **multisig treasuries**: DAO funds without revealing signer coalitions
- **joint custody**: shared assets (families, partnerships) with hidden approval patterns

combined with decaf377 support, OSST integrates naturally with penumbra's shielded pool model.

### why?

the core OSST verification aggregates all contributions into a single equation:

```
g^{Σ μ_i·s_i} = Y^{c̄} · Π u_i^{μ_i}
```

this is a feature of the "share-free" property - the verifier only needs the group public key `Y`, not individual public shares `y_i = g^{x_i}`. without individual shares, you cannot verify each schnorr proof independently:

```
g^{s_i} ≟ u_i · y_i^{c_i}   // requires y_i which verifier doesn't have
```

### implications

**privacy benefits:**
- verifier learns nothing about which specific custodians participated
- protects custodian operational patterns from surveillance
- enables private threshold custody without revealing signer set

**security considerations:**
- a malicious custodian can cause verification to fail without being identified
- denial-of-service attacks are possible if custodians collude to submit bad proofs
- for applications requiring accountability, consider storing individual public shares

### partial mitigation via liveness module

the `osst::liveness` module provides accountability for **reshare contributions**:

```rust
// each dealer signs their contribution individually
let contribution = DealerContribution::sign(commitment, liveness, &secret, context, &mut rng);

// verifier checks each signature against custodian's known public key
contribution.verify_signature(&public_key, context)
```

this allows identifying misbehaving dealers during reshare, but does not address the core OSST verification limitation.

### alternatives for full accountability

if you need identifiable aborts:
- **store public shares**: keep `y_i` for each custodian, verify proofs individually before aggregation
- **use FROST**: has built-in identifiable abort mechanisms
- **add DLEQ proofs**: each custodian proves contribution consistency

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
