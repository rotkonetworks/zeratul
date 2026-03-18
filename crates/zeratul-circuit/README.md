# zeratul-circuit

zk constraint system for shielded transactions using ligerito polynomial commitments over GF(2^32).

## overview

zeratul-circuit implements the constraint system for private transactions:
- **spend circuit**: proves note ownership and merkle inclusion
- **output circuit**: proves correct note commitment creation
- **balance circuit**: proves sum(inputs) = sum(outputs) + fee
- **poker circuit**: shielded pot withdrawal with winner proofs

unlike snark-based systems, we use binary field arithmetic (GF(2^32)) with ligerito for polynomial commitments. this gives us:
- no trusted setup
- transparent proofs
- efficient binary operations (AND/XOR are native)

## security warning

the poseidon hash used here (x^3 sbox over GF(2^32)) has known issues in binary fields. the `wim` crate has migrated to Rescue-Prime over GF(2^128) — this crate should follow. use WIM for new work requiring execution proofs.

## performance

benchmarked on amd ryzen 9 7945hx (release build, RUSTFLAGS="-C target-cpu=native"):

| circuit | wires | constraints | build time | witness gen |
|---------|-------|-------------|------------|-------------|
| spend (20-level merkle) | 542,360 | 542,161 | 44ms | 10.5ms |
| output | 47,666 | 47,636 | 4.2ms | 393µs |
| balance (4-in/4-out) | 4,523 | 3,639 | 1.1ms | - |
| **pot withdrawal (20-level)** | **659,840** | **659,634** | ~55ms | - |
| winner proof only | 114,919 | 114,893 | ~15ms | - |

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

## poker settlement

the `poker` module implements shielded pot withdrawal circuits for mental poker.

dispute resolution is handled by narsil's three-court system (see `crates/narsil/POKER_ARBITRATION.md`):
1. **peer court** — players resolve directly via co-signed state
2. **jury court** — narsil OSST jury arbitrates from action logs
3. **appeal court** — on-chain PolkaVM/JAM contract as final arbiter

the circuits here provide the zk proofs needed for the on-chain appeal path:

| circuit | purpose | constraints |
|---------|---------|-------------|
| `WinnerCircuit` | prove winner_sk matches showdown_hash | 114,893 |
| `PotWithdrawalCircuit` | winner proof + spend proof combined | 659,634 |

## known limitations

1. **poseidon insecurity**: x^3 sbox is not a proper permutation in GF(2^32). needs migration to Rescue-Prime (see WIM crate).

2. **merkle chunk independence**: each 32-bit chunk of 256-bit commitments is hashed independently. correctness but not 256-bit collision resistance.

3. **Range vs RangeDecomposed**: `Range` constraint is NOT zk-sound (prover-side only). always use `RangeDecomposed` for security-critical range proofs.

4. **constraint cost**: zk-sound addition is ~900 constraints per 64-bit add.

## usage

```rust
use zeratul_circuit::spend_circuit::SpendCircuit;
use zeratul_circuit::note::{Note, Value, NullifierKey, MerkleProof};

let circuit = SpendCircuit::build(20);
let witness = circuit.populate_witness(&note, &nk, &merkle_proof);
assert!(circuit.circuit.check(&witness.values).is_ok());

use zeratul_circuit::witness_poly::LigeritoInstance;
let instance = LigeritoInstance::new(circuit.circuit, witness);
assert!(instance.is_satisfied());
```

## license

mit / apache-2.0
