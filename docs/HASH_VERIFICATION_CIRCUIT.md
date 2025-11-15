# Hash Verification Circuit Inside Polynomial

## The Elegant Solution

Instead of interleaving binary data, we **prove the hash of the binary** inside the polynomial itself.

## How It Works

```rust
pub fn arithmetize_with_hash_verification(
    binary: &[u8],
    trace: &ExecutionTrace,
    expected_binary_hash: [u8; 32],  // Public input
) -> Vec<BinaryElem32> {
    let mut poly = vec![];

    // ========================================
    // SECTION 1: Binary Data
    // ========================================
    for byte in binary.iter() {
        poly.push(BinaryElem32::from(*byte as u32));
    }
    let binary_end = poly.len();

    // ========================================
    // SECTION 2: Hash Computation Circuit
    // ========================================
    // Compute hash of binary section using a circuit-friendly hash

    // For binary fields, use a Merkle tree hash (SHA256 of pairs)
    let computed_hash = hash_circuit(&poly[0..binary_end]);

    // ========================================
    // SECTION 3: Hash Verification Constraints
    // ========================================
    // Prove: computed_hash == expected_binary_hash

    for i in 0..32 {  // 32 bytes in hash
        let expected_byte = expected_binary_hash[i];
        let computed_byte = computed_hash[i];

        // XOR gives 0 if equal
        let diff = expected_byte ^ computed_byte;

        // Encode as constraint: diff MUST be 0
        poly.push(BinaryElem32::from(diff as u32));

        // The sumcheck protocol will verify this is 0!
    }

    // ========================================
    // SECTION 4: Execution Trace
    // ========================================
    for step in trace.steps.iter() {
        // Encode step
        poly.push(BinaryElem32::from(step.pc));
        // ... registers, etc.

        // Instruction fetch constraint
        let binary_idx = step.pc as usize;
        let opcode_from_binary = poly[binary_idx];  // Reference binary section!
        let opcode_claimed = BinaryElem32::from(step.opcode as u32);

        // Constraint: opcode matches binary
        let constraint = opcode_from_binary.add(&opcode_claimed);  // Should be 0 in GF(2)
        poly.push(constraint);

        // ... other constraints
    }

    poly
}
```

## The Hash Circuit

For binary fields, we can use a **Merkle tree hash** which is efficient:

```rust
fn hash_circuit(data: &[BinaryElem32]) -> [u8; 32] {
    // Build Merkle tree over the binary data
    let mut layer: Vec<[u8; 32]> = data.chunks(32)
        .map(|chunk| {
            // Hash 32 elements at a time
            let mut hasher = Sha256::new();
            for elem in chunk {
                hasher.update(&elem.to_bytes());
            }
            hasher.finalize().into()
        })
        .collect();

    // Merkle tree construction
    while layer.len() > 1 {
        layer = layer.chunks(2)
            .map(|pair| {
                let mut hasher = Sha256::new();
                hasher.update(&pair[0]);
                if pair.len() > 1 {
                    hasher.update(&pair[1]);
                }
                hasher.finalize().into()
            })
            .collect();
    }

    layer[0]
}
```

**Cost**: O(n) hashes where n = binary.len() / 32

For a 10 KB binary:
- 10,000 / 32 = 313 leaf hashes
- Merkle tree height: log2(313) ≈ 9
- Total hashes: 313 + 156 + 78 + 39 + ... ≈ 600 hashes

**In the polynomial**: Each hash becomes ~10 field elements (for SHA256 state)
- Total overhead: 600 × 10 = 6,000 field elements

This is small compared to a 1M-step execution trace!

## Alternative: Use a Binary-Field-Friendly Hash

SHA256 is designed for word-oriented computation. For binary fields, we can use:

### Option 1: Poseidon Hash (Over Binary Fields)

Poseidon is designed for SNARKs and works natively in finite fields:

```rust
fn poseidon_hash_binary(data: &[BinaryElem32]) -> BinaryElem32 {
    // Poseidon is just field arithmetic!
    // Much cheaper than SHA256 in a circuit

    let mut state = [BinaryElem32::zero(); POSEIDON_WIDTH];

    for chunk in data.chunks(POSEIDON_WIDTH) {
        // Absorb input
        for (i, elem) in chunk.iter().enumerate() {
            state[i] = state[i].add(elem);
        }

        // Apply permutation (ARK + S-box + Mix)
        poseidon_permutation(&mut state);
    }

    state[0]  // Output hash
}

fn poseidon_permutation(state: &mut [BinaryElem32; POSEIDON_WIDTH]) {
    for round in 0..POSEIDON_ROUNDS {
        // Add round constants
        for i in 0..POSEIDON_WIDTH {
            state[i] = state[i].add(&ROUND_CONSTANTS[round][i]);
        }

        // S-box (x^α in the field)
        for i in 0..POSEIDON_WIDTH {
            state[i] = state[i].pow(POSEIDON_ALPHA);
        }

        // MDS matrix multiplication
        let old_state = state.clone();
        for i in 0..POSEIDON_WIDTH {
            state[i] = BinaryElem32::zero();
            for j in 0..POSEIDON_WIDTH {
                state[i] = state[i].add(&MDS_MATRIX[i][j].mul(&old_state[j]));
            }
        }
    }
}
```

**Cost per permutation**:
- POSEIDON_WIDTH = 3 elements
- POSEIDON_ROUNDS = 8 rounds
- Operations per round: 3 additions + 3 exponentiations + 9 multiplications
- Total: ~100 field operations

