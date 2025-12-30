# ligerito-escrow

**Verifiable 3-party escrow for P2P trading on Zcash and Penumbra.**

## The Thinned Design

Unlike full FROST infrastructure (DKG rounds, validator coordination, on-chain state), this is a minimal 2-of-3 escrow:

```
┌─────────────────────────────────────────────────────────────────────┐
│                                                                      │
│  SELLER generates                   PARTIES hold                     │
│  ──────────────────                 ────────────────                 │
│                                                                      │
│  seed (32 bytes)                    Buyer:  share_1 + proof          │
│       │                             Seller: share_2 + proof          │
│       ├── Ligerito VSS ──────────►  Arb:    share_3 + proof          │
│       │   (verification)                                             │
│       │                                                              │
│       └── FROST split ───────────►  Any 2 shares → sign tx           │
│           (signing keys)                                             │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

## What This Provides

| Feature | Implementation |
|---------|----------------|
| **Share verification** | Ligerito polynomial commitment (curve-agnostic) |
| **Key splitting** | `decaf377-frost` for Penumbra, `frost-ristretto255` for Zcash |
| **Reconstruction** | Lagrange interpolation over binary fields |
| **Signing** | Native curve operations after reconstruction |

## What This Removes (vs frost-bridge pallet)

- ❌ On-chain DKG coordination
- ❌ Validator staking/slashing
- ❌ Key rotation
- ❌ Offchain workers
- ❌ gRPC signing service

## Usage

```rust
use ligerito_escrow::{
    EscrowSetup, EscrowParty,
    frost::{create_escrow_penumbra, verify_share_ligerito},
};

// === SELLER SETUP ===
let seed = [0u8; 32]; // Random seed
let escrow = create_escrow_penumbra(&seed)?;

// Publish: escrow.group_public_key (address to lock funds)
// Publish: ligerito_commitment (for share verification)

// === DISTRIBUTE SHARES ===
// Encrypted to each party's public key (like LocalCryptos)
let buyer_package = &escrow.packages[0];    // index 1
let seller_package = &escrow.packages[1];   // index 2
let arb_package = &escrow.packages[2];      // index 3

// === EACH PARTY VERIFIES ===
// Before accepting escrow, verify share is valid
verify_share_ligerito(
    &buyer_package.secret_share,
    &commitment,
    buyer_package.index,
    &proof,
)?;

// === RELEASE (any 2 parties) ===
// Combine shares → reconstruct seed → derive key → sign
let signature = sign_with_shares(
    buyer_package,
    seller_package,
    &transaction_bytes,
)?;
```

## Architecture

```
ligerito-escrow/
├── shares.rs      # Ligerito-based verifiable secret sharing
├── reconstruct.rs # Lagrange interpolation over GF(2³²)
├── escrow.rs      # 3-party escrow state machine
└── frost.rs       # Bridge to FROST for actual signing
```

## Why Ligerito for Verification?

**Feldman VSS** (what FROST uses internally):
- Tied to signing curve
- Different verification for Penumbra vs Zcash

**Ligerito VSS** (our approach):
- Works over binary fields (GF(2³²))
- Same verification for ANY chain
- Enables ZODA-style "encoding is proof"

```
seed ──► Ligerito commit ──► share verification (chain-agnostic)
  │
  └────► FROST split ──────► signing keys (chain-specific)
```

## Integration

### Penumbra
```toml
[dependencies]
ligerito-escrow = { version = "0.1", features = ["frost-penumbra"] }
```

### Zcash (Orchard)
```toml
[dependencies]
ligerito-escrow = { version = "0.1", features = ["frost-zcash"] }
```

## Security Model

1. **Dealer honesty**: Ligerito commitment prevents malicious share distribution
2. **2-of-3 threshold**: No single party can access funds
3. **Non-custodial arbitrator**: Can only release to buyer OR seller
4. **Forward secrecy**: Ephemeral keys per trade

## Comparison to LocalCryptos

| Aspect | LocalCryptos | ligerito-escrow |
|--------|--------------|-----------------|
| Bitcoin | P2SH hash preimage script | Not applicable |
| Ethereum | Solidity contract | Not applicable |
| Zcash | N/A | FROST threshold + Ligerito VSS |
| Penumbra | N/A | FROST threshold + Ligerito VSS |
| Verification | Trust dealer or on-chain | Cryptographic (Ligerito) |
