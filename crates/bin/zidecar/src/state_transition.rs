//! Orchard state transition proofs
//!
//! Proves that processing Orchard actions from blocks correctly updates:
//! 1. Commitment tree (new cmx values appended)
//! 2. Nullifier set (spent nullifiers added)
//!
//! ## Trace Layout
//!
//! For each Orchard action in a block:
//! ```text
//! | Field | Description |
//! |-------|-------------|
//! | action_type | 0=spend, 1=output, 2=both |
//! | nullifier[0..4] | first 16 bytes of nullifier (if spend) |
//! | nullifier[4..8] | |
//! | nullifier[8..12] | |
//! | nullifier[12..16] | |
//! | cmx[0..4] | first 16 bytes of commitment (if output) |
//! | cmx[4..8] | |
//! | cmx[8..12] | |
//! | cmx[12..16] | |
//! | tree_pos | position in commitment tree |
//! | block_height | which block this action is in |
//! ```
//!
//! ## Constraints Proven
//!
//! 1. Each nullifier is unique (not seen before in trace)
//! 2. Each cmx is appended to tree in order
//! 3. Final tree root matches claimed root
//! 4. Final nullifier set root matches claimed root
//!
//! ## Trust Model
//!
//! Given verified checkpoint at height H with roots (T0, N0):
//! - Prove: processing blocks H → H+k produces roots (Tk, Nk)
//! - Client verifies proof, then trusts (Tk, Nk)
//! - No trust in zidecar for state computation

use crate::error::{Result, ZidecarError};
use crate::zebrad::ZebradClient;
use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

/// Fields per Orchard action in trace
pub const FIELDS_PER_ACTION: usize = 12;

/// Orchard action extracted from block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchardAction {
    /// Revealed nullifier (if spend)
    pub nullifier: Option<[u8; 32]>,
    /// Output commitment (if output)
    pub cmx: Option<[u8; 32]>,
    /// Block height containing this action
    pub block_height: u32,
    /// Position within block
    pub action_index: u32,
}

/// State transition trace for ligerito proving
#[derive(Debug, Clone)]
pub struct StateTransitionTrace {
    /// Encoded trace elements
    pub trace: Vec<BinaryElem32>,
    /// Number of actions encoded
    pub num_actions: usize,
    /// Start checkpoint
    pub from_height: u32,
    pub from_tree_root: [u8; 32],
    pub from_nullifier_root: [u8; 32],
    /// End state
    pub to_height: u32,
    pub to_tree_root: [u8; 32],
    pub to_nullifier_root: [u8; 32],
}

/// Incremental tree state for tracking commitment tree
#[derive(Debug, Clone, Default)]
pub struct TreeState {
    /// Current tree root
    pub root: [u8; 32],
    /// Number of commitments in tree
    pub size: u64,
    /// Frontier hashes for incremental updates
    pub frontier: [[u8; 32]; 32],
}

impl TreeState {
    /// Create empty tree state
    pub fn empty() -> Self {
        Self::default()
    }

    /// Append commitment to tree, return new position
    pub fn append(&mut self, cmx: &[u8; 32]) -> u64 {
        let pos = self.size;
        self.size += 1;

        // Simplified: just hash into root
        // Real impl would do proper incremental merkle tree
        let mut hasher = Sha256::new();
        hasher.update(b"ORCHARD_CMX_APPEND");
        hasher.update(&self.root);
        hasher.update(cmx);
        hasher.update(&pos.to_le_bytes());
        self.root = hasher.finalize().into();

        pos
    }

    /// Get current root
    pub fn root(&self) -> [u8; 32] {
        self.root
    }
}

/// Nullifier set state (simplified, real impl uses NOMT)
#[derive(Debug, Clone, Default)]
pub struct NullifierSetState {
    /// Current root
    pub root: [u8; 32],
    /// Number of nullifiers
    pub size: u64,
}

impl NullifierSetState {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add nullifier to set, return new root
    pub fn insert(&mut self, nullifier: &[u8; 32]) -> [u8; 32] {
        self.size += 1;

        // Simplified: just hash into root
        // Real impl uses NOMT for proper SMT
        let mut hasher = Sha256::new();
        hasher.update(b"NULLIFIER_INSERT");
        hasher.update(&self.root);
        hasher.update(nullifier);
        self.root = hasher.finalize().into();

        self.root
    }

    pub fn root(&self) -> [u8; 32] {
        self.root
    }
}

