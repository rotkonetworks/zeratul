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

        let host_public = *blake3::hash(&host_secret).as_bytes();

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
#[allow(dead_code)]
pub struct TableClient {
    endpoint: Endpoint,
    code: TableCode,
    rules: Option<TableRules>,
    my_keypair: ([u8; 32], [u8; 32]),
    seat: Option<u8>,
    view_key: Option<[u8; 32]>,
}

impl TableClient {
    /// connect to a table by code
    pub async fn connect(
        code: &str,
        my_secret: [u8; 32],
    ) -> Result<(Self, TableRules), TableError> {
        let my_public = *blake3::hash(&my_secret).as_bytes();
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

        let rules = match announce {
            Message::TableAnnounce(a) => {
                // verify signature
                // TODO: actual signature verification
                a.rules
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
                seat: None,
                view_key: None,
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
        // TODO: implement full join flow
        Err(TableError::ProtocolError("not implemented".to_string()))
    }

    /// join as spectator
    pub async fn join_as_spectator(&mut self) -> Result<[u8; 32], TableError> {
        // TODO: implement full join flow
        Err(TableError::ProtocolError("not implemented".to_string()))
    }

    /// disconnect from table
    pub async fn disconnect(self) {
        self.endpoint.close().await;
    }
}

// helper functions

#[allow(unused_imports)]
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
    let hash = rules.hash();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"poker.rules.v1");
    hasher.update(secret);
    hasher.update(&hash);
    let sig_hash = hasher.finalize();
    let mut signature = [0u8; 64];
    signature[..32].copy_from_slice(sig_hash.as_bytes());
    signature[32..].copy_from_slice(&hash);
    signature
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
