//! poker-server: websocket game server with OSST jury
//!
//! each room has a wormhole-style invite code. the server holds a 3-of-5
//! OSST jury internally — on settlement, jury nodes "vote" with staggered
//! delays to simulate distributed consensus. every action is logged into
//! a HandTranscript for dispute resolution.

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
use osst::{SecretShare, Contribution, verify as osst_verify};
use osst::curve::{OsstPoint, OsstScalar};
use osst::redpallas::zcash as redpallas;
use osst::nested;
use osst::frost;
use pasta_curves::pallas::{Point as PallasPoint, Scalar as PallasScalar};

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

struct Jury {
    shares: Vec<SecretShare<PallasScalar>>,
    /// jury's verification share in the outer protocol (g^{s₃})
    /// s₃ never existed as a scalar — born distributed via interleaved DKG
    group_pubkey: PallasPoint,
    /// outer group public key (the escrow address)
    outer_group_pubkey: PallasPoint,
}

impl Jury {
    /// OSST authorization: jury nodes prove threshold control (async, non-interactive)
    fn prepare_contributions(&self, payload: &[u8]) -> Vec<(Contribution<PallasPoint>, u64)> {
        let mut rng = rand::thread_rng();
        (0..self.shares.len()).map(|i| {
            let contrib = self.shares[i].contribute(&mut rng, payload);
            let delay_ms = rng.gen_range(800..2500u64);
            (contrib, delay_ms)
        }).collect()
    }

    /// verify OSST proof
    fn verify_osst(&self, contributions: &[Contribution<PallasPoint>], payload: &[u8]) -> bool {
        osst_verify(&self.group_pubkey, &contributions[..JURY_T as usize], JURY_T, payload)
            .unwrap_or(false)
    }

    /// produce the jury's nested FROST signature share for the outer protocol.
    ///
    /// s₃ is NEVER reconstructed. each jury node computes its partial signature
    /// using its share σₖ, and the relay sums the partials into z₃.
    ///
    /// returns: (outer_signature_share, inner_commitments_for_outer_package)
    fn nested_sign(
        &self,
        message: &[u8],
        buyer_share: &SecretShare<PallasScalar>,
    ) -> Option<frost::Signature<PallasPoint>> {
        let mut rng = rand::thread_rng();

        // pick which jury nodes participate (first t)
        let active_indices: Vec<u32> = self.shares[..JURY_T as usize]
            .iter()
            .map(|s| s.index)
            .collect();

        // inner commitment round (with inner binding factors)
        let mut inner_nonces = Vec::new();
        let mut inner_commitments = Vec::new();
        for &k in &active_indices {
            let (nonces, commitments) = nested::inner_commit::<PallasPoint, _>(k, &mut rng);
            inner_nonces.push(nonces);
            inner_commitments.push(commitments);
        }

        // aggregate inner commitments with inner binding
        let r_nested = nested::aggregate_inner_commitments(&inner_commitments, message);

        // buyer commits for outer FROST
        let (buyer_nonces, buyer_frost_commits) =
            frost::commit::<PallasPoint, _>(buyer_share.index, &mut rng);

        // jury's outer commitment: r_nested as hiding, identity as binding
        let jury_outer_commits = frost::SigningCommitments {
            index: JURY_OUTER_INDEX,
            hiding: r_nested,
            binding: PallasPoint::identity(),
        };

        // build outer signing package
        let outer_package = frost::SigningPackage::new(
            message.to_vec(),
            vec![buyer_frost_commits, jury_outer_commits],
        ).ok()?;

        // buyer signs
        let buyer_sig = frost::sign::<PallasPoint>(
            &outer_package, buyer_nonces, buyer_share, &self.outer_group_pubkey,
        ).ok()?;

        // compute outer params for inner holders
        let outer_indices = outer_package.signer_indices();
        let outer_lambda = osst::compute_lagrange_coefficients::<PallasScalar>(&outer_indices).ok()?;
        let nested_pos = outer_indices.iter().position(|&i| i == JURY_OUTER_INDEX)?;

        // outer group commitment
        let outer_gc = {
            let mut r = PallasPoint::identity();
            for &idx in &outer_indices {
                let c = outer_package.get_commitments(idx)?;
                let rho = compute_outer_binding::<PallasPoint>(idx, message, &outer_package);
                r = r.add(&c.hiding).add(&c.binding.mul_scalar(&rho));
            }
            r
        };

        // outer challenge (RedPallas uses BLAKE2b in production, SHA-512 here for demo)
        let outer_challenge = {
            use sha2::{Sha512, Digest};
            let mut h = Sha512::new();
            h.update(b"frost-challenge-v1");
            h.update(OsstPoint::compress(&outer_gc));
            h.update(OsstPoint::compress(&self.outer_group_pubkey));
            h.update(message);
            let hash: [u8; 64] = h.finalize().into();
            PallasScalar::from_bytes_wide(&hash)
        };

        let params = nested::InnerSigningParams {
            outer_challenge,
            outer_lambda: outer_lambda[nested_pos],
        };

        // inner holders sign — s₃ never reconstructed
        let mut inner_sigs = Vec::new();
        for (nonces, &k) in inner_nonces.into_iter().zip(active_indices.iter()) {
            let share = &self.shares[(k - 1) as usize];
            let sig = nested::inner_sign::<PallasPoint>(
                nonces, share, &params, &inner_commitments, &active_indices, message,
            ).ok()?;
            inner_sigs.push(sig);
        }

        // aggregate inner shares → z₃
        let z_nested = nested::aggregate_inner_shares(&inner_sigs);
        let jury_sig_share = frost::SignatureShare {
            index: JURY_OUTER_INDEX,
            response: z_nested,
        };

        // outer aggregation → standard Schnorr signature
        let signature = frost::aggregate::<PallasPoint>(
            &outer_package,
            &[buyer_sig, jury_sig_share],
            &self.outer_group_pubkey,
            None,
        ).ok()?;

        // verify before returning
        if frost::verify_signature(&self.outer_group_pubkey, message, &signature) {
            Some(signature)
        } else {
            None
        }
    }
}

