//! cfr-bot: standalone poker bot that connects via WebSocket
//!
//! Connects to the relay as a regular player. No special access.
//! Uses the Brain (L0 blueprint + optional L4 MoE) for decisions.
//!
//! usage:
//!   cfr-bot --server ws://localhost:3030/room123/ws --strategy strategy.bin --name "alice"

use poker_pvm::cfr::brain::{Brain, FilterStack, Decision};
use poker_pvm::cfr::abstraction::*;
use poker_pvm::*;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tungstenite::{connect, Message};

// --- Wire types (matching poker-server) ---

#[derive(Serialize)]
#[serde(tag = "type")]
enum ClientMsg {
    Join { name: String, pubkey: Option<String> },
    Action { action: String, amount: Option<u64> },
    Chat { text: String },
    StartHand,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ServerMsg {
    Seated { seat: u8 },
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
    InviteLink { url: String },
    Status { phase: String, message: String },
    Chat { seat: u8, name: String, text: String },
    Error { message: String },
    Waiting,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct CardJson { rank: String, suit: String }

#[derive(Debug, Deserialize)]
struct ValidActionJson { kind: String, min_amount: u64, max_amount: u64 }

#[derive(Debug, Deserialize)]
struct PotJson { amount: u64 }

// --- Card parsing ---

fn card_index(card: &CardJson) -> u8 {
    let rank = match card.rank.as_str() {
        "2" => 0, "3" => 1, "4" => 2, "5" => 3, "6" => 4, "7" => 5,
        "8" => 6, "9" => 7, "10" | "T" => 8, "J" => 9, "Q" => 10, "K" => 11, "A" => 12,
        _ => 0,
    };
    let suit = match card.suit.as_str() {
        "clubs" | "c" => 0, "diamonds" | "d" => 1, "hearts" | "h" => 2, "spades" | "s" => 3,
        _ => 0,
    };
    rank + suit * 13
}

fn phase_from_str(s: &str) -> Phase {
    match s {
        "flop" => Phase::Flop, "turn" => Phase::Turn, "river" => Phase::River,
        _ => Phase::Preflop,
    }
}

fn action_from_str(s: &str) -> Action {
    match s {
        "fold" => Action::Fold, "check" => Action::Check, "call" => Action::Call,
        "bet" => Action::Bet, "raise" => Action::Raise, "allin" => Action::AllIn,
        _ => Action::Check,
    }
}

// --- Bot state ---

struct BotState {
    brain: Brain,
    my_seat: Option<u8>,
    my_cards: Option<[u8; 2]>,
    community: [u8; 5],
    community_count: u8,
    stacks: Vec<u64>,
    bets: Vec<u64>,
    pot: u64,
    button: u8,
    hand_number: u32,
    phase: Phase,
    num_players: u8,
}

impl BotState {
    fn new(strategy: &[u8]) -> Self {
        Self {
            brain: Brain::new(strategy),
            my_seat: None,
            my_cards: None,
            community: [0; 5],
            community_count: 0,
            stacks: vec![0; 10],
            bets: vec![0; 10],
            pot: 0,
            button: 0,
            hand_number: 0,
            phase: Phase::Preflop,
            num_players: 2,
        }
    }

    fn game_state(&self) -> GameState {
        let mut gs_stacks = [0u32; MAX_SEATS];
        let mut gs_bets = [0u32; MAX_SEATS];
        let mut seat_state = [SeatState::Empty; MAX_SEATS];
        for (i, &s) in self.stacks.iter().enumerate().take(MAX_SEATS) {
            gs_stacks[i] = s as u32;
            gs_bets[i] = self.bets.get(i).copied().unwrap_or(0) as u32;
            if s > 0 { seat_state[i] = SeatState::Active; }
        }
        GameState {
            stacks: gs_stacks, bets: gs_bets,
            pot: self.pot as u32, community: self.community,
            community_count: self.community_count, phase: self.phase,
            acting_seat: self.my_seat.unwrap_or(0),
            num_players: self.num_players, hand_number: self.hand_number,
            button: self.button, seat_state,
            cards: [[0; 2]; MAX_SEATS],
            round_actions: 0, last_aggressor: 0, action_count: 0,
            last_action_hash: [0; 32], rake: 0,
            rules: Rules { buyin: 1000, small_blind: 5, big_blind: 10,
                turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0 },
        }
    }

    fn decide(&mut self, valid_actions: &[ValidActionJson]) -> (String, Option<u64>) {
        let seat = self.my_seat.unwrap_or(0);
        let cards = self.my_cards.unwrap_or([0, 0]);
        let state = self.game_state();

        let decision = self.brain.decide(&state, &cards, &self.community);

        // map decision to a valid action
        if let Some((action, amount)) = decision.sample(rand_f64()) {
            let action_str = match action {
                Action::Fold => "fold", Action::Check => "check", Action::Call => "call",
                Action::Bet => "bet", Action::Raise => "raise", Action::AllIn => "allin",
            };
            // verify it's valid
            if valid_actions.iter().any(|v| v.kind == action_str) {
                let amt = if amount > 0 { Some(amount as u64) } else { None };
                return (action_str.to_string(), amt);
            }
        }

        // fallback: check > call > fold
        for pref in &["check", "call", "fold"] {
            if valid_actions.iter().any(|v| v.kind == *pref) {
                return (pref.to_string(), None);
            }
        }
        ("fold".to_string(), None)
    }
}

fn rand_f64() -> f64 {
    use std::time::SystemTime;
    let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().subsec_nanos();
    (t as f64) / (u32::MAX as f64)
}

// --- Behavioral timing ---

fn think_delay(action: &str) -> Duration {
    let base = match action {
        "fold" => 1.0,
        "check" => 1.5,
        "call" => 2.5,
        "bet" | "raise" => 4.0,
        "allin" => 6.0,
        _ => 2.0,
    };
    // add jitter ±30%
    let jitter = 0.7 + rand_f64() * 0.6;
    Duration::from_secs_f64(base * jitter)
}

// --- Main ---

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut server_url = "ws://localhost:3030/test/ws".to_string();
    let mut strategy_path = "strategy.bin".to_string();
    let mut bot_name = "player".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--server" => { if i+1 < args.len() { server_url = args[i+1].clone(); i += 1; } }
            "--strategy" => { if i+1 < args.len() { strategy_path = args[i+1].clone(); i += 1; } }
            "--name" => { if i+1 < args.len() { bot_name = args[i+1].clone(); i += 1; } }
            _ => {}
        }
        i += 1;
    }

    let strategy = std::fs::read(&strategy_path)
        .unwrap_or_else(|e| { eprintln!("failed to load {}: {}", strategy_path, e); std::process::exit(1); });

    println!("connecting to {}...", server_url);
    let (mut ws, _response) = connect(&server_url)
        .unwrap_or_else(|e| { eprintln!("failed to connect: {}", e); std::process::exit(1); });

    println!("connected. joining as '{}'", bot_name);

    // join
    let join_msg = serde_json::to_string(&ClientMsg::Join { name: bot_name.clone(), pubkey: None }).unwrap();
    ws.send(Message::Text(join_msg.into())).unwrap();

    let mut state = BotState::new(&strategy);
    let mut hands_played = 0u64;

    // main loop
    loop {
        let msg = match ws.read() {
            Ok(Message::Text(t)) => t.to_string(),
            Ok(Message::Close(_)) => { println!("server closed connection"); break; }
            Err(e) => { eprintln!("ws error: {}", e); break; }
            _ => continue,
        };

        let server_msg: ServerMsg = match serde_json::from_str(&msg) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match server_msg {
            ServerMsg::Seated { seat, .. } => {
                state.my_seat = Some(seat);
                println!("seated at {}", seat);
            }
            ServerMsg::Waiting => {
                println!("waiting for opponent...");
            }
            ServerMsg::OpponentJoined { seat, name } => {
                println!("opponent joined: {} (seat {})", name, seat);
            }
            ServerMsg::HandStarted { hand_number, button, your_cards, stacks } => {
                state.hand_number = hand_number as u32;
                state.button = button;
                state.stacks = stacks;
                state.bets = vec![0; state.stacks.len()];
                state.pot = 0;
                state.community = [0; 5];
                state.community_count = 0;
                state.phase = Phase::Preflop;
                state.num_players = state.stacks.len() as u8;

                if let Some(cards) = your_cards {
                    let c = [card_index(&cards[0]), card_index(&cards[1])];
                    state.my_cards = Some(c);
                }

                let gs = state.game_state();
                state.brain.new_hand(&gs);
                if let Some(cards) = state.my_cards {
                    state.brain.set_hero_cards(state.my_seat.unwrap_or(0), cards, &gs);
                }

                hands_played += 1;
                println!("hand #{} (btn={}, cards={:?})", hand_number, button,
                    state.my_cards.map(|c| format!("{},{}", c[0], c[1])).unwrap_or_default());
            }
            ServerMsg::BlindsPosted { small_blind, big_blind } => {
                if let Some(b) = state.bets.get_mut(small_blind.0 as usize) { *b = small_blind.1; }
                if let Some(b) = state.bets.get_mut(big_blind.0 as usize) { *b = big_blind.1; }
                state.pot = small_blind.1 + big_blind.1;
            }
            ServerMsg::ActionRequired { seat, valid_actions } => {
                if Some(seat) == state.my_seat {
                    let (action, amount) = state.decide(&valid_actions);
                    let delay = think_delay(&action);
                    std::thread::sleep(delay);

                    let msg = serde_json::to_string(&ClientMsg::Action {
                        action: action.clone(), amount,
                    }).unwrap();
                    ws.send(Message::Text(msg.into())).unwrap();
                    println!("  -> {} {}", action, amount.map(|a| a.to_string()).unwrap_or_default());
                }
            }
            ServerMsg::PlayerActed { seat, action, amount, .. } => {
                if Some(seat) != state.my_seat {
                    let gs = state.game_state();
                    state.brain.observe_action(seat, action_from_str(&action), amount as u32, &gs);
                }
                // update bets/pot
                if let Some(b) = state.bets.get_mut(seat as usize) {
                    *b += amount;
                }
                state.pot += amount;
            }
            ServerMsg::CommunityCards { phase, cards } => {
                state.phase = phase_from_str(&phase);
                for (i, card) in cards.iter().enumerate().take(5) {
                    state.community[i] = card_index(card);
                }
                state.community_count = cards.len() as u8;
                let gs = state.game_state();
                let new_cards: Vec<u8> = cards.iter().map(card_index).collect();
                state.brain.reveal_community(&new_cards, &gs);
                // reset bets for new street
                state.bets = vec![0; state.stacks.len()];
            }
            ServerMsg::PotAwarded { seat, amount } => {
                let won = Some(seat) == state.my_seat;
                println!("  {} wins {} {}", if won { "we" } else { "they" }, amount,
                    if won { "(+)" } else { "(-)" });
            }
            ServerMsg::HandComplete { stacks } => {
                state.stacks = stacks;
            }
            ServerMsg::Chat { name, text, .. } => {
                println!("  [{}] {}", name, text);
            }
            ServerMsg::Error { message } => {
                eprintln!("  error: {}", message);
            }
            _ => {}
        }
    }

    println!("disconnected after {} hands", hands_played);
}
