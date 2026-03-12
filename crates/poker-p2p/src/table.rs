//! table - table host and client implementation
//!
//! handles table creation, joining, and participant management.

use iroh::{Endpoint, EndpointAddr, EndpointId};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::protocol::*;
use crate::rendezvous::{
    generate_code, publish_table, resolve_table, PakeClient, PakeServer, RendezvousError,
    TableCode,
};
use crate::ALPN;

/// table state
#[derive(Clone, Debug)]
pub struct Table {
    pub code: TableCode,
    pub rules: TableRules,
    pub host: EndpointId,
    pub participants: Vec<Participant>,
    pub spectators: Vec<Spectator>,
}

/// participant (player) info
#[derive(Clone, Debug)]
pub struct Participant {
    pub seat: u8,
    pub pubkey: [u8; 32],
    pub endpoint_id: EndpointId,
    pub deposit: u128,
    pub channel_confirmed: bool,
}

/// spectator info
#[derive(Clone, Debug)]
pub struct Spectator {
    pub pubkey: [u8; 32],
    pub endpoint_id: EndpointId,
    pub view_key: [u8; 32],
}

/// table host - creates and manages a table
pub struct TableHost {
    endpoint: Endpoint,
    code: TableCode,
    rules: TableRules,
    host_keypair: ([u8; 32], [u8; 32]), // (secret, public)
    participants: Arc<RwLock<Vec<Participant>>>,
    spectators: Arc<RwLock<Vec<Spectator>>>,
    event_tx: mpsc::Sender<TableEvent>,
}

/// events from table
#[derive(Clone, Debug)]
pub enum TableEvent {
    /// new participant joined
    PlayerJoined { seat: u8, pubkey: [u8; 32] },
    /// player confirmed channel
    ChannelConfirmed { seat: u8 },
    /// spectator joined
    SpectatorJoined { pubkey: [u8; 32] },
    /// participant left
    ParticipantLeft { pubkey: [u8; 32] },
    /// ready to start game
    ReadyToStart,
    /// error occurred
    Error(String),
}

impl TableHost {
    /// create a new table with given rules
    pub async fn create(
        rules: TableRules,
        host_secret: [u8; 32],
    ) -> Result<(Self, mpsc::Receiver<TableEvent>, TableCode), TableError> {
        rules.validate().map_err(TableError::InvalidRules)?;

        let host_public = ed25519_dalek::SigningKey::from_bytes(&host_secret)
            .verifying_key()
            .to_bytes();

        let endpoint = Endpoint::builder()
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| TableError::NetworkError(e.to_string()))?;

        let code = generate_code();

        // publish to DHT
        publish_table(&code, endpoint.id(), &rules)
            .await
            .map_err(TableError::Rendezvous)?;

        let (event_tx, event_rx) = mpsc::channel(100);

        let host = Self {
            endpoint,
            code: code.clone(),
            rules,
            host_keypair: (host_secret, host_public),
            participants: Arc::new(RwLock::new(Vec::new())),
            spectators: Arc::new(RwLock::new(Vec::new())),
            event_tx,
        };

        Ok((host, event_rx, code))
    }

    /// get the table code
    pub fn code(&self) -> &TableCode {
        &self.code
    }

    /// get current participant count
    pub async fn player_count(&self) -> usize {
        self.participants.read().await.len()
    }

    /// get current spectator count
    pub async fn spectator_count(&self) -> usize {
        self.spectators.read().await.len()
    }

    /// check if table is ready to start
    pub async fn is_ready(&self) -> bool {
        let participants = self.participants.read().await;
        participants.len() >= 2 && participants.iter().all(|p| p.channel_confirmed)
    }

    /// run the table host (accepts connections)
    pub async fn run(&self) -> Result<(), TableError> {
        tracing::info!("table {} waiting for players", self.code);

        while let Some(incoming) = self.endpoint.accept().await {
            let conn = incoming
                .await
                .map_err(|e| TableError::NetworkError(e.to_string()))?;

            let remote_id = conn.remote_id();

            tracing::info!("connection from {}", remote_id.fmt_short());

            // spawn handler for this connection
            let participants = Arc::clone(&self.participants);
            let spectators = Arc::clone(&self.spectators);
            let rules = self.rules.clone();
            let code = self.code.clone();
            let host_keypair = self.host_keypair;
            let event_tx = self.event_tx.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(
                    conn,
                    remote_id,
                    rules,
                    code,
                    host_keypair,
                    participants,
                    spectators,
                    event_tx,
                )
                .await
                {
                    tracing::error!("connection error: {}", e);
                }
            });
        }

        Ok(())
    }

    /// close the table
    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

