//! session - p2p session management
//!
//! manages the connection lifecycle and message routing for a poker session.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use iroh::EndpointId;
#[cfg(test)]
use iroh::PublicKey;

use crate::protocol::*;

/// active session with connected peers
pub struct Session {
    /// my endpoint id
    pub endpoint_id: EndpointId,
    /// connected peers
    peers: Arc<RwLock<HashMap<EndpointId, PeerConnection>>>,
    /// incoming message channel
    msg_rx: mpsc::Receiver<(EndpointId, Message)>,
    /// outgoing message sender (cloned per peer)
    msg_tx: mpsc::Sender<(EndpointId, Message)>,
}

/// connection to a peer
struct PeerConnection {
    role: Role,
    seat: Option<u8>,
    pubkey: [u8; 32],
}

impl Session {
    /// create a new session
    pub fn new(endpoint_id: EndpointId) -> (Self, mpsc::Sender<(EndpointId, Message)>) {
        let (msg_tx, msg_rx) = mpsc::channel(1000);

        (
            Self {
                endpoint_id,
                peers: Arc::new(RwLock::new(HashMap::new())),
                msg_rx,
                msg_tx: msg_tx.clone(),
            },
            msg_tx,
        )
    }

    /// add a peer to the session
    pub async fn add_peer(&self, endpoint_id: EndpointId, role: Role, seat: Option<u8>, pubkey: [u8; 32]) {
        let mut peers = self.peers.write().await;
        peers.insert(endpoint_id, PeerConnection { role, seat, pubkey });
    }

    /// remove a peer from the session
    pub async fn remove_peer(&self, endpoint_id: &EndpointId) {
        let mut peers = self.peers.write().await;
        peers.remove(endpoint_id);
    }

    /// get number of connected players
    pub async fn player_count(&self) -> usize {
        let peers = self.peers.read().await;
        peers.values().filter(|p| p.role == Role::Player).count()
    }

    /// get number of connected spectators
    pub async fn spectator_count(&self) -> usize {
        let peers = self.peers.read().await;
        peers.values().filter(|p| p.role == Role::Spectator).count()
    }

    /// receive next message
    pub async fn recv(&mut self) -> Option<(EndpointId, Message)> {
        self.msg_rx.recv().await
    }

    /// broadcast message to all players
    pub async fn broadcast_to_players(&self, msg: Message) {
        let peers = self.peers.read().await;
        for (endpoint_id, peer) in peers.iter() {
            if peer.role == Role::Player {
                let _ = self.msg_tx.send((*endpoint_id, msg.clone())).await;
            }
        }
    }

    /// broadcast message to all spectators
    pub async fn broadcast_to_spectators(&self, msg: Message) {
        let peers = self.peers.read().await;
        for (endpoint_id, peer) in peers.iter() {
            if peer.role == Role::Spectator {
                let _ = self.msg_tx.send((*endpoint_id, msg.clone())).await;
            }
        }
    }

    /// send message to specific peer
    pub async fn send_to(&self, endpoint_id: EndpointId, msg: Message) {
        let _ = self.msg_tx.send((endpoint_id, msg)).await;
    }

    /// get player by seat
    pub async fn get_player_by_seat(&self, seat: u8) -> Option<(EndpointId, [u8; 32])> {
        let peers = self.peers.read().await;
        for (endpoint_id, peer) in peers.iter() {
            if peer.seat == Some(seat) {
                return Some((*endpoint_id, peer.pubkey));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_peer_management() {
        // use a valid ed25519 public key (identity point works)
        let endpoint_id = PublicKey::from_bytes(&[
            0xed, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
        ]).unwrap();
        let (session, _tx) = Session::new(endpoint_id);

        let peer_id = PublicKey::from_bytes(&[
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]).unwrap();
        session.add_peer(peer_id, Role::Player, Some(1), [2u8; 32]).await;

        assert_eq!(session.player_count().await, 1);
        assert_eq!(session.spectator_count().await, 0);

        session.remove_peer(&peer_id).await;
        assert_eq!(session.player_count().await, 0);
    }
}
