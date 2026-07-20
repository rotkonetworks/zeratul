//! poker-server: websocket game server with FROST jury
//!
//! jury signing has two modes (selected by NARSIL_ENDPOINT env var):
//! - local: all jury shares in-process (demo/testing)
//! - narsil: calls live narsild validators over HTTP (production)

mod jury;
mod escrow_client;
mod tournament;

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
    /// co-signed game-over: this seat's signature over the agreed final outcome.
    /// Both seats submit independently; when both agree the server calls escrow /settle
    /// (winner payout) then triggers the on-chain payout. Staked tables only.
    Settlement {
        /// seat 0 (player A) final chip stack
        a_stack: u64,
        /// seat 1 (player B) final chip stack
        b_stack: u64,
        /// seat 0 payout Zcash address
        a_addr: String,
        /// seat 1 payout Zcash address
        b_addr: String,
        /// hex SHA-256 action-log hash (both peers derive identically)
        log_hash: String,
        /// hex Ed25519 signature by THIS seat's session key over the escrow settlement_message
        sig: String,
    },
    /// player broadcasts filtered game state to spectators
    Broadcast { data: String },
    Dispute,
    /// zafu_poker_dkg finished: this seat's view of the escrow UA + UFVK
    DkgComplete { escrow_ua: String, orchard_fvk: String },
    /// client detected something wrong with the escrow flow (DKG/deposit/settle/payout).
    /// Forwarded to the escrow's durable journal as a `client_fault` event so operators
    /// can triage and disputes have a record. Staked tables only.
    EscrowFault { phase: String, detail: String },
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
        /// AUTHORITATIVE table class: true iff this table holds real money (external escrow /
        /// FROST DKG wired at creation). The client reads THIS to decide whether to surface
        /// deposit / on-chain UX. Free-play and bot tables are `false`.
        #[serde(default)]
        staked: bool,
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
        /// per-seat on-chain-pinned payout addresses (recovered from the deposit memo).
        /// Surfaced to BOTH clients so each can build the identical co-signed settlement
        /// message at game over. None until the depositor's memo has been scanned.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        seat_payout_addresses: Vec<Option<String>>,
        player_a_deposit: u64,
        player_b_deposit: u64,
        /// MEMPOOL-seen (0-conf) per-seat totals — UX only. Additive; `ready` is unaffected by
        /// these (it stays gated on confirmed deposits).
        #[serde(default)]
        player_a_pending: u64,
        #[serde(default)]
        player_b_pending: u64,
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
        /// Seconds left before the server flips priority to the other seat. SPA renders this
        /// directly instead of running its own 90s counter — so a reconnect mid-wait resumes
        /// the right value instead of restarting at 90.
        fallback_secs_remaining: u64,
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
// frostito jury (3-of-5 FROST, OSST-gated)
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
    /// AUTHORITATIVE table class, decided ONCE at creation: true iff this table holds real
    /// money (external escrow / FROST DKG wired). Every money/deal gate keys on this, NOT on
    /// `bot_friendly` (which only means "chip-only demo"). A free-play non-bot table is
    /// `staked == false` and deals freely; a staked table keeps every deposit/settle guard.
    staked: bool,
    host_seat: Option<u8>,
    players: Vec<Option<Player>>,
    /// spectator channels
    spectators: Vec<mpsc::UnboundedSender<String>>,
    /// deposit tracking (zatoshis per seat) — CONFIRMED only; authoritative for settle/payout.
    deposits: Vec<u64>,
    /// mempool-pending deposit value per seat (0-conf, UX + deal gate only; NEVER money).
    deposits_pending: Vec<u64>,
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
    /// per-room capability token from the escrow; echoed on `/payout/initiate` (fail-closed there)
    payout_token: Option<String>,
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
    /// per-seat co-signed settlement submissions. Each seat POSTs its own signature over the
    /// agreed outcome via ClientMsg::Settlement. When both seats have submitted AND their
    /// claimed (stacks, addrs, log_hash) tuples MATCH, the server calls escrow /settle with
    /// both sigs. Disagreement ⇒ no settle (safe default; Leave→refund remains the fallback).
    settlement_submissions: Vec<Option<SettlementSubmission>>,
}

/// One seat's co-signed settlement claim + signature (from ClientMsg::Settlement).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SettlementSubmission {
    a_stack: u64,
    b_stack: u64,
    a_addr: String,
    b_addr: String,
    log_hash: String,
    /// hex Ed25519 sig by this seat's session key. Excluded from tuple-equality checks
    /// (each seat's sig differs); only the CLAIM must match across seats.
    sig: String,
}

impl SettlementSubmission {
    /// The agreed-outcome tuple both seats must match on (sig excluded).
    fn claim(&self) -> (u64, u64, &str, &str, &str) {
        (self.a_stack, self.b_stack, &self.a_addr, &self.b_addr, &self.log_hash)
    }
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

        // frostito: 2-of-3 escrow (player A + player B + jury)
        // jury's share s₃ born distributed via interleaved DKG — never materialized
        let mut rng = rand::thread_rng();
        let (player_a_share, player_b_share, jury_network, group_pubkey) =
            redpallas::setup_escrow(JURY_N, JURY_T, &mut rng)
                .expect("interleaved DKG should succeed");

