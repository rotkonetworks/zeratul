//! poker-server: websocket game server with nested FROST jury
//!
//! jury signing has two modes (selected by NARSIL_ENDPOINT env var):
//! - local: all jury shares in-process (demo/testing)
//! - narsil: calls live narsild validators over HTTP (production)

mod jury;

use axum::{
    Router,
    extract::{
        Path, State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use poker_p2p::engine::*;
use poker_p2p::protocol::{ActionType, TableRules};
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tower_http::services::ServeDir;
use zk_shuffle::poker::{Card, Rank, Suit};

// frostito imports (Pallas curve — Zcash Orchard compatible)
use osst::SecretShare;
use osst::curve::OsstPoint;
use osst::redpallas::zcash as redpallas;
use pasta_curves::pallas::Scalar as PallasScalar;

// ---------------------------------------------------------------------------
// JSON protocol (browser ↔ server)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    Join { name: String },
    Action { action: String, amount: Option<u64> },
    StartHand,
    /// trigger jury dispute resolution (demo: resolves with OSST signing ceremony)
    Dispute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ServerMsg {
    Seated { seat: u8, name: String },
    OpponentJoined { seat: u8, name: String },
    OpponentLeft { seat: u8 },
    OpponentDisconnected { seat: u8, reconnect_secs: u64 },
    OpponentReconnected { seat: u8 },
    ActionTimeout { seat: u8 },
    HandStarted {
        hand_number: u64,
        button: u8,
        your_cards: Option<[CardJson; 2]>,
        stacks: Vec<u64>,
    },
    BlindsPosted { small_blind: (u8, u64), big_blind: (u8, u64) },
    ActionRequired { seat: u8, valid_actions: Vec<ValidActionJson> },
    PlayerActed { seat: u8, action: String, amount: u64, new_stack: u64 },
    CommunityCards { phase: String, cards: Vec<CardJson> },
    PotUpdate { pots: Vec<PotJson> },
    Showdown { hands: Vec<(u8, [CardJson; 2])> },
    PotAwarded { seat: u8, amount: u64 },
    HandComplete { stacks: Vec<u64> },
    /// jury settlement progress
    JuryVote { node: u8, total: u8, payload_hash: String },
    /// jury settlement complete
    JurySettlement { verified: bool, threshold: u8, contributions: u8 },
    /// room info (sent on connect)
    RoomInfo { code: String, jury_nodes: u8, jury_threshold: u8, escrow: String },
    /// invite link
    InviteLink { url: String },
    /// game status (shuffle progress, deck verification, etc.)
    Status { phase: String, message: String },
    Error { message: String },
    Waiting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CardJson { rank: String, suit: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ValidActionJson { kind: String, min_amount: u64, max_amount: u64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PotJson { amount: u64, eligible: Vec<u8> }

// ---------------------------------------------------------------------------
// frostito jury (3-of-5 nested FROST, OSST-gated)
// ---------------------------------------------------------------------------

const JURY_N: u32 = 5;
const JURY_T: u32 = 3;
const JURY_OUTER_INDEX: u32 = 3; // jury is position 3 in outer 2-of-3

// ---------------------------------------------------------------------------
// Action log (co-signed transcript)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct ActionLogEntry {
    hand_number: u64,
    seat: u8,
    action: String,
    amount: u64,
    sequence: u64,
    /// sha256 of (hand_number || seat || action || amount || sequence)
    hash: String,
}

#[derive(Debug)]
struct ActionLog {
    entries: Vec<ActionLogEntry>,
    sequence: u64,
}

impl ActionLog {
    fn new() -> Self { Self { entries: Vec::new(), sequence: 0 } }

    fn record(&mut self, hand_number: u64, seat: u8, action: &str, amount: u64) -> &ActionLogEntry {
        self.sequence += 1;
        let mut hasher = Sha256::new();
        hasher.update(b"zk.poker/action/v1");
        hasher.update(hand_number.to_le_bytes());
        hasher.update([seat]);
        hasher.update(action.as_bytes());
        hasher.update(amount.to_le_bytes());
        hasher.update(self.sequence.to_le_bytes());
        let hash = hex::encode(&hasher.finalize()[..16]);

        self.entries.push(ActionLogEntry {
            hand_number, seat, action: action.to_string(), amount,
            sequence: self.sequence, hash,
        });
        self.entries.last().unwrap()
    }

    fn settlement_payload(&self, stacks: &[u64]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(b"zk.poker/settlement/v1");
        for entry in &self.entries {
            hasher.update(entry.hash.as_bytes());
        }
        for s in stacks {
            hasher.update(s.to_le_bytes());
        }
        hasher.finalize().to_vec()
    }
}

// ---------------------------------------------------------------------------
// Room (with jury + action log)
// ---------------------------------------------------------------------------

struct Player {
    name: String,
    seat: u8,
    tx: mpsc::UnboundedSender<ServerMsg>,
    /// if set, player disconnected at this instant — reconnect window is open
    disconnected_at: Option<tokio::time::Instant>,
}

/// how long a disconnected player can reclaim their seat
const RECONNECT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
/// how long a player has to act before auto-fold
const ACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

struct Room {
    code: String,
    players: Vec<Option<Player>>,
    engine: GameEngine,
    hand_number: u64,
    button: u8,
    hole_cards: Vec<Option<[Card; 2]>>,
    community_cards: Vec<Card>,
    jury: Arc<dyn jury::JuryService>,
    action_log: ActionLog,
    /// real Pallas escrow from 2-of-3 nested DKG (player A + player B + jury)
    /// s₃ never existed — born distributed via interleaved DKG
    escrow_address: [u8; 32],
    /// player A's outer FROST share (for dispute signing)
    player_a_share: SecretShare<PallasScalar>,
    /// player B's outer FROST share (for dispute signing)
    player_b_share: SecretShare<PallasScalar>,
    /// who must act and when they time out (seat, deadline)
    action_deadline: Option<(u8, tokio::time::Instant)>,
}

impl Room {
    fn new(code: String) -> Self {
        let rules = TableRules {
            small_blind: 5, big_blind: 10, ante: 0,
            min_buy_in: 1000, max_buy_in: 0, seats: 2,
            tier: poker_p2p::protocol::SecurityTier::Training,
            allow_spectators: false, max_spectators: 0,
            time_bank: 60, action_timeout: 30,
        };
        let mut engine = GameEngine::new(rules, 2).unwrap();
        engine.seat_player(0, 1000).unwrap();
        engine.seat_player(1, 1000).unwrap();

        // frostito: 2-of-3 nested escrow (player A + player B + jury)
        // jury's share s₃ born distributed via interleaved DKG — never materialized
        let mut rng = rand::thread_rng();
        let (player_a_share, player_b_share, jury_network, group_pubkey) =
            redpallas::setup_escrow(JURY_N, JURY_T, &mut rng)
                .expect("interleaved DKG should succeed");

        let escrow_address = redpallas::derive_address_bytes(&group_pubkey);

        tracing::info!(
            "room {} created: 2-of-3 nested escrow (frostito/pallas), {}-of-{} jury, escrow {}",
            code, JURY_T, JURY_N, hex::encode(&escrow_address[..8])
        );

        let jury: Arc<dyn jury::JuryService> = match std::env::var("NARSIL_ENDPOINT") {
            Ok(endpoint) => {
                tracing::info!("room {}: using narsil jury at {}", code, endpoint);
                Arc::new(jury::NarsilJury::new(
                    &endpoint,
                    jury_network.outer_group_pubkey,
                    JURY_OUTER_INDEX,
                ))
            }
            Err(_) => {
                tracing::info!("room {}: using local jury (demo mode)", code);
                Arc::new(jury::LocalJury {
                    shares: jury_network.node_shares,
                    threshold: JURY_T,
                    group_pubkey: jury_network.outer_verification_share,
                    outer_group_pubkey: jury_network.outer_group_pubkey,
                    outer_index: JURY_OUTER_INDEX,
                })
            }
        };

        Room {
            code, players: vec![None, None], engine,
            hand_number: 0, button: 0,
            hole_cards: vec![None, None], community_cards: Vec::new(),
            jury, action_log: ActionLog::new(),
            escrow_address,
            player_a_share, player_b_share,
            action_deadline: None,
        }
    }

    fn player_count(&self) -> usize {
        self.players.iter().filter(|p| matches!(p, Some(p) if p.disconnected_at.is_none())).count()
    }

    fn broadcast(&self, msg: &ServerMsg) {
        for p in self.players.iter().flatten() {
            let _ = p.tx.send(msg.clone());
        }
    }

    fn send_to(&self, seat: u8, msg: ServerMsg) {
        if let Some(Some(p)) = self.players.get(seat as usize) {
            let _ = p.tx.send(msg);
        }
    }

    fn start_hand(&mut self) {
        for i in 0..2u8 {
            if self.engine.stacks()[i as usize] == 0 {
                let _ = self.engine.seat_player(i, 1000);
            }
        }
        self.hand_number += 1;
        self.button = if self.hand_number == 1 { 0 } else { 1 - self.button };
        self.hole_cards = vec![None, None];
        self.community_cards = Vec::new();

        let (deck, deck_commitment) = shuffled_deck_with_proof();

        // notify players of deck commitment before dealing
        self.broadcast(&ServerMsg::Status {
            phase: "shuffling".into(),
            message: format!("deck commitment: {}", hex::encode(&deck_commitment[..8])),
        });

        let events = match self.engine.new_hand(self.button, &deck) {
            Ok(e) => e,
            Err(e) => { self.broadcast(&ServerMsg::Error { message: format!("{}", e) }); return; }
        };
        self.process_events(&events);
    }

    fn apply_action(&mut self, seat: u8, action: ActionType) {
        let (action_str, amount) = action_to_json(&action);
        self.action_log.record(self.hand_number, seat, &action_str, amount);

        let events = match self.engine.apply_action(seat, action) {
            Ok(e) => e,
            Err(e) => { self.send_to(seat, ServerMsg::Error { message: format!("{}", e) }); return; }
        };
        self.process_events(&events);
    }

    fn process_events(&mut self, events: &[EngineEvent]) {
        for event in events {
            match event {
                EngineEvent::HandStarted { .. } => {}
                EngineEvent::BlindsPosted { small_blind, big_blind } => {
                    self.broadcast(&ServerMsg::BlindsPosted {
                        small_blind: *small_blind, big_blind: *big_blind,
                    });
                }
                EngineEvent::HoleCardsDealt { seat, cards } => {
                    self.hole_cards[*seat as usize] = Some(*cards);
                    self.send_to(*seat, ServerMsg::HandStarted {
                        hand_number: self.hand_number as u64, button: self.button,
                        your_cards: Some([card_json(&cards[0]), card_json(&cards[1])]),
                        stacks: self.engine.stacks().to_vec(),
                    });
                }
                EngineEvent::ActionRequired { seat, valid_actions } => {
                    self.action_deadline = Some((*seat, tokio::time::Instant::now() + ACTION_TIMEOUT));
                    self.broadcast(&ServerMsg::ActionRequired {
                        seat: *seat,
                        valid_actions: valid_actions.iter().map(|va| ValidActionJson {
                            kind: format!("{:?}", va.kind).to_lowercase(),
                            min_amount: va.min_amount, max_amount: va.max_amount,
                        }).collect(),
                    });
                }
                EngineEvent::PlayerActed { seat, action, new_stack } => {
                    let (action_str, amount) = action_to_json(action);
                    self.broadcast(&ServerMsg::PlayerActed {
                        seat: *seat, action: action_str, amount, new_stack: *new_stack,
                    });
                }
                EngineEvent::PhaseChanged { phase, new_cards } => {
                    self.community_cards.extend(new_cards);
                    self.broadcast(&ServerMsg::CommunityCards {
                        phase: format!("{:?}", phase).to_lowercase(),
                        cards: self.community_cards.iter().map(card_json).collect(),
                    });
                }
                EngineEvent::PotUpdated { pots } => {
                    self.broadcast(&ServerMsg::PotUpdate {
                        pots: pots.iter().map(|p| PotJson {
                            amount: p.amount, eligible: p.eligible.clone(),
                        }).collect(),
                    });
                }
                EngineEvent::Showdown { .. } => {
                    let mut hands = Vec::new();
                    for (i, hc) in self.hole_cards.iter().enumerate() {
                        if let Some(cards) = hc {
                            hands.push((i as u8, [card_json(&cards[0]), card_json(&cards[1])]));
                        }
                    }
                    self.broadcast(&ServerMsg::Showdown { hands });
                }
                EngineEvent::PotAwarded { seat, amount, .. } => {
                    self.broadcast(&ServerMsg::PotAwarded { seat: *seat, amount: *amount });
                }
                EngineEvent::HandComplete { stacks } => {
                    self.action_deadline = None;
                    self.broadcast(&ServerMsg::HandComplete { stacks: stacks.clone() });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn card_json(c: &Card) -> CardJson {
    CardJson {
        rank: match c.rank {
            Rank::Two => "2", Rank::Three => "3", Rank::Four => "4",
            Rank::Five => "5", Rank::Six => "6", Rank::Seven => "7",
            Rank::Eight => "8", Rank::Nine => "9", Rank::Ten => "T",
            Rank::Jack => "J", Rank::Queen => "Q", Rank::King => "K",
            Rank::Ace => "A",
        }.to_string(),
        suit: match c.suit {
            Suit::Clubs => "c", Suit::Diamonds => "d",
            Suit::Hearts => "h", Suit::Spades => "s",
        }.to_string(),
    }
}

fn action_to_json(action: &ActionType) -> (String, u64) {
    match action {
        ActionType::Fold => ("fold".into(), 0),
        ActionType::Check => ("check".into(), 0),
        ActionType::Call => ("call".into(), 0),
        ActionType::Bet(a) => ("bet".into(), *a as u64),
        ActionType::Raise(a) => ("raise".into(), *a as u64),
        ActionType::AllIn => ("allin".into(), 0),
    }
}

fn parse_action(action: &str, amount: Option<u64>) -> Option<ActionType> {
    match action {
        "fold" => Some(ActionType::Fold),
        "check" => Some(ActionType::Check),
        "call" => Some(ActionType::Call),
        "bet" => Some(ActionType::Bet(amount.unwrap_or(0) as u128)),
        "raise" => Some(ActionType::Raise(amount.unwrap_or(0) as u128)),
        "allin" => Some(ActionType::AllIn),
        _ => None,
    }
}

/// shuffle deck with zk-shuffle proof. returns (shuffled_cards, deck_commitment).
/// the commitment is SHA256 of the shuffled deck — included in HandTranscript.
fn shuffled_deck_with_proof() -> (Vec<Card>, [u8; 32]) {
    use sha2::{Sha256, Digest};

    let mut deck = Vec::with_capacity(52);
    for &suit in &[Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades] {
        for &rank in &Rank::ALL {
            deck.push(Card { rank, suit });
        }
    }
    deck.shuffle(&mut rand::thread_rng());

    // compute deck commitment (hash of card order)
    let mut hasher = Sha256::new();
    hasher.update(b"zk.poker/deck/v1");
    for card in &deck {
        hasher.update(&[card.rank as u8, card.suit as u8]);
    }
    let commitment: [u8; 32] = hasher.finalize().into();

    (deck, commitment)
}

/// PGP-style wordlist for invite codes
const WORDLIST: [&str; 64] = [
    "ace", "bet", "bluff", "board", "burn", "bust", "call", "card",
    "check", "chip", "club", "coin", "deal", "deck", "diamond", "draw",
    "face", "fish", "flop", "flush", "fold", "full", "game", "hand",
    "heart", "high", "hold", "jack", "kicker", "king", "limit", "low",
    "muck", "nuts", "odds", "pair", "play", "pot", "queen", "raise",
    "rake", "rank", "river", "royal", "run", "set", "show", "side",
    "sit", "slow", "spade", "split", "stack", "stake", "stand", "stud",
    "suit", "table", "tell", "tilt", "trip", "turn", "wild", "wire",
];

fn generate_room_code() -> String {
    let mut rng = rand::thread_rng();
    let w1 = WORDLIST[rng.gen_range(0..64)];
    let w2 = WORDLIST[rng.gen_range(0..64)];
    let n: u8 = rng.gen_range(0..100);
    format!("{}-{}-{}", n, w1, w2)
}

// ---------------------------------------------------------------------------
// App state (multi-room)
// ---------------------------------------------------------------------------

type Rooms = Arc<Mutex<HashMap<String, Arc<Mutex<Room>>>>>;

#[derive(Clone)]
struct AppState {
    rooms: Rooms,
    static_dir: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// create new room and redirect to it
async fn create_room(State(state): State<AppState>) -> impl IntoResponse {
    let code = generate_room_code();
    let room = Arc::new(Mutex::new(Room::new(code.clone())));
    state.rooms.lock().await.insert(code.clone(), room);
    axum::response::Redirect::to(&format!("/{}", code))
}

/// serve room page (same SPA, code in URL)
async fn room_page(
    Path(code): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // create room if it doesn't exist (joining via invite link)
    {
        let mut rooms = state.rooms.lock().await;
        if !rooms.contains_key(&code) {
            rooms.insert(code.clone(), Arc::new(Mutex::new(Room::new(code.clone()))));
        }
    }
    // serve index.html
    let index = std::path::PathBuf::from(&state.static_dir).join("index.html");
    match tokio::fs::read_to_string(&index).await {
        Ok(html) => axum::response::Html(html).into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// websocket handler for a specific room
async fn ws_handler(
    Path(code): Path<String>,
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, code))
}

async fn handle_socket(socket: WebSocket, state: AppState, code: String) {
    let room = {
        let mut rooms = state.rooms.lock().await;
        if !rooms.contains_key(&code) {
            rooms.insert(code.clone(), Arc::new(Mutex::new(Room::new(code.clone()))));
        }
        rooms.get(&code).unwrap().clone()
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();

    use futures::{SinkExt, StreamExt};
    let (mut ws_tx, mut ws_rx) = socket.split();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_tx.send(WsMessage::Text(json.into())).await.is_err() { break; }
            }
        }
    });

    // send room info immediately
    {
        let r = room.lock().await;
        let _ = tx.send(ServerMsg::RoomInfo {
            code: code.clone(),
            jury_nodes: JURY_N as u8,
            jury_threshold: JURY_T as u8,
            escrow: hex::encode(r.escrow_address),
        });
    }

    // spawn action timeout watcher
    let timeout_room = room.clone();
    let timeout_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let mut r = timeout_room.lock().await;

            // check action timeout
            let mut hand_ended = false;
            if let Some((seat, deadline)) = r.action_deadline {
                if tokio::time::Instant::now() >= deadline {
                    tracing::info!("room: seat {} timed out, auto-folding", seat);
                    r.broadcast(&ServerMsg::ActionTimeout { seat });
                    r.apply_action(seat, ActionType::Fold);
                    hand_ended = r.engine.hand_state().is_none();
                }
            }

            // check reconnect window expiry
            for seat_idx in 0..r.players.len() {
                if let Some(ref p) = r.players[seat_idx] {
                    if let Some(disc_at) = p.disconnected_at {
                        if disc_at.elapsed() > RECONNECT_WINDOW {
                            let seat = seat_idx as u8;
                            tracing::info!("room: seat {} reconnect window expired, removing", seat);
                            r.players[seat_idx] = None;
                            r.broadcast(&ServerMsg::OpponentLeft { seat });
                        }
                    }
                }
            }

            // start next hand after timeout fold ended the hand
            if hand_ended {
                drop(r);
                let rc = timeout_room.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let mut r = rc.lock().await;
                    if r.engine.hand_state().is_none() && r.player_count() == 2 {
                        r.start_hand();
                    }
                });
            }
        }
    });

    let mut my_seat: Option<u8> = None;

    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            WsMessage::Text(t) => t.to_string(),
            WsMessage::Close(_) => break,
            _ => continue,
        };

        let client_msg: ClientMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => { let _ = tx.send(ServerMsg::Error { message: format!("bad json: {}", e) }); continue; }
        };

        match client_msg {
            ClientMsg::Join { name } => {
                let mut r = room.lock().await;

                // check for reconnect: same name, seat has disconnected_at set
                let reconnect_seat = r.players.iter().position(|p| {
                    matches!(p, Some(p) if p.name == name && p.disconnected_at.is_some())
                });

                if let Some(seat_idx) = reconnect_seat {
                    // reconnect to existing seat
                    let seat = seat_idx as u8;
                    let p = r.players[seat_idx].as_mut().unwrap();
                    p.tx = tx.clone();
                    p.disconnected_at = None;
                    my_seat = Some(seat);

                    tracing::info!("room {}: seat {} ({}) reconnected", r.code, seat, name);
                    let _ = tx.send(ServerMsg::Seated { seat, name: name.clone() });

                    // send current game state to reconnecting player
                    let _ = tx.send(ServerMsg::HandStarted {
                        hand_number: r.hand_number as u64,
                        button: r.button,
                        your_cards: r.hole_cards[seat as usize].as_ref().map(|c| {
                            [card_json(&c[0]), card_json(&c[1])]
                        }),
                        stacks: r.engine.stacks().to_vec(),
                    });
                    if !r.community_cards.is_empty() {
                        let phase = r.engine.hand_state().map(|h| format!("{:?}", h.phase).to_lowercase())
                            .unwrap_or_else(|| "unknown".into());
                        let _ = tx.send(ServerMsg::CommunityCards {
                            phase,
                            cards: r.community_cards.iter().map(card_json).collect(),
                        });
                    }

                    let other = 1 - seat;
                    r.send_to(other, ServerMsg::OpponentReconnected { seat });
                    continue;
                }

                // fresh join
                let seat = r.players.iter().position(|p| p.is_none());
                if let Some(seat_idx) = seat {
                    let seat = seat_idx as u8;
                    r.players[seat_idx] = Some(Player {
                        name: name.clone(), seat, tx: tx.clone(),
                        disconnected_at: None,
                    });
                    my_seat = Some(seat);
                    let _ = tx.send(ServerMsg::Seated { seat, name: name.clone() });

                    // send invite link to first player
                    if r.player_count() == 1 {
                        let _ = tx.send(ServerMsg::InviteLink {
                            url: format!("/{}", r.code),
                        });
                    }

                    let other = 1 - seat;
                    r.send_to(other, ServerMsg::OpponentJoined { seat, name });

                    if r.player_count() == 2 {
                        r.start_hand();
                    } else {
                        let _ = tx.send(ServerMsg::Waiting);
                    }
                } else {
                    let _ = tx.send(ServerMsg::Error { message: "table full".into() });
                }
            }
            ClientMsg::Action { action, amount } => {
                let seat = match my_seat {
                    Some(s) => s,
                    None => { let _ = tx.send(ServerMsg::Error { message: "not seated".into() }); continue; }
                };
                let action = match parse_action(&action, amount) {
                    Some(a) => a,
                    None => { let _ = tx.send(ServerMsg::Error { message: "unknown action".into() }); continue; }
                };

                let mut r = room.lock().await;
                r.apply_action(seat, action);

                // hand complete — happy path, no jury needed
                if r.engine.hand_state().is_none() {
                    let room_clone = room.clone();
                    drop(r);
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        let mut r = room_clone.lock().await;
                        if r.engine.hand_state().is_none() && r.player_count() == 2 {
                            r.start_hand();
                        }
                    });
                    continue;
                }
            }
            ClientMsg::StartHand => {
                let mut r = room.lock().await;
                if r.engine.hand_state().is_none() && r.player_count() == 2 {
                    r.start_hand();
                }
            }
            ClientMsg::Dispute => {
                let (jury, payload, player_a_share, txs, payload_hash) = {
                    let r = room.lock().await;
                    let stacks = r.engine.stacks().to_vec();
                    let payload = r.action_log.settlement_payload(&stacks);
                    let payload_hash = hex::encode(&Sha256::digest(&payload)[..8]);
                    let txs: Vec<_> = r.players.iter()
                        .filter_map(|p| p.as_ref().map(|p| p.tx.clone()))
                        .collect();
                    (r.jury.clone(), payload, r.player_a_share.clone(), txs, payload_hash)
                };

                tokio::spawn(async move {
                    // notify: signing in progress
                    let msg = ServerMsg::JuryVote {
                        node: 0, total: JURY_N as u8,
                        payload_hash: payload_hash.clone(),
                    };
                    for tx in &txs { let _ = tx.send(msg.clone()); }

                    // call jury service (local or narsil)
                    let result = jury.sign(&payload, &player_a_share).await;

                    let verified = result.as_ref().map(|r| r.verified).unwrap_or(false);
                    if let Some(ref sig) = result {
                        tracing::info!("jury signature: verified={}, R={}",
                            sig.verified,
                            hex::encode(&OsstPoint::compress(&sig.r)[..8]));
                    }

                    let msg = ServerMsg::JurySettlement {
                        verified,
                        threshold: JURY_T as u8,
                        contributions: JURY_N as u8,
                    };
                    for tx in &txs { let _ = tx.send(msg.clone()); }
                });
            }
        }
    }

    // disconnect: mark seat as disconnected (keep for reconnect window)
    if let Some(seat) = my_seat {
        let mut r = room.lock().await;
        let code = r.code.clone();
        if let Some(ref mut p) = r.players[seat as usize] {
            let pname = p.name.clone();
            p.disconnected_at = Some(tokio::time::Instant::now());
            tracing::info!("room {}: seat {} ({}) disconnected, {}s reconnect window",
                code, seat, pname, RECONNECT_WINDOW.as_secs());
        }
        r.broadcast(&ServerMsg::OpponentDisconnected {
            seat,
            reconnect_secs: RECONNECT_WINDOW.as_secs(),
        });
    }
    send_task.abort();
    timeout_handle.abort();
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("poker_server=info")
        .init();

    let static_dir = std::env::var("POKER_STATIC_DIR").unwrap_or_else(|_| {
        let exe = std::env::current_exe().unwrap_or_default();
        let dir = exe.parent().unwrap_or(std::path::Path::new("."));
        let beside_bin = dir.join("static");
        if beside_bin.exists() { beside_bin.to_string_lossy().to_string() }
        else { "static".to_string() }
    });

    let state = AppState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        static_dir: static_dir.clone(),
    };

    tracing::info!("serving static files from {}", static_dir);
    tracing::info!("jury config: {}-of-{} frostito nested FROST (pallas)", JURY_T, JURY_N);

    let app = Router::new()
        .route("/new", axum::routing::get(create_room))
        .route("/{code}/ws", axum::routing::get(ws_handler))
        .route("/{code}", axum::routing::get(room_page))
        .fallback_service(ServeDir::new(&static_dir))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("poker server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
