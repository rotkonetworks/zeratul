use bytemuck::Pod;
use crate::{CompleteMerkleTree, MerkleRoot, Hash, hash_leaf, hash_siblings};

/// Batched Merkle proof
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BatchedMerkleProof {
    pub siblings: Vec<Hash>,
}

impl BatchedMerkleProof {
    pub fn size_of(&self) -> usize {
        self.siblings.len() * 32
    }
}

/// Create a batched proof for multiple queries
pub fn prove_batch(tree: &CompleteMerkleTree, queries: &[usize]) -> BatchedMerkleProof {
    let mut siblings = Vec::new();
    let depth = tree.get_depth();

    // Make mutable copy of queries
    let mut queries_buff: Vec<usize> = queries.to_vec();
    let mut queries_cnt = queries_buff.len();

    // Process each layer
    for layer_idx in 0..depth {
        queries_cnt = process_layer(
            &tree.layers[layer_idx],
            &mut queries_buff[..queries_cnt],
            &mut siblings,
        );
    }

    BatchedMerkleProof { siblings }
}

/// Process one layer of the tree
fn process_layer(
    layer: &[Hash],
    queries: &mut [usize],
    proof: &mut Vec<Hash>,
) -> usize {
    let mut next_cnt = 0;
    let mut i = 0;

    while i < queries.len() {
        let query = queries[i];
        let sibling = query ^ 1;

        // Update query for next layer
        queries[next_cnt] = query >> 1;
        next_cnt += 1;

        if i == queries.len() - 1 {
            // Last query, always include sibling
            proof.push(layer[sibling]);
            break;
        }

        if query % 2 != 0 {
            // Odd query, include sibling
            proof.push(layer[sibling]);
            i += 1;
        } else {
            // Even query
            if queries[i + 1] != sibling {
                // Next query is not the sibling, include it
                proof.push(layer[sibling]);
                i += 1;
            } else {
                // Next query is the sibling, skip it
                i += 2;
            }
        }
    }

    next_cnt
}

/// Verify a batched proof
pub fn verify_batch<T: Pod>(
    root: &MerkleRoot,
    proof: &BatchedMerkleProof,
    depth: usize,
    leaves: &[T],
    leaf_indices: &[usize],
) -> bool {
    let Some(expected_root) = root.root else {
        return false;
    };

    // Hash leaves and prepare for verification
    let mut layer: Vec<Hash> = leaves.iter()
        .map(hash_leaf)
        .collect();

    // Make mutable copy of indices
    let mut queries: Vec<usize> = leaf_indices.to_vec();
    let mut curr_cnt = queries.len();
    let mut proof_idx = 0;

    // Process each layer
    for _ in 0..depth {
        let (next_cnt, next_proof_idx) = verify_layer(
            &mut layer,
            &mut queries[..curr_cnt],
            &proof.siblings,
            proof_idx,
        );

        curr_cnt = next_cnt;
        proof_idx = next_proof_idx;
    }

    // Check if we've consumed all proof elements and reached the expected root
    proof_idx == proof.siblings.len() && layer[0] == expected_root
}

/// Verify one layer
fn verify_layer(
    layer: &mut Vec<Hash>,
    queries: &mut [usize],
    proof: &[Hash],
    mut proof_idx: usize,
) -> (usize, usize) {
    let mut next_cnt = 0;
    let mut i = 0;

    while i < queries.len() {
        let query = queries[i];
        let sibling = query ^ 1;

        queries[next_cnt] = query >> 1;
        next_cnt += 1;

        if i == queries.len() - 1 {
            // Last element
            let sibling_hash = proof[proof_idx];
            proof_idx += 1;

            layer[next_cnt - 1] = if query % 2 != 0 {
                hash_siblings(&sibling_hash, &layer[i])
            } else {
                hash_siblings(&layer[i], &sibling_hash)
            };
            break;
        }

        if query % 2 != 0 {
            // Odd query
            let sibling_hash = proof[proof_idx];
            proof_idx += 1;

            layer[next_cnt - 1] = hash_siblings(&sibling_hash, &layer[i]);
            i += 1;
        } else {
            // Even query
            if queries[i + 1] != sibling {
                let sibling_hash = proof[proof_idx];
                proof_idx += 1;

                layer[next_cnt - 1] = hash_siblings(&layer[i], &sibling_hash);
                i += 1;
            } else {
                // Next query is sibling
                layer[next_cnt - 1] = hash_siblings(&layer[i], &layer[i + 1]);
                i += 2;
            }
        }
    }

    (next_cnt, proof_idx)
}