        let _local_addr_hex = hex::encode(redpallas::derive_address_bytes(&group_pubkey));
        // SINGLE SOURCE OF TRUTH: decide the table class ONCE, here, from the escrow we were
        // handed. Everything downstream reads `staked` — never re-derives it from bot_friendly.
        let staked = external_escrow.is_staked();
        // Non-staked tables (bot demo + free-play) surface NOTHING escrow-ish: no FROST coords,
        // empty escrow address. This is the backstop that keeps ZEC out of free play even if a
        // creation path handed us stray escrow fields. Staked tables keep their real coords.
        let (frost_relay_url, frost_room_code, payout_token, escrow_address) = if staked {
            // DKG mode: escrow_address stays empty until players' DkgComplete agrees on a UA.
            let dkg_mode = external_escrow.frost_relay_url.is_some()
                && external_escrow.frost_room_code.is_some();
            let escrow_address = external_escrow.address
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| if dkg_mode { String::new() } else { _local_addr_hex });
            (
                external_escrow.frost_relay_url,
                external_escrow.frost_room_code,
                external_escrow.payout_token,
                escrow_address,
            )
        } else {
            (None, None, None, String::new())
        };

        tracing::info!(
            "room {} created: staked={} escrow {}",
            code, staked, &escrow_address[..escrow_address.len().min(24)]
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
        // across seats (e.g. 3 seats × ceil(10000/3) = 3 × 3334 = 10002 ≥ 10000). Only STAKED
        // tables carry an on-chain payout fee; bot + free-play tables move no money.
        let fee_per_seat = if staked { (TX_PAYOUT_FEE_ZAT + seats as u64 - 1) / seats as u64 } else { 0 };
        let required_deposit = buyin + fee_per_seat;

        Room {
            code, max_seats: seats, access, bot_friendly, staked, host_seat: None,
            players: (0..seats).map(|_| None).collect(),
            spectators: Vec::new(),
            deposits: vec![0; seats],
            deposits_pending: vec![0; seats],
            required_deposit,
            fee_per_seat,
            engine, hand_number: 0, button: 0,
            hole_cards: (0..seats).map(|_| None).collect(),
            community_cards: Vec::new(),
            jury, action_log: ActionLog::new(),
            escrow_address,
            frost_relay_url,
            frost_room_code,
            payout_token,
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
            settlement_submissions: vec![None; seats],
        }
    }

    fn player_count(&self) -> usize {
        self.players.iter().filter(|p| matches!(p, Some(p) if p.disconnected_at.is_none())).count()
    }

    /// Find a disconnected seat this (re)joining player should resume. Name must match, the
    /// seat must be in its reconnect window, and — if the seat registered a pubkey — the
    /// pubkey must match (anon/name-only seats skip the pubkey check).
    fn find_reconnect_seat(&self, name: &str, pubkey: Option<&str>) -> Option<usize> {
        self.players.iter().position(|p| {
            matches!(p, Some(p) if
                p.name == name &&
                p.disconnected_at.is_some() &&
                (p.pubkey.is_none() || p.pubkey.as_deref() == pubkey)
            )
        })
    }

    /// True if `name` is already held by a seat that registered a *different* pubkey — a
    /// same-name seat-hijack attempt, which the Join handler rejects.
    fn is_seat_hijack(&self, name: &str, pubkey: Option<&str>) -> bool {
        self.players.iter().any(|p| matches!(p, Some(p) if
            p.name == name && p.pubkey.is_some() && p.pubkey.as_deref() != pubkey
        ))
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

    /// Build the `RoomInfo` control frame describing this room's escrow/DKG coords.
    /// Used both on the old `/ws` path and by the `/p2p` relay bridge so a freshly
    /// joined staked peer immediately learns the FROST relay + deposit addresses.
    fn room_info(&self) -> ServerMsg {
        // Non-staked tables never surface escrow/frost coords: they were zeroed at creation, so
        // reading them straight is already empty. `staked` is the authoritative class flag the
        // client keys its deposit UX on.
        ServerMsg::RoomInfo {
            code: self.code.clone(),
            jury_nodes: JURY_N as u8,
            jury_threshold: JURY_T as u8,
            escrow: self.escrow_address.clone(),
            buyin_zat: self.buyin_zat,
            fee_per_seat: self.fee_per_seat,
            frost_relay_url: self.frost_relay_url.clone(),
            frost_room_code: self.frost_room_code.clone(),
            seat_addresses: self.seat_deposit_addresses.clone(),
            required_deposit: self.required_deposit,
            staked: self.staked,
        }
    }

    /// payout plan for the room — `(seat, zatoshi)` pairs. Derived from real deposits + this
    /// server's OWN poker engine stacks. Each seat's basis is `deposit - fee_per_seat` so the
    /// per-player share of the on-chain fee comes out of the pre-funded amount. Total of all
    /// outputs + `TX_PAYOUT_FEE_ZAT` equals total deposits.
    ///
    /// SETTLE-HIGH-1 / Settle-C1: this is a SERVER-COMPUTED split and is therefore valid ONLY on
    /// server-engine tables (the `/ws` handler), where the server dealt the cards and legitimately
    /// knows the stacks. It MUST NEVER be used on a ZK/relay `/p2p` table: there the server never
    /// saw the cards, `self.engine` is meaningless, and the escrow is the single split authority
    /// (co-signed `/settle` winner-exit, or `/cancel` refund-each-own). The relay Leave/settle
    /// paths no longer call this.
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
    /// and the rest are at 0, the table is settled. Returns the winning seat. Only STAKED
    /// tables settle on-chain; chip-only tables (bot + free-play) and rooms already in payout
    /// return None.
    fn winner_after_bust(&self) -> Option<u8> {
        if !self.staked || self.payout_triggered { return None; }
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

    /// every seated player has CONFIRMED the required amount AND we have their refund address.
    /// Dealing now uses [`Room::deal_ready`] (confirmed+mempool); kept as the confirmed-only
    /// predicate for any money gate that must ignore unconfirmed value.
    #[allow(dead_code)]
    fn deposits_satisfied(&self) -> bool {
        self.players.iter().enumerate().all(|(i, p)| {
            if p.is_none() { return true; }
            self.deposits.get(i).copied().unwrap_or(0) >= self.required_deposit
                && self.seat_payout_addresses.get(i).and_then(|a| a.as_ref()).is_some()
        })
    }

    /// deal gate: every seated player's CONFIRMED+PENDING (mempool) value covers the buyin AND we
    /// have their refund address. Dealing moves no money (settle/rake/payout stay confirmed-only in
    /// the escrow), so a 0-conf mempool sighting is enough to start the hand — a block lands mid-hand.
    fn deal_ready(&self) -> bool {
        self.players.iter().enumerate().all(|(i, p)| {
            if p.is_none() { return true; }
            let conf = self.deposits.get(i).copied().unwrap_or(0);
            let pend = self.deposits_pending.get(i).copied().unwrap_or(0);
            conf + pend >= self.required_deposit
                && self.seat_payout_addresses.get(i).and_then(|a| a.as_ref()).is_some()
        })
    }

    fn start_hand(&mut self) {
        if self.payout_triggered {
            tracing::debug!("room {}: start_hand blocked — payout already triggered", self.code);
            return;
        }
        // DEPOSIT GATE keys on STAKED, not bot_friendly: only real-money tables wait for buyins.
        // Bot demo AND free-play (non-bot, non-staked) tables deal chip-only with no deposit gate.
        // Staked tables deal once both buyins are seen (confirmed OR mempool); settlement stays
        // gated on confirmed value in the escrow.
        if self.staked && !self.deal_ready() {
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

        // PRIVACY GATE (finding #1: operator sees plaintext cards).
        //
        // The server-authoritative engine deals from a plaintext `Vec<Card>` it
        // owns, so anything it deals is visible to the operator. That is only
        // acceptable for chip-only tables (no real value, no privacy stake) —
        // both bot demo AND free-play. For any STAKED table (real value) we
        // MUST NOT leak hole cards to the operator: refuse to deal via the
        // custodial shuffler unless the operator has explicitly opted into
        // insecure custodial mode. The trustless deal (client-side mental-poker
        // ceremony over the /ws/zid blind relay) is the intended replacement;
        // see the checklist in `custodial_deal_allowed()` below.
        if !custodial_deal_allowed(self.staked) {
            tracing::error!(
                "room {}: refusing to deal — custodial plaintext shuffle would leak hole cards \
                 to the operator on a real-value table. Trustless dealing not yet wired on this \
                 code path; set POKER_ALLOW_PLAINTEXT_DEAL=1 only for trusted/test operators.",
                self.code
            );
            self.broadcast(&ServerMsg::Error {
                message: "dealing disabled: trustless (server-blind) shuffle is not yet available \
                          on this table, and custodial plaintext dealing is refused to protect \
                          your hole cards from the operator".into(),
            });
            // roll back the hand-number/button bump so a later retry (e.g. after
            // the operator enables the override, or a trustless path lands) is clean.
            self.hand_number -= 1;
            return;
        }

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

/// Whether the custodial (operator-visible) plaintext deal is permitted.
///
/// SECURITY: the custodial shuffler below produces a plaintext deck that the
/// operator can read in full — it is NOT mental poker, and the SHA256
/// "commitment" hides nothing from the operator (the operator holds the
/// preimage). It is only acceptable where there is no privacy or money at
/// stake, i.e. chip-only bot demo tables.
///
/// For real-value tables it is refused unless the operator explicitly opts in
/// with `POKER_ALLOW_PLAINTEXT_DEAL=1` (trusted/test operators only). The
/// intended replacement is the trustless client-side mental-poker ceremony,
/// whose primitives already exist:
///   - `zk_shuffle::remasking::ElGamalCiphertext` (encrypt / remask / decrypt)
///   - `zk_shuffle::{prove_shuffle, verify_shuffle}` (Chaum-Pedersen shuffle)
///   - `zk_shuffle::reveal::{PossessionProof, RevealProof}` (DLEQ reveal)
///   - client `poker-shuffle-wasm` (`ShuffleKeys`/`ShuffleState`/`RevealState`)
///     already drives the full 2-player ceremony end to end
///   - `web/src/shuffle-filter.ts` already speaks the shuffle_pk/init/done/reveal wire
///   - the blind e2ee relay (`/ws/zid`, `handle_zid_socket`) can carry it without
///     the operator ever seeing a card.
/// What remains is to make the live `/{code}/ws` path route dealing through that
/// ceremony instead of the server engine's plaintext `Vec<Card>` — see the
/// checklist in the agent report / module docs.
fn custodial_deal_allowed(staked: bool) -> bool {
    if !staked {
        return true; // chip-only (bot demo OR free-play): no real value, no privacy stake
    }
    // STAKED table: real value at risk. Refuse the plaintext custodial shuffle unless the
    // operator has explicitly opted into insecure custodial mode.
    matches!(
        std::env::var("POKER_ALLOW_PLAINTEXT_DEAL").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

/// shuffle deck with zk-shuffle proof. returns (shuffled_cards, deck_commitment).
/// the commitment is SHA256 of the shuffled deck — included in HandTranscript.
///
/// SECURITY: this is the CUSTODIAL shuffler — the returned deck is plaintext and
/// fully visible to the operator. Callers MUST gate it behind
/// `custodial_deal_allowed()`. It is not mental poker; see that function's docs.
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
    if !r.staked || r.payout_triggered { return; }
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
const PRIORITY_SIGNER_FALLBACK_SECS: u64 = 90;

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
    // capability token minted by the escrow at room creation — /payout/initiate is
    // gated on it (fail-closed), so forward it or the payout is rejected.
    let payout_token = room.lock().await.payout_token.clone();
    let req = escrow_client::InitiatePayoutReq {
        outputs,
        fee_zat: Some(TX_PAYOUT_FEE_ZAT),
        anchor_height: None,
        payout_token,
    };

    let relay_room = match escrow_client::initiate_payout(&escrow_url, &code, &req).await {
        // `/payout/initiate` opens the relay room before responding, so it is present inline; fall
        // back to a status poll defensively in case a future escrow build defers it.
        Ok(resp) => match resp.relay_room {
            Some(rr) => rr,
            None => match escrow_client::resolve_relay_room(&escrow_url, &code).await {
                Ok(rr) => rr,
                Err(e) => {
                    tracing::error!("payout {}: could not resolve relay room: {}", code, e);
                    let mut r = room.lock().await;
                    r.broadcast(&ServerMsg::PayoutFailed { reason: format!("relay room: {}", e) });
                    return;
                }
            },
        },
        Err(e) => {
            tracing::error!("payout {}: initiate failed: {}", code, e);
            let mut r = room.lock().await;
            r.broadcast(&ServerMsg::PayoutFailed { reason: format!("initiate: {}", e) });
            return;
        }
    };
    // server passed the outputs in here, so the display plan is exactly what we sent.
    drive_payout_signing(rooms, room, escrow_url, code, relay_room, plan, priority_seat).await;
}

/// ZK/relay + refund entry point: the escrow is the SINGLE split authority. Instead of the
/// server computing outputs, it asks the escrow to build the plan — either the co-signed winner
/// plan already recorded at `/settle` (`SettledPayout`) or the self-service refund-each-own plan
/// (`Cancel`). The escrow returns the relay room AND the plan it computed (for display only), and
/// the rest of the flow (signing request broadcast + status poll) is shared with `trigger_payout`.
///
/// This is the ONLY payout path used by the ZK/relay `/p2p` handler: the server never sends a
/// caller-chosen winner split to the escrow on a ZK table (it cannot validate one — cards never
/// touched the server).
enum EscrowComputedPayout {
    /// Drive the on-chain payout for an outcome already verified at `/settle` (winner-exit).
    SettledPayout,
    /// Self-service abandonment refund: escrow refunds each depositor their own confirmed deposit.
    Cancel { reason: String },
}

async fn trigger_escrow_computed_payout(
    rooms: Rooms,
    room: Arc<Mutex<Room>>,
    escrow_url: String,
    code: String,
    kind: EscrowComputedPayout,
    priority_seat: u8,
) {
    // fail-closed capability token — same gate as /payout/initiate.
    let payout_token = room.lock().await.payout_token.clone();
    let resp = match kind {
        EscrowComputedPayout::SettledPayout => {
            let req = escrow_client::InitiateSettledPayoutReq {
                // empty: the escrow spends the co-signed plan it recorded at /settle and ignores
                // these — the server asserts no split of its own on a settled game.
                outputs: Vec::new(),
                fee_zat: Some(TX_PAYOUT_FEE_ZAT),
                anchor_height: None,
                payout_token,
            };
            escrow_client::initiate_settled_payout(&escrow_url, &code, &req).await
        }
        EscrowComputedPayout::Cancel { reason } => {
            let req = escrow_client::CancelRefundReq {
                reason,
                payout_token,
                fee_zat: Some(TX_PAYOUT_FEE_ZAT),
            };
            escrow_client::cancel_refund(&escrow_url, &code, &req).await
        }
    };
    let resp = match resp {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("payout {}: escrow-computed initiate failed: {}", code, e);
            let mut r = room.lock().await;
            r.payout_triggered = false; // allow retry / operator arbitration
            r.broadcast(&ServerMsg::PayoutFailed { reason: format!("initiate: {}", e) });
            return;
        }
    };
    // Resolve the FROST relay room. The settled `/payout/initiate` path returns it inline; the
    // `/cancel` path responds before opening the relay room, so we poll `/payout/status` for it.
    let relay_room = match resp.relay_room {
        Some(rr) => rr,
        None => match escrow_client::resolve_relay_room(&escrow_url, &code).await {
            Ok(rr) => rr,
            Err(e) => {
                tracing::error!("payout {}: could not resolve escrow relay room: {}", code, e);
                let mut r = room.lock().await;
                r.payout_triggered = false; // allow retry / operator arbitration
                r.broadcast(&ServerMsg::PayoutFailed { reason: format!("relay room: {}", e) });
                return;
            }
        },
    };
    // display plan comes from the escrow (the split authority), not from us.
    let plan: Vec<PayoutLineJson> = resp.plan.into_iter()
        .map(|l| PayoutLineJson { seat: l.seat, address: l.address, amount_zat: l.amount_zat })
        .collect();
    drive_payout_signing(rooms, room, escrow_url, code, relay_room, plan, priority_seat).await;
}

/// Shared payout tail: broadcast the `PayoutSigningRequest`, remember it for reconnect replay,
/// then poll `/payout/status` until terminal — flipping the priority signer after the fallback
/// window so an unresponsive seat can't stall the co-sign. Used by every payout shape (server
/// outputs, escrow-computed winner plan, escrow-computed refund).
async fn drive_payout_signing(
    rooms: Rooms,
    room: Arc<Mutex<Room>>,
    escrow_url: String,
    code: String,
    relay_room: String,
    plan: Vec<PayoutLineJson>,
    priority_seat: u8,
) {
    tracing::info!("payout {}: signing room={} priority_seat={}", code, relay_room, priority_seat);

    {
        let mut r = room.lock().await;
        r.broadcast(&ServerMsg::PayoutSigningRequest {
            relay_room: relay_room.clone(),
            plan: plan.clone(),
            priority_seat,
            fallback_secs_remaining: PRIORITY_SIGNER_FALLBACK_SECS,
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
                        fallback_secs_remaining: PRIORITY_SIGNER_FALLBACK_SECS,
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
            if r.deposits_pending.len() >= 2 {
                r.deposits_pending[0] = state.player_a_deposit_pending;
                r.deposits_pending[1] = state.player_b_deposit_pending;
            }

            // DEAL gate counts confirmed + mempool-pending so the hand starts the instant both
            // buyins are visible (no ~75s block wait). Money paths (settle/rake/payout) stay
            // confirmed-only in the escrow — r.deposits still mirrors CONFIRMED value.
            let ready = r.deal_ready() && r.player_count() >= 2;
            let snapshot = (state.player_a_deposit, state.player_b_deposit, ready);
            if r.last_deposit_broadcast != Some(snapshot) {
                r.last_deposit_broadcast = Some(snapshot);
                r.broadcast(&ServerMsg::DepositStatus {
                    escrow_address: r.escrow_address.clone(),
                    seat_addresses: r.seat_deposit_addresses.clone(),
                    seat_payout_addresses: r.seat_payout_addresses.clone(),
                    player_a_deposit: state.player_a_deposit,
                    player_b_deposit: state.player_b_deposit,
                    player_a_pending: state.player_a_deposit_pending,
                    player_b_pending: state.player_b_deposit_pending,
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
    /// true when the user has flagged "looking to play" — surfaced on the board so
    /// others know who to challenge (vs. who's just hanging out / spectating).
    ready: bool,
}

/// one entry in the lobby player board
#[derive(Clone, Serialize, Deserialize)]
struct LobbyPlayer {
    name: String,
    ready: bool,
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
    /// player list update (name + ready-to-play flag)
    Players { players: Vec<LobbyPlayer> },
    /// table list update
    Tables { tables: Vec<serde_json::Value> },
    /// a challenge landed on you — show an Accept/Decline prompt for `table_code`
    Challenge { from: String, table_code: String },
    /// your outgoing challenge created a table — go sit down at `table_code` and wait
    ChallengeSent { to: String, table_code: String },
    /// the player you challenged accepted — they're on their way to the table
    ChallengeAccepted { by: String },
    /// the player you challenged declined
    ChallengeDeclined { by: String },
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
    /// flag/unflag "looking to play" so others see me on the board
    Ready { ready: bool },
    /// challenge player to a game — server mints a table and prompts them
    Challenge { to: String },
    /// accept an incoming challenge for `table_code` from `from`
    AcceptChallenge { from: String, table_code: String },
    /// decline an incoming challenge from `from`
    DeclineChallenge { from: String },
}

type LobbyUsers = Arc<Mutex<HashMap<String, LobbyUser>>>;

/// a connected relay peer: its outgoing frame sink and, once paired, the
/// pubkey of the peer it exchanges frames with.
struct PeerConn {
    tx: mpsc::UnboundedSender<WsMessage>,
    peer: Option<String>,
}

/// blind e2ee relay state (Path B mental poker). the relay pairs two peers by
/// pubkey and forwards opaque frames between them; it never inspects payloads
/// — see zafu packages/zid/src/noise-channel.ts. it only brokers a pubkey
/// *introduction* so a host (who doesn't know the guest's key up front) learns
/// who is connecting; the pubkeys are public and the game payloads stay e2ee.
/// `pending` buffers frames that arrive before the destination peer connects,
/// so the Noise handshake can't race.
#[derive(Default)]
struct ZidRelay {
    /// session-pubkey (hex) -> connection
    peers: HashMap<String, PeerConn>,
    /// dest-pubkey (hex) -> frames waiting for that peer to connect
    pending: HashMap<String, Vec<Vec<u8>>>,
}
type ZidPeers = Arc<Mutex<ZidRelay>>;

/// a peer connected to a room-keyed blind relay: its nick (for join/leave
/// notices) and its outgoing frame sink. game payloads are already e2ee
/// (x25519 → AES-GCM, see web/src/transport.ts) — the relay only forwards the
/// opaque `_enc` envelopes and never inspects them.
struct RelayPeer {
    nick: String,
    tx: mpsc::UnboundedSender<WsMessage>,
}

/// room-keyed blind relay registry: room code -> connected peers. Seats are
/// assigned by join order (first = 0). This is the transport for Path B: both
/// clients run the game engine + mental-poker ceremony locally and exchange
/// only ciphertext through here, so the operator sees no cards and the two
/// players never open a direct socket to each other (no peer IP leak).
type RelayRooms = Arc<Mutex<HashMap<String, Vec<RelayPeer>>>>;

#[derive(Clone)]
struct AppState {
    rooms: Rooms,
    lobby_users: LobbyUsers,
    static_dir: String,
    /// base url of the poker-escrow service, e.g. http://127.0.0.1:3034; None disables remote escrow
    escrow_url: Option<String>,
    /// blind e2ee relay peer registry (Path B mental poker, pubkey-paired Noise)
    zid_peers: ZidPeers,
    /// room-keyed blind relay registry (Path B, room-code addressed)
    relay_rooms: RelayRooms,
    /// non-custodial tournament matchmaker: bracket STATE only, never funds.
    tournaments: Tournaments,
    /// serializes PAID-match escrow provisioning so two players racing on the same match can't
    /// spin up two DKGs. Dedicated (not the tournaments hub lock) so provisioning — which awaits a
    /// network round-trip / DKG — never blocks bracket reads, joins, result reports, or the ticker.
    match_provision_lock: Arc<Mutex<()>>,
}

/// Shared tournament registry (bracket state) + the both-agree result-report ledger.
type Tournaments = Arc<Mutex<TournamentHub>>;

/// Wraps the pure `tournament::Registry` with the anti-cheat result ledger. For FREE tournaments
/// we don't have an on-chain escrow to arbitrate a match, so we gate bracket advancement on BOTH
/// seats independently reporting the SAME winner. `pending[match_id][reporter] = claimed_winner`.
#[derive(Default)]
struct TournamentHub {
    registry: tournament::Registry,
    /// per-tournament, per-match map of reporter -> the winner they claimed.
    /// keyed `(tournament_id, match_id)`.
    reports: HashMap<(String, u32), HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// blind e2ee relay websocket (Path B mental poker).
///
/// the relay is a dumb pubkey-pair router: it forwards opaque frames between
/// two peers and never sees plaintext. peers run the zafu wallet's Noise IK
/// channel (packages/zid/src/noise-channel.ts) end-to-end over this socket, so
/// the operator cannot read cards or alter the game. wire:
///   - first text frame: {"type":"announce","from":<hex pk>,"to":<hex pk>}
///   - thereafter: binary Noise frames, forwarded verbatim to peer `to`.
async fn zid_relay_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_zid_socket(socket, state))
}

async fn handle_zid_socket(socket: WebSocket, state: AppState) {
    use futures::{SinkExt, StreamExt};
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();

    // send task: deliver relay frames (peer-intro text + forwarded binary)
    let send_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if ws_tx.send(frame).await.is_err() { break; }
        }
    });

    // notify a peer which pubkey is connecting to them (public info; lets a
    // host that has no prior knowledge of the guest address its replies)
    fn intro(pk: &str) -> WsMessage {
        WsMessage::Text(format!(r#"{{"type":"peer","pubkey":"{}"}}"#, pk).into())
    }

    let mut me: Option<String> = None; // our pubkey, once announced

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            WsMessage::Text(t) => {
                // only the announce frame is inspected — it carries no secrets
                let v: serde_json::Value = match serde_json::from_str(&t) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("type").and_then(|x| x.as_str()) != Some("announce") { continue; }
                // `to` is optional: a host may not know the guest's key yet
                let from = match v.get("from").and_then(|x| x.as_str()) {
                    Some(f) => f.to_string(),
                    None => continue,
                };
                let to = v.get("to").and_then(|x| x.as_str()).map(|s| s.to_string());

                let mut relay = state.zid_peers.lock().await;
                relay.peers.insert(from.clone(), PeerConn { tx: tx.clone(), peer: to.clone() });
                // flush frames that arrived before we connected
                if let Some(buffered) = relay.pending.remove(&from) {
                    for frame in buffered { let _ = tx.send(WsMessage::Binary(frame.into())); }
                }
                // if we named a peer that is already here, pair both ways and
                // introduce ourselves so they can address us back
                if let Some(dest) = to {
                    if let Some(other) = relay.peers.get_mut(&dest) {
                        if other.peer.is_none() { other.peer = Some(from.clone()); }
                        let _ = other.tx.send(intro(&from));
                    }
                }
                me = Some(from);
            }
            WsMessage::Binary(data) => {
                let Some(my_pk) = me.as_ref() else { continue }; // must announce first
                let data = data.to_vec();
                let mut relay = state.zid_peers.lock().await;
                let dest = relay.peers.get(my_pk).and_then(|c| c.peer.clone());
                match dest {
                    Some(d) => match relay.peers.get(&d) {
                        Some(other) => { let _ = other.tx.send(WsMessage::Binary(data.into())); }
                        None => relay.pending.entry(d).or_default().push(data),
                    },
                    None => { /* not paired yet — drop until introduced */ }
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }

    // deregister on disconnect
    if let Some(pk) = me {
        let mut relay = state.zid_peers.lock().await;
        if relay.peers.get(&pk).map_or(false, |c| c.tx.same_channel(&tx)) {
            relay.peers.remove(&pk);
        }
    }
    send_task.abort();
}

/// room-keyed blind relay websocket (Path B mental poker).
///
/// wire protocol (matches web/src/transport.ts `createRelayTransport`):
///   client -> `{"t":"create","nick":..}`  -> server `{"t":"created","room":<code>}`
///   client -> `{"t":"join","room":..,"nick":..}` -> server
///             `{"t":"joined","room":..,"count":N,"seat":S}` to the joiner and
///             `{"t":"system","text":"<nick> joined"}` to peers already present
///   client -> `{"t":"msg","text":<opaque>}` -> forwarded verbatim to the other
///             peers as `{"t":"msg","text":..,"nick":..,"ts":<ms>}`
///   client -> `{"t":"part"}` -> leave
/// The `text` payload is an already-encrypted envelope; the relay never parses
/// it. Seats are assigned by join order so host/guest doesn't depend on the URL.
async fn relay_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_relay_socket(socket, state))
}

async fn handle_relay_socket(socket: WebSocket, state: AppState) {
    use futures::{SinkExt, StreamExt};
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();

    let send_task = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if ws_tx.send(m).await.is_err() { break; }
        }
    });

    fn frame(v: serde_json::Value) -> WsMessage { WsMessage::Text(v.to_string().into()) }
    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    /// Wrap a `ServerMsg` control frame for the relay wire, distinct from the opaque
    /// `_enc` peer frames the relay forwards blind. The client demuxes on `t == "srv"`.
    fn srv_frame(msg: &ServerMsg) -> WsMessage {
        WsMessage::Text(
            serde_json::json!({ "t": "srv", "msg": msg }).to_string().into(),
        )
    }

    let mut my_room: Option<String> = None;
    let mut my_nick = String::from("anon");

    // --- staked-table (escrow) bridge state -------------------------------
    // Non-None only when this /p2p peer joined a room that has an escrow-aware
    // `Room` in state.rooms (frost_room_code set => ESCROW_URL was wired at
    // create time). For free-play / pure-relay tables these stay None and the
    // handler behaves byte-for-byte as before.
    let mut escrow_room: Option<Arc<Mutex<Room>>> = None;
    let mut escrow_seat: Option<u8> = None;
    // ServerMsg sink registered into Room.players[seat].tx; a forwarder task
    // serializes what the Room broadcasts here into `srv` frames on our socket.
    let mut srv_forward_task: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(Ok(msg)) = ws_rx.next().await {
        let t = match msg {
            WsMessage::Text(t) => t,
            WsMessage::Close(_) => break,
            _ => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&t) { Ok(v) => v, Err(_) => continue };
        match v.get("t").and_then(|x| x.as_str()) {
            Some("create") => {
                if my_room.is_some() { continue; }
                if let Some(n) = v.get("nick").and_then(|x| x.as_str()) { my_nick = n.to_string(); }
                let code = generate_room_code();
                state.relay_rooms.lock().await.entry(code.clone()).or_default();
                let _ = tx.send(frame(serde_json::json!({ "t": "created", "room": code })));
                // client follows up with a `join` for the freshly minted code
            }
            Some("join") => {
                let room = match v.get("room").and_then(|x| x.as_str()) {
                    Some(r) if !r.is_empty() => r.to_string(),
                    _ => { let _ = tx.send(frame(serde_json::json!({ "t": "error", "msg": "missing room" }))); continue; }
                };
                if let Some(n) = v.get("nick").and_then(|x| x.as_str()) { my_nick = n.to_string(); }
                let mut rooms = state.relay_rooms.lock().await;
                let peers = rooms.entry(room.clone()).or_default();
                // Prune peers whose socket has already closed BEFORE the full-check. A client that
                // reconnects (or a user who refreshed after a failed attempt) opens a NEW channel;
                // its OLD channel lingers in `peers` until the disconnect cleanup (peers.retain
                // below) fires. If the reconnect races ahead of that cleanup, the room looks full
                // and the client rejects ITSELF with a bogus "room full", then retries forever
                // (the reconnect loop seen in the field). Dropping dead channels first makes
                // reconnect collision-free and also fixes seat = peers.len() drifting on stale slots.
                peers.retain(|p| !p.tx.is_closed());
                // heads-up: cap at 2 players (spectators are a later, separate path)
                if peers.iter().all(|p| !p.tx.same_channel(&tx)) && peers.len() >= 2 {
                    drop(rooms);
                    let _ = tx.send(frame(serde_json::json!({ "t": "error", "msg": "room full" })));
                    continue;
                }
                let seat = peers.len();
                peers.push(RelayPeer { nick: my_nick.clone(), tx: tx.clone() });
                let count = peers.len();
                my_room = Some(room.clone());
                let _ = tx.send(frame(serde_json::json!({ "t": "joined", "room": room, "count": count, "seat": seat })));
                // let peers already present know someone joined (transport.ts maps
                // a "joined"/"reconnected" system notice to opponent presence)
                for p in peers.iter() {
                    if !p.tx.same_channel(&tx) {
                        let _ = p.tx.send(frame(serde_json::json!({ "t": "system", "text": format!("{} joined", my_nick) })));
                    }
                }
                drop(rooms);

                // --- staked-table bridge --------------------------------------
                // If this room code has an escrow-aware Room in state.rooms AND that
                // room is in DKG/escrow mode (frost_room_code set — only possible
                // when ESCROW_URL was wired at create time), subscribe this relay
                // socket to the Room's ServerMsg control stream and take a seat in
                // Room.players so broadcast()/send_to() reach us. This is what makes
                // the deposit poller + payout signing broadcasts actually land on a
                // P2P client. Guarded so free-play / bot / pure-relay tables (no such
                // Room, or frost_room_code == None) take the byte-for-byte-old path.
                if escrow_room.is_none() {
                    let room_arc = state.rooms.lock().await.get(&room).cloned();
                    if let Some(room_arc) = room_arc {
                        let mut r = room_arc.lock().await;
                        // staked only: DKG-mode escrow => frost coords present.
                        if r.staked && r.frost_room_code.is_some() {
                            // per-connection ServerMsg channel; the Room writes control
                            // frames here via broadcast()/send_to() and we forward them
                            // to the socket as `srv` frames.
                            let (srv_tx, mut srv_rx) = mpsc::unbounded_channel::<ServerMsg>();
                            let ws_sink = tx.clone();
                            let fwd = tokio::spawn(async move {
                                while let Some(m) = srv_rx.recv().await {
                                    if ws_sink.send(srv_frame(&m)).is_err() { break; }
                                }
                            });
                            srv_forward_task = Some(fwd);

                            // Seat/reseat this peer in the Room WITHOUT running the
                            // server-side poker engine. Reconnect: reuse the seat by
                            // relay join order (heads-up) and refresh its tx sink.
                            let seat_u8 = seat as u8;
                            let seat_idx = seat_u8 as usize;
                            if seat_idx < r.players.len() {
                                if let Some(Some(p)) = r.players.get_mut(seat_idx) {
                                    // reconnect: swap the sink, clear disconnect timer
                                    p.tx = srv_tx.clone();
                                    p.disconnected_at = None;
                                } else {
                                    r.players[seat_idx] = Some(Player {
                                        name: my_nick.clone(),
                                        seat: seat_u8,
                                        pubkey: None,
                                        zcash_address: None,
                                        tx: srv_tx.clone(),
                                        disconnected_at: None,
                                    });
                                }
                                escrow_room = Some(room_arc.clone());
                                escrow_seat = Some(seat_u8);
                                // push current room escrow coords immediately so the
                                // client can join the FROST DKG room + show deposit UI.
                                let _ = srv_tx.send(r.room_info());
                                if r.escrow_address.is_empty() || !r.deal_ready() {
                                    let ready = r.deal_ready() && r.player_count() >= 2;
                                    let _ = srv_tx.send(ServerMsg::DepositStatus {
                                        escrow_address: r.escrow_address.clone(),
                                        seat_addresses: r.seat_deposit_addresses.clone(),
                                        seat_payout_addresses: r.seat_payout_addresses.clone(),
                                        player_a_deposit: r.deposits.get(0).copied().unwrap_or(0),
                                        player_b_deposit: r.deposits.get(1).copied().unwrap_or(0),
                                        // reconnect replay: pending is UX-only and refreshed by the
                                        // next escrow poll broadcast; 0 here is fine.
                                        player_a_pending: 0,
                                        player_b_pending: 0,
                                        required: r.required_deposit,
                                        ready,
                                    });
                                }
                                // replay in-flight settlement so a mid-payout reconnect
                                // lands in the signing view.
                                if r.payout_triggered {
                                    if let Some(s) = r.payout_signing_state.clone() {
                                        let elapsed = s.broadcast_at.elapsed().as_secs();
                                        let remaining = PRIORITY_SIGNER_FALLBACK_SECS.saturating_sub(elapsed);
                                        let _ = srv_tx.send(ServerMsg::PayoutSigningRequest {
                                            relay_room: s.relay_room,
                                            plan: s.plan,
                                            priority_seat: s.priority_seat,
                                            fallback_secs_remaining: remaining,
                                        });
                                    }
                                    if let Some(txid) = r.payout_complete_txid.clone() {
                                        let _ = srv_tx.send(ServerMsg::PayoutComplete { txid });
                                    }
                                    if let Some(reason) = r.payout_failed_reason.clone() {
                                        let _ = srv_tx.send(ServerMsg::PayoutFailed { reason });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Some("msg") => {
                let Some(room) = my_room.clone() else { continue };
                let Some(text) = v.get("text").and_then(|x| x.as_str()) else { continue };
                let ts = now_ms();
                let mut rooms = state.relay_rooms.lock().await;
                if let Some(peers) = rooms.get_mut(&room) {
                    // Transport HIGH-2 fix: prune peers whose socket has already closed BEFORE
                    // fanning out. `is_closed()` is true once the send_task broke (the receiver
                    // half was dropped), so `tx.send()` into it returns Ok into an orphaned
                    // channel and the frame is silently swallowed — an action lost to a stale
                    // connection. The stale-channel prune previously ran ONLY in `join`; run it
                    // here too so we never route a frame into a dead peer. (Per-peer, so it is
                    // already multiway-safe.)
                    peers.retain(|p| p.tx.same_channel(&tx) || !p.tx.is_closed());
                    // Fan out to the OTHER live peers, counting real deliveries. `send()` into a
                    // still-open unbounded channel is our best delivery signal short of an app-ack.
                    let mut delivered = 0usize;
                    let mut recipients = 0usize;
                    for p in peers.iter() {
                        if !p.tx.same_channel(&tx) {
                            recipients += 1;
                            if p.tx.send(frame(serde_json::json!({ "t": "msg", "text": text, "nick": my_nick, "ts": ts }))).is_ok() {
                                delivered += 1;
                            }
                        }
                    }
                    // Surface non-delivery so the sender can retransmit on reconnect instead of
                    // assuming the peer got it. Only when there was someone we SHOULD have reached
                    // but the channel was dead — an empty room (peer not yet joined) is not a loss.
                    if recipients == 0 || delivered < recipients {
                        let _ = tx.send(frame(serde_json::json!({
                            "t": "undelivered",
                            "reason": if recipients == 0 { "no_peer" } else { "peer_gone" },
                            "ts": ts,
                        })));
                    }
                }
            }
            // Server-facing CONTROL frame (staked tables only). Carries a ClientMsg
            // under "msg". This is DISTINCT from the opaque `_enc` peer payloads
            // (which arrive as `{"t":"msg","text":..}` and are forwarded blind above):
            // the server NEVER parses card/action content. We only handle the escrow-
            // relevant variants — DkgComplete (drives escrow-address agreement) and
            // Leave (co-signed game-over => settlement/payout). Action / StartHand /
            // any card content is intentionally NOT handled here: no server-side engine
            // runs for a staked P2P table, so those stay peer-to-peer and blind.
            Some("srv") => {
                let Some(room_arc) = escrow_room.clone() else { continue };
                let Some(seat) = escrow_seat else { continue };
                let inner = match v.get("msg") {
                    Some(m) => m.clone(),
                    None => continue,
                };
                let cmsg: ClientMsg = match serde_json::from_value(inner) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                match cmsg {
                    ClientMsg::DkgComplete { escrow_ua, orchard_fvk } => {
                        let mut r = room_arc.lock().await;
                        if r.frost_room_code.is_none() || (seat as usize) >= r.dkg_reported_ua.len() {
                            continue;
                        }
                        tracing::info!(
                            "relay room {} seat {}: dkg complete ua={} fvk_tail=…{}",
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
                            tracing::info!("relay room {}: escrow UA agreed by all players: {}", r.code, escrow_ua);
                            r.escrow_address = escrow_ua.clone();
                            let info = r.room_info();
                            r.broadcast(&info);
                        }
                    }
                    ClientMsg::EscrowFault { phase, detail } => {
                        // Client detected an escrow problem. Log it and forward to the escrow's
                        // durable journal (client_fault event) for operator triage / disputes.
                        // Free-play tables (no ESCROW_URL) have no escrow to report to → just log.
                        let code = { room_arc.lock().await.code.clone() };
                        tracing::warn!(
                            "relay room {} seat {}: CLIENT ESCROW FAULT phase={} detail={}",
                            code, seat, phase, detail,
                        );
                        if let Some(escrow_url) = state.escrow_url.clone() {
                            let (code, phase, detail) = (code, phase.clone(), detail.clone());
                            tokio::spawn(async move {
                                if let Err(e) = escrow_client::report_fault(
                                    &escrow_url, &code, seat as u8, &phase, &detail,
                                ).await {
                                    tracing::warn!("forward client_fault to escrow failed: {}", e);
                                }
                            });
                        }
                    }
                    ClientMsg::Settlement { a_stack, b_stack, a_addr, b_addr, log_hash, sig } => {
                        // Co-signed WINNER payout. Each seat independently signs the agreed
                        // final outcome (seat 0 = player A, seat 1 = player B) with its session
                        // Ed25519 key — the same key pinned on-chain in the deposit memo. We
                        // collect both submissions; only when BOTH seats submit AND their claims
                        // (stacks, addrs, log_hash) MATCH do we call escrow /settle with both
                        // sigs and then trigger the on-chain payout to the actual winner.
                        //
                        // ESCROW_URL unset (free-play / bot tables) ⇒ no-op: nothing to settle.
                        let Some(escrow_url) = state.escrow_url.clone() else {
                            tracing::debug!("settlement ignored — no ESCROW_URL (free-play)");
                            continue;
                        };
                        let (settle_req, code) = {
                            let mut r = room_arc.lock().await;
                            if !r.staked {
                                continue; // chip-only tables (bot + free-play) never settle on-chain
                            }
                            if r.payout_triggered {
                                tracing::info!("relay room {}: seat {} settlement ignored — payout already in flight", r.code, seat);
                                continue;
                            }
                            let idx = seat as usize;
                            if idx >= r.settlement_submissions.len() {
                                continue;
                            }
                            let sub = SettlementSubmission {
                                a_stack, b_stack,
                                a_addr: a_addr.clone(), b_addr: b_addr.clone(),
                                log_hash: log_hash.clone(), sig: sig.clone(),
                            };
                            tracing::info!(
                                "relay room {}: seat {} settlement claim a_stack={} b_stack={}",
                                r.code, seat, a_stack, b_stack,
                            );
                            r.settlement_submissions[idx] = Some(sub);
                            // both seats submitted?
                            let a = r.settlement_submissions.get(0).and_then(|s| s.clone());
                            let b = r.settlement_submissions.get(1).and_then(|s| s.clone());
                            let (Some(a), Some(b)) = (a, b) else {
                                continue; // still waiting for the other seat
                            };
                            // claims must agree exactly (sig excluded) or we refuse to settle.
                            if a.claim() != b.claim() {
                                tracing::warn!(
                                    "relay room {}: SETTLEMENT DISPUTE — seat claims disagree; NOT settling. \
                                     a=({},{},{},{},{}) b=({},{},{},{},{})",
                                    r.code,
                                    a.a_stack, a.b_stack, a.a_addr, a.b_addr, a.log_hash,
                                    b.a_stack, b.b_stack, b.a_addr, b.b_addr, b.log_hash,
                                );
                                continue; // safe default: leave unsettled; Leave→refund remains the fallback
                            }
                            // agreed. build the escrow SettleReq carrying BOTH seats' sigs.
                            // seat 0 signature = player_a_sig, seat 1 = player_b_sig.
                            r.payout_triggered = true;
                            let req = escrow_client::SettleReq {
                                player_a_stack: a.a_stack,
                                player_b_stack: a.b_stack,
                                player_a_address: a.a_addr.clone(),
                                player_b_address: a.b_addr.clone(),
                                action_log_hash: a.log_hash.clone(),
                                player_a_sig: a.sig.clone(),
                                player_b_sig: b.sig.clone(),
                            };
                            (req, r.code.clone())
                        };

                        // /settle (verify co-sign + record plan) then drive the on-chain payout.
                        let room_clone = room_arc.clone();
                        let rooms_clone = state.rooms.clone();
                        tokio::spawn(async move {
                            match escrow_client::settle(&escrow_url, &code, &settle_req).await {
                                Err(e) => {
                                    tracing::error!("settle {}: escrow rejected co-signed outcome: {}", code, e);
                                    let mut r = room_clone.lock().await;
                                    r.payout_triggered = false; // allow retry / Leave fallback
                                    r.broadcast(&ServerMsg::PayoutFailed { reason: format!("settle: {}", e) });
                                    return;
                                }
                                Ok(escrow_client::SettleOutcome::QueuedPendingConfirmation) => {
                                    // Co-sign accepted; a deposit is still confirming. The escrow's
                                    // confirmed-deposit scanner will build + drive the payout the
                                    // instant both deposits confirm — do NOT fail, do NOT double-drive.
                                    // Keep payout_triggered = true so we never resubmit this outcome.
                                    tracing::info!(
                                        "settle {}: co-sign queued pending deposit confirmation; escrow completes on confirm",
                                        code,
                                    );
                                    let r = room_clone.lock().await;
                                    r.broadcast(&ServerMsg::Status {
                                        phase: "settlement".into(),
                                        message: "settlement agreed — waiting for on-chain deposit confirmation".into(),
                                    });
                                    return;
                                }
                                Ok(escrow_client::SettleOutcome::Finalized) => { /* deposits confirmed — drive payout below */ }
                            }
                            tracing::info!("settle {}: escrow accepted co-signed outcome; initiating winner payout", code);
                            // Settle-HIGH-1 fix: the server does NOT compute the winner split.
                            // The escrow already verified BOTH seats' signatures over the outcome
                            // at /settle and recorded the payout_plan; it is the SINGLE split
                            // authority (owns fee handling + rounding). We only ask it to drive
                            // that recorded plan on-chain. No amounts, no addresses derived here —
                            // that removes the second, rake-dropping/differently-rounded split.
                            //
                            // priority signer = seat 0 (arbitrary; drive_payout_signing swaps on timeout).
                            trigger_escrow_computed_payout(
                                rooms_clone, room_clone, escrow_url, code,
                                EscrowComputedPayout::SettledPayout, 0,
                            ).await;
                        });
                        notify_lobby_tables(&state.rooms, &state.lobby_users).await;
                    }
                    ClientMsg::Leave => {
                        // Settle-C1 fix: a `Leave` on a ZK/relay table is an ABANDONMENT, not a
                        // co-signed game-over. Cards never touched the server, so it CANNOT know a
                        // winner and MUST NOT send a caller/server-chosen winner split to the
                        // escrow (the old `settlement_plan()` → unsigned winner payout is gone).
                        //
                        // The only two safe money-exits are:
                        //   1. a completed co-signed `/settle` (winner-exit) — handled in the
                        //      `Settlement` arm above; the two seats sign the agreed outcome; or
                        //   2. refund-EACH-OWN-confirmed — the escrow's self-service `/cancel`
                        //      path, which the escrow computes (no split here). Winner-take-all on
                        //      a disputed abandonment goes through the operator's `/arbitrate`, NOT
                        //      an auto server split.
                        // So a bare `Leave` triggers ONLY the escrow-computed refund-each-own.
                        let mut r = room_arc.lock().await;
                        if r.payout_triggered {
                            tracing::info!("relay room {}: seat {} leave ignored — payout already in flight", r.code, seat);
                            continue;
                        }
                        tracing::info!("relay room {}: seat {} leaving — requesting escrow refund-each-own", r.code, seat);
                        r.payout_triggered = true;
                        // GameOver with an EMPTY payouts vec: the server no longer asserts any
                        // per-seat amounts. The authoritative plan arrives via the escrow-computed
                        // PayoutSigningRequest that trigger_escrow_computed_payout broadcasts.
                        r.broadcast(&ServerMsg::GameOver {
                            reason: format!("seat {} left the table — refunding each deposit", seat),
                            payouts: Vec::new(),
                        });
                        // Any staked seat with a deposit can be refunded — generalize per-seat
                        // (multiway-ready): if NOBODY deposited there is nothing to refund.
                        let any_deposit = r.deposits.iter().any(|&d| d > 0);
                        let code = r.code.clone();
                        r.action_deadline = None;
                        drop(r);
                        if any_deposit {
                            if let Some(escrow_url) = state.escrow_url.clone() {
                                let room_clone = room_arc.clone();
                                let rooms_clone = state.rooms.clone();
                                // priority signer = the seat that stayed (opponent of the leaver),
                                // so the present player signs first; drive_payout_signing swaps on
                                // timeout. Heads-up: opponent of `seat`. (Per-seat generalization
                                // for multiway would rotate through the still-present seats.)
                                let priority = if seat == 0 { 1 } else { 0 };
                                tokio::spawn(async move {
                                    trigger_escrow_computed_payout(
                                        rooms_clone, room_clone, escrow_url, code,
                                        EscrowComputedPayout::Cancel { reason: "leave".into() },
                                        priority,
                                    ).await;
                                });
                            } else {
                                tracing::warn!("no ESCROW_URL configured — skipping on-chain refund");
                            }
                        }
                        notify_lobby_tables(&state.rooms, &state.lobby_users).await;
                    }
                    // Every other ClientMsg (Action, StartHand, ReportDeposit, Chat, …)
                    // is NOT a server-facing control message on a staked P2P table —
                    // ignore. Card/action content is exchanged peer-to-peer via `_enc`.
                    _ => {}
                }
            }
            Some("part") => break,
            _ => {}
        }
    }

    // deregister and notify remaining peers
    if let Some(room) = my_room {
        let mut rooms = state.relay_rooms.lock().await;
        if let Some(peers) = rooms.get_mut(&room) {
            peers.retain(|p| !p.tx.same_channel(&tx));
            for p in peers.iter() {
                let _ = p.tx.send(frame(serde_json::json!({ "t": "system", "text": format!("{} left", my_nick) })));
            }
            if peers.is_empty() { rooms.remove(&room); }
        }
    }

    // staked bridge: mark our Room seat disconnected (keep it for the reconnect
    // window / settlement replay) and stop the ServerMsg forwarder.
    if let (Some(room_arc), Some(seat)) = (escrow_room, escrow_seat) {
        let mut r = room_arc.lock().await;
        if let Some(Some(p)) = r.players.get_mut(seat as usize) {
            p.disconnected_at = Some(tokio::time::Instant::now());
        }
        r.broadcast(&ServerMsg::OpponentDisconnected {
            seat,
            reconnect_secs: RECONNECT_WINDOW.as_secs(),
        });
    }
    if let Some(fwd) = srv_forward_task { fwd.abort(); }

    send_task.abort();
}

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
                    ready: false,
                });

                // broadcast join
                lobby_broadcast(&state.lobby_users, &LobbyMsg::System {
                    text: format!("{} joined the lobby", name),
                }).await;

                // push the refreshed board to EVERYONE (not just the joiner), so the
                // rest of the lobby actually sees the new arrival.
                broadcast_players(&state.lobby_users).await;

                // send table list
                let tables = get_table_list(&state.rooms).await;
                let _ = tx.send(LobbyMsg::Tables { tables });
            }
            LobbyClientMsg::Ready { ready } => {
                if let Some(ref name) = my_name {
                    {
                        let mut users = state.lobby_users.lock().await;
                        if let Some(u) = users.get_mut(name) { u.ready = ready; }
                    }
                    broadcast_players(&state.lobby_users).await;
                }
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
                    if to == *from {
                        let _ = tx.send(LobbyMsg::System { text: "you can't challenge yourself".into() });
                        continue;
                    }
                    // only mint a table if the target is actually online — otherwise the
                    // challenger would sit alone at a ghost table forever.
                    let target_tx = {
                        let users = state.lobby_users.lock().await;
                        users.get(&to).map(|u| u.tx.clone())
                    };
                    let Some(target_tx) = target_tx else {
                        let _ = tx.send(LobbyMsg::System { text: format!("{} is no longer in the lobby", to) });
                        continue;
                    };
                    // create the (free-play) table both players will land on
                    let code = generate_room_code();
                    let room = Arc::new(Mutex::new(Room::new(code.clone())));
                    state.rooms.lock().await.insert(code.clone(), room);

                    // prompt the target, and send the challenger to the table to wait
                    let _ = target_tx.send(LobbyMsg::Challenge { from: from.clone(), table_code: code.clone() });
                    let _ = tx.send(LobbyMsg::ChallengeSent { to: to.clone(), table_code: code });
                }
            }
            LobbyClientMsg::AcceptChallenge { from, table_code: _ } => {
                // tell the original challenger their opponent is on the way (they're
                // already sitting at the table). The accepting client navigates itself.
                if let Some(ref me) = my_name {
                    let target_tx = {
                        let users = state.lobby_users.lock().await;
                        users.get(&from).map(|u| u.tx.clone())
                    };
                    if let Some(t) = target_tx {
                        let _ = t.send(LobbyMsg::ChallengeAccepted { by: me.clone() });
                    }
                }
            }
            LobbyClientMsg::DeclineChallenge { from } => {
                if let Some(ref me) = my_name {
                    let target_tx = {
                        let users = state.lobby_users.lock().await;
                        users.get(&from).map(|u| u.tx.clone())
                    };
                    if let Some(t) = target_tx {
                        let _ = t.send(LobbyMsg::ChallengeDeclined { by: me.clone() });
                    }
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
        // refresh the board so the departed player drops off everyone's list
        broadcast_players(&state.lobby_users).await;
    }
    send_task.abort();
}

async fn lobby_broadcast(users: &LobbyUsers, msg: &LobbyMsg) {
    let users = users.lock().await;
    for user in users.values() {
        let _ = user.tx.send(msg.clone());
    }
}

/// push the current player board (name + ready flag) to every connected user.
async fn broadcast_players(users: &LobbyUsers) {
    let players: Vec<LobbyPlayer> = {
        let users = users.lock().await;
        let mut ps: Vec<LobbyPlayer> = users.values()
            .map(|u| LobbyPlayer { name: u.name.clone(), ready: u.ready })
            .collect();
        // ready players first, then alphabetical — the board reads as a "who can I play" list
        ps.sort_by(|a, b| b.ready.cmp(&a.ready).then(a.name.cmp(&b.name)));
        ps
    };
    lobby_broadcast(users, &LobbyMsg::Players { players }).await;
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
        // GHOST-TABLE FIX: never advertise a room with nobody actively seated. player_count()
        // excludes disconnected seats, so an abandoned/emptied room (the "0/2 BOT" ghost) is
        // skipped here even if it lingers in the map. A real 1-player waiting table still shows.
        if r.player_count() == 0 && !has_spectators { continue; }
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
            // staked = real-money table (real external escrow). Authoritative table class,
            // decided at creation. Drives the honest REAL vs FREE badge on the lobby card.
            "staked": r.staked,
            "buyin_zat": r.buyin_zat,
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

/// client capability probe — drives which options the lobby offers.
/// `escrow_enabled` gates real-money tables; when false the UI shows practice only.
/// Canonical stake ladder — the RELAY is the source of truth for the tiers it offers, not the
/// UI. All amounts are zatoshis (1 ZEC = 100_000_000). `rake_bps`/`rake_cap` are operator revenue
/// and must be defined server-side so a client cannot propose its own rake. The UI renders
/// whatever this returns and falls back to a built-in list only if the fetch fails.
fn stake_ladder() -> serde_json::Value {
    const ZEC: u64 = 100_000_000;
    const MZEC: u64 = ZEC / 1000; // 0.001 ZEC
    serde_json::json!([
        { "id": 0, "name": "Nano",  "blinds": "50/100 zats",   "sb": 50,        "bb": 100,       "buyin": 10_000,      "maxBuyin": 25_000,      "speed": "normal", "timeout": 30, "color": "#1a3a2d", "rakeBps": 0,   "rakeCap": 0 },
        { "id": 1, "name": "Micro", "blinds": "0.00005/0.0001", "sb": 5_000,     "bb": 10_000,    "buyin": MZEC,        "maxBuyin": 5*MZEC/2,    "speed": "normal", "timeout": 30, "color": "#2d5a3d", "rakeBps": 250, "rakeCap": 50_000 },
        { "id": 2, "name": "Low",   "blinds": "0.0005/0.001",   "sb": 50_000,    "bb": 100_000,   "buyin": 10*MZEC,     "maxBuyin": 25*MZEC,     "speed": "normal", "timeout": 30, "color": "#3d5a2d", "rakeBps": 200, "rakeCap": 500_000 },
        { "id": 3, "name": "Mid",   "blinds": "0.005/0.01",     "sb": 500_000,   "bb": 1_000_000, "buyin": ZEC/10,      "maxBuyin": ZEC/4,       "speed": "normal", "timeout": 30, "color": "#5a3d2d", "rakeBps": 150, "rakeCap": 5_000_000 },
        { "id": 4, "name": "High",  "blinds": "0.05/0.1",       "sb": 5_000_000, "bb": ZEC/10,    "buyin": ZEC,         "maxBuyin": 5*ZEC/2,     "speed": "normal", "timeout": 45, "color": "#5a2d3d", "rakeBps": 100, "rakeCap": ZEC/10 }
    ])
}

async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "escrow_enabled": state.escrow_url.is_some(),
        "stakes": stake_ladder(),
    }))
}

// ---------------------------------------------------------------------------
// Tournament HTTP API (non-custodial matchmaker; free-play orchestration)
//
// The server holds bracket STATE only — never funds. A match's actual play happens in the
// existing free-play relay room whose code is deterministic: `tourney-<tid>-m<match_id>`. Those
// relay rooms auto-create on first join, so there's nothing to spawn here. Clients poll
// GET /tournaments/{id}, find their current playable match + its room code, join that free-play
// room to play heads-up, then each side reports the winner to POST /result. The bracket only
// advances once BOTH seats report the SAME winner (lightweight anti-cheat for free games).
// ---------------------------------------------------------------------------

/// deterministic free-play room code for a tournament match.
fn tourney_room_code(tid: &str, match_id: u32) -> String {
    format!("tourney-{}-m{}", tid, match_id)
}

#[derive(Deserialize)]
struct CreateTournamentReq {
    name: String,
    organizer: String,
    #[serde(default)]
    paid: bool,
    #[serde(default)]
    buyin_zat: u64,
    /// unix seconds for a scheduled auto-start; omitted/null = organizer starts it manually.
    #[serde(default)]
    scheduled_start: Option<u64>,
    /// winner's per-round roll-forward in basis points (10000 = 100% doubling; 7500 = ×1.5;
    /// 5000 = flat, winner banks half the pot each round). Paid tournaments only. Default 10000.
    #[serde(default)]
    roll_bps: Option<u16>,
}

async fn create_tournament(
    State(state): State<AppState>,
    Json(req): Json<CreateTournamentReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;
    // bound + sanitize the broadcast name server-side (don't trust the client's 40-char cap).
    let name = safe_text(&req.name, 60);
    let organizer = safe_text(&req.organizer, 40);
    let id = hub.registry.create(
        name, organizer, req.paid, req.buyin_zat,
        req.scheduled_start, req.roll_bps.unwrap_or(10000),
    );
    Json(serde_json::json!({ "id": id }))
}

#[derive(Deserialize)]
struct MatchRoomReq {
    /// the caller's player handle — must be one of the two seated players in this match.
    who: String,
}

/// Get-or-create the STAKED escrow room for a PAID tournament match. This is the one place a paid
/// match turns into a real-money table, so everything money-relevant is decided SERVER-SIDE:
///   * the stake comes from the match (`stake_zat`), never the client — a player can't understate it;
///   * only a seated player (a or b) of a *playable* match may provision it;
///   * the room code is deterministic so both players land in the SAME escrow;
///   * provisioning is idempotent — the first caller creates + DKG-provisions, the second reuses it.
/// Concurrency: validation reads the tournaments hub only briefly and drops it; the escrow provision
/// (a network round-trip / DKG) then runs under a DEDICATED `match_provision_lock`, so racing players
/// can't double-provision a match yet bracket reads / joins / result reports / the ticker are never
/// blocked on escrow I/O. Reuses the exact cash-table escrow path.
async fn tournament_match_room(
    State(state): State<AppState>,
    Path((id, match_id)): Path<(String, u32)>,
    Json(req): Json<MatchRoomReq>,
) -> impl IntoResponse {
    // ── validate under the tournaments lock, then release it before any escrow I/O ──
    let (stake, code) = {
        let hub = state.tournaments.lock().await;
        let Some(t) = hub.registry.get(&id) else {
            return (axum::http::StatusCode::NOT_FOUND, "no such tournament").into_response();
        };
        if !t.paid {
            return (axum::http::StatusCode::BAD_REQUEST, "not a paid tournament").into_response();
        }
        let Some(m) = t.matches.iter().find(|m| m.id == match_id) else {
            return (axum::http::StatusCode::NOT_FOUND, "no such match").into_response();
        };
        // server-authoritative: caller must be a seated player of a match that's ready to play.
        if m.a.as_deref() != Some(req.who.as_str()) && m.b.as_deref() != Some(req.who.as_str()) {
            return (axum::http::StatusCode::FORBIDDEN, "not a player in this match").into_response();
        }
        if !m.is_playable() {
            return (axum::http::StatusCode::BAD_REQUEST, "match is not ready to play").into_response();
        }
        if m.stake_zat == 0 {
            return (axum::http::StatusCode::BAD_REQUEST, "match has no stake").into_response();
        }
        (m.stake_zat, tourney_room_code(&id, match_id))
    }; // tournaments hub lock released here — escrow provisioning below never holds it.

    // already provisioned? reuse it (idempotent for the second player + retries). Cheap fast path
    // that avoids taking the provision lock at all once the room exists.
    if state.rooms.lock().await.contains_key(&code) {
        return Json(serde_json::json!({ "room": code, "stake_zat": stake })).into_response();
    }
    // real-money room needs the escrow service; refuse rather than silently create a free room.
    if state.escrow_url.is_none() {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "real-money tournaments are unavailable (escrow offline)",
        ).into_response();
    }

    // ── serialize provisioning on the DEDICATED lock (not the tournaments hub) ──
    let _guard = state.match_provision_lock.lock().await;
    // re-check under the guard: another racer may have provisioned while we waited for the lock.
    if state.rooms.lock().await.contains_key(&code) {
        return Json(serde_json::json!({ "room": code, "stake_zat": stake })).into_response();
    }
    // heads-up blinds scaled to the stake (~100bb stack): bb = stake/100, sb = bb/2, min 1.
    let bb = (stake / 100).max(1);
    let sb = (bb / 2).max(1);
    // provision the per-match 2-of-3 FROST escrow (same path as a cash table). rake_bps = 0:
    // the org takes nothing — winner gets the pot minus only the network fee.
    let escrow = remote_escrow_for(&state.escrow_url, &code, stake, 0).await;
    if !escrow.is_staked() {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "could not provision match escrow (escrow service unreachable)",
        ).into_response();
    }
    let room = Arc::new(Mutex::new(Room::with_settings(
        code.clone(), sb, bb, stake, 30, 2, TableAccess::Private, false, escrow,
    )));
    state.rooms.lock().await.insert(code.clone(), room);
    spawn_deposit_poller(state.rooms.clone(), state.escrow_url.clone(), code.clone());
    notify_lobby_tables(&state.rooms, &state.lobby_users).await;
    tracing::info!("tournament {} match {} -> staked room {} (stake {} zat)", id, match_id, code, stake);
    Json(serde_json::json!({ "room": code, "stake_zat": stake })).into_response()
}

async fn list_tournaments(State(state): State<AppState>) -> impl IntoResponse {
    let hub = state.tournaments.lock().await;
    let list: Vec<serde_json::Value> = hub
        .registry
        .list()
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "name": t.name,
                "organizer": t.organizer,
                "paid": t.paid,
                "buyin_zat": t.buyin_zat,
                "state": t.state,
                "player_count": t.players.len(),
                "sponsors": t.sponsors,
                "total_prize_zat": t.total_prize(),
            })
        })
        .collect();
    Json(list)
}

