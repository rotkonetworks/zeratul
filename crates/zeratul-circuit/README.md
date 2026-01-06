# zeratul-circuit

zk constraint system for shielded transactions using ligerito polynomial commitments over GF(2^32).

## overview

zeratul-circuit implements the constraint system for private transactions:
- **spend circuit**: proves note ownership and merkle inclusion
- **output circuit**: proves correct note commitment creation
- **balance circuit**: proves sum(inputs) = sum(outputs) + fee

unlike snark-based systems, we use binary field arithmetic (GF(2^32)) with ligerito for polynomial commitments. this gives us:
- no trusted setup
- transparent proofs
- efficient binary operations (AND/XOR are native)

## performance

benchmarked on amd ryzen 9 7945hx (release build, RUSTFLAGS="-C target-cpu=native"):

| circuit | wires | constraints | build time | witness gen |
|---------|-------|-------------|------------|-------------|
| spend (20-level merkle) | 542,360 | 542,161 | 44ms | 10.5ms |
| output | 47,666 | 47,636 | 4.2ms | 393µs |
| balance (4-in/4-out) | 4,523 | 3,639 | 1.1ms | - |
| **pot withdrawal (20-level)** | **659,840** | **659,634** | ~55ms | - |
| winner proof only | 114,919 | 114,893 | ~15ms | - |

### constraint breakdown

**poseidon hash** (per invocation):
- 208 FieldMul constraints (8 full + 56 partial + 8 full rounds)
- ~600 XOR constraints for state mixing
- width=3, rate=2 sponge construction

**64-bit addition** (ripple-carry adder):
- 192 RangeDecomposed bit wires (32 bits × 3 operands × 2 words)
- ~896 constraints per addition (64 bits × 7 constraints/bit × 2 words)
- required for zk-sound balance proofs

**merkle verification** (per level):
- 1 conditional swap (~10 constraints)
- 1 poseidon hash_2 (~800 constraints)
- 20 levels = ~16,000 constraints

## architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      constraint.rs                          │
│  - AND, XOR, Eq (linear)                                   │
│  - Mul (integer), FieldMul (GF(2^32))                      │
│  - Range (unsafe), RangeDecomposed (zk-sound)              │
└─────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│   poseidon.rs   │  │spend_circuit.rs │  │ witness_poly.rs │
│                 │  │                 │  │                 │
│ - s-box: x^3    │  │ - merkle verify │  │ - multilinear   │
│ - cauchy mds    │  │ - nullifier     │  │   encoding      │
│ - shake128 rc   │  │ - commitment    │  │ - ligerito pcs  │
└─────────────────┘  └─────────────────┘  └─────────────────┘
```

## cryptographic details

### binary field: GF(2^32)

irreducible polynomial: `x^32 + x^7 + x^3 + x^2 + 1` (0x1_0000_008D)

operations:
- addition = XOR (free in constraints)
- multiplication = polynomial multiplication mod irreducible

### poseidon parameters

- state width: 3 field elements
- rate: 2 (absorb 2 elements per permutation)
- rounds: 8 full + 56 partial + 8 full
- s-box: x^3 in GF(2^32)
- mds: cauchy matrix with verified mds property
- round constants: SHAKE128 xof with domain separator

### domain separators

generated via SHA-256 of ascii tags:
- `zeratul.notecommit` - note commitment
- `zeratul.nullifier` - nullifier derivation
- `zeratul.merkle.node` - merkle internal nodes

## poker settlement (ghettobox)

the `poker` module implements shielded pot withdrawal for mental poker games.

### happy path: cooperative withdrawal

```
game ends → all players sign → winner withdraws → zero on-chain data
```

when all players agree on the winner:
1. all players sign withdrawal authorization (multi-sig)
2. winner submits signatures + spend proof
3. pot note transferred to winner
4. **no game data revealed on-chain**

### dispute path: on-chain arbitration

```
dispute → post showdown hash → challenge window → winner proves → withdrawal
```

when players don't cooperate:
1. any player posts `ShowdownCommitment` on-chain
2. 24h dispute window opens
3. other players can challenge with counter-proof
4. winner proves they match commitment via `PotWithdrawalCircuit`
5. pot released after window

### circuits

| circuit | purpose | constraints |
|---------|---------|-------------|
| `WinnerCircuit` | prove winner_sk matches showdown_hash | 114,893 |
| `PotWithdrawalCircuit` | winner proof + spend proof combined | 659,634 |

### domain separators

- `zeratul.poker.showdown` - showdown commitment
- `zeratul.poker.pot` - pot binding
- `zeratul.poker.hands` - hand hash for audit

## security notes

### zk soundness

the constraint system is designed for zero-knowledge proofs where the verifier only sees:
- polynomial commitments (merkle roots)
- sumcheck proofs
- random point evaluations

a malicious prover cannot:
- forge balance proofs (ripple-carry addition is fully constrained)
- fake merkle inclusion (conditional swap uses constrained wires)
- compute invalid poseidon hashes (field multiplication is verified)

### known limitations

1. **merkle chunk independence**: each 32-bit chunk of 256-bit commitments is hashed through the tree independently. provides correctness but not 256-bit collision resistance in the traditional sense.

2. **output binding**: current `OutputCircuit` doesn't bind notes to recipients (diversifier/transmission_key not in commitment hash). use `add_full_commitment_constraints` if recipient binding is needed.

3. **Range vs RangeDecomposed**: the `Range` constraint is NOT zk-sound - it's a prover-side check only. always use `RangeDecomposed` for security-critical range proofs.

4. **constraint cost**: zk-sound addition is expensive (~900 constraints per 64-bit add). balance circuits with many inputs/outputs will be large.

## usage

```rust
use zeratul_circuit::spend_circuit::SpendCircuit;
use zeratul_circuit::note::{Note, Value, NullifierKey, MerkleProof};

// build circuit (one-time, can be cached)
let circuit = SpendCircuit::build(20); // 20-level merkle tree

// create witness for a spend
let witness = circuit.populate_witness(&note, &nk, &merkle_proof);

// verify constraints locally (for testing)
assert!(circuit.circuit.check(&witness.values).is_ok());

// for actual proving, encode as polynomial and use ligerito
use zeratul_circuit::witness_poly::LigeritoInstance;
let instance = LigeritoInstance::new(circuit.circuit, witness);
assert!(instance.is_satisfied());
```

## testing

```bash
# run all tests
cargo test

# run benchmarks
cargo test --release bench_ -- --nocapture
```

## license

mit / apache-2.0
