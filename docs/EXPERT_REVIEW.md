# Expert Review: PolkaVM + Ligerito zkVM Architecture

## From Guillermo Angeris (Ligerito Co-Author, Cryptographer)

### On the Hash Verification Circuit

**Issue 1: Hash Choice is Critical**

> "You're proposing to use a simple binary hash like `gf32_hash(a,b) = a + b + (a*b)` for Merkle tree construction. This is **not collision-resistant**!"

**Attack**:
```rust
// Find collision:
// a + b + ab = c + d + cd

// Example: Let a=1, b=0
// Result: 1 + 0 + 0 = 1

// Let c=0, d=1
// Result: 0 + 1 + 0 = 1

// COLLISION! Same hash for different inputs!
```

**The function is linear (in characteristic 2)**:
```
h(a⊕b, c) = (a⊕b) + c + (a⊕b)c
         = a + b + c + ac + bc
         ≠ h(a, b⊕c) in general

But for specific inputs, collisions are trivial to find.
```

**Fix**: Use a **proper cryptographic hash**:
- SHA256 (expensive but proven)
- Poseidon over GF(2^32) (zkSNARK-friendly)
- BLAKE3 (faster than SHA256)
- **NOT** a simple polynomial!

---

### Issue 2: Sumcheck Doesn't Check Constraints The Way You Think

> "You claim: 'The sumcheck protocol verifies that constraint polynomials evaluate to 0'. This is **not quite right**."

**What Ligerito actually does**:

Ligerito proves that a polynomial `P(x)` is:
1. Committed correctly (via Merkle tree)
2. Low-degree (via Reed-Solomon encoding)
3. Evaluates correctly at random points (via sumcheck)

**But it does NOT enforce that specific positions contain specific values!**

The sumcheck checks:
```
∑_x P(x) = claimed_sum
```

It doesn't check:
```
P[i] = 0 for all i
```

**The difference**:
- If you encode `constraint[i] = expected_hash ^ computed_hash`, the polynomial **contains** these values
- But sumcheck verifies the **sum** over a random subset
- A prover could make constraint[i] ≠ 0 for some i, as long as the sum works out!

**Fix**: You need to actually use the constraint values in a way that sumcheck catches!

```rust
// WRONG:
poly.push(expected ^ computed);  // Sumcheck won't catch if this is non-zero

// CORRECT: Multiply by random challenges
let alpha = transcript.challenge();
let constraint = expected ^ computed;
running_product *= (alpha - constraint);  // If any constraint ≠ 0, product ≠ 0
```

Or better: Use a proper constraint system like R1CS or AIR!

---

### Issue 3: You're Reinventing R1CS/AIR

> "What you're building is essentially an R1CS or AIR constraint system on top of Ligerito. But Ligerito is a **polynomial commitment scheme**, not a full zkSNARK system."

**What you need**:
- **Constraint system**: R1CS (rank-1 constraint system) or AIR (algebraic intermediate representation)
- **Polynomial IOP**: Converts constraints to polynomial equations
- **Polynomial commitment**: Ligerito (commits to the polynomials)

**You're trying to do all three in the "arithmetization" step**, which is fragile.

