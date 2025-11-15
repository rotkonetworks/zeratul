# Binary Authenticity: Can We Use Ligerito As-Is?

## What Ligerito Actually Provides

Looking at Ligerito's structure:

```rust
pub struct LigeritoProof {
    pub root: Hash,                    // Merkle root of the polynomial
    pub opened_rows: Vec<Vec<u8>>,     // Queried polynomial values
    pub query_indices: Vec<usize>,     // Which positions were queried
    pub merkle_proofs: BatchedMerkleProof,  // Proofs for the queries
    // ... sumcheck data
}
```

**Key insight**: Ligerito DOES expose `opened_rows` and `query_indices`!

This means the verifier CAN see specific polynomial values at random positions!

## Solution 1: Use Ligerito's Existing Query Mechanism ✅

The verifier already gets to see random polynomial positions during verification. We can use this!

### How It Works

1. **Prover builds polynomial**:
```rust
let mut poly = vec![];

// SECTION 1: Binary (at fixed positions)
for byte in binary.iter() {
    poly.push(BinaryElem32::from(*byte as u32));
}
let binary_end = poly.len();

// SECTION 2: Execution trace + constraints
// ... (trace data)
```

2. **During Ligerito verification**:
```rust
// Ligerito queries random positions in the polynomial
// Some of these queries will hit the binary section!

// Query at position i:
let revealed_value = proof.opened_rows[query_num];
let position = proof.query_indices[query_num];

// If position < binary_end, it's in the binary section
if position < binary.len() {
    // Check: revealed value matches actual binary
    let expected = binary[position];
    let actual = revealed_value;

    if actual != expected {
        return false;  // Prover used wrong binary!
    }
}
```

3. **Security analysis**:
- Ligerito makes 148 random queries (for 100-bit security)
- Binary section takes up (binary.len() / poly.len()) of the polynomial
- Expected queries in binary section: 148 * (binary.len() / poly.len())

**Example**:
- Binary: 10 KB = 10,000 bytes
- Trace: 1M steps × 20 field elements = 20M elements
- Total polynomial size: ~20M elements
- Binary fraction: 10,000 / 20,000,000 = 0.0005
- Expected queries in binary: 148 × 0.0005 = 0.074 queries

**Problem**: Only ~0.074 queries hit the binary section! Very low security!

### Solution 1a: Replicate the Binary

Repeat the binary multiple times in the polynomial:

```rust
let mut poly = vec![];

// Replicate binary 1000 times to increase query coverage
for _ in 0..1000 {
    for byte in binary.iter() {
        poly.push(BinaryElem32::from(*byte as u32));
    }
}
let binary_end = poly.len();

// Now binary section is 1000× larger
// Expected queries: 148 × (10M / 20M) = 74 queries
// Security: If prover uses wrong byte, caught with probability 74/148 ≈ 50%
```

**Still not enough!**

### Solution 1b: Interleave Binary with Trace

```rust
let mut poly = vec![];

for i in 0..trace.len() {
    // Every trace element is followed by a binary byte
    poly.push(trace_element(i));
    poly.push(BinaryElem32::from(binary[i % binary.len()]));
}

// Now binary appears throughout the entire polynomial
// All 148 queries check against binary!
// Security: 148 queries × 32 bits each = ~100-bit security ✅
```

**Verdict**: ✅ **This works!** No modification to Ligerito needed!

---

## Solution 2: Separate Binary Commitment

Don't include binary in the polynomial at all. Instead:

```rust
pub struct PolkaVMProof {
    pub binary_commitment: MerkleRoot,  // Separate commitment to binary
    pub ligerito_proof: LigeritoProof,  // Proof of execution trace
}

pub fn verify_polkavm_proof(
    binary: &[u8],
    proof: &PolkaVMProof,
) -> bool {
    // 1. Check binary commitment
    let binary_merkle_root = merkle_tree::commit(binary);
    if binary_merkle_root != proof.binary_commitment {
        return false;
    }

    // 2. Verify execution trace proof
    verify(&verifier_config, &proof.ligerito_proof)
}
```

**In the polynomial**, instead of including the full binary, we include the **binary commitment** (32 bytes):

```rust
let mut poly = vec![];

// Include binary commitment (32 bytes)
for byte in binary_commitment.iter() {
    poly.push(BinaryElem32::from(*byte as u32));
}

// Now in constraints, we check:
// "opcode fetched matches binary[PC]"
// But we can't actually CHECK this in the polynomial!
```

**Problem**: This doesn't actually prove anything! The polynomial can't reference the separate binary.

**Verdict**: ❌ **Doesn't work** - loses the link between binary and execution.

---

## Solution 3: Hash Checking Inside Polynomial

Include the binary AND prove its hash is correct:

```rust
let mut poly = vec![];

// Section 1: Binary
for byte in binary.iter() {
    poly.push(BinaryElem32::from(*byte as u32));
}
let binary_end = poly.len();

// Section 2: Compute hash of binary section
let computed_hash = sha256(&poly[0..binary_end]);

// Section 3: Expected hash (provided as public input)
// We need to encode: computed_hash == expected_hash as constraints

for i in 0..32 {
    let diff = computed_hash[i] ^ expected_hash[i];
    // Constraint: diff must be 0
    poly.push(BinaryElem32::from(diff));
}

// Section 4: Prove all diff values are zero
// This is checked by the sumcheck protocol!
```

