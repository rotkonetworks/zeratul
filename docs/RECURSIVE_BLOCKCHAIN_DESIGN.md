# Recursive Blockchain with Sampling-Based Verification

## Vision

A blockchain where **validators only need to sample a small portion of each block** to verify correctness, and each block recursively proves the validity of the previous block. This creates a **succinct blockchain** where verification complexity is constant regardless of history length.

## Core Concept

```
Block N:
  - Transactions (full data)
  - State transitions (PolkaVM execution)
  - Ligerito proof: "Block N is valid AND Block N-1 was valid"

Verifier (for Block N):
  1. Sample k random positions from Block N transactions
  2. Verify those k samples execute correctly (PolkaVM + Ligerito)
  3. Verify the recursive proof that Block N-1 was valid
  4. Done! Constant time regardless of block size or chain history
```

## Why This Works

### 1. Ligerito Properties

**Polynomial Commitment Scheme over Binary Fields:**
- **Succinctness**: Proofs are O(log n) size, verification is O(log n + k) where k = samples
- **Merkle Multi-Proofs**: Can prove k random positions efficiently
- **Grand Product Argument**: Single polynomial commitment proves all constraints satisfied
- **Binary Fields GF(2^32)**: Perfect for 32-bit PolkaVM operations

### 2. Sampling Security

**Probabilistic Verification:**
- Sample k random transactions/state transitions
- If block is invalid (even 1% corruption), probability of detecting = 1 - (0.99)^k
- With k=100 samples: 99.99% detection probability
- With k=200 samples: 99.9999% detection probability

**Data Availability:**
- Full block data must be published (validators store it)
- Verifiers only need to fetch k samples
- Reed-Solomon encoding ensures data availability (2D sampling)

## Architecture

### Block Structure

```rust
pub struct RecursiveBlock {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    pub state_transitions: Vec<PolkaVMTrace>,
    pub proof: RecursiveProof,
}

pub struct BlockHeader {
    pub block_number: u64,
    pub parent_hash: Hash,
    pub state_root: Hash,
    pub txns_commitment: LigeritoCommitment,  // Merkle commitment to all txns
    pub state_commitment: LigeritoCommitment, // Merkle commitment to state transitions
    pub timestamp: u64,
}

pub struct RecursiveProof {
    // Proves: "This block's transactions are valid AND previous block was valid"
    pub current_block_proof: LigeritoProof,  // Execution correctness
    pub previous_block_proof: LigeritoProof, // Recursive verification
    pub merkle_proofs: Vec<MerkleProof>,     // k samples from current block
}
```

### Verification Flow

```
┌─────────────────────────────────────────────────────────────┐
│ Block N Verification (Constant Time!)                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│ 1. Sample Generation (O(1))                                 │
│    ├─ Generate k random indices from block hash             │
│    └─ Indices = Hash(block_hash || validator_seed) mod N    │
│                                                              │
│ 2. Sample Verification (O(k))                               │
│    ├─ Fetch k transactions from indices                     │
│    ├─ Verify Merkle proofs for k samples                    │
│    ├─ Execute k transactions in PolkaVM                     │
│    ├─ Extract k execution traces                            │
│    └─ Verify k state transitions match                      │
│                                                              │
│ 3. Proof Verification (O(log n))                            │
│    ├─ Verify Ligerito proof for current block               │
│    │  (proves all constraints satisfied)                    │
│    ├─ Verify recursive proof from Block N-1                 │
│    │  (proves Block N-1 was valid)                          │
│    └─ Check commitment consistency                          │
│                                                              │
│ Total: O(k + log n) - CONSTANT!                             │
└─────────────────────────────────────────────────────────────┘
```

## Recursive Proof Construction

### Prover Workflow (Block Producer)

