# ZODA-based Verifiable Secret Sharing for DKG

Insight from @GuilleAngeris: ZODA can do VSS with "very little additional overhead" for messages >128 bits.

## Background

**Traditional VSS:**
```
Secret Sharing:  Split secret into n shares
Verification:    Publish commitments to polynomial coefficients
Overhead:        Separate commitment scheme (e.g., Pedersen, Feldman)
```

**ZODA-based VSS:**
```
Secret Sharing:  Split secret into n shares (same)
Verification:    ZODA commitment (already computed!)
Overhead:        "Very little additional" (per Guille)
```

## How ZODA-VSS Works

### ZODA Encoding Recap

```rust
// We encode state as Reed-Solomon codewords
let codeword = encode(secret, rate);

// ZODA commitment = Merkle root of codeword
let commitment = merkle_root(codeword);

// Shares = subset of codeword symbols
let shares = codeword.select(indices);
```

### VSS Property

**Key insight:** If parties receive shares + check against commitment, they're guaranteed to decode the same secret.

**Why?**
- ZODA commitment binds to entire codeword
- Reed-Solomon ensures error correction
- Can't produce inconsistent shares without breaking commitment

### Comparison to Traditional VSS

| Property | Feldman VSS | ZODA-VSS |
|----------|-------------|----------|
| Commitment | Polynomial commitments in G1 | Merkle root of RS codeword |
| Verification | Check share against commitments | Check Merkle proof |
| Overhead | O(t) group operations | O(log n) hashes |
| Binding | Discrete log hardness | Collision resistance |
| Integration | Separate crypto | Unified with ZODA |

## Application to Golden DKG

### Traditional Golden DKG

```rust
pub struct GoldenShare {
    polynomial: Poly<G2>,  // VSS commitment
    shares: Vec<SecretShare>,
    evrf_proof: EVRFProof,
}

// Each validator broadcasts:
// 1. EVRF for randomness
// 2. Polynomial commitments for VSS
// 3. Secret shares
```

### Golden ZODA DKG

```rust
pub struct GoldenZodaShare {
    zoda_commitment: [u8; 32],  // Merkle root (free from ZODA!)
    shares: Vec<SecretShare>,
    evrf_proof: EVRFProof,
}

// Each validator broadcasts:
// 1. EVRF for randomness
// 2. ZODA commitment (already computed for other purposes!)
// 3. Secret shares with Merkle proofs
```

**Benefits:**
- No separate polynomial commitment scheme
- Reuse ZODA infrastructure (Ligerito already has it)
- Smaller messages (Merkle root vs G2 elements)
- Unified encoding across entire system

## Implementation for Zeratul

### Phase 1: Enhance frost_zoda.rs

We already have `frost_zoda.rs` which does VSSS (Verifiable Secret Share with ZODA). Extend this:

```rust
// src/frost_zoda.rs (existing)
pub struct ZodaVSSS {
    pub shares: Vec<Share>,
    pub commitments: Vec<ZodaCommitment>,
}

// Add VSS verification using ZODA
impl ZodaVSSS {
    pub fn verify_share(
        &self,
        share_index: usize,
        share: &Share,
        merkle_proof: &MerkleProof,
    ) -> bool {
        // Verify Merkle proof against ZODA commitment
        verify_merkle_proof(
            &self.commitments[share_index],
            share,
            merkle_proof,
        )
    }
}
```

### Phase 2: Create golden_zoda_provider.rs

