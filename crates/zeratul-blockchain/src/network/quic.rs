//! Network Transport for Zeratul
//!
//! Implements networking using litep2p with TCP transport.
//! MVP implementation - QUIC and advanced protocols coming later.
//!
//! ## Architecture
//!
//! - **TCP transport**: litep2p with TCP (QUIC TODO)
//! - **Ed25519 identity**: Peer authentication
//! - **Custom protocols**: DKG and block sync via user protocols
//! - **Validator connections**: Maintain connections to validator set

use anyhow::Result;
use futures::StreamExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use litep2p::{
    Litep2p,
    PeerId as Litep2pPeerId,
    Litep2pEvent,
    config::ConfigBuilder,
    transport::tcp::config::Config as TcpConfig,
    types::multiaddr::Multiaddr,
    protocol::notification::{
        Config as NotificationConfig,
        ConfigBuilder as NotificationConfigBuilder,
        NotificationHandle,
        NotificationEvent,
        ValidationResult,
    },
};

use super::types::{ValidatorEndpoint};
use super::crypto_compat::{Ed25519PrivateKey, Ed25519PublicKey};
use super::dkg::{DKGBroadcast, DKGRequest};
use super::protocols::{BlockAnnounce, ConsensusMessage};

/// Network configuration
#[derive(Clone)]
pub struct NetworkConfig {
    /// Our Ed25519 keypair for peer identity
    pub keypair: (Ed25519PrivateKey, Ed25519PublicKey),

    /// Listen addresses (TCP)
    pub listen_addrs: Vec<SocketAddr>,

    /// Genesis block hash (for protocol versioning)
    pub genesis_hash: [u8; 32],

    /// Bootstrap peer multiaddrs (full format with peer ID)
    pub bootstrap_peers: Vec<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        // Generate dummy keypair for testing
        let secret_bytes = [1u8; 32];
        let private = Ed25519PrivateKey::from_bytes(&secret_bytes).unwrap();
        let public = private.public_key();

        Self {
            keypair: (private, public),
            listen_addrs: vec!["127.0.0.1:9000".parse().unwrap()],
            genesis_hash: [0u8; 32],
            bootstrap_peers: Vec::new(),
        }
    }
}

/// DKG protocol name
const DKG_PROTOCOL: &str = "/zeratul/dkg/1";

/// Block announcement protocol name
const BLOCK_PROTOCOL: &str = "/zeratul/block/1";

/// Network service (simplified MVP version)
pub struct NetworkService {
    /// Configuration
    config: NetworkConfig,

    /// litep2p instance
    litep2p: Litep2p,

    /// Connected peers (validator endpoints)
    peers: Arc<RwLock<HashMap<Litep2pPeerId, ValidatorEndpoint>>>,

    /// DKG notification protocol handle
    dkg_handle: NotificationHandle,

    /// Peers with open DKG substreams
    dkg_peers: Arc<RwLock<std::collections::HashSet<Litep2pPeerId>>>,

    /// DKG message channel (for outgoing broadcasts)
    dkg_tx: mpsc::UnboundedSender<DKGBroadcast>,
    dkg_rx: Arc<RwLock<mpsc::UnboundedReceiver<DKGBroadcast>>>,

    /// Block notification protocol handle
    block_handle: NotificationHandle,

    /// Peers with open block substreams
    block_peers: Arc<RwLock<std::collections::HashSet<Litep2pPeerId>>>,

    /// Consensus message channel (for outgoing broadcasts)
    consensus_tx: mpsc::UnboundedSender<ConsensusMessage>,
    consensus_rx: Arc<RwLock<mpsc::UnboundedReceiver<ConsensusMessage>>>,

    /// Pending peers needing substream opening (with connection time)
    pending_substreams: Vec<(Litep2pPeerId, std::time::Instant)>,
}

/// Network handles for protocol interaction
pub struct NetworkHandles {
    /// Send DKG broadcasts
    pub dkg_tx: mpsc::UnboundedSender<DKGBroadcast>,
    /// Send consensus messages (blocks, votes, finality)
    pub consensus_tx: mpsc::UnboundedSender<ConsensusMessage>,
    /// Receive consensus messages from peers
    pub consensus_rx: mpsc::UnboundedReceiver<ConsensusMessage>,
}

