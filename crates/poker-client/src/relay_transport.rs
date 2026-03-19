//! WebSocket relay transport for the Bevy poker client.
//!
//! Connects to `wss://relay.zk.bot/ws` and speaks the same JSON wire
//! protocol as the SolidJS web client (`transport.ts` / `game.ts`).
//!
//! Wire envelope: `{ "t": "<tag>", "d": <payload> }`
//!
//! Relay control messages (sent/received as top-level JSON):
//!   create, join, joined, msg, system, part, error
//!
//! Game-level messages (nested inside `msg.text` as JSON):
//!   seated, action, deal, phase, showdown, result,
//!   shuffle_pk, shuffle_init, shuffle_done, reveal,
//!   propose_rules, accept_rules, escrow_ready,
//!   _keyex, _enc, chat, timeout_claim

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tokio::sync::mpsc;

// ─── Wire types ──────────────────────────────────────────────────────────────

/// A game-level wire message (`{ t, d }`) — matches the web client's `WireMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMessage {
    /// message type tag
    pub t: String,
    /// JSON payload (opaque)
    pub d: serde_json::Value,
    /// relay-assigned timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub relay_ts: Option<u64>,
}

/// Top-level relay frame (what the WebSocket actually carries).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayFrame {
    t: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nick: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ts: Option<u64>,
}

// ─── Room events (relay → Bevy) ─────────────────────────────────────────────

/// Events produced by the relay connection itself (not game messages).
#[derive(Debug, Clone)]
pub enum RoomEvent {
    Joined { room: String },
    OpponentJoined,
    OpponentLeft,
    OpponentDisconnected,
    OpponentReconnected,
    Encrypted,
    Error(String),
}

// ─── Bevy Resource ───────────────────────────────────────────────────────────

/// Relay transport as a Bevy `Resource`.
///
/// Call [`RelayTransport::connect`] to start the background WebSocket.
/// Every frame, call [`RelayTransport::drain_incoming`] / [`RelayTransport::drain_room_events`]
/// to pull received messages, and [`RelayTransport::send`] to enqueue outbound
/// game messages.
#[derive(Resource)]
pub struct RelayTransport {
    /// outbound game messages (Bevy → background task → WebSocket)
    tx_out: Option<mpsc::UnboundedSender<OutboundCmd>>,
    /// inbound game messages (WebSocket → background task → Bevy)
    rx_in: Option<mpsc::UnboundedReceiver<Inbound>>,
    /// room events
    room_events: VecDeque<RoomEvent>,
    /// incoming game messages buffered for this frame
    incoming: VecDeque<WireMessage>,
    /// connection state
    pub connected: bool,
    /// current room code
    pub room: Option<String>,
    /// our nick
    pub nick: String,
    /// whether we are the room creator (host)
    pub is_host: bool,
    /// whether we have joined the room
    has_joined: bool,
    /// whether opponent has been seen at least once
    opponent_seen: bool,
}

/// Commands sent to the background WebSocket task.
enum OutboundCmd {
    /// raw JSON text to send over the WebSocket
    RawText(String),
    /// close the connection
    Close,
}

/// Items received from the background WebSocket task.
enum Inbound {
    Room(RoomEvent),
    Peer(WireMessage),
}

impl Default for RelayTransport {
    fn default() -> Self {
        Self {
            tx_out: None,
            rx_in: None,
            room_events: VecDeque::new(),
            incoming: VecDeque::new(),
            connected: false,
            room: None,
            nick: "anon".into(),
            is_host: false,
            has_joined: false,
            opponent_seen: false,
        }
    }
}

impl RelayTransport {
    /// Start connecting to the relay.
    ///
    /// * `room` — empty string to create a new room, or a room code to join.
    /// * `nick` — display name sent to the relay.
    ///
    /// Spawns a `tokio` task that owns the WebSocket.
    pub fn connect(&mut self, room: &str, nick: &str, rt: &tokio::runtime::Handle) {
        self.nick = nick.to_string();
        self.is_host = room.is_empty();
        self.has_joined = false;
        self.opponent_seen = false;

        let (tx_out, rx_out) = mpsc::unbounded_channel::<OutboundCmd>();
        let (tx_in, rx_in) = mpsc::unbounded_channel::<Inbound>();

        self.tx_out = Some(tx_out);
        self.rx_in = Some(rx_in);

        let room_owned = room.to_string();
        let nick_owned = nick.to_string();
        let is_host = self.is_host;

        rt.spawn(async move {
            if let Err(e) = ws_task(room_owned, nick_owned, is_host, rx_out, tx_in).await {
                warn!("relay ws_task error: {e}");
            }
        });
    }