async fn get_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let hub = state.tournaments.lock().await;
    match hub.registry.get(&id) {
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no such tournament" })),
        )
            .into_response(),
        Some(t) => {
            // Serialize the full tournament, then annotate each pending (playable) match with the
            // deterministic free-play room code so the client can join straight into the game.
            let mut v = serde_json::to_value(t).unwrap_or(serde_json::json!({}));
            let pending: Vec<serde_json::Value> = t
                .pending_matches()
                .into_iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "round": m.round,
                        "a": m.a,
                        "b": m.b,
                        "room": tourney_room_code(&t.id, m.id),
                        // paid matches route into a STAKED room; each player deposits stake_zat into
                        // that match's own P2P escrow. 0/false for free tournaments.
                        "paid": t.paid,
                        "stake_zat": m.stake_zat,
                    })
                })
                .collect();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("pending".into(), serde_json::json!(pending));
                obj.insert("total_prize_zat".into(), serde_json::json!(t.total_prize()));
            }
            Json(v).into_response()
        }
    }
}

/// map a Result<(), String> to `{ok:true}` (200) or `{error}` (400).
fn ok_or_400(r: Result<(), String>) -> axum::response::Response {
    match r {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct PlayerReq {
    player: String,
}

async fn join_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PlayerReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;
    ok_or_400(hub.registry.join(&id, req.player))
}

async fn leave_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PlayerReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;
    ok_or_400(hub.registry.leave(&id, &req.player))
}

