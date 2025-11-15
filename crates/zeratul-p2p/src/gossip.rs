//! Gossipsub networking with QUIC transport

use crate::types::{PeerId, Message};
use litep2p::{
    Litep2p, Litep2pEvent,
    protocol::libp2p::gossipsub::{Config as GossipsubConfig, Gossipsub, GossipsubEvent},
    transport::quic::config::Config as QuicConfig,
    types::protocol::ProtocolName,
};
use std::collections::HashSet;
use tokio::sync::mpsc;
use tracing::{info, warn, debug};

/// Gossipsub network with QUIC transport
pub struct GossipNetwork<M> {
    litep2p: Litep2p,
    gossipsub_handle: Gossipsub,
    topic: String,
    peers: HashSet<PeerId>,
    _phantom: std::marker::PhantomData<M>,
}

impl<M: Message> GossipNetwork<M> {
    /// Create new gossip network with QUIC
    pub async fn new(
        listen_addr: String,
        topic: String,
    ) -> Result<(Self, mpsc::Receiver<M>), Box<dyn std::error::Error>> {
        info!("Starting QUIC P2P network on {}", listen_addr);

        // Configure gossipsub
        let gossipsub_config = GossipsubConfig::default();
        let (gossipsub_config, gossipsub_handle) = Gossipsub::new(
            gossipsub_config,
            1024 * 1024, // 1MB message size limit
        )?;

        // Configure QUIC transport (UDP-based, low latency, built-in encryption)
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
            _phantom: std::marker::PhantomData,
        };

        // Subscribe to topic
        network.gossipsub_handle.subscribe(
            ProtocolName::from(topic.as_bytes().to_vec())
        )?;

        Ok((network, rx))
    }

    /// Broadcast message to all peers
    pub async fn broadcast(&mut self, msg: &M) -> Result<(), Box<dyn std::error::Error>> {
        let bytes = bincode::serialize(msg)?;

        self.gossipsub_handle.publish(
            ProtocolName::from(self.topic.as_bytes().to_vec()),
            bytes,
        )?;

        Ok(())
    }

    /// Process network events
    pub async fn process_events(
        &mut self,
        tx: mpsc::Sender<M>,
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

                Some(Litep2pEvent::Protocol { event, .. }) => {
                    if let Some(gossipsub_event) = event.downcast_ref::<GossipsubEvent>() {
                        match gossipsub_event {
                            GossipsubEvent::Message { message, .. } => {
                                match bincode::deserialize::<M>(&message) {
                                    Ok(msg) => {
                                        debug!("Received message");
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

    /// Get number of connected peers
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}