impl StateTransitionTrace {
    /// Build trace from block range
    ///
    /// Fetches all Orchard actions from zebrad and encodes them
    /// along with state transition constraints.
    pub async fn build(
        zebrad: &ZebradClient,
        from_height: u32,
        to_height: u32,
        from_tree_root: [u8; 32],
        from_nullifier_root: [u8; 32],
    ) -> Result<Self> {
        info!(
            "building state transition trace: {} -> {}",
            from_height, to_height
        );

        let mut actions = Vec::new();
        let mut tree_state = TreeState {
            root: from_tree_root,
            ..Default::default()
        };
        let mut nullifier_state = NullifierSetState {
            root: from_nullifier_root,
            ..Default::default()
        };

        // Fetch and process each block
        for height in from_height..=to_height {
            if height % 10000 == 0 {
                debug!("processing block {}", height);
            }

            let block_actions = Self::extract_orchard_actions(zebrad, height).await?;

            for (idx, action) in block_actions.into_iter().enumerate() {
                // Update state
                if let Some(ref nf) = action.nullifier {
                    nullifier_state.insert(nf);
                }
                if let Some(ref cmx) = action.cmx {
                    tree_state.append(cmx);
                }

                actions.push(OrchardAction {
                    nullifier: action.nullifier,
                    cmx: action.cmx,
                    block_height: height,
                    action_index: idx as u32,
                });
            }
        }

        info!(
            "extracted {} Orchard actions from {} blocks",
            actions.len(),
            to_height - from_height + 1
        );

        // Encode trace
        let trace = Self::encode_actions(&actions)?;

        info!(
            "encoded trace: {} elements ({} actions × {} fields)",
            trace.len(),
            actions.len(),
            FIELDS_PER_ACTION
        );

        Ok(Self {
            trace,
            num_actions: actions.len(),
            from_height,
            from_tree_root,
            from_nullifier_root,
            to_height,
            to_tree_root: tree_state.root(),
            to_nullifier_root: nullifier_state.root(),
        })
    }

    /// Extract Orchard actions from a single block
    async fn extract_orchard_actions(
        zebrad: &ZebradClient,
        height: u32,
    ) -> Result<Vec<OrchardAction>> {
        let hash = zebrad.get_block_hash(height).await?;
        let block = zebrad.get_block_with_txs(&hash).await?;

        let mut actions = Vec::new();

        // Extract Orchard actions from block transactions
        for tx in &block.tx {
            if let Some(ref orchard_bundle) = tx.orchard {
                for action in &orchard_bundle.actions {
                    actions.push(OrchardAction {
                        nullifier: action.nullifier_bytes(),
                        cmx: action.cmx_bytes(),
                        block_height: height,
                        action_index: actions.len() as u32,
                    });
                }
            }
        }

        Ok(actions)
    }

    /// Encode actions into trace polynomial
    fn encode_actions(actions: &[OrchardAction]) -> Result<Vec<BinaryElem32>> {
        let num_elements = actions.len() * FIELDS_PER_ACTION;
        let trace_size = num_elements.next_power_of_two().max(1024);

        let mut trace = vec![BinaryElem32::zero(); trace_size];

        for (i, action) in actions.iter().enumerate() {
            let offset = i * FIELDS_PER_ACTION;

            // Action type: 0=spend only, 1=output only, 2=both
            let action_type = match (&action.nullifier, &action.cmx) {
                (Some(_), Some(_)) => 2u32,
                (Some(_), None) => 0u32,
                (None, Some(_)) => 1u32,
                (None, None) => continue, // skip empty actions
            };
            trace[offset] = BinaryElem32::from(action_type);

            // Encode nullifier (4 × u32)
            if let Some(nf) = &action.nullifier {
                for j in 0..4 {
                    let val = u32::from_le_bytes([
                        nf[j * 4],
                        nf[j * 4 + 1],
                        nf[j * 4 + 2],
                        nf[j * 4 + 3],
                    ]);
                    trace[offset + 1 + j] = BinaryElem32::from(val);
                }
            }

            // Encode cmx (4 × u32)
            if let Some(cmx) = &action.cmx {
                for j in 0..4 {
                    let val = u32::from_le_bytes([
                        cmx[j * 4],
                        cmx[j * 4 + 1],
                        cmx[j * 4 + 2],
                        cmx[j * 4 + 3],
                    ]);
                    trace[offset + 5 + j] = BinaryElem32::from(val);
                }
            }

            // Tree position (would be computed from tree state)
            trace[offset + 9] = BinaryElem32::from(i as u32);

            // Block height
            trace[offset + 10] = BinaryElem32::from(action.block_height);

            // Running hash (binds all prior actions)
            let running_hash = Self::compute_running_hash(actions, i);
            trace[offset + 11] = BinaryElem32::from(running_hash);
        }

        Ok(trace)
    }

    /// Compute running hash up to action index
    fn compute_running_hash(actions: &[OrchardAction], up_to: usize) -> u32 {
        let mut hasher = Sha256::new();
        hasher.update(b"STATE_TRANSITION_RUNNING");

        for action in actions.iter().take(up_to + 1) {
            if let Some(nf) = &action.nullifier {
                hasher.update(nf);
            }
            if let Some(cmx) = &action.cmx {
                hasher.update(cmx);
            }
            hasher.update(&action.block_height.to_le_bytes());
        }

        let hash = hasher.finalize();
        u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]])
    }
}

/// State transition proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransitionProof {
    /// Serialized ligerito proof
    pub proof_bytes: Vec<u8>,

    /// Checkpoint we're proving from
    pub from_height: u32,
    pub from_tree_root: [u8; 32],
    pub from_nullifier_root: [u8; 32],

    /// State we're proving to
    pub to_height: u32,
    pub to_tree_root: [u8; 32],
    pub to_nullifier_root: [u8; 32],

    /// Number of actions proven
    pub num_actions: usize,

    /// Trace log size (for verifier config)
    pub trace_log_size: u32,
}

