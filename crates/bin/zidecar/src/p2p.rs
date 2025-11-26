//! P2P coordination between zidecar nodes
//!
//! Multiple zidecars form a signer federation that:
//! 1. Gossip checkpoints to each other
//! 2. Coordinate FROST signing ceremonies
//! 3. Share state proofs with clients
//!
//! Uses litep2p for networking.
//!
//! ## Network Topology
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │  Zidecar 1  │◄───►│  Zidecar 2  │◄───►│  Zidecar 3  │
//! │   (ZF)      │     │   (ECC)     │     │ (Community) │
//! └──────┬──────┘     └──────┬──────┘     └──────┬──────┘
//!        │                   │                   │
//!        └───────────────────┼───────────────────┘
//!                            │
//!                   ┌────────▼────────┐
//!                   │  FROST Signing  │
//!                   │    Ceremony     │
//!                   └────────┬────────┘
//!                            │
//!                   ┌────────▼────────┐
//!                   │   Checkpoint    │
//!                   │    Gossip       │
//!                   └─────────────────┘
//! ```
//!
//! ## Protocol Messages
//!
//! - `CheckpointProposal`: Propose new checkpoint for signing
//! - `FrostCommitment`: Round 1 of FROST signing
//! - `FrostShare`: Round 2 of FROST signing
//! - `CheckpointAnnounce`: Gossip signed checkpoint
//! - `StateProofRequest`: Client requests state proof
//! - `StateProofResponse`: Server sends state proof

use crate::checkpoint::{EpochCheckpoint, FrostSignature};
use crate::state_transition::TrustlessStateProof;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Peer identifier (ed25519 public key)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId(pub [u8; 32]);

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0[..8]))
    }
}

/// P2P protocol messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// Propose checkpoint for signing (initiator → all)
    CheckpointProposal(CheckpointProposal),

    /// FROST round 1: commitment (all → all)
    FrostCommitment(FrostCommitmentMsg),

    /// FROST round 2: signature share (all → aggregator)
    FrostShare(FrostShareMsg),

    /// Announce completed checkpoint (aggregator → all)
    CheckpointAnnounce(EpochCheckpoint),

    /// Request state proof (client → server)
    StateProofRequest(StateProofRequest),

    /// Response with state proof (server → client)
    StateProofResponse(Box<TrustlessStateProof>),

    /// Heartbeat for liveness
    Ping(u64),
    Pong(u64),
}

/// Checkpoint proposal for FROST signing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointProposal {
    /// Epoch being checkpointed
    pub epoch_index: u64,
    /// Block height at epoch end
    pub height: u32,
    /// Block hash to sign
    pub block_hash: [u8; 32],
    /// Tree root to sign
    pub tree_root: [u8; 32],
    /// Nullifier root to sign
    pub nullifier_root: [u8; 32],
    /// Proposer's peer ID
    pub proposer: PeerId,
    /// Proposal timestamp
    pub timestamp: u64,
}

impl CheckpointProposal {
    /// Convert to unsigned checkpoint
    pub fn to_checkpoint(&self) -> EpochCheckpoint {
        EpochCheckpoint::new_unsigned(
            self.epoch_index,
            self.height,
            self.block_hash,
            self.tree_root,
            self.nullifier_root,
        )
    }

    /// Message hash for FROST signing
    pub fn message_hash(&self) -> [u8; 32] {
        self.to_checkpoint().message_hash()
    }
}

/// FROST round 1: commitment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrostCommitmentMsg {
    /// Which checkpoint this is for
    pub epoch_index: u64,
    /// Signer's peer ID
    pub signer: PeerId,
    /// Signer's index in the group
    pub signer_index: u16,
    /// Commitment (hiding, binding nonces)
    pub hiding_nonce_commitment: [u8; 32],
    pub binding_nonce_commitment: [u8; 32],
}

