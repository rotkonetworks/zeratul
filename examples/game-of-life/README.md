# Continuous Game of Life with Ligerito Proofs

This demonstrates continuous PVM execution using Conway's Game of Life.

## The Concept

Game of Life is perfect for demonstrating continuous execution:
- **Deterministic**: Each generation follows from the previous
- **State accumulation**: Grid state chains across generations
- **Infinite execution**: Runs continuously until halted
- **Verifiable**: Can prove correct evolution with Ligerito

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                 Game of Life (8x8 grid)                  │
├─────────────────────────────────────────────────────────┤
│  Generation 0:  Initial pattern (e.g., glider)          │
│       ↓                                                  │
│  Generation 1:  Apply Conway's rules → new state        │
│       ↓                                                  │
│  Generation 2:  Apply rules to state from gen 1         │
│       ↓                                                  │
│      ...                                                 │
│       ↓                                                  │
│  Generation N:  Continuous evolution                    │
└─────────────────────────────────────────────────────────┘

Each generation = ~100 PolkaVM steps (iterate grid, apply rules)
```

## Continuous Execution Model (JAM/Graypaper)

### Windowed Proving

Instead of proving all generations at once, we batch into windows:

```
Window 1: Generations 0-99    → Ligerito Proof 1
          State: grid[0] → grid[99]

Window 2: Generations 100-199 → Ligerito Proof 2
          Initial: grid[99] (from Window 1)
          Final: grid[199]

Window 3: Generations 200-299 → Ligerito Proof 3
          Initial: grid[199] (from Window 2)
          ...
```

### State Commitment

Each window's proof includes:
- `initial_state_hash`: Hash of grid at window start
- `final_state_hash`: Hash of grid at window end
- `proof`: Ligerito commitment to execution

Verification checks:
1. Each window's proof verifies ✓
2. `window[i].final_state == window[i+1].initial_state` ✓

## Memory Layout (PolkaVM)

```
Grid: 8x8 = 64 cells
Memory: [cell_0, cell_1, ..., cell_63]

Each cell: 0 (dead) or 1 (alive)

Address mapping:
  cell[x,y] = memory[y * 8 + x]
```

## Conway's Rules (in PolkaVM pseudocode)

```rust
for each cell (x, y):
    neighbors = count_live_neighbors(x, y)

    if cell[x,y] == 1:  // Alive
        if neighbors < 2: cell_next[x,y] = 0  // Dies (underpopulation)
        if neighbors > 3: cell_next[x,y] = 0  // Dies (overpopulation)
        else:             cell_next[x,y] = 1  // Survives
    else:  // Dead
        if neighbors == 3: cell_next[x,y] = 1  // Birth
        else:              cell_next[x,y] = 0  // Stays dead
```

## Implementation Plan

1. **Phase 1**: Simple PolkaVM program that runs Game of Life
   - Manual state setup (glider pattern)
   - Single generation step
   - Verify state evolution

2. **Phase 2**: Continuous execution
   - Run multiple generations
   - Track state through memory Merkle tree
   - Prove execution with Ligerito

3. **Phase 3**: Windowed proving
   - Batch generations into windows
   - Generate proof per window
   - Verify state chain

4. **Phase 4**: Visualization
   - Show grid evolution
   - Display proof generation
   - Benchmark performance

## Performance Target

- **Execution**: 100 generations/second (native speed)
- **Proving**: 1 window (100 gens) per second
- **Proof size**: ~10 KB (O(log² N))
- **Verification**: < 10ms per window

This demonstrates that Ligerito can keep up with continuous PVM execution!
