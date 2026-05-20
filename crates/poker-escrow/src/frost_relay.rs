//! WebSocket client for the FROST relay (`zcli/bin/relay`).
//!
//! Mirrors the wire protocol used by zafu's `frost-relay-client.ts`:
//!   client -> server: `{"t":"create","nick":"..."}`, `{"t":"join","room":"...","nick":"..."}`,
//!                     `{"t":"msg","text":"..."}`, `{"t":"part"}`
//!   server -> client: `{"t":"created","room":"..."}`, `{"t":"joined","room":"...","nick":"...","count":N}`,
//!                     `{"t":"msg","nick":"...","text":"...","seq":N,"ts":T}`, `{"t":"system","text":"..."}`,
//!                     `{"t":"error","msg":"..."}`

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
    MaybeTlsStream, WebSocketStream,
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Serialize)]
#[serde(tag = "t")]
enum ClientMsg<'a> {
    #[serde(rename = "create")]
    Create { nick: &'a str },
    #[serde(rename = "join")]
    Join { room: &'a str, nick: &'a str },
    #[serde(rename = "msg")]
    Msg { text: &'a str },
    #[serde(rename = "part")]
    #[allow(dead_code)]
    Part,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "t")]
enum ServerMsg {
    #[serde(rename = "created")]
    Created { room: String },
    #[serde(rename = "joined")]
    Joined { room: String, nick: String, count: u32 },
    #[serde(rename = "msg")]
    Msg { nick: String, text: String },
    #[serde(rename = "system")]
    System { text: String },
    #[serde(rename = "error")]
    Error { msg: String },
    #[serde(other)]
    Other,
}

/// Events surfaced to the caller (DKG state machine, signing flow, etc).
#[derive(Debug, Clone)]
pub enum RelayEvent {
    /// another participant joined; `count` is the resulting room size
    PeerJoined { nick: String, count: u32 },
    /// payload from another participant (opaque bytes)
    Message { from: String, payload: Vec<u8> },
    /// peer disconnected or room closed
    Closed { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("ws closed")]
    Closed,
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("io: {0}")]
    Io(String),
}

pub struct FrostRelayClient {
    ws: Ws,
    nick: String,
}

impl FrostRelayClient {
    /// Connect to a relay URL (e.g. `ws://127.0.0.1:50053/ws`).
    pub async fn connect(url: &str, nick: String) -> Result<Self, RelayError> {
        let req = url.into_client_request()
            .map_err(|e| RelayError::Connect(e.to_string()))?;
        let (ws, _resp) = connect_async(req).await
            .map_err(|e| RelayError::Connect(e.to_string()))?;
        Ok(Self { ws, nick })
    }

    /// Create a brand-new room. Returns the generated room code.
    pub async fn create_room(&mut self) -> Result<String, RelayError> {
        let nick = self.nick.clone();
        self.send_json(&ClientMsg::Create { nick: &nick }).await?;
        loop {
            match self.read_next().await? {
                ServerMsg::Created { room } => {
                    // server auto-joins the creator; consume the matching `joined` if it arrives
                    self.send_json(&ClientMsg::Join { room: &room, nick: &nick }).await?;
                    return Ok(room);
                }
                ServerMsg::Error { msg } => return Err(RelayError::Protocol(msg)),
                _ => continue,
            }
        }
    }

    /// Join an existing room by code. Returns the participant count after we joined.
    pub async fn join_room(&mut self, room: &str) -> Result<u32, RelayError> {
        let nick = self.nick.clone();
        self.send_json(&ClientMsg::Join { room, nick: &nick }).await?;
        loop {
            match self.read_next().await? {
                ServerMsg::Joined { nick: joiner, count, .. } if joiner == nick => return Ok(count),
                ServerMsg::Error { msg } => return Err(RelayError::Protocol(msg)),
                _ => continue,
            }
        }
    }

    /// Send an opaque payload to all room participants.
    pub async fn send_message(&mut self, payload: &[u8]) -> Result<(), RelayError> {
        let text = std::str::from_utf8(payload)
            .map_err(|e| RelayError::Protocol(format!("payload not UTF-8: {}", e)))?;
        self.send_json(&ClientMsg::Msg { text }).await
    }