#[derive(Deserialize)]
struct WhoReq {
    who: String,
}

async fn start_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<WhoReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;
    ok_or_400(hub.registry.start(&id, &req.who))
}

async fn cancel_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<WhoReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;
    ok_or_400(hub.registry.cancel(&id, &req.who))
}

#[derive(Deserialize)]
struct SponsorReq {
    who: String,
    name: String,
    #[serde(default)]
    logo_url: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    added_prize_zat: u64,
}

/// https-only allowlist for sponsor-supplied URLs — a permissionless (platinum) sponsor must not
/// be able to inject `javascript:`/`data:`/relative URLs into every participant's page (stored XSS
/// / malicious click-through). Anything not plain https is dropped to empty (= no link/logo).
fn safe_https(u: &str) -> String {
    let t = u.trim();
    if t.starts_with("https://") && !t.contains(['\n', '\r', '"', '<', '>', ' ']) { t.to_string() } else { String::new() }
}

/// Sanitize free text the server stores and advertises to every participant. Strips control chars
/// and the HTML tag delimiters `<`/`>`. SolidJS escapes text interpolation today, but we do NOT
/// rely on the client framework for a broadcast field (defense-in-depth against any future
/// innerHTML / OG-meta / native consumer). Keeps ordinary punctuation (& ' ") readable.
fn safe_text(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '<' && *c != '>')
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

