// merkle witness construction for orchard spends (ported from zcli/src/witness.rs)
//
// uses tree state checkpoints to avoid replaying from orchard activation.
// the JS worker fetches blocks and tree states, passes raw data here.

use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::witness::IncrementalWitness;
use incrementalmerkletree::Hashable;
use orchard::note::ExtractedNoteCommitment;
use orchard::tree::{Anchor, MerkleHashOrchard, MerklePath};
use std::collections::HashMap;

/// Deserialize a lightwalletd/zcashd orchard frontier into a CommitmentTree.
///
/// Wire format (zcash_primitives CommitmentTree serialization):
///   [Option<Hash>] left
///   [Option<Hash>] right
///   [CompactSize]  parent_count
///   [Option<Hash>] * parent_count
///
/// Option encoding: 0x00 = None, 0x01 = Some followed by 32 bytes.
pub fn deserialize_tree(data: &[u8]) -> Result<CommitmentTree<MerkleHashOrchard, 32>, String> {
    if data.is_empty() {
        return Ok(CommitmentTree::empty());
    }

    let mut pos = 0;

    let read_option = |pos: &mut usize| -> Result<Option<MerkleHashOrchard>, String> {
        if *pos >= data.len() {
            return Err("frontier truncated reading option tag".into());
        }
        if data[*pos] == 0x01 {
            if *pos + 33 > data.len() {
                return Err("frontier truncated reading hash".into());
            }
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&data[*pos + 1..*pos + 33]);
            *pos += 33;
            Option::from(MerkleHashOrchard::from_bytes(&bytes))
                .map(Some)
                .ok_or_else(|| "invalid frontier hash".to_string())
        } else {
            *pos += 1;
            Ok(None)
        }
    };

    let left = read_option(&mut pos)?;
    let right = read_option(&mut pos)?;

    // read CompactSize parent count
    if pos >= data.len() {
        return CommitmentTree::from_parts(left, right, vec![])
            .map_err(|_| "invalid frontier structure (no parents)".to_string());
    }
    let parent_count = read_compact_size(data, &mut pos)?;

    let mut parents = Vec::with_capacity(parent_count as usize);
    for _ in 0..parent_count {
        parents.push(read_option(&mut pos)?);
    }

    let n_parents = parents.len();
    let has_left = left.is_some();
    let has_right = right.is_some();
    CommitmentTree::from_parts(left, right, parents).map_err(|_| {
        format!(
            "invalid frontier structure (left={} right={} parents={})",
            has_left, has_right, n_parents,
        )
    })
}

fn read_compact_size(data: &[u8], pos: &mut usize) -> Result<u64, String> {
    if *pos >= data.len() {
        return Err("compact size: truncated".into());
    }
    let first = data[*pos];
    *pos += 1;
    match first {
        0x00..=0xfc => Ok(first as u64),
        0xfd => {
            if *pos + 2 > data.len() {
                return Err("compact size: truncated u16".into());
            }
            let v = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
            *pos += 2;
            Ok(v as u64)
        }
        0xfe => {
            if *pos + 4 > data.len() {
                return Err("compact size: truncated u32".into());
            }
            let v = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
            *pos += 4;
            Ok(v as u64)
        }
        0xff => {
            if *pos + 8 > data.len() {
                return Err("compact size: truncated u64".into());
            }
            let v = u64::from_le_bytes([
                data[*pos],
                data[*pos + 1],
                data[*pos + 2],
                data[*pos + 3],
                data[*pos + 4],
                data[*pos + 5],
                data[*pos + 6],
                data[*pos + 7],
            ]);
            *pos += 8;
            Ok(v as u64)
        }
    }
}

/// Compute the tree size from frontier data.
pub fn compute_frontier_tree_size(data: &[u8]) -> Result<u64, String> {
    let tree = deserialize_tree(data)?;
    Ok(tree.size() as u64)
}

/// Compute the tree root from frontier data.
pub fn compute_tree_root(data: &[u8]) -> Result<[u8; 32], String> {
    let tree = deserialize_tree(data)?;
    Ok(tree.root().to_bytes())
}

