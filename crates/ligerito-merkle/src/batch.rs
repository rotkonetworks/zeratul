// src/batch.rs
use bytemuck::Pod;
use crate::{CompleteMerkleTree, MerkleRoot, Hash, hash_leaf, hash_siblings};
use rayon::prelude::*;

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

/// Create a batched proof for multiple queries (0-based indices)
pub fn prove_batch(tree: &CompleteMerkleTree, queries: &[usize]) -> BatchedMerkleProof {
    let mut siblings = Vec::new();
    let depth = tree.get_depth();

    if depth == 0 || queries.is_empty() {
        return BatchedMerkleProof { siblings };
    }

    // Work with 0-based indices directly
    let mut queries_buff = queries.to_vec();
    let mut queries_cnt = queries_buff.len();

    // Process each layer
    for layer_idx in 0..depth {
        queries_cnt = ith_layer(
            &tree.layers[layer_idx],
            queries_cnt,
            &mut queries_buff,
            &mut siblings,
        );
    }

    BatchedMerkleProof { siblings }
}

/// Verify a batched proof (0-based indices)
pub fn verify_batch<T: Pod + Send + Sync>(
    root: &MerkleRoot,
    proof: &BatchedMerkleProof,
    depth: usize,
    leaves: &[T],
    leaf_indices: &[usize],
) -> bool {
    let Some(expected_root) = root.root else {
        return false;
    };

    if depth == 0 {
        // Single leaf tree
        if leaves.len() == 1 && leaf_indices.len() == 1 && leaf_indices[0] == 0 {
            let leaf_hash = hash_leaf(&leaves[0]);
            return leaf_hash == expected_root;
        }
        return false;
    }

    // Hash leaves in parallel
    let mut layer: Vec<Hash> = leaves.par_iter()
        .map(hash_leaf)
        .collect();

    // Work with 0-based indices directly
    let mut queries = leaf_indices.to_vec();

    let mut curr_cnt = queries.len();
    let mut proof_cnt = 0;

    // Process each layer
    for _ in 0..depth {
        let (next_cnt, next_proof_cnt) = verify_ith_layer(
            &mut layer,
            &mut queries,
            curr_cnt,
            &proof.siblings,
            proof_cnt,
        );

        curr_cnt = next_cnt;
        proof_cnt = next_proof_cnt;
    }

    // Check if we've consumed all proof elements and reached the expected root
    curr_cnt == 1 && proof_cnt == proof.siblings.len() && layer[0] == expected_root
}

fn ith_layer(
    current_layer: &[Hash],
    queries_len: usize,
    queries: &mut Vec<usize>,
    proof: &mut Vec<Hash>,
) -> usize {
    let mut next_queries_len = 0;
    let mut i = 0;

    while i < queries_len {
        let query = queries[i];
        let sibling = query ^ 1;

        queries[next_queries_len] = query >> 1;
        next_queries_len += 1;

        if i == queries_len - 1 {
            proof.push(current_layer[sibling]);
            break;
        }

        if query % 2 != 0 {
            proof.push(current_layer[sibling]);
            i += 1;
        } else {
            if queries[i + 1] != sibling {
                proof.push(current_layer[sibling]);
                i += 1;
            } else {
                i += 2;
            }
        }
    }

    next_queries_len
}

fn verify_ith_layer(
    layer: &mut Vec<Hash>,
    queries: &mut Vec<usize>,
    curr_cnt: usize,
    proof: &[Hash],
    mut proof_cnt: usize,
) -> (usize, usize) {
    let mut next_cnt = 0;
    let mut i = 0;

    while i < curr_cnt {
        let query = queries[i];
        let sibling = query ^ 1;

        queries[next_cnt] = query >> 1;
        next_cnt += 1;

        if i == curr_cnt - 1 {
            proof_cnt += 1;
            let pp = proof.get(proof_cnt - 1).copied().unwrap_or_default();
            layer[next_cnt - 1] = if query % 2 != 0 {
                hash_siblings(&pp, &layer[i])
            } else {
                hash_siblings(&layer[i], &pp)
            };
            break;
        }

        if query % 2 != 0 {
            proof_cnt += 1;
            let pp = proof.get(proof_cnt - 1).copied().unwrap_or_default();
            layer[next_cnt - 1] = hash_siblings(&pp, &layer[i]);
            i += 1;
        } else {
            if queries[i + 1] != sibling {
                proof_cnt += 1;
                let pp = proof.get(proof_cnt - 1).copied().unwrap_or_default();
                layer[next_cnt - 1] = hash_siblings(&layer[i], &pp);
                i += 1;
            } else {
                layer[next_cnt - 1] = hash_siblings(&layer[i], &layer[i + 1]);
                i += 2;
            }
        }
    }

    (next_cnt, proof_cnt)
}