/// FROST round 2: signature share
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrostShareMsg {
    /// Which checkpoint this is for
    pub epoch_index: u64,
    /// Signer's peer ID
    pub signer: PeerId,
    /// Signer's index
    pub signer_index: u16,
    /// Signature share (scalar)
    pub signature_share: [u8; 32],
}

/// Client request for state proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateProofRequest {
    /// Request ID for correlation
    pub request_id: u64,
    /// Desired height (0 = latest)
    pub height: u32,
    /// Whether to include full proof or just roots
    pub include_proof: bool,
}

/// P2P network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    /// Listen address
    pub listen_addr: String,
    /// Bootstrap peers
    pub bootstrap_peers: Vec<String>,
    /// Our keypair seed (for deterministic peer ID)
    pub keypair_seed: Option<[u8; 32]>,
    /// FROST signer index (if participating in signing)
    pub signer_index: Option<u16>,
    /// Minimum peers before signing
    pub min_peers_for_signing: usize,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            listen_addr: "/ip4/0.0.0.0/tcp/30333".to_string(),
            bootstrap_peers: vec![],
            keypair_seed: None,
            signer_index: None,
            min_peers_for_signing: 2,
        }
    }
}

/// P2P network state
pub struct P2pNetwork {
    /// Our peer ID
    pub local_peer_id: PeerId,
    /// Connected peers
    peers: Arc<RwLock<HashMap<PeerId, PeerState>>>,
    /// Pending FROST ceremonies
    frost_state: Arc<RwLock<FrostCeremonyState>>,
    /// Outbound message channel
    outbound_tx: mpsc::Sender<(Option<PeerId>, Message)>,
    /// Configuration
    config: P2pConfig,
}

/// State of a connected peer
#[derive(Debug, Clone)]
pub struct PeerState {
    /// Peer's address
    pub addr: String,
    /// Last seen timestamp
    pub last_seen: u64,
    /// Is this peer a signer?
    pub is_signer: bool,
    /// Peer's signer index (if signer)
    pub signer_index: Option<u16>,
}

/// State for ongoing FROST signing ceremony
#[derive(Debug, Default)]
pub struct FrostCeremonyState {
    /// Current proposal being signed
    pub current_proposal: Option<CheckpointProposal>,
    /// Collected commitments (signer_index → commitment)
    pub commitments: HashMap<u16, FrostCommitmentMsg>,
    /// Collected shares (signer_index → share)
    pub shares: HashMap<u16, FrostShareMsg>,
    /// Ceremony phase
    pub phase: FrostPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrostPhase {
    #[default]
    Idle,
    CollectingCommitments,
    CollectingShares,
    Aggregating,
}

impl P2pNetwork {
    /// Create new P2P network
    pub async fn new(config: P2pConfig) -> Result<(Self, mpsc::Receiver<(PeerId, Message)>), P2pError> {
        // Generate or derive peer ID
        let local_peer_id = if let Some(seed) = config.keypair_seed {
            // Deterministic from seed
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(b"ZIDECAR_PEER_ID");
            hasher.update(&seed);
            PeerId(hasher.finalize().into())
        } else {
            // Pseudo-random from timestamp and process info
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(b"ZIDECAR_PEER_ID_RANDOM");
            hasher.update(&std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                .to_le_bytes());
            hasher.update(&std::process::id().to_le_bytes());
            PeerId(hasher.finalize().into())
        };

        let (outbound_tx, _outbound_rx) = mpsc::channel(1024);
        let (inbound_tx, inbound_rx) = mpsc::channel(1024);

        // TODO: Initialize litep2p here
        // let litep2p = litep2p::Litep2p::new(litep2p::config::Config::new()
        //     .with_tcp(litep2p::transport::tcp::Config::new(config.listen_addr.parse()?))
        //     .build())?;

        let network = Self {
            local_peer_id,
            peers: Arc::new(RwLock::new(HashMap::new())),
            frost_state: Arc::new(RwLock::new(FrostCeremonyState::default())),
            outbound_tx,
            config,
        };

        // Spawn network event loop
        // tokio::spawn(network.clone().run_event_loop(litep2p, inbound_tx));

        Ok((network, inbound_rx))
    }

