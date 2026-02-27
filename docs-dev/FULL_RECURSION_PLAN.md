# Full Recursion: PolkaVM ⟷ Ligerito

**The Vision**: Infinite composability where PolkaVM proves Ligerito proves PolkaVM proves Ligerito...

## The Magic

```
Layer 0: Application
    ↓ runs in PolkaVM
Layer 1: PolkaVM trace → Ligerito proof
    ↓ proving happens in PolkaVM!
Layer 2: PolkaVM trace (of proving) → Ligerito proof
    ↓ proving happens in PolkaVM!
Layer 3: PolkaVM trace (of proving the proving) → Ligerito proof
    ↓ ...
Layer N: Final proof
    ↓
On-chain verification (simple trace verification)
```

## Why This Is Amazing

### Incremental Verifiable Computation (IVC)

```rust
// Prove a long computation by proving each step
let mut state = initial_state;
let mut proof = None;

for step in 0..1_000_000 {
    // Run step in PolkaVM
    state = polkavm.execute(step_function, state);
    
    // Prove this step + previous proof
    proof = ligerito.prove_polkavm_execution(
        polkavm.get_trace(),
        previous_proof: proof  // Fold in previous proof!
    );
}

// Final proof proves ALL 1M steps!
// Size: Still ~150 KB (constant!)
```

### Proof Aggregation

```rust
// Aggregate multiple proofs into one
let proof1 = ligerito.prove(computation1);  // 150 KB
let proof2 = ligerito.prove(computation2);  // 150 KB
let proof3 = ligerito.prove(computation3);  // 150 KB

// Verify all 3 in PolkaVM
let verification_trace = polkavm.execute(|proofs| {
    assert!(verify(proof1));
    assert!(verify(proof2));
    assert!(verify(proof3));
});

// Prove the verification
let aggregated = ligerito.prove_polkavm_execution(verification_trace);
// Result: Single 150 KB proof that proves all 3 original proofs!
```

### Cross-Chain Bridging

```rust
// Chain A: Generate proof
let proof_A = ligerito.prove(state_transition_A);

// Chain B: Verify proof_A in PolkaVM
let verification_trace = polkavm_B.verify(proof_A);

// Chain B: Prove the verification
let proof_B = ligerito.prove_polkavm_execution(verification_trace);

// Chain B: Now has a proof that Chain A's proof is valid!
```

## The Architecture

### Core Components

```
┌─────────────────────────────────────────────────┐
│ 1. Ligerito Core (no_std)                      │
│    - Binary field arithmetic                    │
│    - Reed-Solomon encoding                      │
│    - Merkle tree operations                     │
│    - Sumcheck protocol                          │
│    - Compiles to: native + RISC-V               │
└─────────────────────────────────────────────────┘
         ↓                           ↑
         ↓                           ↑
┌─────────────────────┐    ┌─────────────────────┐
│ 2a. Native Prover   │    │ 2b. PolkaVM Prover  │
│    - With rayon     │    │    - Single-thread  │
│    - With GPU       │    │    - Deterministic  │
│    - Fast!          │    │    - Traceable!     │
└─────────────────────┘    └─────────────────────┘
         ↓                           ↓
         ↓                           ↓
┌─────────────────────────────────────────────────┐
│ 3. PolkaVM Tracer                               │
│    - Captures execution trace                   │
│    - Arithmetizes to polynomial                 │
│    - Feeds back to Ligerito                     │
└─────────────────────────────────────────────────┘
         ↓
         ↓
┌─────────────────────────────────────────────────┐
│ 4. On-Chain Verifier                            │
│    - Verifies Ligerito proof using traces       │
│    - Simple, gas-efficient                      │
│    - ~200k gas per proof                        │
└─────────────────────────────────────────────────┘
```

## Implementation Phases

### Phase 1: Foundation (Week 1-2)

**Goal**: Get Ligerito running in PolkaVM

```rust
// 1. Port Ligerito to no_std
#![no_std]
extern crate alloc;

// Remove dependencies:
// - rayon → single-threaded fallback
// - std::collections → alloc::collections
// - std::io → custom serialization

// 2. Compile to RISC-V
cargo build --target riscv32em-unknown-none-elf

// 3. Test in PolkaVM
polkavm-runner ./target/riscv32em.../ligerito.elf
```

**Deliverable**: Ligerito prover runs in PolkaVM (no recursion yet)

### Phase 2: PolkaVM Tracer (Week 3-4)

**Goal**: Capture PolkaVM execution as polynomial

```rust
pub struct PolkaVMTracer {
    // Execution trace
    steps: Vec<ExecutionStep>,
}

pub struct ExecutionStep {
    pc: u32,           // Program counter
    regs: [u32; 16],   // RISC-V registers
    mem_addr: u32,     // Memory access
    mem_value: u32,    // Memory value
    opcode: u8,        // Instruction
}

impl PolkaVMTracer {
    pub fn arithmetize(&self) -> Vec<BinaryElem32> {
        let mut poly = vec![];
        
        for step in &self.steps {
            // Encode step as ~20 field elements
            poly.push(BinaryElem32::from(step.pc));
            for reg in &step.regs {
                poly.push(BinaryElem32::from(*reg));
            }
            poly.push(BinaryElem32::from(step.mem_addr));
            poly.push(BinaryElem32::from(step.mem_value));
            
            // Constraint polynomials
            // e.g., next_pc = current_pc + 4 (or branch target)
            poly.push(self.pc_constraint(step));
            poly.push(self.alu_constraint(step));
            poly.push(self.memory_constraint(step));
        }
        
        poly
    }
}
```

**Deliverable**: PolkaVM → Polynomial conversion working

### Phase 3: First Recursion (Week 5-6)

**Goal**: PolkaVM proves Ligerito