**Recommendation**:
1. Use a proper AIR framework (like Plonky2's AIR or Winterfell)
2. Compile AIR to polynomials
3. Commit those polynomials with Ligerito
4. Don't try to encode constraints directly in the polynomial!

---

## From Henry de Valence (Penumbra, ZCash Sapling)

### On Memory Consistency

> "You hand-waved memory consistency with 'we need a permutation argument'. This is **the hardest part** of building a zkVM!"

**The problem**:

For a memory read at time `t` from address `a`:
```
CONSTRAINT: value_read = last_write(a, t' < t).value
```

This requires:
1. **Sorting** memory operations by (address, time)
2. **Proving** the sorted list is a permutation of the original
3. **Checking** each read matches the previous write (in sorted order)

**Approaches**:

### Option 1: Permutation Argument (Plonk-style)

```rust
// Two polynomials:
P_exec(i) = memory operations in execution order
P_sort(i) = same operations, sorted by (addr, time)

// Prove they're permutations:
// ∏_i (γ + P_exec[i]) = ∏_i (γ + P_sort[i])

// This requires:
// - Grand product argument
// - Multiset equality check
// - O(n) constraints for n memory operations
```

**Cost**: ~3-5x overhead on the entire trace!

### Option 2: Memory Checking via Address Lookup

Use a lookup argument (Plookup):
```rust
// Table of all writes: (addr, time, value)
// For each read: lookup (addr, time_prev, value) in table

// This requires a lookup argument (not in Ligerito!)
```

### Option 3: Small Memory Assumption

For small programs (<1MB memory), include full memory in each step:
```rust
pub struct ExecutionStep {
    pub pc: u32,
    pub regs: [u32; 13],
    pub memory: [u32; 256*1024],  // 1MB memory snapshot
}
```

**Cost**: 1MB × trace_length storage! Infeasible for long traces.

---

**Reality Check**:

> "Every existing zkVM (RISC Zero, SP1, Jolt, Valida) spent **months** getting memory consistency right. This is not a detail - it's the core complexity."

**Recommendation**:
- Start with **no memory** (registers only)
- Or **read-only memory** (just the binary, no writes)
- Add full memory consistency later using proven techniques (Plonk permutation)

---

## From Daniel Micay (GrapheneOS, Security Expert)

### On Security Assumptions

> "You're building a zkVM where an attacker controls the prover. Let me enumerate the attack surface."

**Attack Surface**:

1. **Prover provides fake binary**
   - ✅ Mitigated by hash check (if implemented correctly)

2. **Prover skips instructions**
   - ✅ Mitigated by PC continuity constraints

3. **Prover fakes ALU results**
   - ✅ Mitigated by ALU correctness constraints

4. **Prover manipulates memory**
   - ❌ **NOT MITIGATED** - no memory consistency!

5. **Prover exploits polynomial commitment soundness error**
   - ⚠️ Ligerito has ~2^-100 soundness error
   - Acceptable for most uses

6. **Verifier implementation bugs**
   - ❌ **CRITICAL** - you need formal verification of the verifier!

**Side Channel Attacks**:

The C verifier running in PolkaVM could have:
- **Timing attacks**: Verification time leaks information about the proof
- **Memory access patterns**: Observable by the host
- **Branch prediction**: Microarchitectural leaks

For JAM/Polkadot, this might leak information about parachains!

**Recommendation**:
- Use constant-time verification algorithms
- Avoid data-dependent branches
- Consider using Rust with `subtle` crate instead of C

---

### On Complexity and Attack Surface

> "Every line of code is a potential vulnerability. Let's count the complexity."

**Components**:
1. PolkaVM interpreter (existing, ~10k LOC)
2. Trace extraction (new, ~1k LOC)
3. Arithmetization (new, ~5k LOC)
4. Hash circuit (new, ~1k LOC)
5. Constraint checking (new, ~3k LOC)
6. Ligerito prover (existing, ~10k LOC)
7. C verifier (new, ~2k LOC)

**Total new attack surface**: ~12k LOC of custom cryptographic code!

**Each component needs**:
- Unit tests
- Fuzzing
- Formal verification (for critical paths)
- Security audit

**Budget**: $200k+ for professional security audit.

---

## From Gavin Wood (Polkadot/JAM, Systems Architect)

### On JAM Integration

> "You want this to integrate with JAM. Let's check if it actually fits the model."

**JAM Work Reports Structure**:
```rust
pub struct WorkReport {
    pub core_index: u16,
    pub authorizer: Ed25519Public,
    pub work_result: WorkResult,
    pub auth_proof: Proof,  // <- Your Ligerito proof goes here
}
```

**Requirements**:
1. **Deterministic gas metering**: JAM needs predictable costs
2. **Bounded proof size**: Max ~1MB per work report
3. **Fast verification**: <100ms on validator hardware
4. **Erasure coding compatible**: Proof must be chunkable

**Your Ligerito proof**:
- Size: ~150 KB ✅
- Verification: ~100ms ✅
- Deterministic: ⚠️ Depends on query randomness
- Erasure coding: ❓ Unclear

**Problem: Fiat-Shamir Randomness**

```rust
// Ligerito uses Fiat-Shamir for query selection:
let queries = transcript.challenge_queries(num_queries);

// Different transcript implementations could give different queries!
// This breaks determinism!
```

**Fix**: Use a **canonical transcript** (like Merlin or STROBE) specified in JAM.

---

### On Performance at Scale

> "JAM will have 1000+ cores, each submitting work reports every 6 seconds. Can your system handle this?"

**Throughput analysis**:

| Component | Latency | Throughput (parallel) |
|-----------|---------|----------------------|
| PolkaVM execution | ~100ms | 10 cores/sec |
| Ligerito proving | ~5s | 0.2 cores/sec |
| Verification | ~100ms | 10 cores/sec |

**Bottleneck**: Proving!

At 1000 cores × 1 proof/6sec = 166 proofs/sec needed.

With 0.2 proofs/sec per CPU core, you need:
```
166 / 0.2 = 830 CPU cores just for proving!
```

**This doesn't scale!**

**Solutions**:
1. **Proof aggregation**: Batch multiple work reports into one proof
2. **Specialized hardware**: GPU acceleration (you have WebGPU support)
3. **Optimistic execution**: Only prove on challenges
4. **Recursive composition**: Amortize proving cost

---

### On the Bigger Picture

> "You're trying to solve multiple hard problems simultaneously. Let's decompose."

**Problem 1**: General-purpose zkVM
- Status: Architecture proposed, not implemented
- Complexity: Very high
- Timeline: 6-12 months

**Problem 2**: JAM integration
- Status: Conceptual
- Complexity: High (determinism, gas metering)
- Timeline: 3-6 months

**Problem 3**: Performance optimization
- Status: Basic Ligerito works, needs GPU
- Complexity: Medium
- Timeline: 2-3 months

**Recommendation**: **Pick ONE problem to solve first!**

**Suggested roadmap**:
1. **Phase 1** (2 months): Get Ligerito working well in JAM
   - Just polynomial commitments, no zkVM yet
   - Prove simple computation (data availability, etc.)
   - Integrate with validators

2. **Phase 2** (3 months): Prove simple PVM programs (no memory)
   - Register-only computations
   - No memory consistency (too complex)
   - Prove it works end-to-end

3. **Phase 3** (6 months): Full zkVM with memory
   - Implement permutation argument
   - Add memory consistency
   - Full RV32EM support

**Don't try to boil the ocean!**

---

## Consolidated Issues

### Critical (Must Fix)

1. ❌ **Hash function is not collision-resistant** (Guillermo)
   - Use SHA256, Poseidon, or BLAKE3
   - Not a simple polynomial!

2. ❌ **Memory consistency not implemented** (Henry)
   - Start with no memory or read-only
   - Add permutation argument later

3. ❌ **Sumcheck doesn't check constraints as claimed** (Guillermo)
   - Need proper R1CS/AIR framework
   - Can't just push constraints to polynomial

4. ❌ **No formal security analysis** (Daniel)
   - Need soundness proof
   - Attack surface analysis
   - Security audit

### Important (Should Fix)

5. ⚠️ **Proving doesn't scale** (Gavin)
   - Need proof aggregation or GPU
   - 830 cores for JAM is unacceptable

6. ⚠️ **Fiat-Shamir not deterministic** (Gavin)
   - Need canonical transcript
   - Specify in detail

7. ⚠️ **C verifier has side channels** (Daniel)
   - Use constant-time algorithms
   - Consider Rust instead

### Nice to Have

8. ℹ️ **Too many parallel efforts** (Gavin)
   - Focus on one problem first
   - Iterative development

---

## Verdict

**Guillermo**: "The math needs work. You're using Ligerito wrong."

**Henry**: "Memory consistency is handwaved. This won't work without a permutation argument."

**Daniel**: "12k LOC of new crypto code without formal verification? I'm scared."

**Gavin**: "Interesting idea, but prove it works on something simple first. Don't build the whole zkVM at once."

---

## Recommended Next Steps

1. **Use SHA256 for hash verification** (not a toy hash)
2. **Start with register-only PVM** (no memory yet)
3. **Implement proper constraint system** (R1CS or AIR)
4. **Write formal spec** before implementation
5. **Build incrementally**: registers → read-only memory → full memory
6. **Security audit** after each phase

**Reality**: This is a 12-18 month project, not a weekend hack!

But the core idea is sound - Ligerito + PVM could work. Just needs proper engineering!