    /// Broadcast message to all peers
    pub async fn broadcast(&self, msg: Message) -> Result<(), P2pError> {
        self.outbound_tx
            .send((None, msg))
            .await
            .map_err(|_| P2pError::ChannelClosed)
    }

    /// Send message to specific peer
    pub async fn send(&self, peer: PeerId, msg: Message) -> Result<(), P2pError> {
        self.outbound_tx
            .send((Some(peer), msg))
            .await
            .map_err(|_| P2pError::ChannelClosed)
    }

    /// Get connected peer count
    pub async fn peer_count(&self) -> usize {
        self.peers.read().await.len()
    }

    /// Check if we have enough peers for signing
    pub async fn can_sign(&self) -> bool {
        self.peer_count().await >= self.config.min_peers_for_signing
    }

    /// Initiate FROST checkpoint signing
    pub async fn propose_checkpoint(
        &self,
        epoch_index: u64,
        height: u32,
        block_hash: [u8; 32],
        tree_root: [u8; 32],
        nullifier_root: [u8; 32],
    ) -> Result<(), P2pError> {
        if !self.can_sign().await {
            return Err(P2pError::InsufficientPeers);
        }

        let proposal = CheckpointProposal {
            epoch_index,
            height,
            block_hash,
            tree_root,
            nullifier_root,
            proposer: self.local_peer_id,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        // Update local state
        {
            let mut state = self.frost_state.write().await;
            state.current_proposal = Some(proposal.clone());
            state.commitments.clear();
            state.shares.clear();
            state.phase = FrostPhase::CollectingCommitments;
        }

        // Broadcast proposal
        self.broadcast(Message::CheckpointProposal(proposal)).await
    }

    /// Handle incoming FROST commitment
    pub async fn handle_commitment(&self, msg: FrostCommitmentMsg) -> Result<(), P2pError> {
        let mut state = self.frost_state.write().await;

        if state.phase != FrostPhase::CollectingCommitments {
            return Err(P2pError::InvalidPhase);
        }

        if msg.epoch_index != state.current_proposal.as_ref().map(|p| p.epoch_index).unwrap_or(0) {
            return Err(P2pError::EpochMismatch);
        }

        state.commitments.insert(msg.signer_index, msg);

        // Check if we have enough commitments to proceed
        // In real impl, check against threshold
        if state.commitments.len() >= self.config.min_peers_for_signing {
            state.phase = FrostPhase::CollectingShares;
            // TODO: Compute and broadcast our signature share
        }

        Ok(())
    }

    /// Handle incoming FROST share
    pub async fn handle_share(&self, msg: FrostShareMsg) -> Result<Option<FrostSignature>, P2pError> {
        let mut state = self.frost_state.write().await;

        if state.phase != FrostPhase::CollectingShares {
            return Err(P2pError::InvalidPhase);
        }

        state.shares.insert(msg.signer_index, msg);

        // Check if we have enough shares to aggregate
        if state.shares.len() >= self.config.min_peers_for_signing {
            state.phase = FrostPhase::Aggregating;

            // TODO: Aggregate shares into final signature
            let signature = FrostSignature {
                r: [0u8; 32], // Would be actual aggregated R
                s: [0u8; 32], // Would be actual aggregated s
            };

            state.phase = FrostPhase::Idle;
            return Ok(Some(signature));
        }

        Ok(None)
    }
}

/// P2P errors
#[derive(Debug, Clone)]
pub enum P2pError {
    /// Failed to generate keypair
    KeyGeneration,
    /// Network channel closed
    ChannelClosed,
    /// Not enough peers for signing
    InsufficientPeers,
    /// Wrong ceremony phase
    InvalidPhase,
    /// Epoch doesn't match current ceremony
    EpochMismatch,
    /// Connection failed
    ConnectionFailed(String),
}

impl std::fmt::Display for P2pError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyGeneration => write!(f, "failed to generate keypair"),
            Self::ChannelClosed => write!(f, "network channel closed"),
            Self::InsufficientPeers => write!(f, "not enough peers for signing"),
            Self::InvalidPhase => write!(f, "invalid ceremony phase"),
            Self::EpochMismatch => write!(f, "epoch mismatch"),
            Self::ConnectionFailed(e) => write!(f, "connection failed: {}", e),
        }
    }
}