    /// Block for the next room event. Skips our own echo / unrelated system noise.
    pub async fn recv_event(&mut self) -> Result<RelayEvent, RelayError> {
        loop {
            match self.read_next().await? {
                ServerMsg::Joined { nick, count, .. } if nick != self.nick => {
                    return Ok(RelayEvent::PeerJoined { nick, count });
                }
                ServerMsg::Msg { nick, text } if nick != self.nick => {
                    return Ok(RelayEvent::Message { from: nick, payload: text.into_bytes() });
                }
                // existing participants only see peer-joins via a system text "<nick> joined (N)"
                ServerMsg::System { text } if text.contains("joined") => {
                    let (nick, count) = parse_system_join(&text);
                    return Ok(RelayEvent::PeerJoined { nick, count });
                }
                ServerMsg::System { text }
                    if text.contains("left") || text.contains("disconnected") || text.contains("closed") =>
                {
                    return Ok(RelayEvent::Closed { reason: text });
                }
                ServerMsg::Error { msg } => return Err(RelayError::Protocol(msg)),
                _ => continue,
            }
        }
    }

    /// Try to receive an event with a deadline. Returns `Ok(None)` on timeout.
    pub async fn recv_event_timeout(
        &mut self,
        deadline: Duration,
    ) -> Result<Option<RelayEvent>, RelayError> {
        match tokio::time::timeout(deadline, self.recv_event()).await {
            Ok(r) => r.map(Some),
            Err(_) => Ok(None),
        }
    }

    async fn send_json<T: Serialize>(&mut self, msg: &T) -> Result<(), RelayError> {
        let body = serde_json::to_string(msg)
            .map_err(|e| RelayError::Protocol(e.to_string()))?;
        self.ws.send(Message::Text(body.into())).await
            .map_err(|e| RelayError::Io(e.to_string()))?;
        Ok(())
    }

    async fn read_next(&mut self) -> Result<ServerMsg, RelayError> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let parsed: ServerMsg = serde_json::from_str(&text)
                        .map_err(|e| RelayError::Protocol(format!("bad json: {}", e)))?;
                    return Ok(parsed);
                }
                Some(Ok(Message::Close(_))) | None => return Err(RelayError::Closed),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(RelayError::Io(e.to_string())),
            }
        }
    }
}

/// "<nick> joined (N)" → (nick, N). Tolerant: returns ("unknown", 0) on parse failure.
fn parse_system_join(text: &str) -> (String, u32) {
    let nick = text.split_whitespace().next().unwrap_or("unknown").to_string();
    let count = text
        .rsplit_once('(')
        .and_then(|(_, tail)| tail.split_once(')'))
        .and_then(|(n, _)| n.parse::<u32>().ok())
        .unwrap_or(0);
    (nick, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end smoke test against a running FROST relay on :50053.
    /// Requires `/home/al/Rotko/zcli/bin/relay` running. Skipped by default.
    /// Run with: `cargo test --release --package poker-escrow -- --ignored relay_smoke`
    #[tokio::test]
    #[ignore]
    async fn relay_smoke() {
        let url = "ws://127.0.0.1:50053/ws";
        let mut alice = FrostRelayClient::connect(url, "alice".into()).await.expect("alice connect");
        let room = alice.create_room().await.expect("create");
        let mut bob = FrostRelayClient::connect(url, "bob".into()).await.expect("bob connect");
        let count = bob.join_room(&room).await.expect("bob join");
        assert_eq!(count, 2, "room should have 2 participants after bob joins");
        let alice_seen_bob = alice.recv_event_timeout(Duration::from_secs(2)).await.expect("alice recv");
        // relay reports peer-join via system text "<id_hex>... joined (N)"; we only
        // assert count here because nick in system text is the participant id prefix,
        // not the client-supplied nick.
        assert!(matches!(alice_seen_bob, Some(RelayEvent::PeerJoined { count: 2, .. })),
            "expected PeerJoined count=2, got {:?}", alice_seen_bob);
        bob.send_message(b"hello").await.expect("bob send");
        let received = alice.recv_event_timeout(Duration::from_secs(2)).await.expect("alice recv msg");
        match received {
            Some(RelayEvent::Message { payload, .. }) => {
                assert_eq!(payload, b"hello");
            }
            other => panic!("expected Message, got {:?}", other),
        }
    }
}
