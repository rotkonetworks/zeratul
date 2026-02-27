// merkle witness construction for orchard spends
//
// uses GetTreeState RPC to restore a commitment tree checkpoint near the
// earliest note, then replays only the delta to anchor_height.  this avoids
// replaying the full 99M+ commitment history from orchard activation.

use std::collections::HashMap;

use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::witness::IncrementalWitness;
use incrementalmerkletree::Hashable;
use indicatif::{ProgressBar, ProgressStyle};
use orchard::note::ExtractedNoteCommitment;
use orchard::tree::{Anchor, MerkleHashOrchard, MerklePath};

use crate::client::{CompactBlock, ZidecarClient};
use crate::error::Error;
use crate::wallet::WalletNote;

/// retry compact block fetch with backoff
async fn retry_compact_blocks(
    client: &ZidecarClient,
    start: u32,
    end: u32,
) -> Result<Vec<CompactBlock>, Error> {
    let mut attempts = 0;
    loop {
        match client.get_compact_blocks(start, end).await {
            Ok(blocks) => return Ok(blocks),
            Err(e) => {
                attempts += 1;
                if attempts >= 5 {
                    return Err(e);
                }
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempts)).await;
            }
        }
    }
}

const BATCH_SIZE: u32 = 1000;

/// deserialize a lightwalletd/zcashd orchard frontier into a CommitmentTree.
///
/// wire format (zcash_primitives CommitmentTree serialization):
///   [Option<Hash>] left
///   [Option<Hash>] right
///   [CompactSize]  parent_count
///   [Option<Hash>] * parent_count
///
/// Option encoding: 0x00 = None, 0x01 = Some followed by 32 bytes.
/// CompactSize: 0x00-0xfc = 1 byte, 0xfd = u16 LE, 0xfe = u32 LE, 0xff = u64 LE.
fn deserialize_tree(data: &[u8]) -> Result<CommitmentTree<MerkleHashOrchard, 32>, Error> {
    if data.is_empty() {
        return Ok(CommitmentTree::empty());
    }

    let mut pos = 0;

    let read_option = |pos: &mut usize| -> Result<Option<MerkleHashOrchard>, Error> {
        if *pos >= data.len() {
            return Err(Error::Other("frontier truncated reading option tag".into()));
        }
        if data[*pos] == 0x01 {
            if *pos + 33 > data.len() {
                return Err(Error::Other("frontier truncated reading hash".into()));
            }
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&data[*pos + 1..*pos + 33]);
            *pos += 33;
            Option::from(MerkleHashOrchard::from_bytes(&bytes))
                .map(Some)
                .ok_or_else(|| Error::Other("invalid frontier hash".into()))
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
            .map_err(|_| Error::Other("invalid frontier structure (no parents)".into()));
    }
    let parent_count = read_compact_size(data, &mut pos)?;

    let mut parents = Vec::with_capacity(parent_count as usize);
    for _ in 0..parent_count {
        parents.push(read_option(&mut pos)?);
    }

    let n_parents = parents.len();
    let has_left = left.is_some();
    let has_right = right.is_some();
    CommitmentTree::from_parts(left, right, parents)
        .map_err(|_| Error::Other(format!(
            "invalid frontier structure (left={} right={} parents={})",
            has_left, has_right, n_parents,
        )))
}

fn read_compact_size(data: &[u8], pos: &mut usize) -> Result<u64, Error> {
    if *pos >= data.len() {
        return Err(Error::Other("compact size: truncated".into()));
    }
    let first = data[*pos];
    *pos += 1;
    match first {
        0x00..=0xfc => Ok(first as u64),
        0xfd => {
            if *pos + 2 > data.len() { return Err(Error::Other("compact size: truncated u16".into())); }
            let v = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
            *pos += 2;
            Ok(v as u64)
        }
        0xfe => {
            if *pos + 4 > data.len() { return Err(Error::Other("compact size: truncated u32".into())); }
            let v = u32::from_le_bytes([data[*pos], data[*pos+1], data[*pos+2], data[*pos+3]]);
            *pos += 4;
            Ok(v as u64)
        }
        0xff => {
            if *pos + 8 > data.len() { return Err(Error::Other("compact size: truncated u64".into())); }
            let v = u64::from_le_bytes([
                data[*pos], data[*pos+1], data[*pos+2], data[*pos+3],
                data[*pos+4], data[*pos+5], data[*pos+6], data[*pos+7],
            ]);
            *pos += 8;
            Ok(v as u64)
        }
    }
}