impl NetworkService {
    /// Create a new network service
    pub async fn new(config: NetworkConfig) -> Result<(Self, NetworkHandles)> {
        info!("Starting Zeratul network service (TCP transport)");

        // Convert our Ed25519 key to litep2p keypair
        let private_key_bytes = config.keypair.0.as_bytes();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(private_key_bytes);
        let secret_key = litep2p::crypto::ed25519::SecretKey::try_from_bytes(&mut seed)
            .map_err(|e| anyhow::anyhow!("Invalid secret key: {:?}", e))?;
        let keypair = litep2p::crypto::ed25519::Keypair::from(secret_key);

        // Configure TCP transport
        let tcp_config = TcpConfig {
            listen_addresses: config.listen_addrs.iter()
                .map(|addr| format!("/ip4/{}/tcp/{}", addr.ip(), addr.port()).parse().unwrap())
                .collect(),
            ..Default::default()
        };

        // Configure DKG notification protocol
        let (dkg_config, dkg_handle) = NotificationConfigBuilder::new(DKG_PROTOCOL.into())
            .with_max_size(64 * 1024) // 64KB max message size
            .with_handshake(vec![0u8; 4]) // Simple handshake
            .build();

        // Configure block notification protocol
        let (block_config, block_handle) = NotificationConfigBuilder::new(BLOCK_PROTOCOL.into())
            .with_max_size(1024 * 1024) // 1MB max for block announcements
            .with_handshake(vec![1u8; 4]) // Simple handshake
            .build();

        // Build litep2p with DKG and block protocols
        let litep2p_config = ConfigBuilder::new()
            .with_keypair(keypair)
            .with_tcp(tcp_config)
            .with_notification_protocol(dkg_config)
            .with_notification_protocol(block_config)
            .build();

        let litep2p = Litep2p::new(litep2p_config)?;

        // Create DKG message channel
        let (dkg_tx, dkg_rx) = mpsc::unbounded_channel();

        // Create consensus message channels
        // consensus_tx_out: validator sends messages to network
        // consensus_rx_out: validator receives messages from network
        let (consensus_tx_out, consensus_rx_out) = mpsc::unbounded_channel();
        let (incoming_consensus_tx, incoming_consensus_rx) = mpsc::unbounded_channel();

        let handles = NetworkHandles {
            dkg_tx: dkg_tx.clone(),
            consensus_tx: consensus_tx_out.clone(),
            consensus_rx: incoming_consensus_rx,
        };

        let service = Self {
            config,
            litep2p,
            peers: Arc::new(RwLock::new(HashMap::new())),
            dkg_handle,
            dkg_peers: Arc::new(RwLock::new(std::collections::HashSet::new())),
            dkg_tx,
            dkg_rx: Arc::new(RwLock::new(dkg_rx)),
            block_handle,
            block_peers: Arc::new(RwLock::new(std::collections::HashSet::new())),
            consensus_tx: incoming_consensus_tx, // Used internally to forward received messages
            consensus_rx: Arc::new(RwLock::new(consensus_rx_out)), // Outgoing messages to broadcast
            pending_substreams: Vec::new(),
        };

        Ok((service, handles))
    }

    /// Get our local peer ID
    pub fn local_peer_id(&self) -> &Litep2pPeerId {
        self.litep2p.local_peer_id()
    }

    /// Dial a peer
    pub async fn dial(&mut self, peer_addr: SocketAddr) -> Result<()> {
        let multiaddr_str = match peer_addr {
            SocketAddr::V4(v4) => format!("/ip4/{}/tcp/{}", v4.ip(), v4.port()),
            SocketAddr::V6(v6) => {
                // Check if this is an IPv4-mapped IPv6 address (::ffff:x.x.x.x)
                let segments = v6.ip().segments();
                if segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0xffff {
                    // Extract IPv4 from last two segments
                    let ipv4 = std::net::Ipv4Addr::new(
                        (segments[6] >> 8) as u8,
                        (segments[6] & 0xff) as u8,
                        (segments[7] >> 8) as u8,
                        (segments[7] & 0xff) as u8,
                    );
                    format!("/ip4/{}/tcp/{}", ipv4, v6.port())
                } else {
                    format!("/ip6/{}/tcp/{}", v6.ip(), v6.port())
                }
            }
        };

        debug!(?multiaddr_str, "Generated multiaddr string");

        let multiaddr: Multiaddr = multiaddr_str
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid multiaddr: {:?}", e))?;

        info!(?peer_addr, "Dialing peer");
        self.litep2p.dial_address(multiaddr).await?;

        Ok(())
    }

    /// Connect to bootstrap peers
    pub async fn connect_bootstrap(&mut self) -> Result<()> {
        for peer_addr in &self.config.bootstrap_peers.clone() {
            match peer_addr.parse::<Multiaddr>() {
                Ok(multiaddr) => {
                    info!(?peer_addr, "Dialing bootstrap peer");
                    if let Err(e) = self.litep2p.dial_address(multiaddr).await {
                        warn!(?peer_addr, ?e, "Failed to dial bootstrap peer");
                    }
                }
                Err(e) => {
                    warn!(?peer_addr, ?e, "Invalid multiaddr for bootstrap peer");
                }
            }
        }
        Ok(())
    }

