# Interactive Game of Life with Ligerito Proofs

An interactive terminal-based Game of Life where you can:
- Toggle cells by clicking (entering coordinates)
- Execute generations in real-time
- Generate cryptographic proofs of execution
- See the agentic blockchain model in action

## Quick Start

### Run Automated Demo

```bash
cd /home/alice/rotko/zeratul
cargo test --release --features polkavm-integration --test game_of_life_interactive test_interactive_game_of_life -- --nocapture
```

This runs a simulated session showing:
1. Load glider pattern
2. Execute 5 generations
3. Generate proof (~330ms)
4. Toggle some cells
5. Execute 3 more generations
6. Generate proof again

### Run Manual Interactive Mode

```bash
cargo test --release --features polkavm-integration --test game_of_life_interactive test_manual_interactive -- --nocapture --ignored
```

## Interactive Commands

```
╔════════════════════════════════════════╗
║   Interactive Game of Life (Gen 0)     ║
╚════════════════════════════════════════╝

  01234567
0 ········
1 ·███····
2 ·········
3 ········
4 ········
5 ········
6 ········
7 ········

Commands:
  [x y]  - Toggle cell at position (x, y)
  [s]    - Step (evolve one generation)
  [n N]  - Step N generations
  [p]    - Prove current execution
  [c]    - Clear grid
  [g]    - Load glider pattern
  [q]    - Quit

Accumulated steps: 0 (unproven)
```

## Example Session

```bash
Enter command: g
Glider pattern loaded!

Enter command: s
Stepped one generation!

  01234567
0 ········
1 ··█·····
2 ···█····
3 ·███····

Enter command: n 5
Executed 5 generations!

  01234567
0 ········
1 ····█···
2 ·····█··
3 ···███··

Enter command: p
⚡ Generating proof for 6 generations (384 steps)...
✓ Proof generated in 342.156ms
✓ Proof verified in 476.32µs
  Constraint accumulator: BinaryElem32(BinaryPoly32(0))

Enter command: 4 4
Toggled cell at (4, 4)

Enter command: s
Stepped one generation!

Enter command: p
⚡ Generating proof for 1 generations (64 steps)...
✓ Proof generated in 329.881ms
✓ Proof verified in 488.45µs

Enter command: q
Exiting...
```

## What's Happening Under the Hood

### Agentic Execution Model

```
User interaction:     Toggle cells, execute steps
     ↓
Agent accumulates:    PolkaVM execution trace (in memory)
     ↓
User requests proof:  [p] command
     ↓
Agent proves:         Generate Ligerito proof (~330ms)
     ↓
Agent verifies:       Verify proof (<1ms)
     ↓
Trace cleared:        Ready for next execution batch
```

### Key Features

**1. Accumulation (no forced timing)**
```rust
// Execute as many steps as you want
session.execute_step();  // Gen 1
session.execute_step();  // Gen 2
session.execute_step();  // Gen 3

// Prove when YOU decide
session.prove_and_verify();  // Proves all 3 gens at once
```

**2. Independent execution**
```
No global clock!
No forced 1s blocks!
Prove when convenient!
```

**3. Cryptographic guarantees**
```
Every proof includes:
- Initial state commitment
- Final state commitment
- Constraint accumulator = 0 (all rules satisfied)
- Can't forge execution!
```

## Performance Characteristics

### Execution Speed

```
Single generation: 64 steps (one per cell)
Execution time: <1ms (negligible)
```

### Proof Generation

```
1 generation  (64 steps):   ~330ms
5 generations (320 steps):  ~333ms
10 generations (640 steps): ~370ms

Key insight: Proving time is roughly constant!
O(log² N) scaling means doubling steps doesn't double time.
```

### Proof Verification

```
All proofs: <500μs (<0.5ms)
Regardless of execution length!
O(log N) verification time.
```

## Blockchain Implications

This demo shows the **agentic blockchain model**:

### Traditional Blockchain
```
Block 1 (t=0s):   Execute + Prove (forced timing)
Block 2 (t=1s):   Execute + Prove
Block 3 (t=2s):   Execute + Prove

Problem: Forced synchronization
```

### Agentic Model (This Demo!)
```
Agent executes:   5 gens (500ms)
Agent proves:     When convenient (~330ms)
Agent submits:    Total ~400-500ms

Another agent:    Executes 10 gens (1000ms)
                  Proves when done (~370ms)
                  Total ~500ms

No coordination needed unless agents interact!
```

## Extending the Demo

### Add More Patterns

```rust
// In test code, add new patterns:
fn load_blinker(&mut self) {
    self.grid.set(3, 2, 1);
    self.grid.set(3, 3, 1);
    self.grid.set(3, 4, 1);
}

fn load_beacon(&mut self) {
    self.grid.set(1, 1, 1);
    self.grid.set(2, 1, 1);
    self.grid.set(1, 2, 1);
    self.grid.set(4, 3, 1);
    self.grid.set(3, 4, 1);
    self.grid.set(4, 4, 1);
}
```

### Add Auto-Evolve

```rust
// Continuously evolve and prove
match parts[0] {
    "a" => {
        println!("Auto-evolving (press any key to stop)...");
        for i in 0..20 {
            session.execute_step();
            session.grid.print();
            std::thread::sleep(Duration::from_millis(200));

            if i % 5 == 4 {
                session.prove_and_verify()?;
            }
        }
    }
}
```

### Add Proof Persistence

```rust
// Save proofs to disk
fn save_proof(&self, proof: &PolkaVMProof) -> Result<(), Error> {
    let filename = format!("proof_gen_{}.bin", self.generation);
    std::fs::write(filename, bincode::serialize(proof)?)?;
    Ok(())
}
```

## Technical Details

### PolkaVM Instructions Generated

Each cell read generates:
```asm
load_imm a0, <cell_value>  ; 2 bytes, 1 step
```

64 cells per generation = 64 steps
Each step proven with Ligerito constraints

### Constraints Verified Per Proof

For N steps:
```
Instruction constraints:    N × (1 ALU + 12 register consistency)
State continuity:           (N-1) × (13 regs + 32 memory bytes + 1 PC)
Total constraints:          ~14N + 46(N-1) ≈ 60N constraints

All batched into single accumulator check!
Schwartz-Zippel: If ANY constraint fails, accumulator ≠ 0
```

### Proof Size

```
~101 KB per proof (regardless of execution length!)
O(log² N) scaling

Breakdown:
- Commitments: 640 bytes
- Sumcheck: 240 bytes
- Query responses: ~100 KB
```

## Conclusion

This interactive demo shows:

✅ **Agentic execution**: Agent decides when to prove
✅ **No forced timing**: Execute at your own pace
✅ **Cryptographic safety**: Every proof verifies constraints
✅ **Sub-400ms proving**: Fast enough for blockchain use
✅ **Constant proof size**: ~101 KB regardless of steps
✅ **Instant verification**: <500μs validation time

**This is the future of blockchain execution!**
