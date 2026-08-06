//! poker-relay: WebSocket relay for P2P poker signaling.
//!
//! speaks the relay.zk.bot protocol:
//!   client sends: { t: 'create', nick } | { t: 'join', room, nick } | { t: 'msg', text } | { t: 'part' }
//!   server sends: { t: 'created', room } | { t: 'joined', room, count } | { t: 'msg', nick, text } | { t: 'system', text }
//!
//! no game logic, no crypto. just rooms + message forwarding.
//! all game messages are encrypted (AES-256-GCM) by the client.
//! the relay sees opaque JSON strings.

use axum::{Router, extract::{State, ws::{Message, WebSocket, WebSocketUpgrade}}, response::IntoResponse};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

// ── types ──

struct Player {
    nick: String,
    tx: mpsc::UnboundedSender<Message>,
}

struct Room {
    code: String,
    created_at: Instant,
    ttl: Duration,
    players: Vec<Player>,
    max_players: usize,
}

impl Room {
    fn expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }

    fn broadcast(&self, msg: &str) {
        for p in &self.players {
            let _ = p.tx.send(Message::text(msg.to_string()));
        }
    }

    fn broadcast_except(&self, nick: &str, msg: &str) {
        for p in &self.players {
            if p.nick != nick {
                let _ = p.tx.send(Message::text(msg.to_string()));
            }
        }
    }
}

type Rooms = Arc<Mutex<HashMap<String, Room>>>;

// ── room code generation ──

const WORDS: &[&str] = &[
    "acid", "aqua", "arch", "atom", "axle", "bare", "bark", "beam",
    "bell", "bird", "blot", "blue", "bold", "bolt", "bone", "born",
    "calm", "cape", "cave", "clad", "clay", "coal", "cold", "cone",
    "core", "curl", "dark", "dawn", "deep", "dew", "dial", "dome",
    "drop", "drum", "dune", "dusk", "dust", "echo", "edge", "even",
    "fade", "fern", "film", "fire", "fish", "flag", "foam", "fold",
    "fork", "frog", "gale", "gate", "gaze", "glow", "gold", "gray",
    "grip", "gust", "haze", "helm", "hint", "hive", "hook", "horn",
    "hull", "hunt", "iron", "isle", "jade", "jazz", "keen", "kelp",
    "knot", "lake", "lamp", "lark", "leaf", "lime", "link", "loom",
    "lynx", "malt", "mane", "mare", "mesa", "mill", "mint", "mist",
    "moon", "moss", "moth", "muse", "neon", "nest", "node", "noon",
    "nova", "opal", "orca", "palm", "pane", "peak", "pine", "plum",
    "pond", "port", "pyre", "rain", "reed", "reef", "rift", "ring",
    "rise", "rock", "root", "rose", "ruby", "rush", "rust", "sage",
    "salt", "sand", "seal", "seed", "silk", "snow", "soil", "song",
    "star", "stem", "tide", "tint", "toad", "tree", "turn", "vale",
    "veil", "vine", "void", "volt", "wake", "warm", "wave", "well",
    "whip", "wild", "wind", "wing", "wire", "wolf", "wren", "zinc",
    "zone",
];

fn generate_room_code() -> String {
    let mut rng = rand::thread_rng();
    let w1 = WORDS[rng.gen_range(0..WORDS.len())];
    let w2 = WORDS[rng.gen_range(0..WORDS.len())];
    let w3 = WORDS[rng.gen_range(0..WORDS.len())];
    format!("{w1}-{w2}-{w3}")
}

// ── WebSocket handler ──

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(rooms): State<Rooms>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_session(socket, rooms))
}