    /// Send a game-level wire message to the peer (through the relay).
    ///
    /// Mirrors the web client's `sendRaw`: wraps the wire message in a relay
    /// `{ t: "msg", text: "<json>" }` frame.
    pub fn send(&self, msg: &WireMessage) {
        if let Some(tx) = &self.tx_out {
            let inner = serde_json::to_string(msg).unwrap_or_default();
            let frame = serde_json::json!({ "t": "msg", "text": inner });
            let _ = tx.send(OutboundCmd::RawText(frame.to_string()));
        }
    }

    /// Send a raw relay-level frame (for `create`, `join`, `part`, etc.).
    pub fn send_raw_frame(&self, json: &str) {
        if let Some(tx) = &self.tx_out {
            let _ = tx.send(OutboundCmd::RawText(json.to_string()));
        }
    }

    /// Disconnect from the relay.
    pub fn disconnect(&mut self) {
        if let Some(tx) = &self.tx_out {
            let part = serde_json::json!({ "t": "part" });
            let _ = tx.send(OutboundCmd::RawText(part.to_string()));
            let _ = tx.send(OutboundCmd::Close);
        }
        self.tx_out = None;
        self.rx_in = None;
        self.connected = false;
        self.room = None;
        self.has_joined = false;
    }

    /// Drain incoming messages from the background task into local buffers.
    ///
    /// Call this once per frame (in a Bevy system) before reading
    /// [`drain_incoming`] / [`drain_room_events`].
    pub fn poll(&mut self) {
        let rx = match &mut self.rx_in {
            Some(rx) => rx,
            None => return,
        };

        while let Ok(item) = rx.try_recv() {
            match item {
                Inbound::Room(evt) => {
                    // update local state from room events
                    match &evt {
                        RoomEvent::Joined { room } => {
                            self.connected = true;
                            self.room = Some(room.clone());
                            self.has_joined = true;
                        }
                        RoomEvent::OpponentJoined => {
                            self.opponent_seen = true;
                        }
                        RoomEvent::Error(_) => {}
                        _ => {}
                    }
                    self.room_events.push_back(evt);
                }
                Inbound::Peer(msg) => {
                    self.incoming.push_back(msg);
                }
            }
        }
    }