/// A compact block action - just the cmx commitment.
#[derive(serde::Deserialize)]
pub struct CompactAction {
    pub cmx_hex: String,
}

/// A compact block with height and actions.
#[derive(serde::Deserialize)]
pub struct CompactBlockData {
    pub height: u32,
    pub actions: Vec<CompactAction>,
}

/// Result of merkle path computation.
#[derive(serde::Serialize)]
pub struct MerklePathResult {
    pub anchor_hex: String,
    pub paths: Vec<SerializedMerklePath>,
}

/// A serialized merkle path (32 siblings of 32 bytes each).
#[derive(serde::Serialize)]
pub struct SerializedMerklePath {
    pub position: u64,
    pub path: Vec<PathElement>,
}

#[derive(serde::Serialize)]
pub struct PathElement {
    pub hash: String,
}

/// Build merkle paths for specified note positions.
///
/// Takes a tree state checkpoint, compact blocks to replay, and note positions.
/// Returns the anchor and merkle paths for each note position.
pub fn build_merkle_paths_inner(
    tree_state_hex: &str,
    compact_blocks: &[CompactBlockData],
    note_positions: &[u64],
    _anchor_height: u32,
) -> Result<MerklePathResult, String> {
    // deserialize checkpoint tree
    let tree_bytes =
        hex::decode(tree_state_hex).map_err(|e| format!("invalid tree state hex: {}", e))?;
    let mut tree = deserialize_tree(&tree_bytes)?;

    let mut position_counter = tree.size() as u64;

    // build position -> index map
    let mut position_map: HashMap<u64, usize> = HashMap::new();
    for (i, &pos) in note_positions.iter().enumerate() {
        position_map.insert(pos, i);
    }

    let mut witnesses: Vec<Option<IncrementalWitness<MerkleHashOrchard, 32>>> =
        vec![None; note_positions.len()];

    // replay blocks
    for block in compact_blocks {
        for action in &block.actions {
            let cmx_bytes = hex::decode(&action.cmx_hex)
                .map_err(|e| format!("invalid cmx hex at height {}: {}", block.height, e))?;

            let hash = if cmx_bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&cmx_bytes);
                let cmx = ExtractedNoteCommitment::from_bytes(&arr);
                if bool::from(cmx.is_some()) {
                    MerkleHashOrchard::from_cmx(&cmx.unwrap())
                } else {
                    MerkleHashOrchard::empty_leaf()
                }
            } else {
                MerkleHashOrchard::empty_leaf()
            };

            tree.append(hash.clone())
                .map_err(|_| "merkle tree full".to_string())?;

            // snapshot witness at note positions
            if let Some(&idx) = position_map.get(&position_counter) {
                witnesses[idx] = IncrementalWitness::from_tree(tree.clone());
            }

            // update existing witnesses with new leaf
            for w in witnesses.iter_mut().flatten() {
                if w.witnessed_position()
                    < incrementalmerkletree::Position::from(position_counter)
                {
                    w.append(hash.clone())
                        .map_err(|_| "witness tree full".to_string())?;
                }
            }

            position_counter += 1;
        }
    }

    // extract anchor and paths
    let anchor_root = tree.root();
    let anchor = Anchor::from(anchor_root);

    let mut paths = Vec::with_capacity(note_positions.len());
    for (i, w) in witnesses.into_iter().enumerate() {
        let witness = w.ok_or_else(|| {
            format!(
                "note at position {} not found in tree replay",
                note_positions[i],
            )
        })?;

        let imt_path = witness.path().ok_or_else(|| {
            format!(
                "failed to compute merkle path for note at position {}",
                note_positions[i],
            )
        })?;

        let merkle_path = MerklePath::from(imt_path);
        let auth_path = merkle_path.auth_path();
        let position = u64::from(merkle_path.position());

        let path_elements: Vec<PathElement> = auth_path
            .iter()
            .map(|h| PathElement {
                hash: hex::encode(h.to_bytes()),
            })
            .collect();

        paths.push(SerializedMerklePath {
            position,
            path: path_elements,
        });
    }

    Ok(MerklePathResult {
        anchor_hex: hex::encode(anchor.to_bytes()),
        paths,
    })
}
