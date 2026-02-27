# Ligerito VSS + FROST Escrow Design

## Overview

A 2-of-3 threshold escrow system combining:
- **Ligerito VSS**: Verifiable secret sharing over binary fields
- **FROST-compatible signing**: Works with Zcash (Ristretto255) and Penumbra (decaf377)

## Architecture

```
                         ┌─────────────┐
                         │ Escrow Seed │
                         │  (32 bytes) │
                         └──────┬──────┘
                                │
              ┌─────────────────┼─────────────────┐
              │                 │                 │
              ▼                 ▼                 ▼
    ┌─────────────────┐  ┌───────────┐  ┌─────────────────┐
    │  Ligerito VSS   │  │    KDF    │  │  Chain Address  │
    │                 │  │           │  │                 │
    │ P(x) = S + rx   │  │ sk=H(S)   │  │ pk = sk·G      │
    │ Commitment C    │  │           │  │ funds locked    │
    └────────┬────────┘  └───────────┘  └─────────────────┘
             │
    ┌────────┼────────┐
    │        │        │
    ▼        ▼        ▼
  Buyer   Seller   Arbitrator
  share   share    share
  +proof  +proof   +proof
```

## Security Properties

| Property | Guarantee |
|----------|-----------|
| Threshold | Any 2 of 3 parties can reconstruct |
| Verifiability | Ligerito commitment proves share consistency |
| Non-custodial | No single party can access funds |
| Arbitrator safety | Arb cannot steal (needs 1 other share) |
| Dealer honesty | Verification catches malicious shares |

## Protocol Phases

### Phase 1: Setup (Seller)

```rust
// 1. Generate random seed
let seed: [u8; 32] = random();

// 2. Create VSS polynomial (degree 1 for 2-of-3)
let polynomial = LigeritoVSS::new(seed, threshold=2, parties=3);

// 3. Generate commitment
let commitment = polynomial.commit();  // Ligerito proof root

// 4. Generate shares with opening proofs
let shares = polynomial.create_shares();
// shares[0] = (P(1), proof_1) for Buyer
// shares[1] = (P(2), proof_2) for Seller
// shares[2] = (P(3), proof_3) for Arbitrator

// 5. Derive escrow keypair
let sk = blake2b(seed);
let pk = sk * G;  // On target curve (Ristretto/decaf377)

// 6. Lock funds to pk on chain
```

### Phase 2: Distribution

```rust
// Seller distributes to each party:
struct SharePackage {
    escrow_id: [u8; 32],
    commitment: [u8; 32],      // Ligerito commitment
    public_key: [u8; 32],      // Escrow address
    share: Share,              // Party's share
    proof: LigeritoProof,      // Opening proof
}

// Each party verifies before accepting:
fn verify_share(package: &SharePackage) -> bool {
    ligerito::verify_opening(
        &package.share,
        &package.proof,
        &package.commitment
    )
}
```

### Phase 3: Completion

```rust
// Any 2 parties can reconstruct:
fn complete_escrow(share_a: Share, share_b: Share) -> SecretKey {
    // Lagrange interpolation over binary field
    let seed = reconstruct(share_a, share_b);

    // Derive signing key
    let sk = blake2b(seed);

    // Sign and broadcast transaction
    sk
}
```

## Integration with Zcash/Penumbra

### Zcash (Sapling/Orchard)

```rust
use jubjub::Fr;  // or pasta curves for Orchard

let seed = reconstruct(share_a, share_b);
let sk = Fr::from_bytes(&blake2b(seed));
// Use with Zcash transaction signing
```

### Penumbra

```rust
use decaf377::Fr;

let seed = reconstruct(share_a, share_b);
let sk = Fr::from_bytes(&blake2b(seed));
// Use with Penumbra transaction signing
```

## Why Ligerito for VSS?

### Traditional Feldman VSS
- Commitments: `g^s, g^r` (curve points)
- Verification: curve arithmetic
- Tight coupling to signing curve

### Ligerito VSS (Our Approach)
- Commitments: Polynomial commitment over binary fields
- Verification: Reed-Solomon + sumcheck
- **Curve agnostic**: Same VSS works for any signing scheme
- **ZODA property**: Encoding IS the proof

### The ZODA Insight

From Guillermo Angeris: "ZODA shards ARE Shamir shares"

```
Reed-Solomon encoding = Polynomial evaluation at multiple points
                      = Shamir secret sharing

Ligerito commitment   = Proof that encoding is correct
                      = Verification that all shares are consistent
```

## Comparison to LocalCryptos

| Aspect | LocalCryptos | Ligerito VSS Escrow |
|--------|--------------|---------------------|
| Bitcoin escrow | P2SH script with hash preimages | Not applicable |
| Ethereum escrow | Solidity smart contract | Not applicable |
| Verification | Trust dealer or on-chain | Cryptographic (Ligerito) |
| Privacy | On-chain scripts visible | Off-chain VSS |
| Target chains | BTC, ETH, BCH | Zcash, Penumbra (privacy coins) |

## Implementation Status

- [x] Basic Shamir SSS over binary fields
- [x] Lagrange interpolation
- [x] Merkle-based share verification (weak)
- [ ] **TODO**: Ligerito polynomial commitment integration
- [ ] **TODO**: Opening proof generation
- [ ] **TODO**: Zcash integration (orchard crate)
- [ ] **TODO**: Penumbra integration

## Open Questions

1. **Minimum Ligerito size**: Ligerito is optimized for large polynomials (2^20 elements).
   For 32-byte secrets, should we:
   - Pad to minimum size?
   - Use Ligerito primitives directly without full proof?
   - Batch multiple escrows?

2. **Proof size**: Full Ligerito proofs are ~150KB. For escrow:
   - Use smaller proof variant?
   - Accept larger proofs for stronger security?
   - Compress with SNARKs?

3. **FROST DKG vs seed sharing**: Current design shares a seed and derives keys.
   Alternative: Integrate with FROST's native DKG. Tradeoffs?