```rust
// Proving Block N
pub fn prove_block(
    block_n: &Block,
    previous_proof: &RecursiveProof,
) -> Result<RecursiveProof, Error> {
    // 1. Execute all transactions in PolkaVM
    let mut traces = Vec::new();
    let mut state = load_state();

    for txn in &block_n.transactions {
        let trace = execute_polkavm_transaction(txn, &state)?;
        state = apply_trace(&trace);
        traces.push(trace);
    }

    // 2. Arithmetize all traces into polynomials
    let execution_polynomial = arithmetize_all_traces(&traces)?;

    // 3. Create Ligerito commitment
    let commitment = ligerito::commit(&execution_polynomial)?;

    // 4. Prove execution correctness
    let execution_proof = ligerito::prove(&execution_polynomial)?;

    // 5. Create recursive circuit that verifies previous block
    let recursive_circuit = RecursiveCircuit {
        current_block_constraints: execution_polynomial,
        previous_block_proof: previous_proof.clone(),
        previous_block_commitment: previous_proof.commitment,
    };

    // 6. Prove recursive circuit
    let recursive_proof = ligerito::prove(&recursive_circuit)?;

    // 7. Generate Merkle proofs for sampling
    let merkle_proofs = generate_merkle_proofs(&traces);

    Ok(RecursiveProof {
        current_block_proof: execution_proof,
        previous_block_proof: recursive_proof,
        merkle_proofs,
        commitment,
    })
}
```

### Verifier Workflow (Light Client)

```rust
// Verifying Block N (only needs k samples!)
pub fn verify_block_samples(
    block_header: &BlockHeader,
    proof: &RecursiveProof,
    k: usize,  // Number of samples
) -> Result<bool, Error> {
    // 1. Generate random sample indices
    let indices = generate_sample_indices(&block_header.hash(), k);

    // 2. Fetch k transactions (via network)
    let samples = fetch_samples(&block_header, &indices)?;

    // 3. Verify Merkle proofs for k samples
    for (i, sample) in indices.iter().zip(&samples) {
        verify_merkle_proof(
            &proof.merkle_proofs[i],
            &sample,
            &block_header.txns_commitment,
        )?;
    }

    // 4. Verify k samples execute correctly
    for sample in &samples {
        let expected_trace = execute_polkavm_transaction(&sample.txn)?;
        assert_eq!(expected_trace, sample.trace);
    }

    // 5. Verify Ligerito proof for current block
    let valid_current = ligerito::verify(
        &proof.current_block_proof,
        &block_header.txns_commitment,
    )?;

    // 6. Verify recursive proof (Block N-1 was valid)
    let valid_recursive = ligerito::verify(
        &proof.previous_block_proof,
        &proof.previous_block_commitment,
    )?;

    Ok(valid_current && valid_recursive)
}
```

## Why This is Better Than zkRollups

### zkRollups (e.g., zkSync, StarkNet)

```
Prover Complexity:  O(n log n)     [FFT over all transactions]
Proof Size:         O(log n)       [succinct]
Verifier Time:      O(1)           [but expensive constant]
Verifier Cost:      ~500K gas      [Ethereum verification]

Problem: Proving is VERY expensive (hours for large blocks)
         Verification on-chain is expensive
```

### Ligerito + Sampling (This Design)

```
Prover Complexity:  O(n)           [linear trace extraction + commit]
Proof Size:         O(log n)       [same succinctness]
Verifier Time:      O(k + log n)   [sample k + verify proof]
Verifier Cost:      ~10K gas       [cheap verification]

Advantages:
  ✓ 10-100x faster proving (linear vs n log n)
  ✓ 50x cheaper verification (binary fields vs pairings)
  ✓ Probabilistic security with tunable k
  ✓ Recursive composition (constant verification)
  ✓ No trusted setup needed
```

## Security Analysis

### Sampling Security

**Adversary Model:** Malicious block producer includes invalid transactions

