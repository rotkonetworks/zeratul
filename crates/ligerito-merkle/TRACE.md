# Merkle Tree Trace Function

Implementation of the trace function from JAM Graypaper for creating inclusion proofs.

## Overview

The `trace()` function returns each opposite node from top to bottom as the tree is navigated to arrive at a leaf at a given index. This follows the JAM Graypaper specification for creating justifications of data inclusion.

## Graypaper Definition

From [Graypaper Section "Merklization"](https://graypaper.fluffylabs.dev/):

> We also define the *trace* function T, which returns each opposite node from top to bottom as the tree is navigated to arrive at some leaf corresponding to the item of a given index into the sequence. It is useful in creating justifications of data inclusion.

## Usage

```rust
use ligerito_merkle::{build_merkle_tree, Hash};

// Build a tree with 8 leaves
let leaves: Vec<u64> = (0..8).collect();
let tree = build_merkle_tree(&leaves);

// Get trace for leaf at index 3
let trace: Vec<Hash> = tree.trace(3);

// trace contains sibling hashes from root to leaf (top to bottom)
// trace[0] = sibling at top level
// trace[1] = sibling at middle level  
// trace[2] = sibling at leaf level
```

## Verification

You can verify a leaf using its trace:

```rust
use ligerito_merkle::{hash_leaf, hash_siblings};

let index = 3;
let trace = tree.trace(index);
let leaf_hash = hash_leaf(&leaves[index]);

// Reconstruct root from leaf + trace (bottom to top)
let mut current_hash = leaf_hash;
let mut current_index = index;

for sibling_hash in trace.iter().rev() {
    if current_index % 2 == 0 {
        // Current is left child
        current_hash = hash_siblings(&current_hash, sibling_hash);
    } else {
        // Current is right child
        current_hash = hash_siblings(sibling_hash, &current_hash);
    }
    current_index /= 2;
}

assert_eq!(current_hash, tree.get_root().root.unwrap());
```

## Comparison with BatchedMerkleProof

Ligerito uses two approaches for Merkle proofs:

### 1. Trace (Graypaper-style)
- **Format**: Vector of sibling hashes from root to leaf
- **Use case**: Single-leaf verification, PolkaVM integration
- **Size**: `depth * 32 bytes`
- **Pros**: Simple, matches JAM spec, easy to verify in zkVM
- **Cons**: One trace per leaf (not optimized for batch queries)

### 2. BatchedMerkleProof (Ligerito-style)
- **Format**: Deduplicated siblings for multiple queries
- **Use case**: Multi-leaf verification (148 queries per round)
- **Size**: `num_unique_siblings * 32 bytes` (much smaller for batches)
- **Pros**: Optimized for batch verification, smaller proofs
- **Cons**: More complex reconstruction logic

## Example Tree

For a 4-leaf tree with leaves `[0, 1, 2, 3]`:

```
       root
      /    \
    h01    h23
    / \    / \
   h0 h1  h2 h3
```

Trace for index 2 (leaf h2):
1. Navigate from root → h23 (go right), opposite is **h01**
2. Navigate from h23 → h2 (go left), opposite is **h3**

Result: `trace(2) = [h01, h3]` (top to bottom)

## Integration with PolkaVM

The trace format is ideal for PolkaVM verification because:

1. **Simple iteration**: Verifier walks through trace array linearly
2. **Minimal memory**: No need to track complex batched proof state
3. **JAM-compatible**: Matches work-report `authtrace` format

### PolkaVM Pseudocode

```rust
// In PolkaVM guest program (no_std)
fn verify_inclusion(
    root: Hash,
    leaf_data: &[u8],
    index: usize,
    trace: &[Hash],
) -> bool {
    let mut current = hash_leaf(leaf_data);
    let mut idx = index;
    
    for sibling in trace.iter().rev() {
        current = if idx % 2 == 0 {
            hash_siblings(&current, sibling)
        } else {
            hash_siblings(sibling, &current)
        };
        idx /= 2;
    }
    
    current == root
}
```

## Implementation Notes

- **Zero-based indexing**: Unlike some Merkle libraries, we use 0-based leaf indices
- **Top-to-bottom order**: Trace returns siblings from root to leaf (reversed from typical proof construction)
- **Power-of-two requirement**: Tree must have 2^n leaves
- **No padding**: Empty slots are not allowed

## References

- [JAM Graypaper - Merklization](https://graypaper.fluffylabs.dev/)
- [JAM Work Reports - Authorization Trace](https://graypaper.fluffylabs.dev/)
- [jamit Julia implementation](https://github.com/eigerco/jamit/blob/main/src/crypto/mmr.jl)
- [typeberry TypeScript implementation](https://github.com/eigerco/typeberry/)