async fn handle_connection(
    conn: iroh::endpoint::Connection,
    remote_id: EndpointId,
    rules: TableRules,
    code: TableCode,
    host_keypair: ([u8; 32], [u8; 32]),
    participants: Arc<RwLock<Vec<Participant>>>,
    spectators: Arc<RwLock<Vec<Spectator>>>,
    event_tx: mpsc::Sender<TableEvent>,
) -> Result<(), TableError> {
    // PAKE handshake
    let pake = PakeServer::new(&code);
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| TableError::NetworkError(e.to_string()))?;

    // send our PAKE message
    let msg = pake.message();
    send_message(&mut send, msg).await?;

    // receive client's PAKE message
    let client_msg = recv_message(&mut recv).await?;
    let _session_key = pake.finish(&client_msg).map_err(TableError::Rendezvous)?;

    tracing::info!("PAKE auth successful with {}", remote_id.fmt_short());

    // send table announcement
    let announce = TableAnnounce {
        rules: rules.clone(),
        host_pubkey: host_keypair.1,
        signature: sign_rules(&host_keypair.0, &rules),
    };
    let msg = Message::TableAnnounce(announce);
    send_message(&mut send, &msg.encode_to_vec()).await?;

    // wait for join request
    let request_bytes = recv_message(&mut recv).await?;
    let request = Message::decode_from_slice(&request_bytes)
        .map_err(|e| TableError::ProtocolError(e.to_string()))?;

    match request {
        Message::JoinRequest(req) => {
            handle_join_request(
                req,
                remote_id,
                &rules,
                &mut send,
                participants,
                spectators,
                event_tx,
            )
            .await?;
        }
        _ => {
            return Err(TableError::ProtocolError("expected JoinRequest".to_string()));
        }
    }

    Ok(())
}