async fn sponsor_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SponsorReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;
    // tier/funded/escrow_room/by are stamped by add_sponsor from `who`; placeholders here.
    let sponsor = tournament::Sponsor {
        name: safe_text(&req.name, 60),
        logo_url: safe_https(&req.logo_url),
        url: safe_https(&req.url),
        added_prize_zat: req.added_prize_zat,
        by: req.who.clone(),
        tier: tournament::SponsorTier::Platinum, // overwritten by add_sponsor
        funded: false,
        escrow_room: None,
    };
    ok_or_400(hub.registry.add_sponsor(&id, &req.who, sponsor))
}

#[derive(Deserialize)]
struct RemoveSponsorReq {
    who: String,
    /// the `by` handle of the sponsor entry to remove
    target: String,
}

async fn remove_sponsor_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RemoveSponsorReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;
    ok_or_400(hub.registry.remove_sponsor(&id, &req.who, &req.target))
}

#[derive(Deserialize)]
struct ResultReq {
    match_id: u32,
    winner: String,
    reporter: String,
}

/// Report a match result. FREE-tournament anti-cheat: a single report never advances the bracket.
/// We record `reporter -> claimed winner`; only once BOTH seated players have reported and they
/// AGREE do we call `report_winner` to advance. Disagreement records a conflict and leaves the
/// match unresolved for a manual/organizer path to sort out later.
async fn result_tournament(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ResultReq>,
) -> impl IntoResponse {
    let mut hub = state.tournaments.lock().await;

    // Validate the match exists, is playable, and that reporter + claimed winner are both seated.
    let (seat_a, seat_b) = match hub.registry.get(&id) {
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "no such tournament" })),
            )
                .into_response();
        }
        Some(t) => match t.matches.iter().find(|m| m.id == req.match_id) {
            None => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "no such match" })),
                )
                    .into_response();
            }
            Some(m) => {
                if m.winner.is_some() {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "match already decided" })),
                    )
                        .into_response();
                }
                (m.a.clone(), m.b.clone())
            }
        },
    };

    // reporter must be one of the two seated players
    if Some(&req.reporter) != seat_a.as_ref() && Some(&req.reporter) != seat_b.as_ref() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "reporter is not a player in this match" })),
        )
            .into_response();
    }
    // claimed winner must also be one of the two seated players
    if Some(&req.winner) != seat_a.as_ref() && Some(&req.winner) != seat_b.as_ref() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "winner is not a player in this match" })),
        )
            .into_response();
    }

    // record this reporter's claim
    let entry = hub.reports.entry((id.clone(), req.match_id)).or_default();
    entry.insert(req.reporter.clone(), req.winner.clone());

    // do BOTH seats now agree?
    let a = seat_a.as_ref().and_then(|p| entry.get(p)).cloned();
    let b = seat_b.as_ref().and_then(|p| entry.get(p)).cloned();
    match (a, b) {
        (Some(wa), Some(wb)) if wa == wb => {
            // both seats reported the same winner → advance the bracket
            let winner = wa;
            let res = hub.registry.report_winner(&id, req.match_id, &winner);
            // clear the ledger for this match regardless (decided or errored-out)
            hub.reports.remove(&(id.clone(), req.match_id));
            match res {
                Ok(()) => Json(serde_json::json!({ "ok": true, "advanced": true, "winner": winner }))
                    .into_response(),
                Err(e) => (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": e })),
                )
                    .into_response(),
            }
        }
        (Some(_), Some(_)) => {
            // both reported but they disagree → conflict, leave unresolved for manual resolution
            Json(serde_json::json!({ "ok": true, "conflict": true })).into_response()
        }
        _ => {
            // only one seat has reported so far → wait for the other
            Json(serde_json::json!({ "ok": true, "advanced": false, "awaiting_opponent": true }))
                .into_response()
        }
    }
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
    payout_token: Option<String>,
}