impl StateTransitionProof {
    /// Verify this proof
    ///
    /// In production, this would:
    /// 1. Deserialize ligerito proof
    /// 2. Verify proof against trace commitment
    /// 3. Check public inputs match claimed state
    pub fn verify(&self) -> Result<bool> {
        // TODO: Implement actual ligerito verification
        // For now, check proof is non-empty
        if self.proof_bytes.is_empty() {
            return Ok(false);
        }

        Ok(true)
    }

    /// Get proof size in bytes
    pub fn size(&self) -> usize {
        self.proof_bytes.len()
    }
}

/// Combined proof: checkpoint + state transition
///
/// This is what clients receive and verify
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustlessStateProof {
    /// FROST-signed checkpoint (trust anchor)
    pub checkpoint: crate::checkpoint::EpochCheckpoint,

    /// State transition from checkpoint to current
    pub transition: StateTransitionProof,

    /// Current block header (for freshness)
    pub current_height: u32,
    pub current_hash: [u8; 32],
}

impl TrustlessStateProof {
    /// Full verification
    pub fn verify(
        &self,
        signer_registry: &crate::checkpoint::SignerRegistry,
    ) -> Result<VerifiedState> {
        // 1. Verify FROST checkpoint signature
        signer_registry.verify_checkpoint(&self.checkpoint)
            .map_err(|e| ZidecarError::Validation(e.to_string()))?;

        // 2. Verify transition starts from checkpoint
        if self.transition.from_height != self.checkpoint.height {
            return Err(ZidecarError::Validation(
                "transition doesn't start from checkpoint".into(),
            ));
        }
        if self.transition.from_tree_root != self.checkpoint.tree_root {
            return Err(ZidecarError::Validation(
                "transition tree root doesn't match checkpoint".into(),
            ));
        }
        if self.transition.from_nullifier_root != self.checkpoint.nullifier_root {
            return Err(ZidecarError::Validation(
                "transition nullifier root doesn't match checkpoint".into(),
            ));
        }

        // 3. Verify ligerito state transition proof
        if !self.transition.verify()? {
            return Err(ZidecarError::Validation(
                "state transition proof invalid".into(),
            ));
        }

        // 4. Check freshness (transition reaches current height)
        if self.transition.to_height != self.current_height {
            return Err(ZidecarError::Validation(
                "transition doesn't reach current height".into(),
            ));
        }

        Ok(VerifiedState {
            height: self.current_height,
            tree_root: self.transition.to_tree_root,
            nullifier_root: self.transition.to_nullifier_root,
            checkpoint_epoch: self.checkpoint.epoch_index,
        })
    }
}

/// Result of successful verification
#[derive(Debug, Clone)]
pub struct VerifiedState {
    /// Current verified height
    pub height: u32,
    /// Verified tree root (can query note inclusion)
    pub tree_root: [u8; 32],
    /// Verified nullifier root (can query spend status)
    pub nullifier_root: [u8; 32],
    /// Which checkpoint epoch this derives from
    pub checkpoint_epoch: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_state() {
        let mut tree = TreeState::empty();
        assert_eq!(tree.size, 0);

        let cmx1 = [1u8; 32];
        let pos1 = tree.append(&cmx1);
        assert_eq!(pos1, 0);
        assert_eq!(tree.size, 1);

        let cmx2 = [2u8; 32];
        let pos2 = tree.append(&cmx2);
        assert_eq!(pos2, 1);
        assert_eq!(tree.size, 2);

        // Root should change with each append
        let root1 = tree.root();
        tree.append(&[3u8; 32]);
        assert_ne!(tree.root(), root1);
    }

    #[test]
    fn test_nullifier_set() {
        let mut nf_set = NullifierSetState::empty();
        assert_eq!(nf_set.size, 0);

        let nf1 = [1u8; 32];
        let root1 = nf_set.insert(&nf1);
        assert_eq!(nf_set.size, 1);

        let nf2 = [2u8; 32];
        let root2 = nf_set.insert(&nf2);
        assert_eq!(nf_set.size, 2);

        // Roots should differ
        assert_ne!(root1, root2);
    }

    #[test]
    fn test_running_hash() {
        let actions = vec![
            OrchardAction {
                nullifier: Some([1u8; 32]),
                cmx: Some([2u8; 32]),
                block_height: 100,
                action_index: 0,
            },
            OrchardAction {
                nullifier: Some([3u8; 32]),
                cmx: Some([4u8; 32]),
                block_height: 100,
                action_index: 1,
            },
        ];

        let hash0 = StateTransitionTrace::compute_running_hash(&actions, 0);
        let hash1 = StateTransitionTrace::compute_running_hash(&actions, 1);

        // Different indices should produce different hashes
        assert_ne!(hash0, hash1);

        // Same index should be deterministic
        let hash0_again = StateTransitionTrace::compute_running_hash(&actions, 0);
        assert_eq!(hash0, hash0_again);
    }
}