    /// Run the network event loop (simplified MVP)
    pub async fn run(mut self) -> Result<()> {
        info!("Starting network event loop");

        // Connect to bootstrap peers
        self.connect_bootstrap().await?;

        loop {
            tokio::select! {
                // litep2p events
                event = self.litep2p.next_event() => {
                    match event {
                        Some(Litep2pEvent::ConnectionEstablished { peer, .. }) => {
                            info!(?peer, "Connection established");
                            // Queue peer for substream opening (with delay)
                            self.pending_substreams.push((peer, std::time::Instant::now()));
                        }
                        Some(Litep2pEvent::ConnectionClosed { peer, connection_id }) => {
                            info!(?peer, ?connection_id, "Connection closed");
                            self.peers.write().await.remove(&peer);
                        }
                        Some(ev) => {
                            debug!("litep2p event: {:?}", ev);
                        }
                        None => {
                            warn!("litep2p event stream ended");
                            break;
                        }
                    }
                }

                // DKG protocol events
                event = self.dkg_handle.next() => {
                    match event {
                        Some(NotificationEvent::ValidateSubstream { peer, .. }) => {
                            debug!(?peer, "Validating DKG substream - accepting");
                            self.dkg_handle.send_validation_result(peer, ValidationResult::Accept);
                        }
                        Some(NotificationEvent::NotificationStreamOpened { peer, .. }) => {
                            info!(?peer, "DKG substream opened");
                            self.dkg_peers.write().await.insert(peer);
                        }
                        Some(NotificationEvent::NotificationStreamClosed { peer }) => {
                            info!(?peer, "DKG substream closed");
                            self.dkg_peers.write().await.remove(&peer);
                        }
                        Some(NotificationEvent::NotificationReceived { peer, notification }) => {
                            match bincode::deserialize::<DKGBroadcast>(&notification) {
                                Ok(dkg_msg) => {
                                    info!(?peer, epoch = dkg_msg.epoch, "Received DKG message");
                                    // TODO: Forward to DKG coordinator
                                }
                                Err(e) => {
                                    warn!(?peer, ?e, "Failed to decode DKG message");
                                }
                            }
                        }
                        Some(NotificationEvent::NotificationStreamOpenFailure { peer, error }) => {
                            warn!(?peer, ?error, "DKG substream open failed");
                        }
                        None => {
                            warn!("DKG event stream ended");
                        }
                    }
                }

                // Outgoing DKG message processing
                msg = async {
                    let mut rx = self.dkg_rx.write().await;
                    rx.recv().await
                } => {
                    if let Some(dkg_msg) = msg {
                        info!(epoch = dkg_msg.epoch, "Broadcasting DKG message to peers");

                        // Serialize and broadcast to all peers
                        match bincode::serialize(&dkg_msg) {
                            Ok(data) => {
                                // Get list of peers with open substreams
                                let peers: Vec<_> = self.dkg_peers.read().await.iter().copied().collect();
                                let peer_count = peers.len();

                                for peer in peers {
                                    if let Err(e) = self.dkg_handle.send_sync_notification(peer, data.clone().into()) {
                                        warn!(?peer, ?e, "Failed to send DKG notification");
                                    }
                                }

                                info!(peer_count, "Broadcast DKG message to {} peers", peer_count);
                            }
                            Err(e) => {
                                warn!(?e, "Failed to serialize DKG message");
                            }
                        }
                    }
                }

                // Block protocol events
                event = self.block_handle.next() => {
                    match event {
                        Some(NotificationEvent::ValidateSubstream { peer, .. }) => {
                            debug!(?peer, "Validating block substream - accepting");
                            self.block_handle.send_validation_result(peer, ValidationResult::Accept);
                        }
                        Some(NotificationEvent::NotificationStreamOpened { peer, .. }) => {
                            info!(?peer, "Block substream opened");
                            self.block_peers.write().await.insert(peer);
                        }
                        Some(NotificationEvent::NotificationStreamClosed { peer }) => {
                            info!(?peer, "Block substream closed");
                            self.block_peers.write().await.remove(&peer);
                        }
                        Some(NotificationEvent::NotificationReceived { peer, notification }) => {
                            match serde_json::from_slice::<ConsensusMessage>(&notification) {
                                Ok(msg) => {
                                    match &msg {
                                        ConsensusMessage::BlockAnnounce(block_announce) => {
                                            info!(
                                                ?peer,
                                                height = block_announce.height,
                                                slot = block_announce.timeslot,
                                                "Received block announcement"
                                            );
                                        }
                                        ConsensusMessage::Vote(vote) => {
                                            info!(
                                                ?peer,
                                                height = vote.height,
                                                slot = vote.timeslot,
                                                validator = vote.validator_index,
                                                "Received vote"
                                            );
                                        }
                                        ConsensusMessage::Finality(cert) => {
                                            info!(
                                                ?peer,
                                                height = cert.height,
                                                signers = ?cert.signers,
                                                "Received finality certificate"
                                            );
                                        }
                                    }
                                    // Forward to validator for processing
                                    if let Err(e) = self.consensus_tx.send(msg) {
                                        warn!(?e, "Failed to forward consensus message");
                                    }
                                }
                                Err(e) => {
                                    warn!(?peer, ?e, "Failed to decode consensus message");
                                }
                            }
                        }
                        Some(NotificationEvent::NotificationStreamOpenFailure { peer, error }) => {
                            warn!(?peer, ?error, "Block substream open failed");
                        }
                        None => {
                            warn!("Block event stream ended");
                        }
                    }
                }

                // Outgoing consensus message processing
                msg = async {
                    let mut rx = self.consensus_rx.write().await;
                    rx.recv().await
                } => {
                    if let Some(consensus_msg) = msg {
                        let msg_type = match &consensus_msg {
                            ConsensusMessage::BlockAnnounce(b) => format!("block h={}", b.height),
                            ConsensusMessage::Vote(v) => format!("vote h={} v={}", v.height, v.validator_index),
                            ConsensusMessage::Finality(f) => format!("finality h={}", f.height),
                        };

                        // Serialize and broadcast to all peers
                        match serde_json::to_vec(&consensus_msg) {
                            Ok(data) => {
                                let peers: Vec<_> = self.block_peers.read().await.iter().copied().collect();
                                let peer_count = peers.len();

                                for peer in peers {
                                    if let Err(e) = self.block_handle.send_sync_notification(peer, data.clone().into()) {
                                        warn!(?peer, ?e, "Failed to send consensus message");
                                    }
                                }

                                debug!(peer_count, msg_type, "Broadcast {} to {} peers", msg_type, peer_count);
                            }
                            Err(e) => {
                                warn!(?e, "Failed to serialize consensus message");
                            }
                        }
                    }
                }

                // Process pending substreams with delay
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    let now = std::time::Instant::now();
                    let delay = std::time::Duration::from_millis(100);

                    // Process any pending substreams that have waited long enough
                    let mut remaining = Vec::new();
                    for (peer, connected_at) in std::mem::take(&mut self.pending_substreams) {
                        if now.duration_since(connected_at) >= delay {
                            // Open DKG substream
                            match self.dkg_handle.open_substream(peer).await {
                                Ok(()) => debug!(?peer, "Opened DKG substream"),
                                Err(e) => debug!(?peer, ?e, "Could not open DKG substream"),
                            }

                            // Open block substream
                            match self.block_handle.open_substream(peer).await {
                                Ok(()) => debug!(?peer, "Opened block substream"),
                                Err(e) => debug!(?peer, ?e, "Could not open block substream"),
                            }
                        } else {
                            remaining.push((peer, connected_at));
                        }
                    }
                    self.pending_substreams = remaining;
                }
            }
        }

