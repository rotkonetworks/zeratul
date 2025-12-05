//! P2P Networking for Zeratul testnet
//!
//! Uses litep2p with TCP transport for validator communication.

#![cfg(feature = "networking")]

use crate::types::*;
use litep2p::{
    Litep2p, Litep2pEvent,
    config::ConfigBuilder,
    protocol::request_response::{
        Config as RequestResponseConfig,
        RequestResponseHandle,
        RequestResponseEvent,
        DialOptions,
    },
    transport::tcp::config::Config as TcpConfig,
    PeerId,
};
use futures::StreamExt;
use tokio::sync::mpsc;
use std::collections::HashSet;
use serde::{Serialize, Deserialize};

/// Network message types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkMessage {
    /// New work package submitted
    WorkPackage(WorkPackage),
    /// Block proposal
    Block(Block),
    /// Vote for a block
    Vote(Vote),
    /// Request block at height
    BlockRequest { height: Height },
    /// Response with block
    BlockResponse { block: Option<Block> },
    /// Peer discovery
    Hello { validator_id: Option<ValidatorId> },
}

/// Network configuration
pub struct NetworkConfig {
    /// Listen port
    pub port: u16,
    /// Bootstrap peers (multiaddr strings)
    pub bootstrap_peers: Vec<String>,
    /// Our validator ID (None if not a validator)
    pub validator_id: Option<ValidatorId>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: 30333,
            bootstrap_peers: vec![],
            validator_id: None,
        }
    }
}

/// Network service handle
pub struct NetworkService {
    /// litep2p instance
    litep2p: Litep2p,
    /// Request/response protocol handle
    rr_handle: RequestResponseHandle,
    /// Connected peers
    peers: HashSet<PeerId>,
    /// Incoming message channel
    message_rx: mpsc::UnboundedReceiver<(PeerId, NetworkMessage)>,
    /// Internal message sender
    message_tx: mpsc::UnboundedSender<(PeerId, NetworkMessage)>,
}

impl NetworkService {
    /// Create new network service
    pub async fn new(config: NetworkConfig) -> Result<Self, NetworkError> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        // Configure request/response protocol
        let (rr_config, rr_handle) = RequestResponseConfig::new(
            litep2p::types::protocol::ProtocolName::from("/zeratul/sync/1"),
            vec![],
            1024 * 1024,  // 1MB max request size
            std::time::Duration::from_secs(30),
            None,  // No connection limits
        );

        // Build litep2p config with TCP
        let litep2p_config = ConfigBuilder::new()
            .with_tcp(TcpConfig {
                listen_addresses: vec![
                    format!("/ip4/0.0.0.0/tcp/{}", config.port)
                        .parse()
                        .unwrap()
                ],
                ..Default::default()
            })
            .with_request_response_protocol(rr_config)
            .build();

        let mut litep2p = Litep2p::new(litep2p_config)
            .map_err(|e| NetworkError::InitFailed(e.to_string()))?;

        // Start listening
        litep2p.listen_addresses().for_each(|addr| {
            tracing::info!("Listening on {}", addr);
        });

        Ok(Self {
            litep2p,
            rr_handle,
            peers: HashSet::new(),
            message_rx,
            message_tx,
        })
    }

    /// Get our peer ID
    pub fn local_peer_id(&self) -> PeerId {
        *self.litep2p.local_peer_id()
    }

    /// Connect to a peer
    pub async fn connect(&mut self, addr: &str) -> Result<(), NetworkError> {
        let multiaddr: litep2p::types::multiaddr::Multiaddr = addr
            .parse()
            .map_err(|_| NetworkError::InitFailed("Invalid peer address".into()))?;

        self.litep2p.dial_address(multiaddr).await
            .map_err(|e| NetworkError::SendFailed(e.to_string()))?;

        Ok(())
    }

    /// Send request to specific peer
    pub async fn send_request(&mut self, peer: PeerId, message: NetworkMessage) -> Result<(), NetworkError> {
        let encoded = bincode::serialize(&message)
            .map_err(|e| NetworkError::SerializationError(e.to_string()))?;

        self.rr_handle.send_request(peer, encoded, DialOptions::Dial)
            .await
            .map_err(|e| NetworkError::SendFailed(format!("{:?}", e)))?;

        Ok(())
    }

    /// Process network events (call this in event loop)
    pub async fn poll(&mut self) -> Option<NetworkEvent> {
        tokio::select! {
            // Process litep2p events
            event = self.litep2p.next_event() => {
                match event {
                    Some(Litep2pEvent::ConnectionEstablished { peer, .. }) => {
                        self.peers.insert(peer);
                        return Some(NetworkEvent::PeerConnected(peer));
                    }
                    Some(Litep2pEvent::ConnectionClosed { peer, .. }) => {
                        self.peers.remove(&peer);
                        return Some(NetworkEvent::PeerDisconnected(peer));
                    }
                    _ => {}
                }
            }

            // Process request/response events
            event = self.rr_handle.next() => {
                match event {
                    Some(RequestResponseEvent::RequestReceived { peer, request_id, request, .. }) => {
                        if let Ok(msg) = bincode::deserialize::<NetworkMessage>(&request) {
                            // Send empty response
                            let _ = self.rr_handle.send_response(request_id, vec![]);
                            return Some(NetworkEvent::Message(peer, msg));
                        }
                    }
                    Some(RequestResponseEvent::ResponseReceived { peer, response, .. }) => {
                        if let Ok(msg) = bincode::deserialize::<NetworkMessage>(&response) {
                            return Some(NetworkEvent::Message(peer, msg));
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// Get connected peer count
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get connected peers
    pub fn peers(&self) -> impl Iterator<Item = &PeerId> {
        self.peers.iter()
    }
}

/// Network events
#[derive(Debug)]
pub enum NetworkEvent {
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    Message(PeerId, NetworkMessage),
}

/// Network errors
#[derive(Debug)]
pub enum NetworkError {
    InitFailed(String),
    SerializationError(String),
    SendFailed(String),
    NotConnected,
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::InitFailed(e) => write!(f, "Network init failed: {}", e),
            NetworkError::SerializationError(e) => write!(f, "Serialization error: {}", e),
            NetworkError::SendFailed(e) => write!(f, "Send failed: {}", e),
            NetworkError::NotConnected => write!(f, "Not connected to any peers"),
        }
    }
}

impl std::error::Error for NetworkError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_message_serialization() {
        let msg = NetworkMessage::Hello { validator_id: Some(1) };
        let encoded = bincode::serialize(&msg).unwrap();
        let decoded: NetworkMessage = bincode::deserialize(&encoded).unwrap();

        match decoded {
            NetworkMessage::Hello { validator_id } => {
                assert_eq!(validator_id, Some(1));
            }
            _ => panic!("Wrong message type"),
        }
    }
}