impl std::error::Error for P2pError {}

/// Checkpoint coordinator - manages FROST ceremonies
pub struct CheckpointCoordinator {
    /// P2P network
    network: Arc<P2pNetwork>,
    /// Checkpoint store
    checkpoints: Arc<RwLock<crate::checkpoint::CheckpointStore>>,
    /// Epoch duration in blocks
    epoch_blocks: u32,
}

impl CheckpointCoordinator {
    pub fn new(
        network: Arc<P2pNetwork>,
        checkpoints: Arc<RwLock<crate::checkpoint::CheckpointStore>>,
        epoch_blocks: u32,
    ) -> Self {
        Self {
            network,
            checkpoints,
            epoch_blocks,
        }
    }

    /// Check if we should create a checkpoint at this height
    pub fn should_checkpoint(&self, height: u32, last_checkpoint_height: u32) -> bool {
        height - last_checkpoint_height >= self.epoch_blocks
    }

    /// Run coordinator loop
    pub async fn run(
        self,
        mut height_rx: mpsc::Receiver<u32>,
        zebrad: Arc<crate::zebrad::ZebradClient>,
    ) {
        let mut last_checkpoint_height = {
            let store = self.checkpoints.read().await;
            store.latest().map(|c| c.height).unwrap_or(0)
        };

        while let Some(height) = height_rx.recv().await {
            if self.should_checkpoint(height, last_checkpoint_height) {
                // Get current state from zebrad
                if let Ok(info) = zebrad.get_blockchain_info().await {
                    let epoch_index = (height / self.epoch_blocks) as u64;

                    // Get tree and nullifier roots
                    // In real impl, compute these from processed blocks
                    let tree_root = [0u8; 32]; // TODO
                    let nullifier_root = [0u8; 32]; // TODO

                    // Parse block hash
                    let block_hash = hex::decode(&info.bestblockhash)
                        .ok()
                        .and_then(|b| b.try_into().ok())
                        .unwrap_or([0u8; 32]);

                    // Propose checkpoint
                    if let Err(e) = self.network.propose_checkpoint(
                        epoch_index,
                        height,
                        block_hash,
                        tree_root,
                        nullifier_root,
                    ).await {
                        tracing::warn!("failed to propose checkpoint: {}", e);
                    } else {
                        tracing::info!(
                            "proposed checkpoint for epoch {} at height {}",
                            epoch_index,
                            height
                        );
                        last_checkpoint_height = height;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_id_display() {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe]);
        let peer = PeerId(bytes);
        let s = format!("{}", peer);
        assert!(s.starts_with("deadbeef"));
    }

    #[test]
    fn test_checkpoint_proposal_hash() {
        let proposal = CheckpointProposal {
            epoch_index: 100,
            height: 1000000,
            block_hash: [1u8; 32],
            tree_root: [2u8; 32],
            nullifier_root: [3u8; 32],
            proposer: PeerId([0u8; 32]),
            timestamp: 12345,
        };

        let hash1 = proposal.message_hash();
        let hash2 = proposal.message_hash();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_should_checkpoint_logic() {
        // Test checkpoint logic without full coordinator
        let epoch_blocks = 720u32;

        let should_checkpoint = |height: u32, last: u32| -> bool {
            height - last >= epoch_blocks
        };

        // Haven't reached epoch boundary
        assert!(!should_checkpoint(100, 0));

        // Reached epoch boundary
        assert!(should_checkpoint(720, 0));
        assert!(should_checkpoint(1440, 720));
    }
}