async fn handle_join_request(
    req: JoinRequest,
    remote_id: EndpointId,
    rules: &TableRules,
    send: &mut iroh::endpoint::SendStream,
    participants: Arc<RwLock<Vec<Participant>>>,
    spectators: Arc<RwLock<Vec<Spectator>>>,
    event_tx: mpsc::Sender<TableEvent>,
) -> Result<(), TableError> {
    match req.role {
        Role::Player => {
            let mut participants = participants.write().await;

            // check if table is full
            if participants.len() >= rules.seats as usize {
                let reject = Message::JoinReject(JoinReject {
                    reason: RejectReason::TableFull,
                });
                send_message(send, &reject.encode_to_vec()).await?;
                return Ok(());
            }

            // assign seat (1-indexed)
            let seat = (participants.len() + 1) as u8;

            // create channel params
            let channel_params = ChannelParams {
                channel_id: derive_channel_id(&req.pubkey, seat),
                deposit: rules.min_buy_in,
                participants: participants.iter().map(|p| p.pubkey).collect(),
            };

            let accept = Message::JoinAccept(JoinAccept {
                role: Role::Player,
                seat,
                channel_params: Some(channel_params),
                view_key: None,
            });
            send_message(send, &accept.encode_to_vec()).await?;

            participants.push(Participant {
                seat,
                pubkey: req.pubkey,
                endpoint_id: remote_id,
                deposit: 0,
                channel_confirmed: false,
            });

            let _ = event_tx
                .send(TableEvent::PlayerJoined {
                    seat,
                    pubkey: req.pubkey,
                })
                .await;

            tracing::info!("player joined at seat {}", seat);
        }
        Role::Spectator => {
            if !rules.allow_spectators {
                let reject = Message::JoinReject(JoinReject {
                    reason: RejectReason::SpectatorsFull,
                });
                send_message(send, &reject.encode_to_vec()).await?;
                return Ok(());
            }

            let mut spectators = spectators.write().await;

            if rules.max_spectators > 0 && spectators.len() >= rules.max_spectators as usize {
                let reject = Message::JoinReject(JoinReject {
                    reason: RejectReason::SpectatorsFull,
                });
                send_message(send, &reject.encode_to_vec()).await?;
                return Ok(());
            }

            // generate view key for spectator
            let view_key = derive_view_key(&req.pubkey);

            let accept = Message::JoinAccept(JoinAccept {
                role: Role::Spectator,
                seat: 0,
                channel_params: None,
                view_key: Some(view_key),
            });
            send_message(send, &accept.encode_to_vec()).await?;

            spectators.push(Spectator {
                pubkey: req.pubkey,
                endpoint_id: remote_id,
                view_key,
            });

            let _ = event_tx
                .send(TableEvent::SpectatorJoined { pubkey: req.pubkey })
                .await;

            tracing::info!("spectator joined");
        }
    }

    Ok(())
}

/// table client - joins an existing table
pub struct TableClient {
    endpoint: Endpoint,
    code: TableCode,
    rules: Option<TableRules>,
    my_keypair: ([u8; 32], [u8; 32]),
    host_pubkey: Option<[u8; 32]>,
    seat: Option<u8>,
    view_key: Option<[u8; 32]>,
    send: Option<iroh::endpoint::SendStream>,
    recv: Option<iroh::endpoint::RecvStream>,
}

impl TableClient {
    /// connect to a table by code
    pub async fn connect(
        code: &str,
        my_secret: [u8; 32],
    ) -> Result<(Self, TableRules), TableError> {
        let my_public = ed25519_dalek::SigningKey::from_bytes(&my_secret)
            .verifying_key()
            .to_bytes();
        let code = TableCode::new(code.to_string());

        // resolve from DHT
        let (endpoint_id, _rules_hash) = resolve_table(&code)
            .await
            .map_err(TableError::Rendezvous)?;

        tracing::info!("found table host: {}", endpoint_id.fmt_short());

        let endpoint = Endpoint::builder()
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| TableError::NetworkError(e.to_string()))?;

        // create endpoint address from just the endpoint id
        let endpoint_addr = EndpointAddr::from(endpoint_id);
        let conn = endpoint
            .connect(endpoint_addr, ALPN)
            .await
            .map_err(|e| TableError::NetworkError(e.to_string()))?;

        // PAKE handshake
        let pake = PakeClient::new(&code);
        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .map_err(|e| TableError::NetworkError(e.to_string()))?;

        // receive host's PAKE message
        let host_msg = recv_message(&mut recv).await?;

        // send our PAKE message
        send_message(&mut send, pake.message()).await?;

        let _session_key = pake.finish(&host_msg).map_err(TableError::Rendezvous)?;

        tracing::info!("PAKE auth successful");

        // receive table announcement
        let announce_bytes = recv_message(&mut recv).await?;
        let announce = Message::decode_from_slice(&announce_bytes)
            .map_err(|e| TableError::ProtocolError(e.to_string()))?;

        let (rules, host_pubkey) = match announce {
            Message::TableAnnounce(a) => {
                // verify signature against host pubkey
                if !verify_rules_signature(&a.host_pubkey, &a.rules, &a.signature) {
                    return Err(TableError::ProtocolError(
                        "invalid table announce signature".to_string(),
                    ));
                }
                (a.rules, a.host_pubkey)
            }
            _ => {
                return Err(TableError::ProtocolError(
                    "expected TableAnnounce".to_string(),
                ));
            }
        };