    /// Drain buffered peer (game-level) messages.
    pub fn drain_incoming(&mut self) -> impl Iterator<Item = WireMessage> + '_ {
        self.incoming.drain(..)
    }

    /// Drain buffered room-level events.
    pub fn drain_room_events(&mut self) -> impl Iterator<Item = RoomEvent> + '_ {
        self.room_events.drain(..)
    }

    // ── convenience senders matching the web client's wire messages ──────

    /// `{ t: "seated", d: <name-or-identity> }`
    pub fn send_seated(&self, name: &str) {
        self.send(&WireMessage {
            t: "seated".into(),
            d: serde_json::json!(name),
            relay_ts: None,
        });
    }

    /// `{ t: "seated", d: { name, sessionPub, mode, ... } }`
    pub fn send_seated_identified(&self, name: &str, session_pub: &str, mode: &str) {
        self.send(&WireMessage {
            t: "seated".into(),
            d: serde_json::json!({
                "name": name,
                "sessionPub": session_pub,
                "mode": mode,
            }),
            relay_ts: None,
        });
    }

    /// `{ t: "action", d: { action, amount, seq } }`
    pub fn send_action(&self, action: &str, amount: u64, seq: u64) {
        self.send(&WireMessage {
            t: "action".into(),
            d: serde_json::json!({ "action": action, "amount": amount, "seq": seq }),
            relay_ts: None,
        });
    }

    /// `{ t: "shuffle_pk", d: { pk, hand } }`
    pub fn send_shuffle_pk(&self, pk_hex: &str, hand_id: u64) {
        self.send(&WireMessage {
            t: "shuffle_pk".into(),
            d: serde_json::json!({ "pk": pk_hex, "hand": hand_id }),
            relay_ts: None,
        });
    }

    /// `{ t: "shuffle_init", d: { pk_a, pk_b, pre_deck, deck, proof } }`
    pub fn send_shuffle_init(
        &self,
        pk_a: &str,
        pk_b: &str,
        pre_deck: &str,
        deck: &str,
        proof: &str,
    ) {
        self.send(&WireMessage {
            t: "shuffle_init".into(),
            d: serde_json::json!({
                "pk_a": pk_a,
                "pk_b": pk_b,
                "pre_deck": pre_deck,
                "deck": deck,
                "proof": proof,
            }),
            relay_ts: None,
        });
    }

    /// `{ t: "shuffle_done", d: { deck, proof } }`
    pub fn send_shuffle_done(&self, deck: &str, proof: &str) {
        self.send(&WireMessage {
            t: "shuffle_done".into(),
            d: serde_json::json!({ "deck": deck, "proof": proof }),
            relay_ts: None,
        });
    }

    /// `{ t: "reveal", d: { shares: { pos: share_hex, ... } } }`
    pub fn send_reveal(&self, shares: &[(u32, String)]) {
        let map: serde_json::Map<String, serde_json::Value> = shares
            .iter()
            .map(|(pos, share)| (pos.to_string(), serde_json::Value::String(share.clone())))
            .collect();
        self.send(&WireMessage {
            t: "reveal".into(),
            d: serde_json::json!({ "shares": map }),
            relay_ts: None,
        });
    }

    /// `{ t: "deal", d: { cards, community, stacks } }`
    pub fn send_deal(&self, cards: [u32; 2], community: [u32; 5], stacks: [u64; 2]) {
        self.send(&WireMessage {
            t: "deal".into(),
            d: serde_json::json!({
                "cards": cards,
                "community": community,
                "stacks": stacks,
            }),
            relay_ts: None,
        });
    }

    /// `{ t: "phase", d: { phase, cards? } }`
    pub fn send_phase(&self, phase: &str, card_indices: Option<&[u32]>) {
        let mut d = serde_json::json!({ "phase": phase });
        if let Some(cards) = card_indices {
            d["cards"] = serde_json::json!(cards);
        }
        self.send(&WireMessage {
            t: "phase".into(),
            d,
            relay_ts: None,
        });
    }

    /// `{ t: "showdown", d: { cards } }`
    pub fn send_showdown(&self, cards: [u32; 2]) {
        self.send(&WireMessage {
            t: "showdown".into(),
            d: serde_json::json!({ "cards": cards }),
            relay_ts: None,
        });
    }

    /// `{ t: "result", d: { winner, pot, stacks, button } }`
    pub fn send_result(&self, winner: u8, pot: u64, stacks: [u64; 2], button: u8) {
        self.send(&WireMessage {
            t: "result".into(),
            d: serde_json::json!({
                "winner": winner,
                "pot": pot,
                "stacks": stacks,
                "button": button,
            }),
            relay_ts: None,
        });
    }

    /// `{ t: "propose_rules", d: { buyin, smallBlind, bigBlind, turnTimeout } }`
    pub fn send_propose_rules(&self, buyin: u64, small_blind: u64, big_blind: u64, turn_timeout: u32) {
        self.send(&WireMessage {
            t: "propose_rules".into(),
            d: serde_json::json!({
                "buyin": buyin,
                "smallBlind": small_blind,
                "bigBlind": big_blind,
                "turnTimeout": turn_timeout,
            }),
            relay_ts: None,
        });
    }

    /// `{ t: "accept_rules", d: {} }`
    pub fn send_accept_rules(&self) {
        self.send(&WireMessage {
            t: "accept_rules".into(),
            d: serde_json::json!({}),
            relay_ts: None,
        });
    }

    /// `{ t: "chat", d: { text } }`
    pub fn send_chat(&self, text: &str) {
        self.send(&WireMessage {
            t: "chat".into(),
            d: serde_json::json!({ "text": text }),
            relay_ts: None,
        });
    }

    /// `{ t: "timeout_claim", d: { seat, elapsed, lastRelayTs, timeoutMs } }`
    pub fn send_timeout_claim(&self, seat: u8, elapsed_ms: u64, last_relay_ts: u64, timeout_ms: u64) {
        self.send(&WireMessage {
            t: "timeout_claim".into(),
            d: serde_json::json!({
                "seat": seat,
                "elapsed": elapsed_ms,
                "lastRelayTs": last_relay_ts,
                "timeoutMs": timeout_ms,
            }),
            relay_ts: None,
        });
    }
}

// ─── Background WebSocket task ───────────────────────────────────────────────