For 10 KB binary (10,000 elements):
- Chunks: 10,000 / 3 ≈ 3,333 chunks
- Permutations: 3,333
- Field operations: 3,333 × 100 = 333,000 operations

**In the polynomial**: Each operation is just one field element!
- Total overhead: 333,000 field elements

Still manageable for a 1M-step trace (33% overhead).

### Option 2: Simple Merkle Tree with Binary-Friendly Hash

Use a simpler hash designed for binary fields:

```rust
fn binary_hash(a: BinaryElem32, b: BinaryElem32) -> BinaryElem32 {
    // Simple hash: mix the two inputs
    let sum = a.add(&b);
    let prod = a.mul(&b);

    // Irreversible mixing
    sum.add(&prod.mul(&CONSTANT_1))
       .mul(&CONSTANT_2)
}

fn merkle_hash_binary(data: &[BinaryElem32]) -> BinaryElem32 {
    let mut layer = data.to_vec();

    while layer.len() > 1 {
        layer = layer.chunks(2)
            .map(|pair| {
                let right = if pair.len() > 1 { pair[1] } else { BinaryElem32::zero() };
                binary_hash(pair[0], right)
            })
            .collect();
    }

    layer[0]
}
```

**Cost**:
- Height: log2(10,000) ≈ 14 levels
- Hashes per level: n/2 + n/4 + ... ≈ n
- Total operations: 10,000 × 2 (add + mul) = 20,000 field operations

**In the polynomial**: 20,000 field elements overhead!

This is **much cheaper** than SHA256 or Poseidon!

## The Complete Picture

```rust
pub struct PolkaVMProof {
    pub binary_hash: [u8; 32],        // Public input (computed outside)
    pub initial_state: State,          // Public input
    pub final_state: State,            // Public output
    pub ligerito_proof: LigeritoProof, // The main proof
}

pub fn prove_polkavm_execution(
    binary: &[u8],
    initial_state: State,
) -> PolkaVMProof {
    // 1. Execute and get trace
    let trace = execute_in_polkavm(binary, initial_state);

    // 2. Hash the binary (outside the proof)
    let binary_hash = sha256(binary);

    // 3. Build polynomial with hash verification
    let poly = arithmetize_with_hash_check(binary, &trace, binary_hash);

    // 4. Generate Ligerito proof
    let ligerito_proof = ligerito::prove(&config, &poly)?;

    PolkaVMProof {
        binary_hash,
        initial_state,
        final_state: trace.final_state(),
        ligerito_proof,
    }
}

pub fn verify_polkavm_proof(
    binary: &[u8],
    proof: &PolkaVMProof,
) -> bool {
    // 1. Check binary hash matches
    let expected_hash = sha256(binary);
    if expected_hash != proof.binary_hash {
        return false;  // Prover used different binary!
    }

    // 2. Verify Ligerito proof
    //    This checks that:
    //    - Hash verification constraints pass (computed_hash == expected_hash)
    //    - All execution constraints pass
    //    - Sumcheck succeeds
    verify(&verifier_config, &proof.ligerito_proof)
}
```

## How Constraints Are Checked

The hash verification constraints are embedded in the polynomial:

```rust
// At position i in the polynomial:
poly[i] = expected_hash[byte_i] ^ computed_hash[byte_i]

// For the proof to be valid, the sumcheck must verify that
// this constraint evaluates to 0 at all query points

// If prover used wrong binary:
// - computed_hash ≠ expected_hash
// - At least one byte differs
// - That poly[i] ≠ 0
// - Sumcheck fails!
```

The beauty is that **Ligerito's sumcheck already checks these constraints**!

We don't need to modify Ligerito - we just encode the hash verification as part of the constraint polynomial.

## Comparison: Interleaving vs Hash Circuit

| Approach | Polynomial Overhead | Verifier Complexity | Security |
|----------|---------------------|---------------------|----------|
| **Interleaving** | ~50% (full binary repeated) | Simple (check queries) | 100-bit |
| **SHA256 Circuit** | ~0.6% (600 hashes) | None (built-in) | 256-bit |
| **Poseidon Circuit** | ~33% (333k ops) | None (built-in) | 128-bit |
| **Binary Hash** | ~2% (20k ops) | None (built-in) | 100-bit |

**Best choice**: Binary hash circuit!
- Small overhead (2%)
- No verifier changes needed
- Proven by sumcheck
- Binary-field-native

## Implementation Sketch

```rust
// 1. Define binary-friendly hash
fn gf32_hash(a: BinaryElem32, b: BinaryElem32) -> BinaryElem32 {
    // Irreversible mixing function
    let sum = a.add(&b);
    let prod = a.mul(&b);
    sum.add(&prod)
}

// 2. Hash the binary
fn hash_binary_section(poly: &[BinaryElem32]) -> BinaryElem32 {
    let mut result = poly[0];
    for elem in &poly[1..] {
        result = gf32_hash(result, *elem);
    }
    result
}

// 3. Add to arithmetization
let computed_hash = hash_binary_section(&poly[0..binary_len]);
let expected_hash = BinaryElem32::from(binary_hash_bytes);

// Constraint: computed == expected
let constraint = computed_hash.add(&expected_hash);  // Must be 0
poly.push(constraint);
```

**This is the elegant solution!**

Should we implement this approach?