**Problem**: SHA256 computation inside the polynomial is EXPENSIVE!
- SHA256 has ~30,000 gates
- Would bloat the polynomial significantly

**Alternative**: Use a simpler hash (Merkle tree hash):

```rust
// Instead of SHA256, use recursive hashing (like Merkle tree)
fn hash_binary_section(poly: &[BinaryElem32]) -> [u8; 32] {
    let mut hashes = poly.chunks(2)
        .map(|chunk| hash_siblings(chunk[0], chunk[1]))
        .collect::<Vec<_>>();

    while hashes.len() > 1 {
        hashes = hashes.chunks(2)
            .map(|chunk| hash_siblings(chunk[0], chunk[1]))
            .collect();
    }

    hashes[0]
}
```

This is cheaper but still adds O(n) hashing operations to the polynomial.

**Verdict**: ⚠️ **Expensive** - Adds significant overhead.

---

## Solution 4: Use Ligerito's Merkle Tree Structure

Actually, Ligerito ALREADY commits the polynomial via Merkle tree!

The `proof.root` IS a commitment to the entire polynomial (including binary section).

But the verifier doesn't compute this root themselves - they trust the proof.

**Wait...** Let me check how Ligerito verification actually works:

```rust
// From our trace function work:
pub fn verify_batch(root: &Hash, proof: &BatchedMerkleProof, queries: &[usize]) -> bool {
    // Verifier checks:
    // 1. Opened values hash to leaf nodes
    // 2. Merkle proofs connect leaves to root
    // 3. Root matches the claimed root
}
```

So the verifier:
1. Gets a claimed root from the proof
2. Verifies Merkle proofs connect to that root
3. **Doesn't check what the root SHOULD be**

**The root is just trusted!**

This is fine for Ligerito because the **sumcheck protocol** ensures the polynomial is correct.

But for our zkVM, we need an additional check:

```rust
pub fn verify_polkavm_proof(
    binary: &[u8],
    trace_output: &[u8],  // Expected output
    proof: &LigeritoProof,
) -> bool {
    // The proof includes opened polynomial values at query positions
    // Some of these MUST match the binary

    for (query_idx, position) in proof.query_indices.iter().enumerate() {
        if *position < binary.len() {
            // This query is in the binary section
            let expected = binary[*position];
            let actual = proof.opened_rows[query_idx][0];  // First element of row

            if actual != expected {
                return false;  // Binary mismatch!
            }
        }
    }

    // Standard Ligerito verification
    verify(&verifier_config, proof)
}
```

**But this only works if queries actually hit the binary section!**

Hence: **Solution 1b (interleaving) is necessary**.

---

## Recommended Solution: Interleaved Binary (1b)

```rust
pub fn arithmetize_polkavm_with_binary_checks(
    binary: &[u8],
    trace: &ExecutionTrace,
) -> Vec<BinaryElem32> {
    let mut poly = vec![];

    for (i, step) in trace.steps.iter().enumerate() {
        // Add trace data
        poly.push(BinaryElem32::from(step.pc));
        for reg in &step.regs {
            poly.push(BinaryElem32::from(*reg));
        }

        // INTERLEAVE: Add binary bytes
        // This ensures queries check binary correctness
        for j in 0..4 {  // Add 4 bytes of binary per trace step
            let binary_idx = (i * 4 + j) % binary.len();
            poly.push(BinaryElem32::from(binary[binary_idx] as u32));
        }

        // Add constraints
        // ... (PC continuity, ALU correctness, etc.)
    }

    poly
}

pub fn verify_polkavm_proof(
    binary: &[u8],
    proof: &LigeritoProof,
) -> bool {
    // Check that queried binary values match
    for (query_idx, position) in proof.query_indices.iter().enumerate() {
        // Determine if this position is in a binary slot
        // (every 4th element in our interleaving scheme)
        if position % (13 + 4) >= 13 {  // 13 reg values + 4 binary bytes
            let binary_slot = position % (13 + 4) - 13;
            let trace_step = position / (13 + 4);
            let binary_idx = (trace_step * 4 + binary_slot) % binary.len();

            let expected = binary[binary_idx];
            let actual = proof.opened_rows[query_idx][0];

            if actual as u8 != expected {
                return false;
            }
        }
    }

    // Standard Ligerito verification
    verify(&verifier_config, proof)
}
```

**Advantages**:
- ✅ No modification to Ligerito
- ✅ Uses existing query mechanism
- ✅ Full 100-bit security (148 queries check binary)
- ✅ Simple verifier logic

**Disadvantages**:
- ⚠️ Increases polynomial size (adds binary data throughout)
- ⚠️ More complex arithmetization

---

## Final Answer

**We do NOT need to modify Ligerito!**

We can use its existing query mechanism by **interleaving the binary throughout the polynomial**.

The verifier:
1. Checks that queried positions in binary sections match the actual binary
2. Runs standard Ligerito verification
3. Both checks must pass

This gives us full security without any changes to Ligerito itself!

**Implementation Plan**:
1. Design interleaving scheme (binary bytes between trace elements)
2. Modify arithmetization to include interleaved binary
3. Add binary-check logic to verifier
4. Test with simple program

Ready to implement this?
