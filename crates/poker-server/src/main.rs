//! poker-server: websocket game server with nested FROST jury
//!
//! jury signing has two modes (selected by NARSIL_ENDPOINT env var):
//! - local: all jury shares in-process (demo/testing)
//! - narsil: calls live narsild validators over HTTP (production)

mod jury;
mod escrow_client;

use axum::{
    Router,
    extract::{
        Path, State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    Json,
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
    Join { name: String, #[serde(default)] pubkey: Option<String>, #[serde(default)] zcash_address: Option<String> },
    Action { action: String, amount: Option<u64> },
    Chat { text: String },
    StartHand,
    AllowPlayer { pubkey: String },
    /// player reports their ZEC deposit to escrow address
    ReportDeposit { txid: String, amount: u64 },
    /// player leaves — triggers settlement and payout
    Leave,
    /// player broadcasts filtered game state to spectators
    Broadcast { data: String },
    Dispute,
    /// zafu_poker_dkg finished: this seat's view of the escrow UA + UFVK
    DkgComplete { escrow_ua: String, orchard_fvk: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ServerMsg {
    Seated { seat: u8, name: String },
    OpponentJoined { seat: u8, name: String },
    OpponentLeft { seat: u8 },
    /// reconnect window expired during settlement — remaining player should sign payout.
    /// distinct from OpponentLeft because the abandoner didn't choose to leave, and the
    /// settlement view shows different copy ("opponent disconnected and didn't return").
    OpponentAbandoned { seat: u8 },
    OpponentDisconnected { seat: u8, reconnect_secs: u64 },
    /// Hand action is paused while ≥1 seated player is in their reconnect window.
    /// SPA stops the local action-timer countdown until ActionResumed arrives.
    ActionPaused { seat: u8 },
    /// All disconnected players returned; action timer for `seat` resumes with `seconds_left`.
    ActionResumed { seat: u8, seconds_left: u64 },
    OpponentReconnected { seat: u8 },
    ActionTimeout { seat: u8 },
    TimerTick { seat: u8, seconds_left: u64 },
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
    RoomInfo {
        code: String,
        jury_nodes: u8,
        jury_threshold: u8,
        escrow: String,
        /// FROST relay URL — `Some` when escrow is running in DKG mode; clients use this to join the DKG room.
        #[serde(skip_serializing_if = "Option::is_none")]
        frost_relay_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        frost_room_code: Option<String>,
        /// per-seat diversified deposit UAs — present once escrow has surfaced them
        #[serde(skip_serializing_if = "Vec::is_empty")]
        seat_addresses: Vec<Option<String>>,
        /// total each player must deposit (buyin + fee_per_seat), in zatoshi
        required_deposit: u64,
        /// table buy-in component of `required_deposit`, in zatoshi
        #[serde(default)]
        buyin_zat: u64,
        /// per-seat share of the on-chain payout fee component, in zatoshi
        #[serde(default)]
        fee_per_seat: u64,
    },
    /// invite link
    InviteLink { url: String },
    /// game status (shuffle progress, deck verification, etc.)
    Status { phase: String, message: String },
    /// table chat
    Chat { seat: u8, name: String, text: String },
    /// game over — settlement complete, payouts ready
    GameOver {
        reason: String,
        payouts: Vec<(u8, u64)>,  // (seat, amount)
    },
    /// deposit status update — broadcast on change by the server's escrow-poll loop
    DepositStatus {
        escrow_address: String,
        /// per-seat diversified deposit UAs (None if escrow hasn't surfaced them yet)
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        seat_addresses: Vec<Option<String>>,
        player_a_deposit: u64,
        player_b_deposit: u64,
        required: u64,
        ready: bool,
    },
    Error { message: String },
    Waiting,
    /// FROST relay coords + payout plan + priority signer — sent when a payout is initiated
    /// (Leave button or 60s timeout). The SPA shows the plan; the priority signer's zafu
    /// runs `join_sign_pczt` against the relay room.
    PayoutSigningRequest {
        relay_room: String,
        plan: Vec<PayoutLineJson>,
        priority_seat: u8,
    },
    /// Final payout state — tx is on chain.
    PayoutComplete { txid: String },
    /// Payout never completed (player didn't approve, signing timed out, broadcast rejected).
    PayoutFailed { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayoutLineJson {
    pub seat: u8,
    pub address: String,
    pub amount_zat: u64,
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

    fn record(&mut self, hand_number: u64, seat: u8, action: &str, amount: u64, room_code: &str) -> &ActionLogEntry {
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
            sequence: self.sequence, hash: hash.clone(),
        });

        // flush to disk — append JSONL per room
        let log_dir = std::path::Path::new("logs");
        let _ = std::fs::create_dir_all(log_dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
            .open(log_dir.join(format!("{}.jsonl", room_code)))
        {
            use std::io::Write;
            let _ = writeln!(f, r#"{{"seq":{},"hand":{},"seat":{},"action":"{}","amount":{},"hash":"{}","ts":{}}}"#,
                self.sequence, hand_number, seat, action, amount, hash,
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
        }

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
    pubkey: Option<String>,
    /// Zcash shielded address for payouts/refunds
    zcash_address: Option<String>,
    tx: mpsc::UnboundedSender<ServerMsg>,
    disconnected_at: Option<tokio::time::Instant>,
}

/// table access control
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum TableAccess {
    /// anyone can join
    Public,
    /// only players whose session pubkey is in the allow list (contacts/mutuals)
    Mutuals { allowed_pubkeys: Vec<String> },
    /// invite-only — only players who received the room code
    Private,
}

impl Default for TableAccess {
    fn default() -> Self { Self::Public }
}

const RECONNECT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
const ACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// On-chain fee zafu/zcli use for Orchard-only txes (matches `complete_pczt_tx` default).
/// Players collectively pre-fund this at deposit time so the payout never goes underwater.
const TX_PAYOUT_FEE_ZAT: u64 = 10_000;

struct Room {
    code: String,
    max_seats: usize,
    access: TableAccess,
    /// table is open to autojoin by bots
    bot_friendly: bool,
    host_seat: Option<u8>,
    players: Vec<Option<Player>>,
    /// spectator channels
    spectators: Vec<mpsc::UnboundedSender<String>>,
    /// deposit tracking (zatoshis per seat)
    deposits: Vec<u64>,
    /// what each seated player must deposit = buyin + `fee_per_seat`. The extra `fee_per_seat`
    /// portion is pooled across players to cover the on-chain payout tx fee at settlement; the
    /// game engine still treats only `buyin` as chips.
    required_deposit: u64,
    /// per-seat share of the payout tx fee, baked into `required_deposit`. settlement_plan
    /// subtracts this from each seat's deposit basis so total outputs + fee = total deposits.
    fee_per_seat: u64,
    engine: GameEngine,
    hand_number: u64,
    button: u8,
    hole_cards: Vec<Option<[Card; 2]>>,
    community_cards: Vec<Card>,
    jury: Arc<dyn jury::JuryService>,
    action_log: ActionLog,
    /// what we display to clients — Zcash UA (`u1...`) when external escrow is wired, hex of the in-process Pallas point otherwise
    escrow_address: String,
    /// FROST relay coords — `Some` only when escrow runs in DKG mode; forwarded to clients via RoomInfo
    frost_relay_url: Option<String>,
    frost_room_code: Option<String>,
    /// per-seat DKG UA report — must agree across seats before we trust it as escrow_address
    dkg_reported_ua: Vec<Option<String>>,
    /// per-seat deposit UAs synced from poker-escrow's /room/{code}
    seat_deposit_addresses: Vec<Option<String>>,
    /// per-seat personal payout addresses (recovered from deposit memo)
    seat_payout_addresses: Vec<Option<String>>,
    /// last DepositStatus we broadcast — so the 5s poll only re-broadcasts on change
    last_deposit_broadcast: Option<(u64, u64, bool)>,
    /// last broadcast pause state — so we only emit ActionPaused/Resumed on transitions
    last_paused: bool,
    player_a_share: SecretShare<PallasScalar>,
    player_b_share: SecretShare<PallasScalar>,
    action_deadline: Option<(u8, tokio::time::Instant)>,
    /// table buy-in in zatoshi (= engine.rules.min_buy_in, kept for RoomInfo breakdown)
    buyin_zat: u64,
    /// once a settlement has been triggered (Leave OR bust), no new hands start and the
    /// auto-rebuy at start_hand is dead. Stays true until the room is recycled.
    payout_triggered: bool,
    /// Cached `PayoutSigningRequest` payload so reconnecting clients can be replayed into
    /// the settlement view instead of the live table. Set when the request is broadcast,
    /// cleared when PayoutComplete / PayoutFailed lands.
    payout_signing_state: Option<PayoutSigningState>,
    /// Set after `PayoutComplete{txid}` is broadcast so a late reconnect can still see the
    /// final txid + settlement summary.
    payout_complete_txid: Option<String>,
    /// Set after `PayoutFailed{reason}` is broadcast.
    payout_failed_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct PayoutSigningState {
    relay_room: String,
    plan: Vec<PayoutLineJson>,
    priority_seat: u8,
    /// time we last bumped priority_seat (or first broadcast). Used by the fallback timer.
    broadcast_at: tokio::time::Instant,
}

impl Room {
    fn new(code: String) -> Self {
        Self::with_settings(code, 5, 10, 1000, 30, 2, TableAccess::Public, false, RemoteEscrow::default())
    }

    fn with_settings(code: String, sb: u64, bb: u64, buyin: u64, timeout: u32, seats: usize, access: TableAccess, bot_friendly: bool, external_escrow: RemoteEscrow) -> Self {
        let seats = seats.clamp(2, 9);
        let rules = TableRules {
            small_blind: sb as u128, big_blind: bb as u128, ante: 0,
            min_buy_in: buyin as u128, max_buy_in: 0, seats: seats as u8,
            tier: poker_p2p::protocol::SecurityTier::Training,
            allow_spectators: false, max_spectators: 0,
            time_bank: 60, action_timeout: timeout,
        };
        let engine = GameEngine::new(rules, seats as u8).unwrap();

        // frostito: 2-of-3 nested escrow (player A + player B + jury)
        // jury's share s₃ born distributed via interleaved DKG — never materialized
        let mut rng = rand::thread_rng();
        let (player_a_share, player_b_share, jury_network, group_pubkey) =
            redpallas::setup_escrow(JURY_N, JURY_T, &mut rng)
                .expect("interleaved DKG should succeed");

        let local_addr_hex = hex::encode(redpallas::derive_address_bytes(&group_pubkey));
        let frost_relay_url = external_escrow.frost_relay_url;
        let frost_room_code = external_escrow.frost_room_code;
        // DKG mode: escrow_address stays empty until players' DkgComplete agrees on a UA
        let dkg_mode = frost_relay_url.is_some() && frost_room_code.is_some();
        let escrow_address = external_escrow.address.unwrap_or_else(
            || if dkg_mode { String::new() } else { local_addr_hex },
        );

        tracing::info!(
            "room {} created: 2-of-3 nested escrow (frostito/pallas), {}-of-{} jury, escrow {}",
            code, JURY_T, JURY_N, &escrow_address[..escrow_address.len().min(24)]
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

        // ceil-div so total covers the on-chain fee even when fee doesn't divide evenly
        // across seats (e.g. 3 seats × ceil(10000/3) = 3 × 3334 = 10002 ≥ 10000)
        let fee_per_seat = (TX_PAYOUT_FEE_ZAT + seats as u64 - 1) / seats as u64;
        let required_deposit = if bot_friendly { buyin } else { buyin + fee_per_seat };

        Room {
            code, max_seats: seats, access, bot_friendly, host_seat: None,
            players: (0..seats).map(|_| None).collect(),
            spectators: Vec::new(),
            deposits: vec![0; seats],
            required_deposit,
            fee_per_seat: if bot_friendly { 0 } else { fee_per_seat },
            engine, hand_number: 0, button: 0,
            hole_cards: (0..seats).map(|_| None).collect(),
            community_cards: Vec::new(),
            jury, action_log: ActionLog::new(),
            escrow_address,
            frost_relay_url,
            frost_room_code,
            dkg_reported_ua: vec![None; seats],
            seat_deposit_addresses: vec![None; seats],
            seat_payout_addresses: vec![None; seats],
            last_deposit_broadcast: None,
            last_paused: false,
            player_a_share, player_b_share,
            action_deadline: None,
            buyin_zat: buyin,
            payout_triggered: false,
            payout_signing_state: None,
            payout_complete_txid: None,
            payout_failed_reason: None,
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

    /// payout plan for the room — `(seat, zatoshi)` pairs. Derived from real deposits, not
    /// engine defaults: a seat that never deposited is excluded outright. Each seat's basis is
    /// `deposit - fee_per_seat` so the per-player share of the on-chain fee comes out of the
    /// pre-funded amount rather than reducing winnings post-hoc. Total of all outputs +
    /// `TX_PAYOUT_FEE_ZAT` equals total deposits.
    fn settlement_plan(&self) -> Vec<(u8, u64)> {
        let initial = self.engine.rules.min_buy_in as u64;
        let stacks = self.engine.stacks();
        let hand_played = self.hand_number > 0;
        let mut out = Vec::new();
        for i in 0..self.max_seats {
            let deposited = self.deposits.get(i).copied().unwrap_or(0);
            if deposited == 0 {
                continue;
            }
            let basis = deposited.saturating_sub(self.fee_per_seat);
            let amount = if hand_played {
                let stack = stacks.get(i).copied().unwrap_or(0);
                let delta = (stack as i128) - (initial as i128);
                ((basis as i128) + delta).max(0) as u64
            } else {
                basis
            };
            if amount > 0 {
                out.push((i as u8, amount));
            }
        }
        out
    }

    /// Real-table bust check: after a hand ends, if exactly one seated player has chips > 0
    /// and the rest are at 0, the table is settled. Returns the winning seat. Bot tables and
    /// rooms already in payout return None.
    fn winner_after_bust(&self) -> Option<u8> {
        if self.bot_friendly || self.payout_triggered { return None; }
        let stacks = self.engine.stacks();
        let mut alive = Vec::new();
        for i in 0..self.max_seats {
            if self.players[i].is_some() && stacks.get(i).copied().unwrap_or(0) > 0 {
                alive.push(i as u8);
            }
        }
        let seated = self.players.iter().filter(|p| p.is_some()).count();
        if seated >= 2 && alive.len() == 1 { Some(alive[0]) } else { None }
    }

    /// every seated player has deposited the required amount AND we have their refund address
    fn deposits_satisfied(&self) -> bool {
        self.players.iter().enumerate().all(|(i, p)| {
            if p.is_none() { return true; }
            self.deposits.get(i).copied().unwrap_or(0) >= self.required_deposit
                && self.seat_payout_addresses.get(i).and_then(|a| a.as_ref()).is_some()
        })
    }

    fn start_hand(&mut self) {
        if self.payout_triggered {
            tracing::debug!("room {}: start_hand blocked — payout already triggered", self.code);
            return;
        }
        // bot tables play chip-only — no deposit gate. Real-ZEC tables wait until escrow confirms deposits.
        if !self.bot_friendly && !self.deposits_satisfied() {
            tracing::debug!("room {}: start_hand blocked — deposits not satisfied", self.code);
            return;
        }
        // bot tables auto-rebuy busted players for endless demo play. real tables follow real poker:
        // bust = out, no free chips. winner_after_bust() upstream catches that case and triggers payout
        // before we get here on real tables, so any 0-stack seat reaching this loop on a real table
        // would block the engine — by intent.
        let buyin = self.engine.rules.min_buy_in as u64;
        if self.bot_friendly {
            for i in 0..self.max_seats as u8 {
                if self.players[i as usize].is_some() && self.engine.stacks()[i as usize] == 0 {
                    let _ = self.engine.seat_player(i, buyin);
                }
            }
        }
        self.hand_number += 1;
        self.button = (self.button + 1) % self.max_seats as u8;
        // skip empty seats for button
        for _ in 0..self.max_seats {
            if self.players[self.button as usize].is_some() { break; }
            self.button = (self.button + 1) % self.max_seats as u8;
        }
        self.hole_cards = vec![None; self.max_seats];
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
        self.action_log.record(self.hand_number, seat, &action_str, amount, &self.code);

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
                    let live_stacks: Vec<u64> = self.engine.hand_state()
                        .map(|h| h.seats.iter().map(|s| s.chips).collect())
                        .unwrap_or_else(|| self.engine.stacks().to_vec());
                    self.send_to(*seat, ServerMsg::HandStarted {
                        hand_number: self.hand_number as u64, button: self.button,
                        your_cards: Some([card_json(&cards[0]), card_json(&cards[1])]),
                        stacks: live_stacks,
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
                        seat: *seat, action: action_str.clone(), amount, new_stack: *new_stack,
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

/// Real-table heads-up bust handler: build the settlement plan, mark the room payout-triggered,
/// broadcast GameOver, and spawn the on-chain payout flow with the winner as priority signer.
/// Caller must hold the room mutex. No-op for bot tables or when payout already triggered.
fn finish_table_after_bust(r: &mut Room, state: &AppState, room: Arc<Mutex<Room>>, winner_seat: u8) {
    if r.bot_friendly || r.payout_triggered { return; }
    r.payout_triggered = true;

    let payouts = r.settlement_plan();
    let payout_str: Vec<String> = payouts.iter().map(|(s, a)| format!("seat{}={}", s, a)).collect();
    tracing::info!("settlement (bust): room={} winner=seat{} payouts=[{}]",
        r.code, winner_seat, payout_str.join(", "));
    r.broadcast(&ServerMsg::GameOver {
        reason: format!("seat {} busted; settling to winner", 1 - winner_seat),
        payouts: payouts.clone(),
    });
    r.action_deadline = None;

    let pczt_plan: Vec<PayoutLineJson> = payouts.iter().filter_map(|(s, amt)| {
        r.seat_payout_addresses.get(*s as usize)
            .and_then(|opt| opt.as_ref())
            .map(|addr| PayoutLineJson { seat: *s, address: addr.clone(), amount_zat: *amt })
    }).collect();

    if pczt_plan.is_empty() { return; }
    let Some(escrow_url) = state.escrow_url.clone() else {
        tracing::warn!("no ESCROW_URL configured — skipping on-chain payout");
        return;
    };
    let code = r.code.clone();
    let rooms = state.rooms.clone();
    tokio::spawn(async move {
        trigger_payout(rooms, room, escrow_url, code, pczt_plan, winner_seat).await;
    });
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

/// Cadence at which poker-server polls poker-escrow's GET /room/{code} to sync deposit state.
const DEPOSIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// 5-minute delay then remove the room from the global map. Lets clients still
/// fetch the txid + lobby see the result for a window; after that the room is gone.
fn schedule_room_cleanup(rooms: Rooms, code: String, delay: std::time::Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let mut map = rooms.lock().await;
        if map.remove(&code).is_some() {
            tracing::info!("room {}: cleanup removed from rooms map (post-payout)", code);
        }
    });
}

/// POST `/payout/initiate` to escrow, broadcast `PayoutSigningRequest`, then poll
/// `/payout/status` until Broadcast or Failed; broadcast the final result. After a
/// successful broadcast, schedules a 5-min cleanup that removes the room from `rooms`
/// so it stops showing in the lobby and the escrow scanner can spin down.
async fn trigger_payout(
    rooms: Rooms,
    room: Arc<Mutex<Room>>,
    escrow_url: String,
    code: String,
    plan: Vec<PayoutLineJson>,
    priority_seat: u8,
) {
    let outputs: Vec<escrow_client::PayoutOutputReq> = plan.iter()
        .map(|p| escrow_client::PayoutOutputReq { address: p.address.clone(), amount_zat: p.amount_zat })
        .collect();
    let req = escrow_client::InitiatePayoutReq {
        outputs,
        fee_zat: Some(TX_PAYOUT_FEE_ZAT),
        anchor_height: None,
    };

    let relay_room = match escrow_client::initiate_payout(&escrow_url, &code, &req).await {
        Ok(resp) => resp.relay_room,
        Err(e) => {
            tracing::error!("payout {}: initiate failed: {}", code, e);
            let mut r = room.lock().await;
            r.broadcast(&ServerMsg::PayoutFailed { reason: format!("initiate: {}", e) });
            return;
        }
    };
    tracing::info!("payout {}: signing room={} priority_seat={}", code, relay_room, priority_seat);

    {
        let mut r = room.lock().await;
        r.broadcast(&ServerMsg::PayoutSigningRequest {
            relay_room: relay_room.clone(),
            plan: plan.clone(),
            priority_seat,
        });
        // remember the broadcast so reconnecting clients can be replayed into the settlement view
        r.payout_signing_state = Some(PayoutSigningState {
            relay_room: relay_room.clone(),
            plan: plan.clone(),
            priority_seat,
            broadcast_at: tokio::time::Instant::now(),
        });
    }

    // poll status until terminal. Also acts as the fallback timer: every 30s of pending,
    // check if PRIORITY_SIGNER_FALLBACK_SECS has elapsed since the LAST priority broadcast;
    // if yes, swap to the other seat so the responsive player can sign instead.
    const PRIORITY_SIGNER_FALLBACK_SECS: u64 = 90;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        match escrow_client::get_payout_status(&escrow_url, &code).await {
            Ok(escrow_client::PayoutStatus::Broadcast { txid, .. }) => {
                tracing::info!("payout {} broadcast: tx={}", code, txid);
                {
                    let mut r = room.lock().await;
                    r.broadcast(&ServerMsg::PayoutComplete { txid: txid.clone() });
                    r.payout_signing_state = None;
                    r.payout_complete_txid = Some(txid);
                }
                schedule_room_cleanup(rooms.clone(), code.clone(), std::time::Duration::from_secs(300));
                return;
            }
            Ok(escrow_client::PayoutStatus::Failed { reason }) => {
                tracing::error!("payout {} failed: {}", code, reason);
                let mut r = room.lock().await;
                r.broadcast(&ServerMsg::PayoutFailed { reason: reason.clone() });
                r.payout_signing_state = None;
                r.payout_failed_reason = Some(reason);
                return;
            }
            Ok(_) => {
                // still pending — check if it's time to swap priority signer
                let swap = {
                    let r = room.lock().await;
                    match &r.payout_signing_state {
                        Some(s) if s.broadcast_at.elapsed() >= std::time::Duration::from_secs(PRIORITY_SIGNER_FALLBACK_SECS) => {
                            // find a seat to swap to. Heads-up: just flip 0↔1.
                            let new_seat = if s.priority_seat == 0 { 1 } else { 0 };
                            Some((new_seat, s.relay_room.clone(), s.plan.clone()))
                        }
                        _ => None,
                    }
                };
                if let Some((new_seat, relay_room, plan)) = swap {
                    tracing::info!("payout {}: priority-signer fallback fired, swapping to seat {}", code, new_seat);
                    let mut r = room.lock().await;
                    r.broadcast(&ServerMsg::PayoutSigningRequest {
                        relay_room: relay_room.clone(),
                        plan: plan.clone(),
                        priority_seat: new_seat,
                    });
                    r.payout_signing_state = Some(PayoutSigningState {
                        relay_room,
                        plan,
                        priority_seat: new_seat,
                        broadcast_at: tokio::time::Instant::now(),
                    });
                }
            }
            Err(e) => tracing::warn!("payout {} status poll error: {}", code, e),
        }
    }
}

/// Per-room background task: every 5s pulls the latest escrow state, mirrors seat addresses
/// + per-seat deposit amounts into Room, broadcasts a DepositStatus on any change, and auto-
/// starts the hand once both players have deposited. Exits when the room disappears.
fn spawn_deposit_poller(rooms: Rooms, escrow_url: Option<String>, code: String) {
    let Some(escrow_url) = escrow_url else {
        tracing::warn!("deposit poller {}: ESCROW_URL not configured, skipping", code);
        return;
    };
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(DEPOSIT_POLL_INTERVAL).await;
            let room_arc = match rooms.lock().await.get(&code).cloned() {
                Some(r) => r,
                None => {
                    tracing::info!("deposit poller {}: room gone, exiting", code);
                    return;
                }
            };
            let state = match escrow_client::get_room_state(&escrow_url, &code).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("deposit poller {}: {}", code, e);
                    continue;
                }
            };
            // escrow still doing DKG — nothing to sync yet
            let Some(escrow_ua) = state.escrow_address.clone() else { continue; };

            let mut r = room_arc.lock().await;
            // settlement in flight — no further sync, no log noise, no DepositStatus broadcasts
            if r.payout_triggered { continue; }
            if r.escrow_address.is_empty() && !escrow_ua.is_empty() {
                r.escrow_address = escrow_ua;
            }
            for (i, addr) in state.seat_addresses.iter().enumerate() {
                if i < r.seat_deposit_addresses.len() && r.seat_deposit_addresses[i].is_none() {
                    r.seat_deposit_addresses[i] = addr.clone();
                }
            }
            for (i, addr) in state.seat_payout_addresses.iter().enumerate() {
                if i < r.seat_payout_addresses.len() && r.seat_payout_addresses[i].is_none() {
                    r.seat_payout_addresses[i] = addr.clone();
                }
            }
            if r.deposits.len() >= 2 {
                r.deposits[0] = state.player_a_deposit;
                r.deposits[1] = state.player_b_deposit;
            }

            let ready = r.deposits_satisfied() && r.player_count() >= 2;
            let snapshot = (state.player_a_deposit, state.player_b_deposit, ready);
            if r.last_deposit_broadcast != Some(snapshot) {
                r.last_deposit_broadcast = Some(snapshot);
                r.broadcast(&ServerMsg::DepositStatus {
                    escrow_address: r.escrow_address.clone(),
                    seat_addresses: r.seat_deposit_addresses.clone(),
                    player_a_deposit: state.player_a_deposit,
                    player_b_deposit: state.player_b_deposit,
                    required: r.required_deposit,
                    ready,
                });
                let breakdown = if r.fee_per_seat > 0 {
                    format!(" (={} buyin + {} fee per seat)", r.buyin_zat, r.fee_per_seat)
                } else { String::new() };
                tracing::info!(
                    "deposit sync room={}: a={} b={} required={}{} ready={}",
                    code, state.player_a_deposit, state.player_b_deposit,
                    r.required_deposit, breakdown, ready,
                );
            }

            if ready && !r.payout_triggered && r.engine.hand_state().is_none() {
                r.start_hand();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// App state (multi-room)
// ---------------------------------------------------------------------------

type Rooms = Arc<Mutex<HashMap<String, Arc<Mutex<Room>>>>>;

/// lobby user — connected to the global chat
struct LobbyUser {
    name: String,
    tx: mpsc::UnboundedSender<LobbyMsg>,
}

/// lobby message types
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum LobbyMsg {
    /// global chat
    Chat { from: String, text: String },
    /// whisper (private message)
    Whisper { from: String, to: String, text: String },
    /// system message
    System { text: String },
    /// player list update
    Players { names: Vec<String> },
    /// table list update
    Tables { tables: Vec<serde_json::Value> },
    /// challenge from another player
    Challenge { from: String, table_code: String },
}

/// lobby client message (from browser)
#[derive(Deserialize)]
#[serde(tag = "type")]
enum LobbyClientMsg {
    /// set display name
    Join { name: String },
    /// global chat: /msg or just text
    Chat { text: String },
    /// whisper: /w name message
    Whisper { to: String, text: String },
    /// challenge player to a game
    Challenge { to: String },
}

type LobbyUsers = Arc<Mutex<HashMap<String, LobbyUser>>>;

#[derive(Clone)]
struct AppState {
    rooms: Rooms,
    lobby_users: LobbyUsers,
    static_dir: String,
    /// base url of the poker-escrow service, e.g. http://127.0.0.1:3034; None disables remote escrow
    escrow_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// lobby websocket — global chat, whispers, player list
async fn lobby_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_lobby_socket(socket, state))
}

async fn handle_lobby_socket(socket: WebSocket, state: AppState) {
    let (tx, mut rx) = mpsc::unbounded_channel::<LobbyMsg>();

    use futures::{SinkExt, StreamExt};
    let (mut ws_tx, mut ws_rx) = socket.split();

    // send task
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_tx.send(WsMessage::Text(json.into())).await.is_err() { break; }
            }
        }
    });

    let mut my_name: Option<String> = None;

    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            WsMessage::Text(t) => t.to_string(),
            WsMessage::Close(_) => break,
            _ => continue,
        };

        let client_msg: LobbyClientMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match client_msg {
            LobbyClientMsg::Join { name } => {
                // remove old entry if re-joining with different name
                if let Some(ref old) = my_name {
                    state.lobby_users.lock().await.remove(old);
                }
                my_name = Some(name.clone());

                state.lobby_users.lock().await.insert(name.clone(), LobbyUser {
                    name: name.clone(),
                    tx: tx.clone(),
                });

                // broadcast join
                lobby_broadcast(&state.lobby_users, &LobbyMsg::System {
                    text: format!("{} joined the lobby", name),
                }).await;

                // send player list
                let names: Vec<String> = state.lobby_users.lock().await
                    .keys().cloned().collect();
                let _ = tx.send(LobbyMsg::Players { names });

                // send table list
                let tables = get_table_list(&state.rooms).await;
                let _ = tx.send(LobbyMsg::Tables { tables });
            }
            LobbyClientMsg::Chat { text } => {
                if let Some(ref name) = my_name {
                    lobby_broadcast(&state.lobby_users, &LobbyMsg::Chat {
                        from: name.clone(), text,
                    }).await;
                }
            }
            LobbyClientMsg::Whisper { to, text } => {
                if let Some(ref from) = my_name {
                    let users = state.lobby_users.lock().await;
                    if let Some(target) = users.get(&to) {
                        let msg = LobbyMsg::Whisper { from: from.clone(), to: to.clone(), text: text.clone() };
                        let _ = target.tx.send(msg.clone());
                        let _ = tx.send(msg); // echo to sender
                    } else {
                        let _ = tx.send(LobbyMsg::System { text: format!("user '{}' not found", to) });
                    }
                }
            }
            LobbyClientMsg::Challenge { to } => {
                if let Some(ref from) = my_name {
                    // create a private table
                    let code = generate_room_code();
                    let room = Arc::new(Mutex::new(Room::new(code.clone())));
                    state.rooms.lock().await.insert(code.clone(), room);

                    // notify both players
                    let msg = LobbyMsg::Challenge { from: from.clone(), table_code: code.clone() };
                    let users = state.lobby_users.lock().await;
                    if let Some(target) = users.get(&to) {
                        let _ = target.tx.send(msg);
                    }
                    let _ = tx.send(LobbyMsg::System { text: format!("challenged {} — table {}", to, code) });
                }
            }
        }
    }

    // cleanup
    if let Some(ref name) = my_name {
        state.lobby_users.lock().await.remove(name);
        lobby_broadcast(&state.lobby_users, &LobbyMsg::System {
            text: format!("{} left the lobby", name),
        }).await;
    }
    send_task.abort();
}

async fn lobby_broadcast(users: &LobbyUsers, msg: &LobbyMsg) {
    let users = users.lock().await;
    for user in users.values() {
        let _ = user.tx.send(msg.clone());
    }
}

/// push updated table list to all lobby users
async fn notify_lobby_tables(rooms: &Rooms, lobby_users: &LobbyUsers) {
    let tables = get_table_list(rooms).await;
    lobby_broadcast(lobby_users, &LobbyMsg::Tables { tables }).await;
}

async fn get_table_list(rooms: &Rooms) -> Vec<serde_json::Value> {
    let rooms = rooms.lock().await;
    let mut tables = Vec::new();
    for (code, room) in rooms.iter() {
        let r = room.lock().await;
        let has_spectators = !r.spectators.is_empty();
        // public tables: shown as joinable
        // private/mutuals with spectators: shown as watchable (live broadcast)
        // private without spectators: hidden
        if r.access != TableAccess::Public && !has_spectators { continue; }
        tables.push(serde_json::json!({
            "code": code,
            "players": r.player_count(),
            "max_players": r.max_seats,
            "waiting": r.player_count() < 2,
            "access": match &r.access {
                TableAccess::Public => "public",
                _ => "private",
            },
            "bot_friendly": r.bot_friendly,
            "live": has_spectators,
            "blinds": format!("{}/{}", r.engine.rules.small_blind, r.engine.rules.big_blind),
            "hand_number": r.hand_number,
            "spectators": r.spectators.len(),
        }));
    }
    tables
}

/// list public tables (for lobby)
async fn list_tables(State(state): State<AppState>) -> impl IntoResponse {
    Json(get_table_list(&state.rooms).await)
}

/// create new room and redirect to it
/// query params for table creation
#[derive(Deserialize, Default)]
struct CreateTableParams {
    sb: Option<u64>,
    bb: Option<u64>,
    buyin: Option<u64>,
    timeout: Option<u32>,
    seats: Option<usize>,
    /// "public" (default), "private", or "mutuals"
    access: Option<String>,
    /// comma-separated session pubkeys for mutuals mode
    allowed: Option<String>,
    /// open to bot autojoin (only meaningful on public tables)
    bot: Option<bool>,
    /// rake fee in basis points (e.g. 250 = 2.5%), forwarded to poker-escrow
    rake_bps: Option<u16>,
}

/// Result of asking poker-escrow to provision a room: optional UA (None when DKG-pending)
/// plus optional FROST relay coords that clients need to participate in DKG.
#[derive(Default)]
struct RemoteEscrow {
    address: Option<String>,
    frost_relay_url: Option<String>,
    frost_room_code: Option<String>,
}

/// fetch escrow setup from poker-escrow if configured; logs and returns default on failure.
async fn remote_escrow_for(
    escrow_url: &Option<String>,
    code: &str,
    required_deposit: u64,
    rake_bps: u16,
) -> RemoteEscrow {
    let Some(url) = escrow_url.as_deref() else { return RemoteEscrow::default(); };
    match escrow_client::create_escrow(url, code, required_deposit, rake_bps).await {
        Ok(setup) => {
            if setup.dkg_mode {
                tracing::info!(
                    "escrow provisioned room {} in DKG mode (relay={:?} room={:?})",
                    code, setup.frost_relay_url, setup.frost_room_code,
                );
            } else if let Some(ref addr) = setup.escrow_address {
                let preview = &addr[..addr.len().min(24)];
                tracing::info!("escrow service provisioned room {} -> {}", code, preview);
            }
            RemoteEscrow {
                address: setup.escrow_address,
                frost_relay_url: setup.frost_relay_url,
                frost_room_code: setup.frost_room_code,
            }
        }
        Err(e) => {
            tracing::warn!("escrow service unreachable for room {}: {} — falling back to in-process", code, e);
            RemoteEscrow::default()
        }
    }
}

async fn create_room(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<CreateTableParams>,
) -> impl IntoResponse {
    let code = generate_room_code();
    let sb = params.sb.unwrap_or(5);
    let bb = params.bb.unwrap_or(10);
    let buyin = params.buyin.unwrap_or(1000);
    let timeout = params.timeout.unwrap_or(30);
    let seats = params.seats.unwrap_or(2);
    let access = match params.access.as_deref() {
        Some("private") => TableAccess::Private,
        Some("mutuals") => {
            let allowed = params.allowed.as_deref().unwrap_or("")
                .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            TableAccess::Mutuals { allowed_pubkeys: allowed }
        }
        _ => TableAccess::Public,
    };

    let bot_friendly = params.bot.unwrap_or(false) && matches!(access, TableAccess::Public);
    let rake_bps = params.rake_bps.unwrap_or(0);
    // bot tables are chip-only demos — no FROST escrow, no DKG, no on-chain anything
    let external_escrow = if bot_friendly {
        RemoteEscrow { address: Some(String::new()), ..Default::default() }
    } else {
        remote_escrow_for(&state.escrow_url, &code, buyin, rake_bps).await
    };
    let room = Arc::new(Mutex::new(Room::with_settings(code.clone(), sb, bb, buyin, timeout, seats, access, bot_friendly, external_escrow)));
    state.rooms.lock().await.insert(code.clone(), room);
    if !bot_friendly {
        spawn_deposit_poller(state.rooms.clone(), state.escrow_url.clone(), code.clone());
    }
    notify_lobby_tables(&state.rooms, &state.lobby_users).await;
    axum::response::Redirect::to(&format!("/{}", code))
}

/// serve room page (same SPA, code in URL)
async fn room_page(
    Path(code): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // ignore non-room paths (favicon, assets, api, etc.)
    if code.contains('.') || code == "api" || code == "ws" || code == "new" || code == "health" {
        return (axum::http::StatusCode::NOT_FOUND, "not found").into_response();
    }

    // create room if it doesn't exist (joining via invite link or crawler hit)
    let needs_create = !state.rooms.lock().await.contains_key(&code);
    if needs_create {
        let external_escrow = remote_escrow_for(&state.escrow_url, &code, 1000, 0).await;
        let room = Room::with_settings(code.clone(), 5, 10, 1000, 30, 2, TableAccess::Public, false, external_escrow);
        state.rooms.lock().await.entry(code.clone()).or_insert_with(|| Arc::new(Mutex::new(room)));
        spawn_deposit_poller(state.rooms.clone(), state.escrow_url.clone(), code.clone());
    }
    // serve index.html
    let index = std::path::PathBuf::from(&state.static_dir).join("index.html");
    match tokio::fs::read_to_string(&index).await {
        Ok(html) => axum::response::Html(html).into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// websocket handler for a specific room
/// spectator WebSocket — read-only, receives broadcast events from players
async fn spectate_handler(
    Path(code): Path<String>,
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_spectate(socket, state, code))
}

async fn handle_spectate(socket: WebSocket, state: AppState, code: String) {
    use futures::{SinkExt, StreamExt};
    let (mut ws_tx, mut ws_rx) = socket.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // register as spectator
    {
        let rooms = state.rooms.lock().await;
        if let Some(room) = rooms.get(&code) {
            let mut r = room.lock().await;
            r.spectators.push(tx);
            let count = r.spectators.len();
            // notify players of spectator count
            r.broadcast(&ServerMsg::Status {
                phase: "spectators".into(),
                message: format!("{}", count),
            });
        }
    }

    // send loop: forward broadcast events to spectator
    let send_task = tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if ws_tx.send(WsMessage::Text(data.into())).await.is_err() { break; }
        }
    });

    // read loop: spectators can't send anything meaningful, just keep alive
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            WsMessage::Close(_) => break,
            WsMessage::Ping(d) => {} // tungstenite handles pong
            _ => {} // ignore spectator messages
        }
    }

    send_task.abort();
}

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
            escrow: r.escrow_address.clone(),
            buyin_zat: r.buyin_zat,
            fee_per_seat: r.fee_per_seat,
            frost_relay_url: r.frost_relay_url.clone(),
            frost_room_code: r.frost_room_code.clone(),
            seat_addresses: r.seat_deposit_addresses.clone(),
            required_deposit: r.required_deposit,
        });
    }

    // spawn action timeout watcher
    let timeout_room = room.clone();
    let timeout_state = state.clone();
    let timeout_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let mut r = timeout_room.lock().await;

            // Pause the action timer entirely while any seated player is in their reconnect
            // window. Reasoning: if A is acting but B disconnects, A's choice still depends
            // on game state B will see when they return; ticking down or auto-folding either
            // way penalises a network blip. The reconnect-window check below still runs, so
            // a player who never returns hits abandonment after 60s and the game settles.
            let any_disconnected = r.players.iter()
                .any(|p| matches!(p, Some(pl) if pl.disconnected_at.is_some()));

            // emit edge transitions for the SPA to swap between "paused" overlay + live timer
            if any_disconnected != r.last_paused {
                r.last_paused = any_disconnected;
                if any_disconnected {
                    if let Some((seat, _)) = r.action_deadline {
                        r.broadcast(&ServerMsg::ActionPaused { seat });
                    }
                } else if let Some((seat, deadline)) = r.action_deadline {
                    let secs_left = deadline.saturating_duration_since(tokio::time::Instant::now()).as_secs();
                    r.broadcast(&ServerMsg::ActionResumed { seat, seconds_left: secs_left });
                }
            }

            // check action timeout (skipped while paused)
            let mut hand_ended = false;
            if !any_disconnected {
                if let Some((seat, deadline)) = r.action_deadline {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    let secs_left = remaining.as_secs();

                    if remaining.is_zero() {
                        tracing::info!("room: seat {} timed out, auto-folding", seat);
                        r.action_deadline = None;
                        r.broadcast(&ServerMsg::ActionTimeout { seat });
                        if r.engine.hand_state().is_some() {
                            r.apply_action(seat, ActionType::Fold);
                            hand_ended = r.engine.hand_state().is_none();
                        }
                    } else {
                        // send timer tick every second so frontend shows countdown
                        r.broadcast(&ServerMsg::TimerTick { seat, seconds_left: secs_left });
                    }
                }
            } else if let Some((seat, deadline)) = r.action_deadline {
                // hold the deadline steady while paused — slide it forward by one second per
                // tick so when everyone reconnects, the actor still has the same time-left
                // as when the pause started.
                r.action_deadline = Some((seat, deadline + std::time::Duration::from_secs(1)));
            }

            // check reconnect window expiry. During payout (post-settlement), don't auto-evict
            // disconnected players — they're going to settlement view; the room cleanup task
            // tears the whole room down on PayoutComplete anyway.
            //
            // For real tables NOT yet in settlement: abandoning past the window auto-triggers
            // settlement with the remaining online player as priority signer. Bot tables and
            // pre-deposit rooms fall back to the legacy OpponentLeft behavior.
            let in_settlement = r.payout_triggered;
            let mut abandoners: Vec<u8> = Vec::new();
            for seat_idx in 0..r.players.len() {
                if let Some(ref p) = r.players[seat_idx] {
                    if let Some(disc_at) = p.disconnected_at {
                        if disc_at.elapsed() > RECONNECT_WINDOW {
                            abandoners.push(seat_idx as u8);
                        }
                    }
                }
            }
            for seat in &abandoners {
                let seat_idx = *seat as usize;
                if in_settlement {
                    // silent drop; settlement UI is the user's surface now
                    r.players[seat_idx] = None;
                    continue;
                }
                // remove abandoner first so winner_after_abandon sees the remaining alone
                r.players[seat_idx] = None;
                if r.bot_friendly {
                    tracing::info!("room: seat {} reconnect window expired (bot table), removing", seat);
                    r.broadcast(&ServerMsg::OpponentLeft { seat: *seat });
                    continue;
                }
                // find remaining online seat; if none, room is empty — just leave it
                let remaining: Vec<u8> = r.players.iter().enumerate()
                    .filter_map(|(i, p)| p.as_ref().filter(|pl| pl.disconnected_at.is_none()).map(|_| i as u8))
                    .collect();
                if remaining.len() != 1 {
                    tracing::info!("room: seat {} abandoned but no single remaining online seat — skip auto-settle", seat);
                    r.broadcast(&ServerMsg::OpponentLeft { seat: *seat });
                    continue;
                }
                let winner_seat = remaining[0];
                tracing::info!("room {}: seat {} abandoned mid-game; auto-settling to seat {}", r.code, seat, winner_seat);
                r.broadcast(&ServerMsg::OpponentAbandoned { seat: *seat });
                finish_table_after_bust(&mut r, &timeout_state, timeout_room.clone(), winner_seat);
            }

            // start next hand after timeout fold ended the hand
            if hand_ended {
                if let Some(winner) = r.winner_after_bust() {
                    finish_table_after_bust(&mut r, &timeout_state, timeout_room.clone(), winner);
                    drop(r);
                    continue;
                }
                drop(r);
                let rc = timeout_room.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let mut r = rc.lock().await;
                    if !r.payout_triggered && r.engine.hand_state().is_none() && r.player_count() >= 2 {
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
            ClientMsg::Join { name, pubkey, zcash_address } => {
                let mut r = room.lock().await;

                // settlement in flight — table is closed to new joiners + non-reconnect entries.
                // We still allow a previously-seated player to come back (their seat slot is
                // matched below via disconnected_at), so they can land in the settlement view.
                if r.payout_triggered {
                    let was_seated = r.players.iter().any(|p| matches!(p, Some(p) if p.name == name));
                    if !was_seated {
                        let _ = tx.send(ServerMsg::Error {
                            message: "table closed — payout in progress".into(),
                        });
                        continue;
                    }
                }

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

                    // Settlement-mode reconnect: replay the payout state instead of resuming
                    // the live table. This handles the leaver-reloads-during-settlement case
                    // (they should land in the settlement view to finish signing), and any
                    // intermediate reconnect after PayoutComplete / PayoutFailed.
                    if r.payout_triggered {
                        if let Some(s) = r.payout_signing_state.clone() {
                            let _ = tx.send(ServerMsg::PayoutSigningRequest {
                                relay_room: s.relay_room,
                                plan: s.plan,
                                priority_seat: s.priority_seat,
                            });
                        }
                        if let Some(txid) = r.payout_complete_txid.clone() {
                            let _ = tx.send(ServerMsg::PayoutComplete { txid });
                        }
                        if let Some(reason) = r.payout_failed_reason.clone() {
                            let _ = tx.send(ServerMsg::PayoutFailed { reason });
                        }
                        // notify peers we're back but don't re-send game state
                        for i in 0..r.max_seats as u8 {
                            if i != seat { r.send_to(i, ServerMsg::OpponentReconnected { seat }); }
                        }
                        continue;
                    }

                    // send current game state to reconnecting player.
                    let live_stacks: Vec<u64> = r.engine.hand_state()
                        .map(|h| h.seats.iter().map(|s| s.chips).collect())
                        .unwrap_or_else(|| r.engine.stacks().to_vec());
                    let _ = tx.send(ServerMsg::HandStarted {
                        hand_number: r.hand_number as u64,
                        button: r.button,
                        your_cards: r.hole_cards[seat as usize].as_ref().map(|c| {
                            [card_json(&c[0]), card_json(&c[1])]
                        }),
                        stacks: live_stacks,
                    });
                    if !r.community_cards.is_empty() {
                        let phase = r.engine.hand_state().map(|h| format!("{:?}", h.phase).to_lowercase())
                            .unwrap_or_else(|| "unknown".into());
                        let _ = tx.send(ServerMsg::CommunityCards {
                            phase,
                            cards: r.community_cards.iter().map(card_json).collect(),
                        });
                    }

                    // resync turn: if it's this seat's action, re-emit ActionRequired
                    if let Some((act_seat, valid)) = r.engine.pending_action() {
                        if act_seat == seat {
                            let _ = tx.send(ServerMsg::ActionRequired {
                                seat: act_seat,
                                valid_actions: valid.iter().map(|va| ValidActionJson {
                                    kind: format!("{:?}", va.kind).to_lowercase(),
                                    min_amount: va.min_amount, max_amount: va.max_amount,
                                }).collect(),
                            });
                        }
                    }

                    // notify all other players of reconnect
                    for i in 0..r.max_seats as u8 {
                        if i != seat { r.send_to(i, ServerMsg::OpponentReconnected { seat }); }
                    }
                    continue;
                }

                // access control
                match &r.access {
                    TableAccess::Mutuals { allowed_pubkeys } => {
                        let pk = pubkey.as_deref().unwrap_or("");
                        if !allowed_pubkeys.iter().any(|a| a == pk) {
                            let _ = tx.send(ServerMsg::Error { message: "mutuals-only table".into() });
                            continue;
                        }
                    }
                    // Private: having the room code is the auth (you got it via invite)
                    // Public: anyone can join
                    _ => {}
                }

                // fresh join
                let seat = r.players.iter().position(|p| p.is_none());
                if let Some(seat_idx) = seat {
                    let seat = seat_idx as u8;
                    r.players[seat_idx] = Some(Player {
                        name: name.clone(), seat, pubkey: pubkey.clone(),
                        zcash_address: zcash_address.clone(),
                        tx: tx.clone(), disconnected_at: None,
                    });
                    my_seat = Some(seat);
                    // first player is the host (table leader)
                    if r.host_seat.is_none() { r.host_seat = Some(seat); }
                    let _ = tx.send(ServerMsg::Seated { seat, name: name.clone() });

                    // notify existing players
                    for i in 0..r.max_seats as u8 {
                        if i != seat && r.players[i as usize].is_some() {
                            r.send_to(i, ServerMsg::OpponentJoined { seat, name: name.clone() });
                        }
                    }
                    // seat player in engine
                    let buyin = r.engine.rules.min_buy_in as u64;
                    let _ = r.engine.seat_player(seat, buyin);

                    if r.player_count() < 2 {
                        let _ = tx.send(ServerMsg::InviteLink {
                            url: format!("/{}", r.code),
                        });
                        let _ = tx.send(ServerMsg::Waiting);
                    } else if r.engine.hand_state().is_none() {
                        // auto-start once 2 players seated (deposit gating bypassed until real ZEC is wired)
                        r.start_hand();
                    }
                    drop(r);
                    notify_lobby_tables(&state.rooms, &state.lobby_users).await;
                } else {
                    let _ = tx.send(ServerMsg::Error { message: "table full".into() });
                }
            }
            ClientMsg::Chat { text } => {
                if let Some(seat) = my_seat {
                    let r = room.lock().await;
                    let player_name = r.players[seat as usize].as_ref()
                        .map(|p| p.name.clone()).unwrap_or_default();
                    r.broadcast(&ServerMsg::Chat { seat, name: player_name, text });
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
                // refuse to advance engine state while any seated player is in the reconnect
                // window. mirrors the SPA's button-disabled state; defensive against a buggy
                // / malicious client that submits an action despite the paused overlay.
                let any_disconnected = r.players.iter()
                    .any(|p| matches!(p, Some(pl) if pl.disconnected_at.is_some()));
                if any_disconnected {
                    let _ = tx.send(ServerMsg::Error { message: "hand paused — opponent offline".into() });
                    continue;
                }
                r.apply_action(seat, action);

                // hand complete — happy path, no jury needed
                if r.engine.hand_state().is_none() {
                    // real tables: if a player just busted (heads-up bust), settle to the winner.
                    // bot tables fall through and auto-rebuy in start_hand for endless demo play.
                    if let Some(winner) = r.winner_after_bust() {
                        finish_table_after_bust(&mut r, &state, room.clone(), winner);
                        drop(r);
                        continue;
                    }
                    let room_clone = room.clone();
                    drop(r);
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        let mut r = room_clone.lock().await;
                        if !r.payout_triggered && r.engine.hand_state().is_none() && r.player_count() >= 2 {
                            r.start_hand();
                        }
                    });
                    continue;
                }
            }
            ClientMsg::StartHand => {
                let mut r = room.lock().await;
                if r.engine.hand_state().is_none() && r.player_count() >= 2 {
                    r.start_hand();
                }
            }
            ClientMsg::AllowPlayer { pubkey } => {
                let mut r = room.lock().await;
                let seat = my_seat.unwrap_or(255);
                if r.host_seat != Some(seat) {
                    let _ = tx.send(ServerMsg::Error { message: "only the host can manage access".into() });
                    continue;
                }
                match &mut r.access {
                    TableAccess::Mutuals { ref mut allowed_pubkeys } => {
                        if !allowed_pubkeys.contains(&pubkey) {
                            allowed_pubkeys.push(pubkey.clone());
                            let _ = tx.send(ServerMsg::Status {
                                phase: "access".into(),
                                message: format!("allowed {}", &pubkey[..8.min(pubkey.len())]),
                            });
                        }
                    }
                    _ => {
                        let _ = tx.send(ServerMsg::Error { message: "table is not in mutuals mode".into() });
                    }
                }
            }
            ClientMsg::Leave => {
                if let Some(seat) = my_seat {
                    let mut r = room.lock().await;
                    if r.payout_triggered {
                        tracing::info!("room {}: seat {} leave ignored — payout already in flight", r.code, seat);
                        continue;
                    }
                    tracing::info!("room {}: seat {} leaving", r.code, seat);
                    r.payout_triggered = !r.bot_friendly;

                    let payouts = r.settlement_plan();

                    r.broadcast(&ServerMsg::GameOver {
                        reason: format!("seat {} left the table", seat),
                        payouts: payouts.clone(),
                    });

                    let payout_str: Vec<String> = payouts.iter()
                        .map(|(s, a)| format!("seat{}={}", s, a)).collect();
                    tracing::info!("settlement: room={} payouts=[{}]",
                        r.code, payout_str.join(", "));

                    // build the PCZT payout plan (seat, address, amount) — skip for bot tables
                    // or when nothing was actually deposited
                    let pczt_plan: Vec<PayoutLineJson> = if r.bot_friendly {
                        Vec::new()
                    } else {
                        payouts.iter().filter_map(|(s, amt)| {
                            r.seat_payout_addresses.get(*s as usize)
                                .and_then(|opt| opt.as_ref())
                                .map(|addr| PayoutLineJson {
                                    seat: *s, address: addr.clone(), amount_zat: *amt,
                                })
                        }).collect()
                    };

                    let code = r.code.clone();
                    let leaving_seat = seat;

                    r.action_deadline = None;

                    // Keep the leaver seated through settlement on real tables so they receive
                    // PayoutSigningRequest / PayoutComplete / PayoutFailed broadcasts. Their
                    // websocket tx must stay in r.players to be hit by broadcast(). The seat is
                    // cleared on the cleanup pass (5min post-PayoutComplete) when the room itself
                    // is removed. On bot tables (no payout), drop immediately as before.
                    if !pczt_plan.is_empty() {
                        // notify OTHER seats only — the leaver doesn't need to know they left,
                        // and broadcasting to them flips their SPA's view to 'waiting' for ~3s
                        // before the PayoutSigningRequest arrives, which looks like a deposit
                        // re-prompt.
                        for i in 0..r.max_seats as u8 {
                            if i != seat { r.send_to(i, ServerMsg::OpponentLeft { seat }); }
                        }
                    } else {
                        r.players[seat as usize] = None;
                        r.broadcast(&ServerMsg::OpponentLeft { seat });
                    }
                    drop(r);

                    // trigger on-chain payout if there's anything to pay out + escrow is wired
                    if !pczt_plan.is_empty() {
                        if let Some(escrow_url) = state.escrow_url.clone() {
                            let room_clone = room.clone();
                            let rooms_clone = state.rooms.clone();
                            let plan_clone = pczt_plan.clone();
                            tokio::spawn(async move {
                                trigger_payout(rooms_clone, room_clone, escrow_url, code, plan_clone, leaving_seat).await;
                            });
                        } else {
                            tracing::warn!("no ESCROW_URL configured — skipping on-chain payout");
                        }
                    }

                    notify_lobby_tables(&state.rooms, &state.lobby_users).await;
                }
            }
            ClientMsg::ReportDeposit { txid, amount } => {
                if let Some(seat) = my_seat {
                    let mut r = room.lock().await;
                    if (seat as usize) < r.deposits.len() {
                        r.deposits[seat as usize] += amount;
                        tracing::info!("deposit: room={} seat={} amount={} txid={}",
                            r.code, seat, amount, &txid[..txid.len().min(16)]);

                        // check if all seated players have deposited enough
                        let all_deposited = r.players.iter().enumerate().all(|(i, p)| {
                            p.is_none() || r.deposits[i] >= r.required_deposit
                        });

                        // broadcast deposit status
                        r.broadcast(&ServerMsg::DepositStatus {
                            escrow_address: r.escrow_address.clone(),
                            seat_addresses: r.seat_deposit_addresses.clone(),
                            player_a_deposit: r.deposits.get(0).copied().unwrap_or(0),
                            player_b_deposit: r.deposits.get(1).copied().unwrap_or(0),
                            required: r.required_deposit,
                            ready: all_deposited,
                        });

                        // auto-start game when both deposited
                        if all_deposited && r.player_count() >= 2 && r.engine.hand_state().is_none() {
                            r.start_hand();
                        }
                    }
                }
            }
            ClientMsg::Broadcast { data } => {
                // player sends filtered game state — fan out to all spectators
                let mut r = room.lock().await;
                r.spectators.retain(|tx| tx.send(data.clone()).is_ok());
            }
            ClientMsg::DkgComplete { escrow_ua, orchard_fvk } => {
                let Some(seat) = my_seat else {
                    let _ = tx.send(ServerMsg::Error { message: "not seated".into() });
                    continue;
                };
                let mut r = room.lock().await;
                if r.frost_room_code.is_none() || (seat as usize) >= r.dkg_reported_ua.len() {
                    continue;
                }
                tracing::info!(
                    "room {} seat {}: dkg complete ua={} fvk_tail=…{}",
                    r.code, seat, &escrow_ua[..escrow_ua.len().min(24)],
                    &orchard_fvk[orchard_fvk.len().saturating_sub(8)..],
                );
                r.dkg_reported_ua[seat as usize] = Some(escrow_ua.clone());

                let seated: Vec<usize> = r.players.iter().enumerate()
                    .filter_map(|(i, p)| p.as_ref().map(|_| i))
                    .collect();
                let all_match = seated.len() >= 2 && seated.iter().all(|&i| {
                    r.dkg_reported_ua.get(i).and_then(|u| u.as_deref()) == Some(escrow_ua.as_str())
                });
                if all_match && r.escrow_address != escrow_ua {
                    tracing::info!("room {}: escrow UA agreed by all players: {}", r.code, escrow_ua);
                    r.escrow_address = escrow_ua.clone();
                    let info = ServerMsg::RoomInfo {
                        code: r.code.clone(),
                        jury_nodes: JURY_N as u8,
                        jury_threshold: JURY_T as u8,
                        escrow: r.escrow_address.clone(),
                        buyin_zat: r.buyin_zat,
                        fee_per_seat: r.fee_per_seat,
                        frost_relay_url: r.frost_relay_url.clone(),
                        frost_room_code: r.frost_room_code.clone(),
                        seat_addresses: r.seat_deposit_addresses.clone(),
                        required_deposit: r.required_deposit,
                    };
                    r.broadcast(&info);
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
        drop(r);
        notify_lobby_tables(&state.rooms, &state.lobby_users).await;
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

    let escrow_url = std::env::var("ESCROW_URL").ok().filter(|s| !s.is_empty());

    let state = AppState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        lobby_users: Arc::new(Mutex::new(HashMap::new())),
        static_dir: static_dir.clone(),
        escrow_url: escrow_url.clone(),
    };

    tracing::info!("serving static files from {}", static_dir);
    tracing::info!("jury config: {}-of-{} frostito nested FROST (pallas)", JURY_T, JURY_N);
    match &escrow_url {
        Some(u) => tracing::info!("escrow service: {}", u),
        None => tracing::info!("escrow service: disabled (set ESCROW_URL to enable)"),
    }

    let app = Router::new()
        .route("/ws/lobby", axum::routing::get(lobby_ws_handler))
        .route("/api/tables", axum::routing::get(list_tables))
        .route("/new", axum::routing::get(create_room))
        .route("/{code}/ws", axum::routing::get(ws_handler))
        .route("/{code}/spectate", axum::routing::get(spectate_handler))
        .route("/{code}", axum::routing::get(room_page))
        .fallback_service(ServeDir::new(&static_dir))
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("poker server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