/// helper: compute outer FROST binding factor
fn compute_outer_binding<P: OsstPoint>(
    index: u32,
    message: &[u8],
    package: &frost::SigningPackage<P>,
) -> P::Scalar {
    use sha2::{Sha512, Digest};
    let mut encoded = Vec::new();
    for idx in package.signer_indices() {
        let c = package.get_commitments(idx).unwrap();
        encoded.extend_from_slice(&c.index.to_le_bytes());
        encoded.extend_from_slice(&c.hiding.compress());
        encoded.extend_from_slice(&c.binding.compress());
    }
    let mut h = Sha512::new();
    h.update(b"frost-binding-v1");
    h.update(index.to_le_bytes());
    h.update((message.len() as u64).to_le_bytes());
    h.update(message);
    h.update(&encoded);
    P::Scalar::from_bytes_wide(&h.finalize().into())
}

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
}

struct Room {
    code: String,
    players: Vec<Option<Player>>,
    engine: GameEngine,
    hand_number: u64,
    button: u8,
    hole_cards: Vec<Option<[Card; 2]>>,
    community_cards: Vec<Card>,
    jury: Jury,
    action_log: ActionLog,
    /// real Pallas escrow from 2-of-3 nested DKG (player A + player B + jury)
    /// s₃ never existed — born distributed via interleaved DKG
    escrow_address: [u8; 32],
    /// player A's outer FROST share (for dispute signing)
    player_a_share: SecretShare<PallasScalar>,
    /// player B's outer FROST share (for dispute signing)
    player_b_share: SecretShare<PallasScalar>,
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

        let jury = Jury {
            shares: jury_network.node_shares,
            group_pubkey: jury_network.outer_verification_share,
            outer_group_pubkey: jury_network.outer_group_pubkey,
        };

        Room {
            code, players: vec![None, None], engine,
            hand_number: 0, button: 0,
            hole_cards: vec![None, None], community_cards: Vec::new(),
            jury, action_log: ActionLog::new(),
            escrow_address,
            player_a_share, player_b_share,
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
        for i in 0..2u8 {
            if self.engine.stacks()[i as usize] == 0 {
                let _ = self.engine.seat_player(i, 1000);
            }
        }
        self.hand_number += 1;
        self.button = if self.hand_number == 1 { 0 } else { 1 - self.button };
        self.hole_cards = vec![None, None];
        self.community_cards = Vec::new();

        let deck = shuffled_deck();
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
    deck.shuffle(&mut rand::thread_rng());
    deck
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
                let seat = r.players.iter().position(|p| p.is_none());
                if let Some(seat_idx) = seat {
                    let seat = seat_idx as u8;
                    r.players[seat_idx] = Some(Player { name: name.clone(), seat, tx: tx.clone() });
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
                let r = room.lock().await;
                let stacks = r.engine.stacks().to_vec();
                let payload = r.action_log.settlement_payload(&stacks);

                // phase 1: OSST authorization (async, non-interactive)
                let prepared = r.jury.prepare_contributions(&payload);
                let osst_verified = r.jury.verify_osst(
                    &prepared.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
                    &payload,
                );
                let payload_hash = hex::encode(&Sha256::digest(&payload)[..8]);

                // phase 2: nested FROST signing (s₃ never reconstructed)
                // disputing player (seat 0 = player A) + jury → 2-of-3
                let signature = if osst_verified {
                    r.jury.nested_sign(&payload, &r.player_a_share)
                } else {
                    None
                };

                let frost_verified = signature.is_some();
                if let Some(ref sig) = signature {
                    tracing::info!(
                        "room {}: nested FROST signature produced (R={})",
                        r.code, hex::encode(&OsstPoint::compress(&sig.r)[..8])
                    );
                }

                let txs: Vec<_> = r.players.iter()
                    .filter_map(|p| p.as_ref().map(|p| p.tx.clone()))
                    .collect();

                drop(r);

                tokio::spawn(async move {
                    // stream jury votes with staggered delays
                    for (i, (_, delay_ms)) in prepared.iter().enumerate() {
                        tokio::time::sleep(std::time::Duration::from_millis(*delay_ms)).await;
                        let msg = ServerMsg::JuryVote {
                            node: i as u8 + 1,
                            total: JURY_N as u8,
                            payload_hash: payload_hash.clone(),
                        };
                        for tx in &txs { let _ = tx.send(msg.clone()); }
                    }

                    let msg = ServerMsg::JurySettlement {
                        verified: osst_verified && frost_verified,
                        threshold: JURY_T as u8,
                        contributions: JURY_N as u8,
                    };
                    for tx in &txs { let _ = tx.send(msg.clone()); }
                });
            }
        }
    }

    // cleanup on disconnect
    if let Some(seat) = my_seat {
        let mut r = room.lock().await;
        r.players[seat as usize] = None;
        r.broadcast(&ServerMsg::OpponentLeft { seat });
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