/// compute the tree size from frontier — parse to a CommitmentTree and use .size()
pub fn frontier_tree_size(data: &[u8]) -> Result<u64, Error> {
    let tree = deserialize_tree(data)?;
    Ok(tree.size() as u64)
}

/// find a checkpoint height whose tree size is just before `target_position`.
/// uses binary search over GetTreeState RPCs.
async fn find_checkpoint_height(
    client: &ZidecarClient,
    target_position: u64,
    activation: u32,
    tip: u32,
) -> Result<(u32, u64), Error> {
    let mut lo = activation;
    let mut hi = tip;
    let mut best_height = activation;
    let mut best_size = 0u64;

    while lo + 100 < hi {
        let mid = lo + (hi - lo) / 2;
        let (tree_hex, actual_height) = client.get_tree_state(mid).await?;
        let tree_bytes = hex::decode(&tree_hex)
            .map_err(|e| Error::Other(format!("invalid tree hex: {}", e)))?;
        let size = frontier_tree_size(&tree_bytes)?;

        if size <= target_position {
            best_height = actual_height;
            best_size = size;
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }

    Ok((best_height, best_size))
}

/// build merkle witnesses for a set of notes.
///
/// uses tree state checkpoints to avoid replaying from activation.
/// finds a checkpoint before the earliest note, deserializes the frontier,
/// then replays only the blocks between checkpoint and anchor_height.
pub async fn build_witnesses(
    client: &ZidecarClient,
    notes: &[WalletNote],
    anchor_height: u32,
    mainnet: bool,
    script: bool,
) -> Result<(Anchor, Vec<MerklePath>), Error> {
    let activation = if mainnet { 1_687_104 } else { 1_842_420 };

    if anchor_height < activation {
        return Err(Error::Other("anchor height before orchard activation".into()));
    }

    // find earliest note position — we need a checkpoint before this
    let earliest_position = notes.iter().map(|n| n.position).min()
        .ok_or_else(|| Error::Other("no notes to build witnesses for".into()))?;

    if !script {
        eprintln!("earliest note position: {}", earliest_position);
    }

    // find a checkpoint height whose tree size is just before our earliest note
    let (checkpoint_height, checkpoint_size) = find_checkpoint_height(
        client, earliest_position, activation, anchor_height,
    ).await?;

    if !script {
        eprintln!("checkpoint: height={} size={} (target={})",
            checkpoint_height, checkpoint_size, earliest_position);
    }

    // get tree state at checkpoint and deserialize
    let (tree_hex, _) = client.get_tree_state(checkpoint_height).await?;
    let tree_bytes = hex::decode(&tree_hex)
        .map_err(|e| Error::Other(format!("invalid tree hex: {}", e)))?;
    let mut tree = deserialize_tree(&tree_bytes)?;

    // verify size matches
    let actual_size = tree.size() as u64;
    if actual_size != checkpoint_size {
        if !script {
            eprintln!("warning: tree.size()={} vs computed={}", actual_size, checkpoint_size);
        }
    }

    let mut position_counter = checkpoint_size;

    // build position → note index map
    let mut position_map: HashMap<u64, usize> = HashMap::new();
    for (i, note) in notes.iter().enumerate() {
        position_map.insert(note.position, i);
    }

    // start replay from checkpoint_height + 1 since the tree state
    // at checkpoint_height already includes that block's actions
    let replay_start = checkpoint_height + 1;
    let replay_blocks = anchor_height - replay_start;
    let pb = if !script && is_terminal::is_terminal(std::io::stderr()) {
        let pb = ProgressBar::new(replay_blocks as u64);
        pb.set_style(ProgressStyle::default_bar()
            .template("[{elapsed}] {bar:50.green/blue} {pos:>7}/{len:7} {per_sec} building witnesses")
            .unwrap()
            .progress_chars("#>-"));
        Some(pb)
    } else {
        None
    };

    if !script {
        eprintln!("replaying {} blocks for merkle witnesses...", replay_blocks);
    }

    let mut witnesses: Vec<Option<IncrementalWitness<MerkleHashOrchard, 32>>> = vec![None; notes.len()];
    let mut current = replay_start;

    while current <= anchor_height {
        let end = (current + BATCH_SIZE - 1).min(anchor_height);
        let blocks = retry_compact_blocks(client, current, end).await?;

        for block in &blocks {
            for action in &block.actions {
                let cmx = ExtractedNoteCommitment::from_bytes(&action.cmx);
                let hash = if bool::from(cmx.is_some()) {
                    MerkleHashOrchard::from_cmx(&cmx.unwrap())
                } else {
                    MerkleHashOrchard::empty_leaf()
                };

                tree.append(hash.clone())
                    .map_err(|_| Error::Other("merkle tree full".into()))?;

                // snapshot witness at our note positions
                if let Some(&idx) = position_map.get(&position_counter) {
                    witnesses[idx] = IncrementalWitness::from_tree(tree.clone());
                }

                // update existing witnesses with new leaf
                for w in witnesses.iter_mut().flatten() {
                    if w.witnessed_position() < incrementalmerkletree::Position::from(position_counter) {
                        w.append(hash.clone()).map_err(|_| Error::Other("witness tree full".into()))?;
                    }
                }

                position_counter += 1;
            }
        }

        current = end + 1;
        if let Some(ref pb) = pb {
            pb.set_position((current - replay_start) as u64);
        }
    }

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    // extract paths and verify consistency
    let anchor_root = tree.root();
    let anchor = Anchor::from(anchor_root);

    // verify our tree root matches the network's tree state at anchor_height
    let (anchor_tree_hex, _) = client.get_tree_state(anchor_height).await?;
    let anchor_tree_bytes = hex::decode(&anchor_tree_hex)
        .map_err(|e| Error::Other(format!("invalid anchor tree hex: {}", e)))?;
    let anchor_tree = deserialize_tree(&anchor_tree_bytes)?;
    let network_root = anchor_tree.root();
    if anchor_root != network_root {
        return Err(Error::Other(format!(
            "tree root mismatch at height {} (ours={}, network={})",
            anchor_height,
            hex::encode(anchor_root.to_bytes()),
            hex::encode(network_root.to_bytes()),
        )));
    }

    let mut paths = Vec::with_capacity(notes.len());
    for (i, w) in witnesses.into_iter().enumerate() {
        let witness = w.ok_or_else(|| Error::Other(format!(
            "note at position {} not found in tree replay (checkpoint started at {})",
            notes[i].position, checkpoint_size,
        )))?;

        let imt_path = witness.path().ok_or_else(|| Error::Other(format!(
            "failed to compute merkle path for note at position {}", notes[i].position,
        )))?;
        paths.push(MerklePath::from(imt_path));
    }

    if !script {
        eprintln!("witnesses built — anchor: {}", hex::encode(anchor.to_bytes()));
    }

    Ok((anchor, paths))
}

#[cfg(test)]
mod tests {
    use super::*;
    use incrementalmerkletree::frontier::CommitmentTree;
    use incrementalmerkletree::witness::IncrementalWitness;
    use incrementalmerkletree::{Hashable, Level, Position};
    use orchard::tree::MerkleHashOrchard;

    fn test_hash(i: u8) -> MerkleHashOrchard {
        // create a deterministic test hash
        let mut bytes = [0u8; 32];
        bytes[0] = i;
        bytes[31] = i;
        Option::from(MerkleHashOrchard::from_bytes(&bytes)).unwrap()
    }

    #[test]
    fn witness_from_scratch() {
        // build tree from scratch, witness a leaf, verify path
        let mut tree: CommitmentTree<MerkleHashOrchard, 32> = CommitmentTree::empty();
        for i in 0..5 {
            tree.append(test_hash(i)).unwrap();
        }
        // witness position 4 (the last leaf)
        let mut witness = IncrementalWitness::from_tree(tree.clone()).unwrap();
        assert_eq!(witness.witnessed_position(), Position::from(4));

        // append more leaves
        for i in 5..20 {
            let h = test_hash(i);
            tree.append(h.clone()).unwrap();
            witness.append(h).unwrap();
        }

        // check roots match
        let tree_root = tree.root();
        let witness_root = witness.root();
        assert_eq!(tree_root, witness_root, "witness root should match tree root");

        // extract path and verify
        let path = witness.path().unwrap();
        let leaf = test_hash(4);
        let mut cur = leaf;
        let pos = u64::from(path.position());
        for (level, sibling) in path.path_elems().iter().enumerate() {
            let (l, r) = if (pos >> level) & 1 == 0 {
                (cur, *sibling)
            } else {
                (*sibling, cur)
            };
            cur = MerkleHashOrchard::combine(Level::from(level as u8), &l, &r);
        }
        assert_eq!(cur, tree_root, "path root should match tree root (from scratch)");
    }

    #[test]
    fn witness_from_checkpoint() {
        // build a tree with enough leaves to have multiple parent levels
        let mut tree1: CommitmentTree<MerkleHashOrchard, 32> = CommitmentTree::empty();
        for i in 0u32..100 {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let h: MerkleHashOrchard = Option::from(MerkleHashOrchard::from_bytes(&bytes)).unwrap();
            tree1.append(h).unwrap();
        }

        // serialize and reconstruct (simulating checkpoint)
        let left = tree1.left().clone();
        let right = tree1.right().clone();
        let parents = tree1.parents().clone();
        let mut tree2 = CommitmentTree::<MerkleHashOrchard, 32>::from_parts(
            left, right, parents,
        ).unwrap();
        assert_eq!(tree1.root(), tree2.root());

        // append more, then witness
        for i in 100u32..120 {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let h: MerkleHashOrchard = Option::from(MerkleHashOrchard::from_bytes(&bytes)).unwrap();
            tree1.append(h.clone()).unwrap();
            tree2.append(h).unwrap();
        }
        assert_eq!(tree1.root(), tree2.root());

        let mut witness1 = IncrementalWitness::from_tree(tree1.clone()).unwrap();
        let mut witness2 = IncrementalWitness::from_tree(tree2.clone()).unwrap();

        // append 500 more leaves
        for i in 120u32..620 {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let h: MerkleHashOrchard = Option::from(MerkleHashOrchard::from_bytes(&bytes)).unwrap();
            tree1.append(h.clone()).unwrap();
            tree2.append(h.clone()).unwrap();
            witness1.append(h.clone()).unwrap();
            witness2.append(h).unwrap();
        }

        assert_eq!(tree1.root(), tree2.root(), "tree roots differ");
        assert_eq!(witness1.root(), witness2.root(), "witness roots differ");
        assert_eq!(tree1.root(), witness1.root(), "tree1 vs witness1 root");

        let path1 = witness1.path().unwrap();
        let path2 = witness2.path().unwrap();

        // verify using imt's own root method
        let mut leaf_bytes = [0u8; 32];
        leaf_bytes[0..4].copy_from_slice(&119u32.to_le_bytes());
        let leaf = Option::from(MerkleHashOrchard::from_bytes(&leaf_bytes)).unwrap();

        let root1 = path1.root(leaf);
        let root2 = path2.root(leaf);
        assert_eq!(root1, tree1.root(), "path1 root mismatch");
        assert_eq!(root2, tree2.root(), "path2 root mismatch");
    }

    #[test]
    fn witness_from_padded_checkpoint() {
        // test with a tree that has EXTRA trailing None parents
        // (simulating deserialization from a 31-parent frontier)
        let mut tree1: CommitmentTree<MerkleHashOrchard, 32> = CommitmentTree::empty();
        for i in 0u32..50 {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let h: MerkleHashOrchard = Option::from(MerkleHashOrchard::from_bytes(&bytes)).unwrap();
            tree1.append(h).unwrap();
        }

        // tree1 has parents with ~6 entries. pad to 31 with Nones.
        let left = tree1.left().clone();
        let right = tree1.right().clone();
        let mut parents = tree1.parents().clone();
        let original_parents_len = parents.len();
        // pad to 31 parents (like the network frontier)
        while parents.len() < 31 {
            parents.push(None);
        }

        let tree2 = CommitmentTree::<MerkleHashOrchard, 32>::from_parts(
            left.clone(), right.clone(), parents,
        ).unwrap();

        // also make tree3 with no padding
        let tree3 = CommitmentTree::<MerkleHashOrchard, 32>::from_parts(
            left, right, tree1.parents().clone(),
        ).unwrap();

        eprintln!("tree1 parents: {}, tree2 (padded): 31, tree3 (original): {}",
            tree1.parents().len(), tree3.parents().len());
        assert_eq!(tree1.root(), tree2.root(), "padded tree root should match");
        assert_eq!(tree1.root(), tree3.root(), "original tree root should match");

        // append and witness from padded tree
        let mut tree1c = tree1.clone();
        let mut tree2c = tree2.clone();
        for i in 50u32..60 {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let h: MerkleHashOrchard = Option::from(MerkleHashOrchard::from_bytes(&bytes)).unwrap();
            tree1c.append(h.clone()).unwrap();
            tree2c.append(h).unwrap();
        }

        let mut w1 = IncrementalWitness::from_tree(tree1c.clone()).unwrap();
        let mut w2 = IncrementalWitness::from_tree(tree2c.clone()).unwrap();

        for i in 60u32..200 {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let h: MerkleHashOrchard = Option::from(MerkleHashOrchard::from_bytes(&bytes)).unwrap();
            tree1c.append(h.clone()).unwrap();
            tree2c.append(h.clone()).unwrap();
            w1.append(h.clone()).unwrap();
            w2.append(h).unwrap();
        }

        assert_eq!(tree1c.root(), w1.root(), "w1 root");
        assert_eq!(tree2c.root(), w2.root(), "w2 root");
        assert_eq!(tree1c.root(), tree2c.root(), "tree roots");

        let p1 = w1.path().unwrap();
        let p2 = w2.path().unwrap();

        let mut leaf_bytes = [0u8; 32];
        leaf_bytes[0..4].copy_from_slice(&59u32.to_le_bytes());
        let leaf = Option::from(MerkleHashOrchard::from_bytes(&leaf_bytes)).unwrap();

        let r1 = p1.root(leaf);
        let r2 = p2.root(leaf);

        eprintln!("w1 root: {}", hex::encode(w1.root().to_bytes()));
        eprintln!("p1 root: {}", hex::encode(r1.to_bytes()));
        eprintln!("p2 root: {}", hex::encode(r2.to_bytes()));
        eprintln!("w1 filled: {}, w2 filled: {}", w1.filled().len(), w2.filled().len());
        eprintln!("original parents: {}", original_parents_len);

        assert_eq!(r1, tree1c.root(), "p1 root mismatch (unpadded)");
        assert_eq!(r2, tree2c.root(), "p2 root mismatch (PADDED - this is the real test)");
    }
}
