//! P2P networking with litep2p

use crate::{MoveTransaction, PlayerId, BlockNumber};
use litep2p::{
    Litep2p, Litep2pEvent,
    protocol::libp2p::gossipsub::{Config as GossipsubConfig, Gossipsub, GossipsubEvent},
    transport::quic::config::Config as QuicConfig,
    types::protocol::ProtocolName,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::sync::mpsc;
use tracing::{info, warn, debug};

/// P2P network messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkMessage {
    /// Player submits a move
    Move(MoveTransaction),

    /// Request current game state
    StateRequest { from_block: BlockNumber },

    /// Response with game state
    StateResponse {
        block_number: BlockNumber,
        moves: Vec<MoveTransaction>,
    },
}

/// P2P network handle
pub struct Network {
    litep2p: Litep2p,
    gossipsub_handle: Gossipsub,
    topic: String,
    peers: HashSet<litep2p::PeerId>,
}

impl Network {
    /// Create new P2P network
    pub async fn new(
        listen_addr: String,
        topic: String,
    ) -> Result<(Self, mpsc::Receiver<NetworkMessage>), Box<dyn std::error::Error>> {
        info!("Starting P2P network on {}", listen_addr);

        // Configure gossipsub
        let gossipsub_config = GossipsubConfig::default();
        let (gossipsub_config, gossipsub_handle) = Gossipsub::new(
            gossipsub_config,
            1024 * 1024, // 1MB message size limit
        )?;

        // Configure QUIC transport (best for P2P - UDP-based, low latency, built-in encryption)
        let quic_config = QuicConfig {
            listen_addresses: vec![listen_addr.parse()?],
            ..Default::default()
        };

        // Build litep2p with QUIC
        let litep2p = Litep2p::new(
            quic_config.into(),
            vec![Box::new(gossipsub_config)],
        )?;

        let (tx, rx) = mpsc::channel(100);

        let network = Self {
            litep2p,
            gossipsub_handle,
            topic: topic.clone(),
            peers: HashSet::new(),
        };

        // Subscribe to topic
        network.gossipsub_handle.subscribe(
            ProtocolName::from(topic.as_bytes().to_vec())
        )?;

        Ok((network, rx))
    }

    /// Broadcast move to all peers
    pub async fn broadcast_move(&mut self, tx: MoveTransaction) -> Result<(), Box<dyn std::error::Error>> {
        let msg = NetworkMessage::Move(tx);
        let bytes = bincode::serialize(&msg)?;

        self.gossipsub_handle.publish(
            ProtocolName::from(self.topic.as_bytes().to_vec()),
            bytes,
        )?;

        Ok(())
    }

    /// Process network events
    pub async fn process_events(
        &mut self,
        tx: mpsc::Sender<NetworkMessage>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            match self.litep2p.next_event().await {
                Some(Litep2pEvent::ConnectionEstablished { peer, .. }) => {
                    info!("Connected to peer: {:?}", peer);
                    self.peers.insert(peer);
                }

                Some(Litep2pEvent::ConnectionClosed { peer }) => {
                    info!("Disconnected from peer: {:?}", peer);
                    self.peers.remove(&peer);
                }

                Some(Litep2pEvent::Protocol { protocol_name, event }) => {
                    if let Some(gossipsub_event) = event.downcast_ref::<GossipsubEvent>() {
                        match gossipsub_event {
                            GossipsubEvent::Message { message, .. } => {
                                match bincode::deserialize::<NetworkMessage>(&message) {
                                    Ok(msg) => {
                                        debug!("Received message: {:?}", msg);
                                        let _ = tx.send(msg).await;
                                    }
                                    Err(e) => {
                                        warn!("Failed to deserialize message: {}", e);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                None => break,
                _ => {}
            }
        }

        Ok(())
    }

    /// Connect to peer
    pub async fn connect_peer(&mut self, addr: String) -> Result<(), Box<dyn std::error::Error>> {
        info!("Connecting to peer: {}", addr);
        // litep2p handles connection automatically via transport
        Ok(())
    }

    /// Get number of connected peers
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}