        Ok(())
    }

    /// Broadcast DKG message (placeholder)
    pub async fn broadcast_dkg(&self, msg: DKGBroadcast) -> Result<()> {
        info!(epoch = msg.epoch, "Broadcasting DKG message");

        // TODO TODO TODO: Implement actual broadcasting via litep2p user protocol
        // For MVP, just log
        let peer_count = self.peers.read().await.len();
        info!(peer_count, "Would broadcast to {} peers", peer_count);

        Ok(())
    }

    /// Get preferred initiator for a connection (JAM spec)
    ///
    /// P(a, b) = a if (a[31] > 127) XOR (b[31] > 127) XOR (a < b), else b
    pub fn preferred_initiator<'a>(a: &'a Ed25519PublicKey, b: &'a Ed25519PublicKey) -> &'a Ed25519PublicKey {
        let a_bytes = a.as_bytes();
        let b_bytes = b.as_bytes();

        let a_high = a_bytes[31] > 127;
        let b_high = b_bytes[31] > 127;
        let a_less = a_bytes < b_bytes;

        if a_high ^ b_high ^ a_less {
            a
        } else {
            b
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preferred_initiator() {
        let mut a_bytes = [1u8; 32];
        let mut b_bytes = [2u8; 32];

        a_bytes[31] = 200; // > 127
        b_bytes[31] = 50;  // < 127

        let a_priv = Ed25519PrivateKey::from_bytes(&a_bytes[..]).unwrap();
        let b_priv = Ed25519PrivateKey::from_bytes(&b_bytes[..]).unwrap();

        let a = a_priv.public_key();
        let b = b_priv.public_key();

        let preferred = NetworkService::preferred_initiator(&a, &b);

        // a_high=true, b_high=false, a_less=true
        // true XOR false XOR true = false, so should pick b
        assert_eq!(preferred, &b);
    }

    #[tokio::test]
    async fn test_network_creation() {
        let config = NetworkConfig::default();
        let result = NetworkService::new(config).await;
        assert!(result.is_ok());
    }
}