**Attack Success Probability:**
```
Let:
  n = total transactions in block
  m = number of invalid transactions
  k = number of samples

Fraud detection probability = 1 - (1 - m/n)^k

Examples:
  - 1% invalid (m/n = 0.01), k=100: 63% detection
  - 1% invalid, k=200: 87% detection
  - 1% invalid, k=500: 99.3% detection

  - 10% invalid (m/n = 0.1), k=100: 99.997% detection
  - 10% invalid, k=50: 99.5% detection
```

**Practical Parameters:**
- k=200 samples per block
- Each validator uses different random seed
- Multiple validators → high probability at least one detects fraud
- If fraud detected → block rejected + slashing

### Recursive Security

**Induction Hypothesis:**
```
Base case: Genesis block is valid by definition
Inductive step: If Block N-1 is valid, and we verify:
  1. Block N's proof is valid (Ligerito soundness)
  2. Block N's recursive proof verifies Block N-1
  Then Block N is valid

Result: By induction, all blocks are valid
```

**Soundness:**
- Ligerito is computationally sound (grand product argument)
- Cannot forge proof without breaking hash function (SHA256)
- Sampling provides probabilistic guarantee of correctness

## Implementation Roadmap

### Phase 1: Basic PolkaVM Proving (Weeks 5-10)
**Goal:** Prove single PolkaVM transaction execution

1. **Constraint Generation** (Week 5-7)
   - Implement constraints for core 9 instructions
   - Arithmetize PolkaVM traces
   - Add memory constraints

2. **Single Transaction Proving** (Week 8-10)
   - Prove fibonacci(10) execution
   - Prove simple token transfer
   - Benchmark proof generation time

**Milestone:** Can prove any PolkaVM transaction

### Phase 2: Sampling Infrastructure (Weeks 11-13)
**Goal:** Implement sampling-based verification

1. **Merkle Multi-Proofs** (Week 11)
   - Implement k-sample Merkle proofs
   - Optimize for Ligerito's binary Merkle trees
   - Add commitment scheme

2. **Sample Verification** (Week 12)
   - Generate random indices from block hash
   - Fetch and verify k samples
   - Parallel sample verification

3. **Testing** (Week 13)
   - Test with different k values
   - Measure detection probabilities
   - Benchmark verification time

**Milestone:** Can verify blocks by sampling

### Phase 3: Recursive Proving (Weeks 14-17)
**Goal:** Implement recursive proof composition

1. **Proof-in-Proof Circuit** (Week 14-15)
   - Create circuit that verifies Ligerito proof
   - Arithmetize proof verification
   - Test recursive composition

2. **Block Chain Integration** (Week 16)
   - Implement block structure
   - Create prover for block producers
   - Create verifier for light clients

3. **End-to-End Testing** (Week 17)
   - Prove chain of 10 blocks
   - Verify with sampling
   - Measure performance

**Milestone:** Recursive blockchain working end-to-end

### Phase 4: Production Optimization (Weeks 18-22)

1. **Parallelization**
   - Parallel trace extraction
   - Parallel constraint generation
   - Parallel sample verification

2. **Hardware Acceleration**
   - Use Ligerito's hardware-accel feature
   - GPU FFT for polynomials
   - Carryless multiplication in GF(2^32)

3. **Network Layer**
   - P2P block propagation
   - Sample request/response protocol
   - Data availability sampling

**Milestone:** Production-ready blockchain

## Performance Estimates

### Proving Performance

**Single Transaction (fibonacci(10)):**
```
Trace extraction:    ~1ms    (PolkaVM execution)
Arithmetization:     ~5ms    (convert to polynomials)
Commitment:          ~10ms   (Merkle tree)
Proof generation:    ~50ms   (grand product)
Total:               ~66ms per transaction
```

**Block with 1000 Transactions:**
```
Parallel proving:    ~5 seconds  (16 cores)
Proof size:          ~10 KB
```

### Verification Performance