        rules.validate().map_err(TableError::InvalidRules)?;

        Ok((
            Self {
                endpoint,
                code,
                rules: Some(rules.clone()),
                my_keypair: (my_secret, my_public),
                host_pubkey: Some(host_pubkey),
                seat: None,
                view_key: None,
                send: Some(send),
                recv: Some(recv),
            },
            rules,
        ))
    }

    /// get the table rules
    pub fn rules(&self) -> Option<&TableRules> {
        self.rules.as_ref()
    }

    /// join as player
    pub async fn join_as_player(&mut self) -> Result<(u8, ChannelParams), TableError> {
        let send = self.send.as_mut().ok_or(TableError::ProtocolError("no connection".into()))?;
        let recv = self.recv.as_mut().ok_or(TableError::ProtocolError("no connection".into()))?;
        let rules = self.rules.as_ref().ok_or(TableError::ProtocolError("no rules".into()))?;

        // sign rules hash using ed25519
        let rules_acceptance_sig = sign_rules(&self.my_keypair.0, rules);

        let request = Message::JoinRequest(JoinRequest {
            role: Role::Player,
            pubkey: self.my_keypair.1,
            tier_proof: None,
            rules_acceptance_sig,
        });
        send_message(send, &request.encode_to_vec()).await?;

        // receive response
        let response_bytes = recv_message(recv).await?;
        let response = Message::decode_from_slice(&response_bytes)
            .map_err(|e| TableError::ProtocolError(e.to_string()))?;

        match response {
            Message::JoinAccept(accept) => {
                if accept.role != Role::Player {
                    return Err(TableError::ProtocolError("unexpected role in accept".into()));
                }
                let channel_params = accept.channel_params
                    .ok_or(TableError::ProtocolError("no channel params in accept".into()))?;
                self.seat = Some(accept.seat);
                tracing::info!("joined as player at seat {}", accept.seat);
                Ok((accept.seat, channel_params))
            }
            Message::JoinReject(reject) => {
                Err(TableError::ProtocolError(format!("join rejected: {:?}", reject.reason)))
            }
            _ => Err(TableError::ProtocolError("unexpected response to join".into())),
        }
    }

    /// join as spectator
    pub async fn join_as_spectator(&mut self) -> Result<[u8; 32], TableError> {
        let send = self.send.as_mut().ok_or(TableError::ProtocolError("no connection".into()))?;
        let recv = self.recv.as_mut().ok_or(TableError::ProtocolError("no connection".into()))?;
        let rules = self.rules.as_ref().ok_or(TableError::ProtocolError("no rules".into()))?;

        let rules_acceptance_sig = sign_rules(&self.my_keypair.0, rules);

        let request = Message::JoinRequest(JoinRequest {
            role: Role::Spectator,
            pubkey: self.my_keypair.1,
            tier_proof: None,
            rules_acceptance_sig,
        });
        send_message(send, &request.encode_to_vec()).await?;

        let response_bytes = recv_message(recv).await?;
        let response = Message::decode_from_slice(&response_bytes)
            .map_err(|e| TableError::ProtocolError(e.to_string()))?;

        match response {
            Message::JoinAccept(accept) => {
                let view_key = accept.view_key
                    .ok_or(TableError::ProtocolError("no view key in accept".into()))?;
                self.view_key = Some(view_key);
                tracing::info!("joined as spectator");
                Ok(view_key)
            }
            Message::JoinReject(reject) => {
                Err(TableError::ProtocolError(format!("join rejected: {:?}", reject.reason)))
            }
            _ => Err(TableError::ProtocolError("unexpected response to join".into())),
        }
    }

    /// get our seat number
    pub fn seat(&self) -> Option<u8> {
        self.seat
    }

    /// run message loop, forwarding decoded messages to the returned channel
    pub async fn run_message_loop(&mut self) -> Result<mpsc::Receiver<Message>, TableError> {
        let mut recv = self.recv.take()
            .ok_or(TableError::ProtocolError("no recv stream".into()))?;
        let mut send = self.send.take()
            .ok_or(TableError::ProtocolError("no send stream".into()))?;

        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            loop {
                match recv_message(&mut recv).await {
                    Ok(bytes) => {
                        match Message::decode_from_slice(&bytes) {
                            Ok(Message::Ping(nonce)) => {
                                // respond with pong
                                let pong = Message::Pong(nonce);
                                if let Err(e) = send_message(&mut send, &pong.encode_to_vec()).await {
                                    tracing::error!("failed to send pong: {}", e);
                                    break;
                                }
                            }
                            Ok(msg) => {
                                if tx.send(msg).await.is_err() {
                                    tracing::info!("message loop consumer dropped");
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("failed to decode message: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("recv error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// send a message to the table
    pub async fn send(&mut self, msg: &Message) -> Result<(), TableError> {
        let send = self.send.as_mut().ok_or(TableError::ProtocolError("no connection".into()))?;
        send_message(send, &msg.encode_to_vec()).await
    }

    /// disconnect from table
    pub async fn disconnect(self) {
        self.endpoint.close().await;
    }
}

// helper functions

use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn send_message(
    send: &mut iroh::endpoint::SendStream,
    msg: &[u8],
) -> Result<(), TableError> {
    send.write_all(&(msg.len() as u32).to_le_bytes())
        .await
        .map_err(|e| TableError::NetworkError(e.to_string()))?;
    send.write_all(msg)
        .await
        .map_err(|e| TableError::NetworkError(e.to_string()))?;
    Ok(())
}

async fn recv_message(recv: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>, TableError> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(|e| TableError::NetworkError(e.to_string()))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 1024 * 1024 {
        return Err(TableError::ProtocolError("message too large".to_string()));
    }
    let mut msg = vec![0u8; len];
    recv.read_exact(&mut msg)
        .await
        .map_err(|e| TableError::NetworkError(e.to_string()))?;
    Ok(msg)
}

fn sign_rules(secret: &[u8; 32], rules: &TableRules) -> [u8; 64] {
    use ed25519_dalek::{SigningKey, Signer};
    let hash = rules.hash();
    let mut msg = Vec::with_capacity(32 + 14);
    msg.extend_from_slice(b"poker.rules.v1");
    msg.extend_from_slice(&hash);
    let signing_key = SigningKey::from_bytes(secret);
    signing_key.sign(&msg).to_bytes()
}

fn verify_rules_signature(pubkey: &[u8; 32], rules: &TableRules, signature: &[u8; 64]) -> bool {
    use ed25519_dalek::{VerifyingKey, Verifier, Signature};
    let Ok(verifying_key) = VerifyingKey::from_bytes(pubkey) else {
        return false;
    };
    let hash = rules.hash();
    let mut msg = Vec::with_capacity(32 + 14);
    msg.extend_from_slice(b"poker.rules.v1");
    msg.extend_from_slice(&hash);
    let sig = Signature::from_bytes(signature);
    verifying_key.verify(&msg, &sig).is_ok()
}

fn derive_channel_id(pubkey: &[u8; 32], seat: u8) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"poker.channel.v1");
    hasher.update(pubkey);
    hasher.update(&[seat]);
    *hasher.finalize().as_bytes()
}

fn derive_view_key(pubkey: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"poker.viewkey.v1");
    hasher.update(pubkey);
    *hasher.finalize().as_bytes()
}

/// build the message bytes that a PlayerAction signature covers
fn action_sign_payload(action: &PlayerAction) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"poker.action.v1");
    msg.extend_from_slice(&action.hand_number.to_le_bytes());
    msg.push(action.seat);
    msg.extend_from_slice(&action.sequence.to_le_bytes());
    msg.extend_from_slice(&parity_scale_codec::Encode::encode(&action.action));
    msg
}

/// sign a player action with ed25519
pub fn sign_action(secret: &[u8; 32], action: &mut PlayerAction) {
    use ed25519_dalek::{SigningKey, Signer};
    let payload = action_sign_payload(action);
    let signing_key = SigningKey::from_bytes(secret);
    action.signature = signing_key.sign(&payload).to_bytes();
}

/// verify a player action's signature against the claimed seat's pubkey.
/// also checks sequence > last_sequence for replay protection.
pub fn verify_action(
    pubkey: &[u8; 32],
    action: &PlayerAction,
    last_sequence: u64,
) -> Result<(), ActionVerifyError> {
    // replay protection: sequence must be strictly increasing
    if action.sequence <= last_sequence {
        return Err(ActionVerifyError::ReplayedSequence);
    }
    // verify ed25519 signature
    use ed25519_dalek::{VerifyingKey, Verifier, Signature};
    let verifying_key = VerifyingKey::from_bytes(pubkey)
        .map_err(|_| ActionVerifyError::InvalidPubkey)?;
    let payload = action_sign_payload(action);
    let sig = Signature::from_bytes(&action.signature);
    verifying_key.verify(&payload, &sig)
        .map_err(|_| ActionVerifyError::BadSignature)?;
    Ok(())
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ActionVerifyError {
    #[error("replayed sequence number")]
    ReplayedSequence,
    #[error("invalid public key")]
    InvalidPubkey,
    #[error("bad signature")]
    BadSignature,
}

/// table errors
#[derive(Debug, thiserror::Error)]
pub enum TableError {
    #[error("invalid rules: {0}")]
    InvalidRules(#[from] RulesError),
    #[error("network error: {0}")]
    NetworkError(String),
    #[error("rendezvous error: {0}")]
    Rendezvous(#[from] RendezvousError),
    #[error("protocol error: {0}")]
    ProtocolError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_verify_rules() {
        let secret = [0x42u8; 32];
        let rules = TableRules::default();

        let sig = sign_rules(&secret, &rules);
        let pubkey = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();

        assert!(verify_rules_signature(&pubkey, &rules, &sig));

        // wrong pubkey should fail
        let wrong_pubkey = [0u8; 32];
        assert!(!verify_rules_signature(&wrong_pubkey, &rules, &sig));

        // different rules should fail
        let different_rules = TableRules::training();
        assert!(!verify_rules_signature(&pubkey, &different_rules, &sig));
    }

    #[test]
    fn test_sign_verify_training_rules() {
        let secret = [0x11u8; 32];
        let rules = TableRules::training();

        let sig = sign_rules(&secret, &rules);
        let pubkey = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();

        assert!(verify_rules_signature(&pubkey, &rules, &sig));
    }

    #[test]
    fn test_sign_verify_action() {
        let secret = [0x55u8; 32];
        let pubkey = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();

        let mut action = PlayerAction {
            hand_number: 42,
            seat: 2,
            action: ActionType::Raise(100_000),
            sequence: 1,
            signature: [0u8; 64],
        };
        sign_action(&secret, &mut action);

        // valid signature, sequence > 0
        assert!(verify_action(&pubkey, &action, 0).is_ok());

        // replay: same sequence
        assert!(verify_action(&pubkey, &action, 1).is_err());

        // wrong pubkey
        let wrong = [0u8; 32];
        assert!(verify_action(&wrong, &action, 0).is_err());
    }

    #[test]
    fn test_derive_channel_id_deterministic() {
        let pubkey = [0xAAu8; 32];
        let id1 = derive_channel_id(&pubkey, 1);
        let id2 = derive_channel_id(&pubkey, 1);
        let id3 = derive_channel_id(&pubkey, 2);
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_message_encode_join_request() {
        let request = Message::JoinRequest(JoinRequest {
            role: Role::Player,
            pubkey: [0x42u8; 32],
            tier_proof: None,
            rules_acceptance_sig: [0u8; 64],
        });
        let encoded = request.encode_to_vec();
        let decoded = Message::decode_from_slice(&encoded).unwrap();
        match decoded {
            Message::JoinRequest(req) => {
                assert_eq!(req.role, Role::Player);
                assert_eq!(req.pubkey, [0x42u8; 32]);
            }
            _ => panic!("wrong message type"),
        }
    }

    #[test]
    fn test_message_encode_join_accept() {
        let accept = Message::JoinAccept(JoinAccept {
            role: Role::Player,
            seat: 3,
            channel_params: Some(ChannelParams {
                channel_id: [0x01u8; 32],
                deposit: 1_000_000,
                participants: vec![[0x02u8; 32]],
            }),
            view_key: None,
        });
        let encoded = accept.encode_to_vec();
        let decoded = Message::decode_from_slice(&encoded).unwrap();
        match decoded {
            Message::JoinAccept(a) => {
                assert_eq!(a.seat, 3);
                assert!(a.channel_params.is_some());
            }
            _ => panic!("wrong message type"),
        }
    }

    #[test]
    fn test_player_action_encode_sign_verify() {
        let secret = [0x55u8; 32];
        let pubkey = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();

        let mut action = PlayerAction {
            hand_number: 42,
            seat: 2,
            action: ActionType::Raise(100_000),
            sequence: 1,
            signature: [0u8; 64],
        };
        sign_action(&secret, &mut action);

        // encode and decode roundtrip
        let msg = Message::Action(action);
        let encoded = msg.encode_to_vec();
        let decoded = Message::decode_from_slice(&encoded).unwrap();

        match decoded {
            Message::Action(pa) => {
                assert_eq!(pa.hand_number, 42);
                assert_eq!(pa.seat, 2);
                assert_eq!(pa.sequence, 1);
                // verify using the new verify_action function
                assert!(verify_action(&pubkey, &pa, 0).is_ok());
                // replay should fail
                assert!(verify_action(&pubkey, &pa, 1).is_err());
            }
            _ => panic!("wrong message type"),
        }
    }

    #[test]
    fn test_full_protocol_flow_encode() {
        // simulate: host creates announce → client signs rules → client sends join
        let host_secret = [0x01u8; 32];
        let client_secret = [0x02u8; 32];
        let rules = TableRules::default();

        // host creates signed announcement
        let host_pubkey = ed25519_dalek::SigningKey::from_bytes(&host_secret)
            .verifying_key()
            .to_bytes();
        let announce_sig = sign_rules(&host_secret, &rules);
        let announce = Message::TableAnnounce(TableAnnounce {
            rules: rules.clone(),
            host_pubkey,
            signature: announce_sig,
        });

        // encode → decode → verify
        let encoded = announce.encode_to_vec();
        let decoded = Message::decode_from_slice(&encoded).unwrap();
        let verified_rules = match decoded {
            Message::TableAnnounce(a) => {
                assert!(verify_rules_signature(&a.host_pubkey, &a.rules, &a.signature));
                a.rules
            }
            _ => panic!("wrong message type"),
        };

        // client signs rules and sends join request
        let client_pubkey = ed25519_dalek::SigningKey::from_bytes(&client_secret)
            .verifying_key()
            .to_bytes();
        let rules_sig = sign_rules(&client_secret, &verified_rules);
        let join = Message::JoinRequest(JoinRequest {
            role: Role::Player,
            pubkey: client_pubkey,
            tier_proof: None,
            rules_acceptance_sig: rules_sig,
        });

        let join_encoded = join.encode_to_vec();
        let join_decoded = Message::decode_from_slice(&join_encoded).unwrap();
        match join_decoded {
            Message::JoinRequest(req) => {
                assert_eq!(req.pubkey, client_pubkey);
                // host verifies client's rules signature
                assert!(verify_rules_signature(&req.pubkey, &rules, &req.rules_acceptance_sig));
            }
            _ => panic!("wrong message type"),
        }
    }
}