impl RemoteEscrow {
    /// THE single source of truth for "does this table hold real money?" — decided once, at
    /// room creation. A table is staked iff it has real external escrow: either FROST DKG
    /// coords (`frost_room_code`) OR a real (non-empty) escrow address. The bot/free-play
    /// paths mint escrow with `address: Some("")` and no frost coords ⇒ not staked.
    fn is_staked(&self) -> bool {
        self.frost_room_code.is_some()
            || self.address.as_deref().map(|a| !a.is_empty()).unwrap_or(false)
    }
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
                payout_token: setup.payout_token,
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
    // don't offer what we can't honor: real-money tables need the escrow service wired.
    // without it, refuse to create — the UI also hides the option, this is the backstop.
    if !bot_friendly && state.escrow_url.is_none() {
        tracing::warn!("refused real-money table create — escrow service not configured");
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "real-money tables are unavailable right now (escrow offline)",
        ).into_response();
    }
    let rake_bps = params.rake_bps.unwrap_or(0);
    // bot tables are chip-only demos — no FROST escrow, no DKG, no on-chain anything
    let external_escrow = if bot_friendly {
        RemoteEscrow { address: Some(String::new()), ..Default::default() }
    } else {
        remote_escrow_for(&state.escrow_url, &code, buyin, rake_bps).await
    };
    // staked class is decided here from the escrow we actually got — if the escrow service was
    // unreachable, `external_escrow` is empty ⇒ non-staked ⇒ no deposit poller.
    let staked = external_escrow.is_staked();
    let room = Arc::new(Mutex::new(Room::with_settings(code.clone(), sb, bb, buyin, timeout, seats, access, bot_friendly, external_escrow)));
    state.rooms.lock().await.insert(code.clone(), room);
    if staked {
        spawn_deposit_poller(state.rooms.clone(), state.escrow_url.clone(), code.clone());
    }
    notify_lobby_tables(&state.rooms, &state.lobby_users).await;
    axum::response::Redirect::to(&format!("/{}", code)).into_response()
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

    // create room if it doesn't exist (joining via invite link or crawler hit). This is a bare
    // GET — never provision real escrow here (a crawler must not mint a staked table or trigger
    // FROST DKG). Default to a FREE-PLAY table; real-money tables are created only via POST /new.
    let needs_create = !state.rooms.lock().await.contains_key(&code);
    if needs_create {
        let room = Room::new(code.clone());
        state.rooms.lock().await.entry(code.clone()).or_insert_with(|| Arc::new(Mutex::new(room)));
    }
    // serve index.html
    let index = std::path::PathBuf::from(&state.static_dir).join("index.html");
    match tokio::fs::read_to_string(&index).await {
        Ok(html) => axum::response::Html(html).into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Serve the SPA shell for a CLIENT-SIDE route (`/t/…`, `/play/…`, `/watch/…`). Unlike `room_page`
/// this creates NO room and has no side effects — the room is provisioned only when a player
/// actually connects over WS — so a crawler or a deep-link preview can never mint a table.
async fn serve_spa(State(state): State<AppState>) -> impl IntoResponse {
    let index = std::path::PathBuf::from(&state.static_dir).join("index.html");
    match tokio::fs::read_to_string(&index).await {
        Ok(html) => axum::response::Html(html).into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// HTML-attribute escape — tournament names + handles are user-controlled, so every interpolated
/// value MUST be escaped before it goes into a `<meta content="…">` (injection / broken-tag guard).
fn og_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Serve the SPA shell for `/t/<id>` with per-tournament Open Graph tags spliced into `<head>`, so a
/// pasted tournament link previews its name + prize + registration in Discord/Twitter/Telegram
/// (crawlers don't run the SPA, so the meta must be server-rendered). Read-only — never mutates the
/// tournament. Falls back to the plain shell if the tournament is unknown.
async fn tournament_page(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let meta = {
        let hub = state.tournaments.lock().await;
        hub.registry.get(&id).map(|t| {
            use tournament::TournState::*;
            let n = t.players.len();
            let status = match t.state {
                Registering => "registering",
                Running => "in progress",
                Finished => "finished",
                Cancelled => "cancelled",
            };
            let prize = t.total_prize();
            let money = if prize > 0 {
                format!("{:.4} ZEC prize pool · ", prize as f64 / 1e8)
            } else if t.paid {
                format!("{:.4} ZEC buy-in · ", t.buyin_zat as f64 / 1e8)
            } else {
                "free entry · ".to_string()
            };
            // return RAW strings; og_escape is applied once at emit time below (double-escaping bug).
            (t.name.clone(), format!("{}{} players · {} · heads-up single elimination", money, n, status))
        })
    };
    let host = headers.get(axum::http::header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("zkbtc.org");
    let (title, desc) = match meta {
        Some((name, d)) => (format!("{} — zk.poker tournament", name), d),
        None => ("zk.poker tournament".to_string(), "trustless heads-up hold'em on Zcash".to_string()),
    };
    let og = format!(
        "<meta property=\"og:title\" content=\"{t}\"><meta property=\"og:description\" content=\"{d}\">\
         <meta property=\"og:type\" content=\"website\"><meta property=\"og:site_name\" content=\"zk.poker\">\
         <meta property=\"og:url\" content=\"https://{h}/t/{i}\">\
         <meta name=\"twitter:card\" content=\"summary\"><meta name=\"twitter:title\" content=\"{t}\">\
         <meta name=\"twitter:description\" content=\"{d}\">",
        t = og_escape(&title), d = og_escape(&desc), h = og_escape(host), i = og_escape(&id),
    );
    let index = std::path::PathBuf::from(&state.static_dir).join("index.html");
    match tokio::fs::read_to_string(&index).await {
        Ok(html) => axum::response::Html(html.replacen("<head>", &format!("<head>{}", og), 1)).into_response(),
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
        // keepalive: ping every 20s so half-dead sockets (e.g. idle through the deposit
        // wait) and proxy idle-timeouts surface promptly instead of at the next game action.
        let mut ping = tokio::time::interval(std::time::Duration::from_secs(20));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if ws_tx.send(WsMessage::Text(json.into())).await.is_err() { break; }
                        }
                    }
                    None => break,
                },
                _ = ping.tick() => {
                    if ws_tx.send(WsMessage::Ping(Vec::new().into())).await.is_err() { break; }
                }
            }
        }
    });

    // send room info immediately
    {
        let r = room.lock().await;
        let _ = tx.send(r.room_info());
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
                    // keep the seat (disconnected) so the player can still reconnect into the
                    // settlement view to sign / see the payout txid; room cleanup tears the
                    // whole room down post-payout anyway. Nulling it here locks a >60s-offline
                    // player out via the "table closed" guard in the Join handler.
                    continue;
                }
                // remove abandoner first so winner_after_abandon sees the remaining alone
                r.players[seat_idx] = None;
                if !r.staked {
                    // chip-only tables (bot + free-play) never auto-settle on-chain: just vacate.
                    tracing::info!("room: seat {} reconnect window expired (chip-only table), removing", seat);
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

                // reconnect match: name + disconnected_at + pubkey (if originally set)
                // pubkey check: anon joins (pubkey=None) keep name-only fallback; pubkey-set joins require the same pubkey
                let reconnect_seat = r.find_reconnect_seat(&name, pubkey.as_deref());
                // detect a hijack attempt: name matches a seated player but pubkey doesn't (and
                // they registered with one). Reject with a clear error rather than silently
                // sliding into the fresh-join path (which would hit "table full" anyway, but
                // the message wouldn't tell them why).
                if reconnect_seat.is_none() {
                    let hijack = r.is_seat_hijack(&name, pubkey.as_deref());
                    if hijack {
                        let _ = tx.send(ServerMsg::Error {
                            message: "name in use by another player on this table".into(),
                        });
                        continue;
                    }
                }

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
                            let elapsed = s.broadcast_at.elapsed().as_secs();
                            let remaining = PRIORITY_SIGNER_FALLBACK_SECS.saturating_sub(elapsed);
                            let _ = tx.send(ServerMsg::PayoutSigningRequest {
                                relay_room: s.relay_room,
                                plan: s.plan,
                                priority_seat: s.priority_seat,
                                fallback_secs_remaining: remaining,
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

                    // phase-aware reconnect: pick the message set that drives the SPA to the
                    // correct view (waiting / deposit / game). Settlement-mode case is handled above.
                    if r.engine.hand_state().is_some() {
                        // hand running: resume game view
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
                    } else if r.staked && !r.deal_ready() {
                        // staked table, deposits pending: push to deposit view via DepositStatus
                        let ready = r.deal_ready() && r.player_count() >= 2;
                        let _ = tx.send(ServerMsg::DepositStatus {
                            escrow_address: r.escrow_address.clone(),
                            seat_addresses: r.seat_deposit_addresses.clone(),
                            seat_payout_addresses: r.seat_payout_addresses.clone(),
                            player_a_deposit: r.deposits.get(0).copied().unwrap_or(0),
                            player_b_deposit: r.deposits.get(1).copied().unwrap_or(0),
                            player_a_pending: 0,
                            player_b_pending: 0,
                            required: r.required_deposit,
                            ready,
                        });
                    }
                    // else: SPA stays on whatever it was (waiting / lobby). Seated above is enough.

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
                // integrity: on STAKED (real-money) tables a single wallet may not occupy two
                // seats (playing yourself defeats the escrow) and a wallet is required. Keyed on
                // session pubkey; anon joins (pubkey=None) are allowed on chip-only tables
                // (bot + free-play).
                if r.staked {
                    match pubkey.as_deref() {
                        Some(pk) if r.players.iter().flatten().any(|p| p.pubkey.as_deref() == Some(pk)) => {
                            let _ = tx.send(ServerMsg::Error {
                                message: "this wallet is already seated at this table — you can't play yourself".into(),
                            });
                            continue;
                        }
                        None => {
                            let _ = tx.send(ServerMsg::Error {
                                message: "real-money tables require a zafu wallet".into(),
                            });
                            continue;
                        }
                        _ => {}
                    }
                }
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
                    let code = r.code.clone();

                    // Non-staked tables (bot + free-play) move no money: Leave is a plain
                    // seat-vacate. Do NOT set payout_triggered (that would brick the room and kill
                    // its auto-rebuy / next hand), do NOT run the settlement/payout flow.
                    if !r.staked {
                        r.players[seat as usize] = None;
                        r.action_deadline = None;
                        r.broadcast(&ServerMsg::OpponentLeft { seat });
                        let empty = r.player_count() == 0;
                        drop(r);
                        // last player left an ephemeral free/bot table → schedule room GC.
                        if empty {
                            let rooms = state.rooms.clone();
                            let code = code.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                                let mut rooms = rooms.lock().await;
                                if let Some(room) = rooms.get(&code) {
                                    if room.lock().await.player_count() == 0 {
                                        rooms.remove(&code);
                                        tracing::info!("room {}: GC'd (empty free/bot table)", code);
                                    }
                                }
                            });
                        }
                        notify_lobby_tables(&state.rooms, &state.lobby_users).await;
                        continue;
                    }

                    // STAKED table: run the real-money settlement flow (unchanged).
                    r.payout_triggered = true;

                    let payouts = r.settlement_plan();

                    r.broadcast(&ServerMsg::GameOver {
                        reason: format!("seat {} left the table", seat),
                        payouts: payouts.clone(),
                    });

                    let payout_str: Vec<String> = payouts.iter()
                        .map(|(s, a)| format!("seat{}={}", s, a)).collect();
                    tracing::info!("settlement: room={} payouts=[{}]",
                        r.code, payout_str.join(", "));

                    // build the PCZT payout plan (seat, address, amount) — skip when nothing
                    // was actually deposited
                    let pczt_plan: Vec<PayoutLineJson> = payouts.iter().filter_map(|(s, amt)| {
                        r.seat_payout_addresses.get(*s as usize)
                            .and_then(|opt| opt.as_ref())
                            .map(|addr| PayoutLineJson {
                                seat: *s, address: addr.clone(), amount_zat: *amt,
                            })
                    }).collect();

                    let leaving_seat = seat;

                    r.action_deadline = None;

                    // Keep the leaver seated through settlement on real tables so they receive
                    // PayoutSigningRequest / PayoutComplete / PayoutFailed broadcasts. Their
                    // websocket tx must stay in r.players to be hit by broadcast(). The seat is
                    // cleared on the cleanup pass (5min post-PayoutComplete) when the room itself
                    // is removed. When nothing was deposited, drop immediately.
                    if !pczt_plan.is_empty() {
                        // real tables: skip OpponentLeft entirely. GameOver (already broadcast
                        // above) + PayoutSigningRequest (incoming from trigger_payout) drive
                        // both browsers straight into the settlement view. Sending OpponentLeft
                        // would flip the non-leaver's SPA through the 'waiting' (sharelink +
                        // deposit) screen for ~3s before settlement arrives.
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
                    // integrity: never trust a client-reported deposit on a STAKED table — those are
                    // credited only by spawn_deposit_poller reading confirmed on-chain escrow state.
                    // This path exists for chip-only (bot/demo) play only.
                    if r.staked {
                        tracing::warn!("room {} seat {}: rejected client ReportDeposit on staked table", r.code, seat);
                        r.send_to(seat, ServerMsg::Error {
                            message: "deposits are verified on-chain, not self-reported".into(),
                        });
                        continue;
                    }
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
                            seat_payout_addresses: r.seat_payout_addresses.clone(),
                            player_a_deposit: r.deposits.get(0).copied().unwrap_or(0),
                            player_b_deposit: r.deposits.get(1).copied().unwrap_or(0),
                            player_a_pending: 0,
                            player_b_pending: 0,
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
                    let info = r.room_info();
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
            // Co-signed settlement only flows over the P2P `srv` relay (staked tables);
            // the centralized/free-play server engine has no such outcome to settle.
            ClientMsg::Settlement { .. } => {}
            // Escrow faults only originate on staked P2P-relay tables; no escrow here.
            ClientMsg::EscrowFault { .. } => {}
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

/// Args resolve in order: CLI flag → env var → default. Values also load from a `.env`
/// in the working directory at startup (silent if absent).
#[derive(clap::Parser, Debug)]
#[command(version, about = "WebSocket poker server for browser-based heads-up play")]
struct Args {
    /// Static asset directory (SPA bundle). Defaults to a `static/` next to the binary, else `./static`.
    #[arg(long, env = "POKER_STATIC_DIR")]
    static_dir: Option<String>,
    /// poker-escrow base URL; unset = real-ZEC features disabled
    #[arg(long, env = "ESCROW_URL")]
    escrow_url: Option<String>,
    /// HTTP port to bind
    #[arg(long, env = "PORT", default_value_t = 3000)]
    port: u16,
}

fn default_static_dir() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().unwrap_or(std::path::Path::new("."));
    let beside_bin = dir.join("static");
    if beside_bin.exists() { beside_bin.to_string_lossy().to_string() } else { "static".to_string() }
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = <Args as clap::Parser>::parse();

    tracing_subscriber::fmt()
        .with_env_filter("poker_server=info")
        .init();

    let static_dir = args.static_dir.unwrap_or_else(default_static_dir);
    let escrow_url = args.escrow_url.filter(|s| !s.is_empty());

    let state = AppState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        lobby_users: Arc::new(Mutex::new(HashMap::new())),
        static_dir: static_dir.clone(),
        escrow_url: escrow_url.clone(),
        zid_peers: Arc::new(Mutex::new(ZidRelay::default())),
        relay_rooms: Arc::new(Mutex::new(HashMap::new())),
        tournaments: Arc::new(Mutex::new(TournamentHub::default())),
        match_provision_lock: Arc::new(Mutex::new(())),
    };

    // scheduled-tournament ticker: every 5s, auto-start any tournament whose scheduled start time
    // has passed (≥2 players) or auto-cancel it (too few). The schedule is the authorization, so no
    // organizer needs to be present. Cheap: a no-op unless a tournament is actually due.
    {
        let tourneys = state.tournaments.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                ticker.tick().await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let mut hub = tourneys.lock().await;
                for (id, outcome) in hub.registry.tick(now) {
                    tracing::info!("tournament {} auto-{}", id, outcome);
                }
            }
        });
    }

    tracing::info!("serving static files from {}", static_dir);
    tracing::info!("jury config: {}-of-{} frostito FROST (pallas)", JURY_T, JURY_N);
    match &escrow_url {
        Some(u) => tracing::info!("escrow service: {}", u),
        None => tracing::info!("escrow service: disabled (set ESCROW_URL to enable)"),
    }

    let app = Router::new()
        // blind room relay. NOTE: prod HAProxy routes any `/ws*` path to a
        // separate relay (:50053), so the client dials `/p2p` (routes to us by
        // default). `/ws` kept as an alias for local/dev without HAProxy.
        .route("/p2p", axum::routing::get(relay_handler))
        .route("/ws", axum::routing::get(relay_handler))
        // HAProxy routes ALL `/ws*` to the separate FROST relay (:50053), which
        // shadowed lobby chat + the zid e2ee relay. Serve them off non-`/ws`
        // paths so they reach us; `/ws/*` kept as aliases for local/dev.
        .route("/lobby", axum::routing::get(lobby_ws_handler))
        .route("/zid", axum::routing::get(zid_relay_handler))
        .route("/ws/lobby", axum::routing::get(lobby_ws_handler))
        .route("/ws/zid", axum::routing::get(zid_relay_handler))
        .route("/api/tables", axum::routing::get(list_tables))
        .route("/api/config", axum::routing::get(get_config))
        // non-custodial tournament matchmaker (bracket state only; free-play orchestration)
        .route(
            "/tournaments",
            axum::routing::post(create_tournament).get(list_tournaments),
        )
        .route("/tournaments/{id}", axum::routing::get(get_tournament))
        .route("/tournaments/{id}/join", axum::routing::post(join_tournament))
        .route("/tournaments/{id}/leave", axum::routing::post(leave_tournament))
        .route("/tournaments/{id}/start", axum::routing::post(start_tournament))
        .route("/tournaments/{id}/cancel", axum::routing::post(cancel_tournament))
        .route("/tournaments/{id}/sponsor", axum::routing::post(sponsor_tournament))
        .route("/tournaments/{id}/sponsor/remove", axum::routing::post(remove_sponsor_tournament))
        .route("/tournaments/{id}/result", axum::routing::post(result_tournament))
        .route("/tournaments/{id}/match/{mid}/room", axum::routing::post(tournament_match_room))
        .route("/new", axum::routing::get(create_room))
        // explicit SPA deep-link routes (serve the shell so a hard reload / pasted link boots the
        // app). Registered BEFORE `/{code}` so `/t` etc. don't get parsed as room codes. Kept as a
        // finite explicit set — NOT a catch-all — so a missing asset still 404s (never mask a
        // broken build with index.html).
        .route("/t", axum::routing::get(serve_spa))
        .route("/t/{id}", axum::routing::get(tournament_page))
        .route("/t/{id}/m/{mid}", axum::routing::get(serve_spa))
        .route("/play/{code}", axum::routing::get(serve_spa))
        .route("/watch/{code}", axum::routing::get(serve_spa))
        .route("/{code}/ws", axum::routing::get(ws_handler))
        .route("/{code}/spectate", axum::routing::get(spectate_handler))
        .route("/{code}", axum::routing::get(room_page))
        .fallback_service(ServeDir::new(&static_dir))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", args.port);
    tracing::info!("poker server listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_player(name: &str, pubkey: Option<&str>, disconnected: bool) -> Player {
        let (tx, _rx) = mpsc::unbounded_channel();
        Player {
            name: name.into(),
            seat: 0,
            pubkey: pubkey.map(str::to_string),
            zcash_address: None,
            tx,
            disconnected_at: disconnected.then(tokio::time::Instant::now),
        }
    }

    // ---- tournament orchestration: both-agree result gate ----

    /// Drive the `/result` both-agree logic directly against a TournamentHub (mirrors the handler
    /// without the axum plumbing) to prove a single report never advances the bracket.
    fn report(
        hub: &mut TournamentHub,
        tid: &str,
        match_id: u32,
        reporter: &str,
        winner: &str,
    ) -> Result<bool, String> {
        // returns Ok(true) if advanced, Ok(false) if awaiting/conflict, Err on validation fail
        let (a, b) = {
            let t = hub.registry.get(tid).ok_or("no tournament")?;
            let m = t.matches.iter().find(|m| m.id == match_id).ok_or("no match")?;
            if m.winner.is_some() {
                return Err("already decided".into());
            }
            (m.a.clone(), m.b.clone())
        };
        if Some(&reporter.to_string()) != a.as_ref() && Some(&reporter.to_string()) != b.as_ref() {
            return Err("reporter not seated".into());
        }
        if Some(&winner.to_string()) != a.as_ref() && Some(&winner.to_string()) != b.as_ref() {
            return Err("winner not seated".into());
        }
        let entry = hub.reports.entry((tid.to_string(), match_id)).or_default();
        entry.insert(reporter.to_string(), winner.to_string());
        let wa = a.as_ref().and_then(|p| entry.get(p)).cloned();
        let wb = b.as_ref().and_then(|p| entry.get(p)).cloned();
        match (wa, wb) {
            (Some(x), Some(y)) if x == y => {
                hub.registry.report_winner(tid, match_id, &x)?;
                hub.reports.remove(&(tid.to_string(), match_id));
                Ok(true)
            }
            (Some(_), Some(_)) => Ok(false), // conflict
            _ => Ok(false),                  // awaiting opponent
        }
    }

    #[test]
    fn tourney_result_needs_both_seats_to_agree() {
        let mut hub = TournamentHub::default();
        let tid = hub.registry.create("Friday", "alice", false, 0, None, 10000);
        hub.registry.join(&tid, "alice".into()).unwrap();
        hub.registry.join(&tid, "bob".into()).unwrap();
        hub.registry.start(&tid, "alice").unwrap();
        let mid = hub.registry.get(&tid).unwrap().pending_matches()[0].id;

        // one report alone does NOT advance
        assert_eq!(report(&mut hub, &tid, mid, "alice", "alice"), Ok(false));
        assert!(
            hub.registry.get(&tid).unwrap().matches.iter().find(|m| m.id == mid).unwrap().winner.is_none(),
            "single report must not decide the match"
        );

        // second seat agrees → advances (and this is a 2-player bracket, so champion)
        assert_eq!(report(&mut hub, &tid, mid, "bob", "alice"), Ok(true));
        assert_eq!(
            hub.registry.get(&tid).unwrap().matches.iter().find(|m| m.id == mid).unwrap().winner.as_deref(),
            Some("alice")
        );
        assert_eq!(hub.registry.get(&tid).unwrap().champion().map(|s| s.as_str()), Some("alice"));
        // ledger cleared after advancing
        assert!(hub.reports.get(&(tid.clone(), mid)).is_none());
    }

    #[test]
    fn tourney_result_conflict_leaves_match_unresolved() {
        let mut hub = TournamentHub::default();
        let tid = hub.registry.create("Sat", "alice", false, 0, None, 10000);
        hub.registry.join(&tid, "alice".into()).unwrap();
        hub.registry.join(&tid, "bob".into()).unwrap();
        hub.registry.start(&tid, "alice").unwrap();
        let mid = hub.registry.get(&tid).unwrap().pending_matches()[0].id;

        assert_eq!(report(&mut hub, &tid, mid, "alice", "alice"), Ok(false));
        // bob claims himself → disagreement → conflict, NOT advanced
        assert_eq!(report(&mut hub, &tid, mid, "bob", "bob"), Ok(false));
        assert!(
            hub.registry.get(&tid).unwrap().matches.iter().find(|m| m.id == mid).unwrap().winner.is_none(),
            "conflicting reports must leave the match undecided"
        );
    }

    #[test]
    fn tourney_result_rejects_outsiders() {
        let mut hub = TournamentHub::default();
        let tid = hub.registry.create("Sun", "alice", false, 0, None, 10000);
        hub.registry.join(&tid, "alice".into()).unwrap();
        hub.registry.join(&tid, "bob".into()).unwrap();
        hub.registry.start(&tid, "alice").unwrap();
        let mid = hub.registry.get(&tid).unwrap().pending_matches()[0].id;
        assert!(report(&mut hub, &tid, mid, "carol", "alice").is_err(), "non-seated reporter rejected");
        assert!(report(&mut hub, &tid, mid, "alice", "carol").is_err(), "non-seated winner rejected");
    }

    #[test]
    fn tourney_room_code_is_deterministic() {
        assert_eq!(tourney_room_code("t3", 7), "tourney-t3-m7");
    }

    #[test]
    fn reconnect_matches_disconnected_seat_by_name() {
        let mut room = Room::new("t".into());
        room.players[0] = Some(test_player("anon", None, true));
        assert_eq!(room.find_reconnect_seat("anon", None), Some(0));
    }

    #[test]
    fn no_reconnect_for_connected_seat() {
        let mut room = Room::new("t".into());
        room.players[0] = Some(test_player("anon", None, false));
        assert_eq!(room.find_reconnect_seat("anon", None), None);
    }

    #[test]
    fn reconnect_requires_matching_pubkey() {
        let mut room = Room::new("t".into());
        room.players[0] = Some(test_player("b", Some("pk1"), true));
        assert_eq!(room.find_reconnect_seat("b", Some("pk1")), Some(0));
        assert_eq!(room.find_reconnect_seat("b", Some("pk2")), None);
        assert_eq!(room.find_reconnect_seat("b", None), None);
    }

    #[test]
    fn anon_seat_reconnects_by_name_only() {
        // a seat that registered no pubkey accepts a name-only reconnect regardless of pubkey
        let mut room = Room::new("t".into());
        room.players[0] = Some(test_player("anon", None, true));
        assert_eq!(room.find_reconnect_seat("anon", Some("whatever")), Some(0));
    }

    #[test]
    fn hijack_detected_for_mismatched_pubkey() {
        let mut room = Room::new("t".into());
        room.players[0] = Some(test_player("b", Some("pk1"), false));
        assert!(room.is_seat_hijack("b", Some("pk2")));
        assert!(room.is_seat_hijack("b", None));
        assert!(!room.is_seat_hijack("b", Some("pk1")));
        assert!(!room.is_seat_hijack("other", Some("pk2")));
    }

    #[test]
    fn disconnected_seat_retained_for_reconnect_but_not_counted() {
        // mirrors the settlement fix: a disconnected seat stays present so it can reconnect,
        // yet does not count as an active player.
        let mut room = Room::new("t".into());
        room.players[0] = Some(test_player("anon", None, true));
        room.players[1] = Some(test_player("b", None, false));
        assert_eq!(room.player_count(), 1);
        assert_eq!(room.find_reconnect_seat("anon", None), Some(0));
    }
}