```rust
// src/dkg/golden_zoda_provider.rs
use zeratul_circuit::zoda::encode;

pub struct GoldenZodaProvider {
    ceremonies: HashMap<EpochIndex, GoldenZodaState>,
}

struct GoldenZodaState {
    /// EVRF for randomness
    evrf: EVRF,

    /// Secret polynomial (our contribution)
    polynomial: Polynomial,

    /// ZODA commitment to polynomial (VSS commitment)
    zoda_commitment: ZodaCommitment,

    /// Received shares from other validators
    received_shares: HashMap<ValidatorIndex, (Share, MerkleProof)>,
}

impl DKGProvider for GoldenZodaProvider {
    fn start_ceremony(...) -> Result<Self::Message> {
        // 1. Generate EVRF for randomness
        let evrf = EVRF::new(randomness);

        // 2. Generate secret polynomial
        let poly = Polynomial::random(threshold);

        // 3. Encode polynomial as ZODA codeword
        let codeword = encode(poly.coefficients(), rate);

        // 4. Compute ZODA commitment (Merkle root)
        let commitment = merkle_root(codeword);

        // 5. Generate shares for each validator
        let shares: Vec<_> = (0..n)
            .map(|i| (poly.evaluate(i), merkle_proof(i)))
            .collect();

        Ok(GoldenZodaMessage {
            evrf_proof: evrf.proof(),
            zoda_commitment: commitment,
            shares,  // With Merkle proofs
        })
    }

    fn handle_message(...) -> Result<Option<Self::Message>> {
        // 1. Verify EVRF proof
        verify_evrf(&msg.evrf_proof)?;

        // 2. Verify our share against ZODA commitment
        let (share, proof) = &msg.shares[our_index];
        verify_merkle_proof(&msg.zoda_commitment, share, proof)?;

        // 3. Store verified share
        state.received_shares.insert(from, (share.clone(), proof.clone()));

        // 4. If we have all shares, compute final secret
        if state.received_shares.len() == n {
            state.secret_share = combine_shares(&state.received_shares);
            state.completed = true;
        }

        Ok(None)  // Golden is 1-round, no response needed
    }
}
```

### Phase 3: Unified ZODA Stack

```rust
// Everything uses ZODA commitments

// Privacy proofs
let proof = ligerito.prove(witness);
assert!(proof.zoda_commitment.verify());

// DKG shares
let dkg_share = golden_zoda.generate_share();
assert!(dkg_share.zoda_commitment.verify());

// State commitments
let state_commit = accidental_computer.commit(state);
assert!(state_commit.zoda_commitment.verify());

// All the same underlying infrastructure!
```

## Network Overhead Analysis

### Traditional Golden (BLS VSS)

```
Message size per validator:
- EVRF proof: ~96 bytes (G1 + G2 elements)
- Polynomial commitments: t × 96 bytes (G2 elements)
- Shares: n × 32 bytes (Fr scalars)

Total: ~96 + (t × 96) + (n × 32) bytes

For n=100, t=67: ~7KB per validator
```

### Golden ZODA (ZODA-VSS)

```
Message size per validator:
- EVRF proof: ~96 bytes (same)
- ZODA commitment: 32 bytes (Merkle root)
- Shares: n × 64 bytes (share + Merkle proof)

Total: ~96 + 32 + (n × 64) bytes

For n=100, t=67: ~6.5KB per validator
```

**Savings:** ~10% smaller messages + unified crypto stack

## Caveat from Guille

> "collectively construct a shared secret" vs "someone constructs the shared secret and compute the additional column"

**Impact on Golden:**
- Each validator is a dealer (constructs their own polynomial)
- Not a problem! Golden already works this way
- ZODA-VSS fits perfectly

**Would be an issue for:**
- Fully distributed DKG where no single dealer exists
- Not applicable to Golden's design

## Security Considerations

**ZODA-VSS Security:**
- Binding: Collision-resistant hash (SHA3/BLAKE3)
- Soundness: Reed-Solomon minimum distance
- Completeness: Merkle proof verification

**vs Traditional VSS:**
- Binding: Discrete log assumption
- Soundness: Polynomial evaluation
- Completeness: Commitment verification

**Both are secure!** Different assumptions, similar guarantees.

## Timeline

**Now (MVP):**
- Use FrostProvider (basic FROST)
- Get 4-validator testnet working

**Phase 2 (1-2 months):**
- Implement GoldenZodaProvider
- Benchmark vs traditional Golden
- Use in testnet

**Phase 3 (Production):**
- Deploy golden_zoda for mainnet
- Full ZODA stack (privacy + DKG + state)
- Unified cryptography

## References

- Guille's tweet: https://twitter.com/GuilleAngeris/status/...
- ZODA paper: [link to paper]
- frost_zoda.rs: Already implemented in Zeratul
- Ligerito: Already using ZODA encoding

## Next Steps

1. Finish basic FROST implementation (current)
2. Test 4-validator DKG
3. Prototype GoldenZodaProvider
4. Benchmark overhead (should be "very little")
5. Deploy to testnet

---

**Key insight:** By reusing ZODA commitments for VSS, we get a fully unified cryptographic stack with minimal overhead. This is a unique advantage of Zeratul's design!
