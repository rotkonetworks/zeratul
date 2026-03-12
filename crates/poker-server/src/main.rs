//! poker-server: websocket game server for browser heads-up poker
//!
//! runs GameEngine authoritatively, relays state to two browser clients.

use axum::{
    Router,
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use poker_p2p::engine::*;
use poker_p2p::protocol::{ActionType, TableRules};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tower_http::services::ServeDir;
use zk_shuffle::poker::{Card, Rank, Suit};

// ---------------------------------------------------------------------------
// JSON protocol (browser ↔ server)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    /// player wants to sit down
    Join { name: String },
    /// player action
    Action { action: String, amount: Option<u64> },
    /// start new hand (host only, or auto)
    StartHand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ServerMsg {
    /// assigned seat
    Seated { seat: u8, name: String },
    /// another player joined
    OpponentJoined { seat: u8, name: String },
    /// opponent left
    OpponentLeft { seat: u8 },
    /// hand started
    HandStarted {
        hand_number: u64,
        button: u8,
        your_cards: Option<[CardJson; 2]>,
        stacks: Vec<u64>,
    },
    /// blinds posted
    BlindsPosted { small_blind: (u8, u64), big_blind: (u8, u64) },
    /// it's your turn
    ActionRequired {
        seat: u8,
        valid_actions: Vec<ValidActionJson>,
    },
    /// a player acted
    PlayerActed { seat: u8, action: String, amount: u64, new_stack: u64 },
    /// community cards revealed
    CommunityCards { phase: String, cards: Vec<CardJson> },
    /// pots updated
    PotUpdate { pots: Vec<PotJson> },
    /// showdown — reveal opponent cards
    Showdown { hands: Vec<(u8, [CardJson; 2])> },
    /// pot awarded
    PotAwarded { seat: u8, amount: u64 },
    /// hand complete
    HandComplete { stacks: Vec<u64> },
    /// error
    Error { message: String },
    /// waiting for opponent
    Waiting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CardJson {
    rank: String,
    suit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ValidActionJson {
    kind: String,
    min_amount: u64,
    max_amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PotJson {
    amount: u64,
    eligible: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Game room state
// ---------------------------------------------------------------------------

struct Player {
    name: String,
    seat: u8,
    tx: mpsc::UnboundedSender<ServerMsg>,
}

struct Room {
    players: Vec<Option<Player>>,
    engine: GameEngine,
    hand_number: u64,
    button: u8,
    /// hole cards per seat (kept server-side, sent privately)
    hole_cards: Vec<Option<[Card; 2]>>,
    /// all community cards dealt so far
    community_cards: Vec<Card>,
}

impl Room {
    fn new() -> Self {
        let rules = TableRules {
            small_blind: 5,
            big_blind: 10,
            ante: 0,
            min_buy_in: 1000,
            max_buy_in: 0,
            seats: 2,
            tier: poker_p2p::protocol::SecurityTier::Training,
            allow_spectators: false,
            max_spectators: 0,
            time_bank: 60,
            action_timeout: 30,
        };
        let mut engine = GameEngine::new(rules, 2).unwrap();
        engine.seat_player(0, 1000).unwrap();
        engine.seat_player(1, 1000).unwrap();
        Room {
            players: vec![None, None],
            engine,
            hand_number: 0,
            button: 0,
            hole_cards: vec![None, None],
            community_cards: Vec::new(),
        }
    }

    fn player_count(&self) -> usize {
        self.players.iter().filter(|p| p.is_some()).count()
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
        // auto-rebuy if busted (stack=0 means empty seat to engine)
        for i in 0..2u8 {
            if self.engine.stacks()[i as usize] == 0 {
                let _ = self.engine.seat_player(i, 1000);
            }
        }

        self.hand_number += 1;
        // alternate button
        self.button = if self.hand_number == 1 { 0 } else { 1 - self.button };
        self.hole_cards = vec![None, None];
        self.community_cards = Vec::new();

        let deck = shuffled_deck();
        let events = match self.engine.new_hand(self.button, &deck) {
            Ok(e) => e,
            Err(e) => {
                self.broadcast(&ServerMsg::Error { message: format!("{}", e) });
                return;
            }
        };
        self.process_events(&events);
    }

    fn apply_action(&mut self, seat: u8, action: ActionType) {
        let events = match self.engine.apply_action(seat, action.clone()) {
            Ok(e) => e,
            Err(e) => {
                self.send_to(seat, ServerMsg::Error { message: format!("{}", e) });
                return;
            }
        };
        self.process_events(&events);
    }

    fn process_events(&mut self, events: &[EngineEvent]) {
        for event in events {
            match event {
                EngineEvent::HandStarted { .. } => {
                    // wait for HoleCardsDealt to send full HandStarted per player
                }
                EngineEvent::BlindsPosted { small_blind, big_blind } => {
                    self.broadcast(&ServerMsg::BlindsPosted {
                        small_blind: *small_blind,
                        big_blind: *big_blind,
                    });
                }
                EngineEvent::HoleCardsDealt { seat, cards } => {
                    self.hole_cards[*seat as usize] = Some(*cards);
                    self.send_to(*seat, ServerMsg::HandStarted {
                        hand_number: self.hand_number as u64,
                        button: self.button,
                        your_cards: Some([card_json(&cards[0]), card_json(&cards[1])]),
                        stacks: self.engine.stacks().to_vec(),
                    });
                }
                EngineEvent::ActionRequired { seat, valid_actions } => {
                    let msg = ServerMsg::ActionRequired {
                        seat: *seat,
                        valid_actions: valid_actions.iter().map(|va| ValidActionJson {
                            kind: format!("{:?}", va.kind).to_lowercase(),
                            min_amount: va.min_amount,
                            max_amount: va.max_amount,
                        }).collect(),
                    };
                    // send to all so opponent knows who's acting
                    self.broadcast(&msg);
                }
                EngineEvent::PlayerActed { seat, action, new_stack } => {
                    let (action_str, amount) = action_to_json(action);
                    self.broadcast(&ServerMsg::PlayerActed {
                        seat: *seat,
                        action: action_str,
                        amount,
                        new_stack: *new_stack,
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
                            amount: p.amount,
                            eligible: p.eligible.clone(),
                        }).collect(),
                    });
                }
                EngineEvent::Showdown { .. } => {
                    // reveal all hole cards
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

fn shuffled_deck() -> Vec<Card> {
    let mut deck = Vec::with_capacity(52);
    for &suit in &[Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades] {
        for &rank in &Rank::ALL {
            deck.push(Card { rank, suit });
        }
    }
    let mut rng = rand::thread_rng();
    deck.shuffle(&mut rng);
    deck
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

type SharedRoom = Arc<Mutex<Room>>;

#[derive(Clone)]
struct AppState {
    room: SharedRoom,
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();

    // split socket into sender/receiver using futures
    use futures::{SinkExt, StreamExt};
    let (mut ws_tx, mut ws_rx) = socket.split();

    // forward server messages to websocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_tx.send(WsMessage::Text(json.into())).await.is_err() {
                    break;
                }
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
            Err(e) => {
                let _ = tx.send(ServerMsg::Error { message: format!("bad json: {}", e) });
                continue;
            }
        };

        match client_msg {
            ClientMsg::Join { name } => {
                let mut room = state.room.lock().await;
                let seat = room.players.iter().position(|p| p.is_none());
                if let Some(seat_idx) = seat {
                    let seat = seat_idx as u8;
                    room.players[seat_idx] = Some(Player {
                        name: name.clone(),
                        seat,
                        tx: tx.clone(),
                    });
                    my_seat = Some(seat);
                    let _ = tx.send(ServerMsg::Seated { seat, name: name.clone() });
                    let other = 1 - seat;
                    room.send_to(other, ServerMsg::OpponentJoined { seat, name });
                    if room.player_count() == 2 {
                        room.start_hand();
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
                    None => {
                        let _ = tx.send(ServerMsg::Error { message: "not seated".into() });
                        continue;
                    }
                };
                let action = match parse_action(&action, amount) {
                    Some(a) => a,
                    None => {
                        let _ = tx.send(ServerMsg::Error { message: "unknown action".into() });
                        continue;
                    }
                };
                let mut room = state.room.lock().await;
                room.apply_action(seat, action);
                if room.engine.hand_state().is_none() {
                    let room_clone = state.room.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        let mut room = room_clone.lock().await;
                        if room.engine.hand_state().is_none() && room.player_count() == 2 {
                            room.start_hand();
                        }
                    });
                }
            }
            ClientMsg::StartHand => {
                let mut room = state.room.lock().await;
                if room.engine.hand_state().is_none() && room.player_count() == 2 {
                    room.start_hand();
                }
            }
        }
    }

    if let Some(seat) = my_seat {
        let mut room = state.room.lock().await;
        room.players[seat as usize] = None;
        room.broadcast(&ServerMsg::OpponentLeft { seat });
    }
    send_task.abort();
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("poker_server=info")
        .init();

    let state = AppState {
        room: Arc::new(Mutex::new(Room::new())),
    };

    // serve static files from ./static/ or relative to the binary
    let static_dir = std::env::var("POKER_STATIC_DIR").unwrap_or_else(|_| {
        let exe = std::env::current_exe().unwrap_or_default();
        let dir = exe.parent().unwrap_or(std::path::Path::new("."));
        // check for static/ next to binary, then CWD
        let beside_bin = dir.join("static");
        if beside_bin.exists() {
            beside_bin.to_string_lossy().to_string()
        } else {
            "static".to_string()
        }
    });
    tracing::info!("serving static files from {}", static_dir);

    let app = Router::new()
        .route("/ws", axum::routing::get(ws_handler))
        .fallback_service(ServeDir::new(&static_dir))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("poker server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