async fn handle_session(socket: WebSocket, rooms: Rooms) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    let mut nick = String::new();
    let mut current_room: Option<String> = None;

    // forward channel messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // process incoming messages
    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match &msg {
            Message::Text(t) => t.clone(),
            Message::Close(_) => break,
            _ => continue,
        };

        let parsed: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let t = parsed["t"].as_str().unwrap_or("");

        match t {
            "create" => {
                nick = parsed["nick"].as_str().unwrap_or("anon").to_string();
                let code = generate_room_code();
                let room = Room {
                    code: code.clone(),
                    created_at: Instant::now(),
                    ttl: Duration::from_secs(3600),
                    players: Vec::new(),
                    max_players: 10,
                };
                rooms.lock().await.insert(code.clone(), room);
                let _ = tx.send(Message::text(json!({ "t": "created", "room": code }).to_string()));
            }

            "join" => {
                let room_code = parsed["room"].as_str().unwrap_or("").to_string();
                if let Some(n) = parsed["nick"].as_str() {
                    nick = n.to_string();
                }

                // leave current room if any
                if let Some(ref old) = current_room {
                    let mut rooms = rooms.lock().await;
                    if let Some(room) = rooms.get_mut(old) {
                        room.players.retain(|p| p.nick != nick);
                        let sys = json!({ "t": "system", "text": format!("{nick} left") }).to_string();
                        room.broadcast(&sys);
                        if room.players.is_empty() {
                            rooms.remove(old);
                        }
                    }
                }

                let mut rooms = rooms.lock().await;

                // auto-create room if it doesn't exist
                if !rooms.contains_key(&room_code) {
                    rooms.insert(room_code.clone(), Room {
                        code: room_code.clone(),
                        created_at: Instant::now(),
                        ttl: Duration::from_secs(3600),
                        players: Vec::new(),
                        max_players: 10,
                    });
                }

                if let Some(room) = rooms.get_mut(&room_code) {
                    if room.expired() {
                        let _ = tx.send(Message::text(json!({ "t": "error", "msg": "room expired" }).to_string()));
                        continue;
                    }
                    if room.players.len() >= room.max_players {
                        let _ = tx.send(Message::text(json!({ "t": "error", "msg": "room full" }).to_string()));
                        continue;
                    }

                    // remove stale entry with same nick (reconnect)
                    room.players.retain(|p| p.nick != nick);

                    room.players.push(Player { nick: nick.clone(), tx: tx.clone() });
                    current_room = Some(room_code.clone());

                    let count = room.players.len();
                    let join_msg = json!({ "t": "joined", "room": room_code, "count": count }).to_string();
                    room.broadcast(&join_msg);

                    // notify others
                    let sys = json!({ "t": "system", "text": format!("{nick} joined ({count} players)") }).to_string();
                    room.broadcast_except(&nick, &sys);
                }
            }

            "msg" => {
                let text = parsed["text"].as_str().unwrap_or("").to_string();
                if text.is_empty() || nick.is_empty() { continue; }

                if let Some(ref code) = current_room {
                    let rooms = rooms.lock().await;
                    if let Some(room) = rooms.get(code) {
                        let relay_msg = json!({
                            "t": "msg",
                            "nick": nick,
                            "text": text,
                            "ts": std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        }).to_string();
                        room.broadcast_except(&nick, &relay_msg);
                    }
                }
            }

            "part" => {
                if let Some(ref code) = current_room {
                    let mut rooms = rooms.lock().await;
                    if let Some(room) = rooms.get_mut(code) {
                        room.players.retain(|p| p.nick != nick);
                        let sys = json!({ "t": "system", "text": format!("{nick} left") }).to_string();
                        room.broadcast(&sys);
                        if room.players.is_empty() {
                            rooms.remove(code);
                        }
                    }
                }
                current_room = None;
            }

            _ => {}
        }
    }

    // player disconnected - clean up
    if let Some(ref code) = current_room {
        let mut rooms = rooms.lock().await;
        if let Some(room) = rooms.get_mut(code) {
            room.players.retain(|p| p.nick != nick);
            let count = room.players.len();
            let sys = json!({ "t": "system", "text": format!("{nick} disconnected ({count} players)") }).to_string();
            room.broadcast(&sys);
            if room.players.is_empty() {
                rooms.remove(code);
            }
        }
    }

    send_task.abort();
}

// ── cleanup task ──

async fn cleanup_expired(rooms: Rooms) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut rooms = rooms.lock().await;
        rooms.retain(|_, room| !room.expired());
    }
}

// ── main ──

#[tokio::main]
async fn main() {
    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
    tokio::spawn(cleanup_expired(rooms.clone()));

    let static_dir = std::env::var("POKER_STATIC_DIR").unwrap_or_else(|_| "static".into());

    let app = Router::new()
        .route("/ws", axum::routing::get(ws_handler))
        .route("/ws/lobby", axum::routing::get(ws_handler))
        .fallback_service(ServeDir::new(&static_dir).append_index_html_on_directories(true))
        .layer(CorsLayer::permissive())
        .with_state(rooms);

    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    println!("poker-relay listening on {addr}");
    println!("static: {static_dir}");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