```rust
// Application
let app_state = vec![...];  // Some data

// Prove in PolkaVM
let proof = polkavm.execute(|state| {
    // This runs Ligerito prover IN PolkaVM
    ligerito::prove(&config, &state)
});

// Get PolkaVM trace
let trace = polkavm.get_execution_trace();

// Prove the proving!
let meta_proof = ligerito::prove_polkavm_execution(&trace);
```

**Deliverable**: One level of recursion working

### Phase 4: Full IVC (Week 7-8)

**Goal**: Unbounded recursion

```rust
pub struct IVCProof {
    current_state: Hash,
    step_count: u64,
    ligerito_proof: LigeritoProof,
}

impl IVCProof {
    pub fn new(initial_state: &[u8]) -> Self {
        IVCProof {
            current_state: hash(initial_state),
            step_count: 0,
            ligerito_proof: empty_proof(),
        }
    }
    
    pub fn step(&mut self, computation: impl Fn(&[u8]) -> Vec<u8>) {
        // Run computation in PolkaVM
        let new_state = polkavm.execute(computation, &self.current_state);
        let trace = polkavm.get_trace();
        
        // Prove: old_proof is valid AND computation is correct
        self.ligerito_proof = ligerito::prove_ivc_step(
            &trace,
            &self.ligerito_proof,  // Fold in previous proof!
        );
        
        self.current_state = hash(&new_state);
        self.step_count += 1;
    }
    
    pub fn verify(&self) -> bool {
        // Final proof is constant size regardless of step_count!
        ligerito::verify(&self.ligerito_proof)
    }
}

// Usage:
let mut ivc = IVCProof::new(&initial_state);
for i in 0..1_000_000 {
    ivc.step(|state| compute_next(state, i));
}
// Proof size: Still ~150 KB!
```

**Deliverable**: IVC with constant-size proofs

### Phase 5: On-Chain Integration (Week 9-10)

**Goal**: Deploy verifier to Ethereum/Polkadot

```solidity
contract LigeritoVerifier {
    struct Proof {
        bytes32 root;
        bytes32[] openedRows;
        uint256[] queryIndices;
        bytes32[][] traces;  // Using traces for simplicity!
    }
    
    function verifyIVC(
        bytes32 initialState,
        bytes32 finalState,
        uint64 stepCount,
        Proof calldata proof
    ) public view returns (bool) {
        // Verify Ligerito proof with traces
        for (uint i = 0; i < proof.queryIndices.length; i++) {
            if (!verifyTrace(
                proof.root,
                proof.openedRows[i],
                proof.queryIndices[i],
                proof.traces[i]
            )) {
                return false;
            }
        }
        
        // Verify state commitment
        require(
            computeCommitment(proof) == finalState,
            "State mismatch"
        );
        
        return true;
    }
}
```

**Deliverable**: On-chain verifier deployed

## Technical Challenges & Solutions

### Challenge 1: Memory Constraints in PolkaVM

**Problem**: Large polynomials need lots of memory

**Solution**: 
- Use smaller polynomial sizes in PolkaVM (2^16 - 2^20)
- Stream processing for large data
- Compress intermediate values

### Challenge 2: No Parallelism in PolkaVM

**Problem**: No rayon for parallel FFT

**Solution**:
- Already have single-threaded fallback
- PolkaVM JIT is fast enough that single-thread is acceptable
- 2-10x slower than parallel native, but still practical

### Challenge 3: Determinism

**Problem**: Random number generation for Fiat-Shamir

**Solution**:
- Use deterministic RNG from transcript
- Already implemented (Merlin/SHA256 transcripts)
- No changes needed!

### Challenge 4: Proof Size Growth

**Problem**: Recursive proofs might grow

**Solution**:
- Ligerito proofs are constant size
- IVC proof size stays ~150 KB regardless of steps
- This is the magic of PCS-based IVC!

## Performance Estimates

### Native Ligerito
- Prove 2^20: ~500ms
- Verify: ~50ms
- Proof size: ~147 KB

### PolkaVM Ligerito (2-10x slower)
- Prove 2^20: ~1-5 seconds
- Verify: ~100-500ms
- Proof size: ~147 KB (same!)

### Full Recursion (PolkaVM proves PolkaVM proves...)
- Each layer: +1-5 seconds
- Depth 3 recursion: ~5-15 seconds total
- Proof size: Still ~147 KB (constant!)

**This is AMAZING for blockchain!**

Compare to other zkVMs:
- RISC Zero: Minutes to hours for large computations
- SP1: Similar (faster but still slow)
- Our approach: Seconds with constant proof size!

## The Killer Feature: Constant-Size IVC

```rust
// Prove 1 step
let proof_1 = ivc.step(compute);  // 150 KB, 2 seconds

// Prove 1,000 steps
for _ in 0..1000 {
    ivc.step(compute);
}
// Still 150 KB! Still ~2 seconds per step!

// Prove 1,000,000 steps
for _ in 0..1_000_000 {
    ivc.step(compute);
}
// STILL 150 KB! STILL ~2 seconds per step!
```

**This is the power of recursive composition!**

## Next Steps

1. **This week**: Port Ligerito to no_std
2. **Next week**: Build PolkaVM tracer
3. **Week 3**: First recursive proof
4. **Week 4**: Full IVC implementation
5. **Week 5**: On-chain deployment

## The Dream

```
Deploy smart contract with 1 line of code:
    verify_computation(proof)

Run UNLIMITED computation off-chain:
    - PolkaVM executes at near-native speed
    - Ligerito proves recursively
    - Final proof: 150 KB

Verify on-chain:
    - ~200k gas
    - Accepts proof of arbitrary computation
    - Fully trustless!
```

**This is the future of blockchain scalability!**

Ready to build this? 🚀