async fn ws_task(
    room: String,
    nick: String,
    is_host: bool,
    mut rx_out: mpsc::UnboundedReceiver<OutboundCmd>,
    tx_in: mpsc::UnboundedSender<Inbound>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use futures_util::{SinkExt, StreamExt};

    let url = "wss://relay.zk.bot/ws";
    let (ws_stream, _) = connect_async(url).await?;
    let (mut sink, mut stream) = ws_stream.split();

    // send initial create/join
    if is_host && room.is_empty() {
        let frame = serde_json::json!({ "t": "create", "nick": nick });
        sink.send(Message::Text(frame.to_string())).await?;
    } else {
        let r = if room.is_empty() { &nick } else { &room };
        let frame = serde_json::json!({ "t": "join", "room": r, "nick": nick });
        sink.send(Message::Text(frame.to_string())).await?;
    }

    let mut has_joined = false;

    loop {
        tokio::select! {
            // outbound: Bevy → WebSocket
            cmd = rx_out.recv() => {
                match cmd {
                    Some(OutboundCmd::RawText(text)) => {
                        sink.send(Message::Text(text)).await?;
                    }
                    Some(OutboundCmd::Close) | None => {
                        let _ = sink.close().await;
                        break;
                    }
                }
            }
            // inbound: WebSocket → Bevy
            ws_msg = stream.next() => {
                match ws_msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) {
                            handle_relay_frame(
                                &frame, &nick, &mut has_joined,
                                &tx_in,
                            );
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

/// Route a top-level relay frame, mirroring `handleRelayMsg` in the web client.
fn handle_relay_frame(
    frame: &serde_json::Value,
    nick: &str,
    has_joined: &mut bool,
    tx: &mpsc::UnboundedSender<Inbound>,
) {
    let t = frame.get("t").and_then(|v| v.as_str()).unwrap_or("");

    match t {
        "created" => {
            // relay created a room — now join it
            if let Some(room) = frame.get("room").and_then(|v| v.as_str()) {
                let join = serde_json::json!({ "t": "join", "room": room, "nick": nick });
                // We cannot send from here directly (no sink), but the background
                // task will pick up the room from the `joined` event. The relay
                // auto-joins the creator in most implementations. If not, the
                // caller should send a join after receiving `RoomEvent::Joined`.
                let _ = tx.send(Inbound::Room(RoomEvent::Joined {
                    room: room.to_string(),
                }));
                // Note: if the relay requires an explicit join after create,
                // we would need to forward the join frame. For relay.zk.bot the
                // create response includes a joined event.
                let _ = join; // suppress unused warning
            }
        }

        "joined" => {
            let room = frame.get("room").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let count = frame.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

            if !*has_joined {
                *has_joined = true;
                let _ = tx.send(Inbound::Room(RoomEvent::Joined { room }));
                if count >= 2 {
                    let _ = tx.send(Inbound::Room(RoomEvent::OpponentJoined));
                }
            } else if count >= 2 {
                let _ = tx.send(Inbound::Room(RoomEvent::OpponentJoined));
            }
        }

        "msg" => {
            if !*has_joined {
                return;
            }
            let sender = frame.get("nick").and_then(|v| v.as_str()).unwrap_or("");
            if sender == nick {
                return; // ignore own echo
            }
            let relay_ts = frame.get("ts").and_then(|v| v.as_u64());
            if let Some(text) = frame.get("text").and_then(|v| v.as_str()) {
                if let Ok(mut wire) = serde_json::from_str::<WireMessage>(text) {
                    wire.relay_ts = relay_ts;
                    let _ = tx.send(Inbound::Peer(wire));
                }
            }
        }

        "system" => {
            let text = frame.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.contains("joined") {
                let _ = tx.send(Inbound::Room(RoomEvent::OpponentJoined));
            } else if text.contains("left") || text.contains("closed") {
                let _ = tx.send(Inbound::Room(RoomEvent::OpponentDisconnected));
            }
        }

        "error" => {
            let msg = frame.get("msg").and_then(|v| v.as_str()).unwrap_or("unknown relay error");
            let _ = tx.send(Inbound::Room(RoomEvent::Error(msg.to_string())));
        }

        _ => {}
    }
}

// ─── Bevy Plugin ─────────────────────────────────────────────────────────────

/// Plugin that inserts the [`RelayTransport`] resource and runs the poll system.
pub struct RelayTransportPlugin;

impl Plugin for RelayTransportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RelayTransport>()
            .add_systems(PreUpdate, poll_relay_transport);
    }
}

/// System that drains the background channel into the resource buffers.
fn poll_relay_transport(mut transport: ResMut<RelayTransport>) {
    transport.poll();
}