**Sampling Verification (k=200):**
```
Sample generation:   <1ms
Sample fetching:     ~100ms  (network)
Sample execution:    ~13ms   (200 txns × 66μs)
Merkle verification: ~2ms    (200 proofs)
Ligerito verify:     ~5ms    (proof verification)
Recursive verify:    ~5ms    (verify Block N-1 proof)
Total:               ~125ms
```

**Light Client Sync:**
```
Sync 1000 blocks:    ~125 seconds  (with sampling)
Sync 1M blocks:      ~35 hours     (vs weeks for full node)

With checkpoints every 1000 blocks:
Sync 1M blocks:      ~2 hours      (verify 1000 recursive proofs)
```

## Comparison with Other Systems

### vs Ethereum

| Metric | Ethereum | This Design |
|--------|----------|-------------|
| Full Node Sync | ~1 week | Not needed |
| Light Client Sync | Trust validators | Verify samples (trustless) |
| Verification per Block | Execute all txns | Sample k + verify proof |
| Verification Time | ~500ms (all txns) | ~125ms (k=200 samples) |
| Historical Proof | Re-execute from genesis | Single recursive proof |
| Proof Size | N/A | ~10 KB per block |

### vs zkRollups (zkSync, StarkNet)

| Metric | zkRollups | This Design |
|--------|-----------|-------------|
| Proving Time | 10-60 mins | ~5 seconds |
| Proving Method | STARK/SNARK | Ligerito PCS |
| Field | Large prime | GF(2^32) |
| Verification | On-chain (~500K gas) | Off-chain sampling |
| Recursion | Yes (expensive) | Yes (cheap) |
| Trusted Setup | No | No |

### vs Mina Protocol

| Metric | Mina | This Design |
|--------|------|-------------|
| Recursive SNARKs | Yes | Yes (polynomial commitments) |
| Blockchain Size | ~22 KB constant | ~10 KB per recent block |
| Proving Method | Pickles (SNARK) | Ligerito (PCS) |
| Verification | ~1 SNARK verify | k samples + 1 PCS verify |
| Full Node Requirement | Still needed | Still needed |
| Light Client | Full verification | Probabilistic sampling |

## Open Research Questions

1. **Optimal Sampling Strategy**
   - Fixed k vs adaptive sampling
   - Stratified sampling by transaction type
   - Weighted sampling by transaction value

2. **Data Availability**
   - 2D Reed-Solomon encoding
   - Erasure coding parameters
   - Recovery from partial data

3. **Finality**
   - Probabilistic finality from sampling
   - Economic finality from staking
   - Hybrid approach

4. **Fraud Proofs**
   - Challenge-response protocol
   - Fraud proof generation
   - Slashing conditions

## Next Steps

1. **Complete Phase 1** (Constraint Generation)
   - Implement constraints for core 9 PolkaVM instructions
   - Test with simple programs
   - Benchmark single transaction proving

2. **Design Recursive Circuit**
   - Specify circuit for proof-in-proof
   - Arithmetize Ligerito verification
   - Estimate recursion overhead

3. **Prototype Block Structure**
   - Define block format
   - Implement commitment scheme
   - Test with mock data

4. **Build PoC Chain**
   - 3-block recursive chain
   - Sampling-based verification
   - Measure performance

## Conclusion

This design combines:
- **Ligerito** (efficient polynomial commitments over binary fields)
- **PolkaVM** (deterministic RISC-V execution)
- **Sampling** (probabilistic correctness verification)
- **Recursion** (constant-time verification of unbounded history)

Result: A blockchain where **light clients can verify the entire chain history by checking a single recursive proof and sampling a small number of transactions per block**.

This is more efficient than zkRollups (faster proving), more decentralized than optimistic rollups (no trust assumptions), and more practical than full recursion (sampling reduces verification cost).

**The key innovation**: Ligerito's binary field arithmetic makes recursive proving ~10x faster than STARK-based systems, enabling practical recursive blockchains.
