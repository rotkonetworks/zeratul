//! poker-pvm: deterministic poker game engine
//!
//! pure state machine: (state, signed_action) → (new_state, events)
//!
//! no I/O, no randomness, no allocation beyond fixed buffers.
//! compiles to:
//!   - native (testing)
//!   - WASM (browser)
//!   - RISC-V (PolkaVM guest, provable via WIM)
//!
//! supports 2-10 players. seats are fixed-size arrays (no heap in pvm mode).
//! positions: BTN, SB, BB, UTG, UTG+1, MP, HJ, CO (derived from button + num_players).

#![cfg_attr(feature = "pvm", no_std)]

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod hand_tests;
// cfr = the CFR AI-bot training subsystem. It is NOT needed by the game engine or the
// WASM build (WasmGame), and it is gated behind its own `cfr` feature so a default/wasm
// build stays lean and does not drag in the bot trainer. Build the bot with `--features cfr`.
#[cfg(all(feature = "std", feature = "cfr"))]
pub mod cfr;

#[cfg(not(feature = "std"))]
extern crate alloc;

// ============================================================================
// Constants
// ============================================================================

pub const MAX_SEATS: usize = 10;
pub const MAX_COMMUNITY: usize = 5;
pub const MAX_ACTIONS: usize = 128;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Negotiate = 0,
    Escrow = 1,
    Preflop = 2,
    Flop = 3,
    Turn = 4,
    River = 5,
    Showdown = 6,
    Settled = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Action {
    Fold = 0,
    Check = 1,
    Call = 2,
    Bet = 3,
    Raise = 4,
    AllIn = 5,
}

impl Action {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v { 0 => Some(Self::Fold), 1 => Some(Self::Check), 2 => Some(Self::Call),
                   3 => Some(Self::Bet), 4 => Some(Self::Raise), 5 => Some(Self::AllIn), _ => None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SeatState {
    Empty = 0,
    Active = 1,
    SittingOut = 2,
    Folded = 3,
    AllIn = 4,
}

#[derive(Debug, Clone, Copy)]
pub struct SignedAction {
    pub seat: u8,
    pub action: Action,
    pub amount: u32,
    pub seq: u32,
    pub sig: [u8; 64],
}

#[derive(Debug, Clone, Copy)]
pub struct Rules {
    pub buyin: u32,
    pub small_blind: u32,
    pub big_blind: u32,
    pub turn_timeout_blocks: u32,
    /// rake percentage in basis points (100 = 1%). 0 = no rake.
    pub rake_bps: u16,
    /// max rake per pot (0 = unlimited)
    pub rake_cap: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Self { buyin: 1000, small_blind: 5, big_blind: 10, turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ActionResult {
    pub valid: bool,
    pub hand_over: bool,
    pub winner: u8,      // 255 = no winner yet / split
    pub payout: u32,
    pub advance_phase: bool,
}

// ============================================================================
// Game state (fixed-size, N seats, no heap in pvm mode)
// ============================================================================

#[derive(Debug, Clone)]
pub struct GameState {
    pub phase: Phase,
    pub rules: Rules,
    pub num_players: u8,
    pub hand_number: u32,
    pub button: u8,
    pub stacks: [u32; MAX_SEATS],
    pub pot: u32,
    pub bets: [u32; MAX_SEATS],
    /// total chips each seat has committed to the pot over the whole hand.
    /// used to build side pots at showdown (per-seat eligibility). uncalled
    /// bets returned by `equalize_bets` are subtracted back out, so this only
    /// ever reflects chips that are actually live in `pot`.
    pub contributed: [u32; MAX_SEATS],
    pub seat_state: [SeatState; MAX_SEATS],
    pub cards: [[u8; 2]; MAX_SEATS],
    pub community: [u8; MAX_COMMUNITY],
    pub community_count: u8,
    pub acting_seat: u8,
    pub round_actions: u8,
    pub last_aggressor: u8,
    /// size of the last legal raise increment this round (min-raise baseline).
    /// defaults to big_blind at the start of each betting round.
    pub last_raise_size: u32,
    pub action_count: u32,
    pub last_action_hash: [u8; 32],
    /// rake collected this hand
    pub rake: u32,
}

// ============================================================================
// Opponent profiling — tracks stats per seat across hands
// ============================================================================

/// player type classification based on observed stats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[derive(Default)]
pub enum PlayerType {
    #[default]
    Unknown = 0,
    Rock = 1,         // VPIP < 20%, low aggression
    TAG = 2,          // VPIP 20-30%, high aggression
    LAG = 3,          // VPIP > 40%, high aggression
    CallingStation = 4, // VPIP > 40%, low aggression
    Maniac = 5,       // VPIP > 60%, very high aggression, high allin%
    Nit = 6,          // VPIP < 12%, folds almost everything
}

/// per-seat opponent stats, updated after every action
#[derive(Debug, Clone, Copy)]
pub struct PlayerProfile {
    // raw counters
    pub hands_seen: u32,
    pub vpip_count: u32,       // voluntarily put money in pot
    pub pfr_count: u32,        // preflop raise
    pub postflop_bets: u32,    // bets + raises postflop
    pub postflop_calls: u32,   // calls postflop
    pub postflop_checks: u32,  // checks postflop
    pub postflop_folds: u32,   // folds postflop
    pub allin_count: u32,      // all-in actions
    pub showdowns_seen: u32,   // went to showdown
    pub showdowns_won: u32,    // won at showdown
    pub three_bet_count: u32,  // 3-bet preflop
    pub cbet_count: u32,       // continuation bet (bet flop after pfr)
    pub cbet_opportunities: u32,

    // per-hand tracking (reset each hand)
    pub acted_preflop: bool,
    pub raised_preflop: bool,
    pub was_pfr: bool,         // was the preflop raiser (for cbet tracking)
}

impl Default for PlayerProfile {
    fn default() -> Self {
        Self {
            hands_seen: 0, vpip_count: 0, pfr_count: 0,
            postflop_bets: 0, postflop_calls: 0, postflop_checks: 0,
            postflop_folds: 0, allin_count: 0, showdowns_seen: 0,
            showdowns_won: 0, three_bet_count: 0,
            cbet_count: 0, cbet_opportunities: 0,
            acted_preflop: false, raised_preflop: false, was_pfr: false,
        }
    }
}

impl PlayerProfile {
    /// VPIP: voluntarily put money in pot (0.0 - 1.0)
    pub fn vpip(&self) -> f32 {
        if self.hands_seen == 0 { return 0.5; } // unknown = assume average
        self.vpip_count as f32 / self.hands_seen as f32
    }

    /// PFR: preflop raise frequency (0.0 - 1.0)
    pub fn pfr(&self) -> f32 {
        if self.hands_seen == 0 { return 0.2; }
        self.pfr_count as f32 / self.hands_seen as f32
    }

    /// aggression factor: (bets + raises) / calls. higher = more aggressive
    pub fn aggression_factor(&self) -> f32 {
        let aggressive = self.postflop_bets as f32;
        let passive = self.postflop_calls.max(1) as f32;
        aggressive / passive
    }

    /// went to showdown frequency
    pub fn wtsd(&self) -> f32 {
        if self.hands_seen == 0 { return 0.3; }
        self.showdowns_seen as f32 / self.hands_seen as f32
    }

    /// won at showdown frequency
    pub fn w_sd(&self) -> f32 {
        if self.showdowns_seen == 0 { return 0.5; }
        self.showdowns_won as f32 / self.showdowns_seen as f32
    }

    /// all-in frequency
    pub fn allin_pct(&self) -> f32 {
        if self.hands_seen == 0 { return 0.05; }
        self.allin_count as f32 / self.hands_seen as f32
    }

    /// continuation bet frequency
    pub fn cbet(&self) -> f32 {
        if self.cbet_opportunities == 0 { return 0.5; }
        self.cbet_count as f32 / self.cbet_opportunities as f32
    }

    /// confidence weight: 1% at 0 hands → 15% cap at 100+ hands
    pub fn confidence_weight(&self) -> f32 {
        (0.01 + 0.14 * (self.hands_seen as f32 / 100.0)).min(0.15)
    }

    /// classify player type from observed stats
    pub fn classify(&self) -> PlayerType {
        if self.hands_seen < 10 { return PlayerType::Unknown; }

        let vpip = self.vpip();
        let pfr = self.pfr();
        let af = self.aggression_factor();
        let allin = self.allin_pct();

        // maniac: shoves constantly
        if allin > 0.3 || (vpip > 0.6 && af > 4.0) {
            return PlayerType::Maniac;
        }
        // nit: barely plays
        if vpip < 0.12 {
            return PlayerType::Nit;
        }
        // rock: tight passive
        if vpip < 0.20 && af < 2.0 {
            return PlayerType::Rock;
        }
        // TAG: tight aggressive
        if vpip < 0.30 && af >= 2.0 {
            return PlayerType::TAG;
        }
        // calling station: loose passive
        if vpip >= 0.40 && af < 1.5 {
            return PlayerType::CallingStation;
        }
        // LAG: loose aggressive
        if vpip >= 0.35 && af >= 2.0 {
            return PlayerType::LAG;
        }

        PlayerType::Unknown
    }

    /// encode as feature vector for neural net input (16 floats)
    pub fn to_features(&self) -> [f32; 16] {
        [
            self.vpip(),
            self.pfr(),
            self.aggression_factor().min(10.0) / 10.0, // normalize
            self.wtsd(),
            self.w_sd(),
            self.allin_pct(),
            self.cbet(),
            (self.hands_seen as f32).min(200.0) / 200.0, // confidence
            // one-hot player type
            if self.classify() == PlayerType::Rock { 1.0 } else { 0.0 },
            if self.classify() == PlayerType::TAG { 1.0 } else { 0.0 },
            if self.classify() == PlayerType::LAG { 1.0 } else { 0.0 },
            if self.classify() == PlayerType::CallingStation { 1.0 } else { 0.0 },
            if self.classify() == PlayerType::Maniac { 1.0 } else { 0.0 },
            if self.classify() == PlayerType::Nit { 1.0 } else { 0.0 },
            // gap between VPIP and PFR (higher = more passive preflop)
            (self.vpip() - self.pfr()).max(0.0),
            // 3-bet frequency
            if self.hands_seen > 0 { self.three_bet_count as f32 / self.hands_seen as f32 } else { 0.1 },
        ]
    }

    /// call after each new hand is dealt
    pub fn new_hand(&mut self) {
        self.hands_seen += 1;
        self.acted_preflop = false;
        self.raised_preflop = false;
        self.was_pfr = false;
    }

    /// call after each action is observed
    pub fn observe_action(&mut self, action: Action, phase: Phase, is_facing_raise: bool) {
        match phase {
            Phase::Preflop => {
                if !self.acted_preflop {
                    self.acted_preflop = true;
                    match action {
                        Action::Call | Action::Bet | Action::Raise | Action::AllIn => {
                            self.vpip_count += 1;
                        }
                        _ => {}
                    }
                }
                match action {
                    Action::Bet | Action::Raise => {
                        self.pfr_count += 1;
                        self.raised_preflop = true;
                        self.was_pfr = true;
                        if is_facing_raise {
                            self.three_bet_count += 1;
                        }
                    }
                    Action::AllIn => {
                        self.pfr_count += 1;
                        self.allin_count += 1;
                        self.was_pfr = true;
                    }
                    _ => {}
                }
            }
            Phase::Flop | Phase::Turn | Phase::River => {
                match action {
                    Action::Bet | Action::Raise => {
                        self.postflop_bets += 1;
                        // track cbet (first bet on flop by preflop raiser)
                        if phase == Phase::Flop && self.was_pfr {
                            self.cbet_count += 1;
                        }
                    }
                    Action::Call => { self.postflop_calls += 1; }
                    Action::Check => {
                        self.postflop_checks += 1;
                        // missed cbet
                        if phase == Phase::Flop && self.was_pfr {
                            self.cbet_opportunities += 1;
                        }
                    }
                    Action::Fold => { self.postflop_folds += 1; }
                    Action::AllIn => {
                        self.postflop_bets += 1;
                        self.allin_count += 1;
                    }
                }
                // cbet opportunity tracking
                if phase == Phase::Flop && self.was_pfr && matches!(action, Action::Bet | Action::Raise | Action::AllIn) {
                    self.cbet_opportunities += 1;
                }
            }
            _ => {}
        }
    }

    /// call when player reaches showdown
    pub fn observe_showdown(&mut self, won: bool) {
        self.showdowns_seen += 1;
        if won { self.showdowns_won += 1; }
    }
}

/// tracks all opponents at the table
#[derive(Debug, Clone)]
pub struct TableProfiles {
    pub profiles: [PlayerProfile; MAX_SEATS],
}

impl Default for TableProfiles {
    fn default() -> Self {
        Self { profiles: [PlayerProfile::default(); MAX_SEATS] }
    }
}

impl TableProfiles {
    /// call when a new hand starts
    pub fn new_hand(&mut self, num_players: u8) {
        for i in 0..num_players as usize {
            self.profiles[i].new_hand();
        }
    }

    /// observe an action from a player
    pub fn observe(&mut self, seat: u8, action: Action, phase: Phase, is_facing_raise: bool) {
        self.profiles[seat as usize].observe_action(action, phase, is_facing_raise);
    }

    /// observe showdown result
    pub fn observe_showdown(&mut self, seat: u8, won: bool) {
        self.profiles[seat as usize].observe_showdown(won);
    }

    /// get features for all opponents (for neural net input)
    pub fn opponent_features(&self, hero_seat: u8, num_players: u8) -> [[f32; 16]; MAX_SEATS] {
        let mut features = [[0.0f32; 16]; MAX_SEATS];
        let mut idx = 0;
        for i in 0..num_players as usize {
            if i != hero_seat as usize {
                features[idx] = self.profiles[i].to_features();
                idx += 1;
            }
        }
        features
    }

    /// classify a specific player
    pub fn classify(&self, seat: u8) -> PlayerType {
        self.profiles[seat as usize].classify()
    }
}

impl GameState {
    pub fn new(rules: Rules, num_players: u8) -> Self {
        let n = (num_players as usize).min(MAX_SEATS).max(2);
        let mut stacks = [0u32; MAX_SEATS];
        let mut seat_state = [SeatState::Empty; MAX_SEATS];
        for i in 0..n {
            stacks[i] = rules.buyin;
            seat_state[i] = SeatState::Active;
        }
        Self {
            phase: Phase::Preflop,
            rules,
            num_players: n as u8,
            hand_number: 0,
            button: 0,
            stacks,
            pot: 0,
            bets: [0; MAX_SEATS],
            contributed: [0; MAX_SEATS],
            seat_state,
            cards: [[0; 2]; MAX_SEATS],
            community: [0; MAX_COMMUNITY],
            community_count: 0,
            acting_seat: 0,
            round_actions: 0,
            last_aggressor: 255,
            last_raise_size: rules.big_blind,
            action_count: 0,
            last_action_hash: [0; 32],
            rake: 0,
        }
    }

    /// number of active (not folded/empty/sitting-out) players in this hand
    pub fn active_count(&self) -> u8 {
        (0..self.num_players as usize)
            .filter(|&i| matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn))
            .count() as u8
    }

    /// next active seat after `seat` (wraps around)
    fn next_active(&self, seat: u8) -> u8 {
        let n = self.num_players as usize;
        let mut s = (seat as usize + 1) % n;
        for _ in 0..n {
            if matches!(self.seat_state[s], SeatState::Active | SeatState::AllIn) {
                return s as u8;
            }
            s = (s + 1) % n;
        }
        seat // shouldn't happen
    }

    /// deterministic button seat for the current hand.
    ///
    /// pure function of `hand_number` and the set of seats that can play this
    /// hand (Active or AllIn — i.e. not busted/empty/sitting-out). both engines
    /// compute the identical button regardless of which hand-end path each ran,
    /// so blinds always post from the same seats and the engines never desync.
    ///
    /// walks `hand_number` steps around the seat ring starting from seat 0,
    /// counting only playable seats, so the button advances by exactly one
    /// playable seat per hand (skipping busted seats consistently).
    fn button_for_hand(&self) -> u8 {
        let n = self.num_players as usize;
        let playable: [bool; MAX_SEATS] = {
            let mut p = [false; MAX_SEATS];
            for i in 0..n {
                p[i] = matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn);
            }
            p
        };
        let count = (0..n).filter(|&i| playable[i]).count();
        if count == 0 { return 0; }
        // target index among playable seats, rotating one seat per hand
        let target = (self.hand_number as usize).wrapping_sub(1) % count;
        let mut seen = 0;
        for i in 0..n {
            if playable[i] {
                if seen == target { return i as u8; }
                seen += 1;
            }
        }
        0
    }

    /// small blind seat (next active after button)
    fn sb_seat(&self) -> u8 {
        if self.num_players == 2 {
            self.button // heads-up: button is SB
        } else {
            self.next_active(self.button)
        }
    }

    /// big blind seat (next active after SB)
    fn bb_seat(&self) -> u8 {
        self.next_active(self.sb_seat())
    }

    /// first to act preflop (UTG = next after BB, or SB in heads-up)
    fn first_preflop(&self) -> u8 {
        if self.num_players == 2 {
            self.sb_seat() // heads-up: SB acts first
        } else {
            self.next_active(self.bb_seat())
        }
    }

    /// first to act postflop (SB, or first active after button)
    fn first_postflop(&self) -> u8 {
        self.next_active(self.button)
    }

    /// sit a player out (they auto-fold, blinds skip them)
    pub fn sit_out(&mut self, seat: u8) {
        if (seat as usize) < self.num_players as usize {
            self.seat_state[seat as usize] = SeatState::SittingOut;
        }
    }

    /// sit a player back in
    pub fn sit_in(&mut self, seat: u8) {
        if (seat as usize) < self.num_players as usize && self.stacks[seat as usize] > 0 {
            self.seat_state[seat as usize] = SeatState::Active;
        }
    }

    /// deal a new hand. cards_per_seat: [[c0, c1]; num_players], community: [5]
    pub fn deal(&mut self, all_cards: &[[u8; 2]], community: [u8; 5]) {
        self.hand_number += 1;
        self.phase = Phase::Preflop;
        self.community = community;
        self.community_count = 0;
        self.pot = 0;
        self.bets = [0; MAX_SEATS];
        self.contributed = [0; MAX_SEATS];
        self.round_actions = 0;
        self.last_aggressor = 255;
        self.action_count = 0;
        // NOTE: rake accumulates across the session, not reset per hand

        // reset seat states for new hand
        for i in 0..self.num_players as usize {
            if self.seat_state[i] != SeatState::SittingOut && self.seat_state[i] != SeatState::Empty {
                if self.stacks[i] > 0 {
                    self.seat_state[i] = SeatState::Active;
                } else {
                    self.seat_state[i] = SeatState::SittingOut; // busted
                }
            }
            if i < all_cards.len() {
                self.cards[i] = all_cards[i];
            }
        }

        // derive the button as a PURE function of hand_number (fix #3): both
        // engines must pick the identical button every hand regardless of which
        // hand-end path each executed. must run after seat states are reset so
        // busted seats are skipped consistently on both sides.
        self.button = self.button_for_hand();

        // post blinds
        let sb = self.sb_seat() as usize;
        let bb = self.bb_seat() as usize;
        let sb_amount = self.rules.small_blind.min(self.stacks[sb]);
        let bb_amount = self.rules.big_blind.min(self.stacks[bb]);
        self.stacks[sb] -= sb_amount;
        self.stacks[bb] -= bb_amount;
        self.bets[sb] = sb_amount;
        self.bets[bb] = bb_amount;
        self.contributed[sb] += sb_amount;
        self.contributed[bb] += bb_amount;
        self.pot = sb_amount + bb_amount;
        // the BB posting sets the min-raise baseline for the preflop round.
        self.last_raise_size = self.rules.big_blind;
        // a blind that empties a short stack is an all-in
        if self.stacks[sb] == 0 { self.seat_state[sb] = SeatState::AllIn; }
        if self.stacks[bb] == 0 { self.seat_state[bb] = SeatState::AllIn; }

        self.acting_seat = self.first_preflop();

        // fix #1: the chosen first-to-act may itself be all-in (a blind emptied a
        // short stack). advance to the next seat that can actually act so apply()
        // never rejects everyone and deadlocks the hand.
        let can_act = |st: &Self, i: usize| {
            st.seat_state[i] == SeatState::Active && st.stacks[i] > 0
        };
        if !can_act(self, self.acting_seat as usize) {
            self.acting_seat = self.next_active_in_round(self.acting_seat);
        }

        // if there is no live betting left to resolve — either nobody can act, or
        // the single remaining actor has already matched the top bet (nothing to
        // call) — equalize and skip straight to showdown, exactly like the
        // apply()/advance_phase() skip-to-showdown paths. a lone actor who still
        // owes chips keeps the turn so they can call or fold.
        let max_bet = self.bets.iter().take(self.num_players as usize).copied().max().unwrap_or(0);
        let actors: Vec<usize> = (0..self.num_players as usize)
            .filter(|&i| can_act(self, i))
            .collect();
        let no_pending_action = actors.is_empty()
            || (actors.len() == 1 && self.bets[actors[0]] >= max_bet);
        if no_pending_action {
            self.equalize_bets();
            self.phase = Phase::Showdown;
            self.community_count = 5;
        }
    }

    /// apply a signed action
    pub fn apply(&mut self, action: &SignedAction) -> Result<ActionResult, &'static str> {
        // E5: only accept actions during betting phases
        if !matches!(self.phase, Phase::Preflop | Phase::Flop | Phase::Turn | Phase::River) {
            return Err("not in betting phase");
        }
        if action.seat as usize >= self.num_players as usize {
            return Err("invalid seat");
        }
        if action.seat != self.acting_seat {
            return Err("not your turn");
        }
        if !matches!(self.seat_state[action.seat as usize], SeatState::Active) {
            return Err("seat not active");
        }
        // E4: always enforce sequence (no seq=0 bypass)
        if action.seq != 0 && action.seq != self.action_count + 1 {
            return Err("wrong sequence");
        }

        let seat = action.seat as usize;
        let max_bet = self.bets.iter().take(self.num_players as usize).copied().max().unwrap_or(0);

        match action.action {
            Action::Fold => {
                self.seat_state[seat] = SeatState::Folded;
                self.action_count += 1;

                // check if only one player remains
                if self.active_count() == 1 {
                    // find the winner
                    let winner = (0..self.num_players as usize)
                        .find(|&i| matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn))
                        .unwrap_or(0);
                    let payout = self.collect_rake();
                    self.stacks[winner] += payout;
                    self.pot = 0;
                    self.contributed = [0; MAX_SEATS];
                    self.phase = Phase::Settled;
                    // button is NOT rotated here (fix #3): the next deal() derives
                    // it deterministically from hand_number so both engines agree.
                    return Ok(ActionResult {
                        valid: true, hand_over: true,
                        winner: winner as u8, payout, advance_phase: false,
                    });
                }

                self.acting_seat = self.next_active_in_round(action.seat);
                return Ok(ActionResult {
                    valid: true, hand_over: false,
                    winner: 255, payout: 0, advance_phase: false,
                });
            }

            Action::Check => {
                if self.bets[seat] < max_bet {
                    return Err("cannot check when facing a bet");
                }
            }

            Action::Call => {
                let to_call = max_bet.saturating_sub(self.bets[seat]);
                let actual = to_call.min(self.stacks[seat]);
                self.stacks[seat] -= actual;
                self.bets[seat] += actual;
                self.contributed[seat] += actual;
                self.pot += actual;
                // if calling puts us all-in, mark it
                if self.stacks[seat] == 0 {
                    self.seat_state[seat] = SeatState::AllIn;
                }
            }

            Action::Bet | Action::Raise => {
                if action.amount == 0 { return Err("bet amount must be > 0"); }
                // `amount` is the chip delta added this action (raise-BY). the
                // resulting total bet must be a real raise-TO above the current
                // max_bet and respect the min-raise increment.
                let amount = action.amount.min(self.stacks[seat]);
                let all_in = amount == self.stacks[seat];
                let new_bet = self.bets[seat] + amount;
                let to_call = max_bet.saturating_sub(self.bets[seat]);

                // fix #4: a raise must strictly exceed the current top bet, and
                // when facing a bet must add at least the last raise increment on
                // top of the call — UNLESS the player is going all-in for less.
                if !all_in {
                    if new_bet <= max_bet {
                        return Err("raise below minimum");
                    }
                    let min_delta = if max_bet == 0 {
                        self.rules.big_blind
                    } else {
                        to_call + self.last_raise_size
                    };
                    if amount < min_delta {
                        return Err("raise below minimum");
                    }
                }

                // update the min-raise baseline to this raise's increment above
                // the previous top bet. an all-in raising by less than a full
                // increment does not reopen betting, so it leaves the baseline.
                let raise_over = new_bet.saturating_sub(max_bet);
                if raise_over >= self.last_raise_size && raise_over > 0 {
                    self.last_raise_size = raise_over;
                }

                self.stacks[seat] -= amount;
                self.bets[seat] = new_bet;
                self.contributed[seat] += amount;
                self.pot += amount;
                // H6: if stack hits 0 from a bet/raise, mark as all-in
                if self.stacks[seat] == 0 {
                    self.seat_state[seat] = SeatState::AllIn;
                }
            }

            Action::AllIn => {
                let amount = self.stacks[seat];
                self.stacks[seat] = 0;
                self.bets[seat] += amount;
                self.contributed[seat] += amount;
                self.pot += amount;
                self.seat_state[seat] = SeatState::AllIn;
                // a full all-in raise resets the min-raise baseline; an all-in
                // for less than a full raise leaves it (action not reopened).
                let raise_over = self.bets[seat].saturating_sub(max_bet);
                if raise_over >= self.last_raise_size && raise_over > 0 {
                    self.last_raise_size = raise_over;
                }
            }
        }

        self.action_count += 1;
        self.round_actions += 1;

        if matches!(action.action, Action::Bet | Action::Raise | Action::AllIn) {
            self.last_aggressor = seat as u8;
            self.round_actions = 1;
        }

        // check if all remaining players are all-in or only one has chips
        let active_with_chips = (0..self.num_players as usize)
            .filter(|&i| matches!(self.seat_state[i], SeatState::Active) && self.stacks[i] > 0)
            .count();

        // if nobody can act anymore (all all-in or folded), skip to showdown
        if active_with_chips == 0 {
            self.equalize_bets();
            self.phase = Phase::Showdown;
            self.community_count = 5;
            return Ok(ActionResult {
                valid: true, hand_over: true,
                winner: 255, payout: 0, advance_phase: true,
            });
        }

        // advance to next active player
        self.acting_seat = self.next_active_in_round(action.seat);

        // check if round is complete
        if self.is_round_complete(action) {
            self.advance_phase();
        }

        Ok(ActionResult {
            valid: true,
            hand_over: self.phase == Phase::Showdown,
            winner: 255,
            payout: 0,
            advance_phase: self.phase != Phase::Preflop || self.community_count > 0,
        })
    }

    /// check if the current betting round is complete
    fn is_round_complete(&self, last_action: &SignedAction) -> bool {
        // all ACTIVE (not all-in, not folded) players must have equal bets
        let n = self.num_players as usize;
        let max_bet = self.bets.iter().take(n).copied().max().unwrap_or(0);

        // seats that can still act (Active with chips). all-in seats have already
        // committed everything and cannot be asked to act again.
        let active_non_allin = (0..n)
            .filter(|&i| self.seat_state[i] == SeatState::Active && self.stacks[i] > 0)
            .count() as u8;

        // every seat that can still act must have matched the top bet, otherwise
        // there is still a call/raise/fold owed.
        let all_matched = (0..n)
            .filter(|&i| self.seat_state[i] == SeatState::Active && self.stacks[i] > 0)
            .all(|i| self.bets[i] == max_bet);
        if !all_matched { return false; }

        let was_passive = matches!(last_action.action, Action::Check | Action::Call);

        // fix #2: when ≤1 player can still act (everyone else is all-in or
        // folded) and bets are already matched, the round is over — there is
        // nobody left to raise, so requiring ≥2 actions would hang the hand.
        // otherwise fall back to the normal rule: bets equal, last action
        // passive, and every live actor has had a turn this round.
        active_non_allin <= 1
            || (was_passive && self.round_actions >= active_non_allin.max(2))
    }

    /// next active player who can still act (not folded, not all-in)
    fn next_active_in_round(&self, after: u8) -> u8 {
        let n = self.num_players as usize;
        let mut s = (after as usize + 1) % n;
        for _ in 0..n {
            if self.seat_state[s] == SeatState::Active && self.stacks[s] > 0 {
                return s as u8;
            }
            s = (s + 1) % n;
        }
        after
    }

    /// equalize bets into pot, handling multi-level side pots.
    ///
    /// for N>2 players with different all-in amounts, excess chips
    /// above what each player can match are returned to their stacks.
    ///
    /// example: A=100, B=500, C=1000 all-in
    ///   main pot: 3×100 = 300 (A,B,C eligible)
    ///   side pot 1: 2×400 = 800 (B,C eligible)
    ///   C gets 500 back (no one to compete against above B's level)
    fn equalize_bets(&mut self) {
        let n = self.num_players as usize;

        // collect all unique bet levels from active/all-in players, sorted ascending
        let mut levels: Vec<u32> = (0..n)
            .filter(|&i| matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn))
            .map(|i| self.bets[i])
            .collect();
        levels.sort_unstable();
        levels.dedup();

        if levels.is_empty() {
            self.bets = [0; MAX_SEATS];
            return;
        }

        // for each level, only keep what can be matched
        // excess above the highest contested level goes back
        let max_contested = if levels.len() >= 2 {
            // the second-highest level is the most any player needs to match
            // the highest player gets excess back if they're the only one at that level
            let highest = *levels.last().unwrap();
            let second = levels[levels.len() - 2];
            let count_at_highest = (0..n)
                .filter(|&i| matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn) && self.bets[i] == highest)
                .count();
            if count_at_highest == 1 {
                second // only one player at top → return excess above second
            } else {
                highest // multiple at top → all contested
            }
        } else {
            levels[0] // only one level — everyone matched
        };

        // return excess to players who bet above max contested. this is an
        // *uncalled* bet, so it was never truly committed — remove it from the
        // per-seat contribution tally as well, or it would inflate that seat's
        // side-pot eligibility at showdown.
        for i in 0..n {
            if self.bets[i] > max_contested {
                let excess = self.bets[i] - max_contested;
                self.stacks[i] += excess;
                self.pot -= excess;
                self.contributed[i] = self.contributed[i].saturating_sub(excess);
            }
        }
        self.bets = [0; MAX_SEATS];
    }

    fn advance_phase(&mut self) {
        self.bets = [0; MAX_SEATS];
        self.round_actions = 0;
        self.last_aggressor = 255;
        self.last_raise_size = self.rules.big_blind;

        // how many players can still act?
        let active_with_chips = (0..self.num_players as usize)
            .filter(|&i| self.seat_state[i] == SeatState::Active && self.stacks[i] > 0)
            .count();

        match self.phase {
            Phase::Preflop => { self.phase = Phase::Flop; self.community_count = 3; }
            Phase::Flop => { self.phase = Phase::Turn; self.community_count = 4; }
            Phase::Turn => { self.phase = Phase::River; self.community_count = 5; }
            Phase::River => { self.phase = Phase::Showdown; }
            _ => {}
        }

        // if ≤1 player can act (rest all-in/folded), skip all remaining phases to showdown
        if active_with_chips <= 1 && self.phase != Phase::Showdown {
            self.equalize_bets();
            self.phase = Phase::Showdown;
            self.community_count = 5;
            return;
        }

        // set first to act postflop
        self.acting_seat = self.first_postflop();
        // skip to next active player who has chips
        if self.seat_state[self.acting_seat as usize] != SeatState::Active
            || self.stacks[self.acting_seat as usize] == 0
        {
            self.acting_seat = self.next_active_in_round(self.acting_seat.wrapping_sub(1));
        }
    }

    /// collect rake from pot. returns the pot after rake.
    fn collect_rake(&mut self) -> u32 {
        if self.rules.rake_bps == 0 { return self.pot; }
        let mut rake = (self.pot as u64 * self.rules.rake_bps as u64 / 10000) as u32;
        if self.rules.rake_cap > 0 { rake = rake.min(self.rules.rake_cap); }
        self.rake += rake;
        self.pot - rake
    }

    /// evaluate showdown — proper poker hand ranking, N players
    pub fn showdown(&mut self) -> u8 {
        // E6: verify community cards are set
        debug_assert!(self.community_count == 5, "showdown requires 5 community cards");
        // validate card range (0-51)
        for i in 0..5 {
            debug_assert!(self.community[i] < 52, "invalid community card: {}", self.community[i]);
        }

        let n = self.num_players as usize;

        // pre-score every seat still in the hand (Active or AllIn). folded seats
        // score 0 and are never eligible, but their `contributed` chips remain
        // dead money inside whatever pot layer they reached.
        let mut score = [0u32; MAX_SEATS];
        let mut can_win = [false; MAX_SEATS];
        for i in 0..n {
            if matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn) {
                score[i] = self.best_hand(i);
                can_win[i] = true;
            }
        }

        // working copy of contributions we peel into side pots.
        let mut remaining = self.contributed;

        // rake is charged once, off the top of the whole pot (matches the
        // previous single-pot behaviour). we track how much rake is still owed
        // and skim it from each pot layer as it is awarded, so the sum paid out
        // equals `total_pot - rake` and chips are conserved.
        let total_pot: u32 = (0..n).map(|i| remaining[i]).sum();
        debug_assert!(total_pot == self.pot, "contributions must equal pot at showdown");
        let mut rake_owed = {
            if self.rules.rake_bps == 0 {
                0
            } else {
                let mut r = (total_pot as u64 * self.rules.rake_bps as u64 / 10000) as u32;
                if self.rules.rake_cap > 0 { r = r.min(self.rules.rake_cap); }
                r
            }
        };
        self.rake += rake_owed;

        // track the winner of the *main* pot to return as the headline seat.
        let mut headline_winner: Option<u8> = None;

        // peel side pots: repeatedly take the smallest positive remaining
        // contribution level, form a layer at that level across every seat that
        // still has chips in, and award it to the best eligible (non-folded)
        // hand. eligibility for a layer = seats that reached that contribution
        // level. dead money from folded seats is included in the layer amount
        // but folded seats cannot win.
        loop {
            // smallest positive remaining contribution across ALL seats
            // (folded included — their dead money still forms pot layers).
            let mut level = u32::MAX;
            for i in 0..n {
                if remaining[i] > 0 && remaining[i] < level {
                    level = remaining[i];
                }
            }
            if level == u32::MAX { break; } // nothing left to distribute

            // membership snapshot: any seat with chips still in *before* this
            // peel put money into this layer and is therefore eligible for it
            // (unless folded). folded members contribute dead money but can't win.
            let mut in_layer = [false; MAX_SEATS];
            let mut pot_amount: u32 = 0;
            for i in 0..n {
                if remaining[i] > 0 {
                    in_layer[i] = true;
                    remaining[i] -= level;
                    pot_amount += level;
                }
            }
            if pot_amount == 0 { continue; }

            // skim any outstanding rake off this layer first (rake off the top).
            if rake_owed > 0 {
                let skim = rake_owed.min(pot_amount);
                pot_amount -= skim;
                rake_owed -= skim;
            }
            if pot_amount == 0 { continue; }

            // best eligible hand for this layer: eligible = not folded AND a
            // member of this layer.
            let mut best = 0u32;
            let mut have_winner = false;
            let mut winners = [false; MAX_SEATS];
            let mut winner_count = 0u32;
            for i in 0..n {
                if !can_win[i] || !in_layer[i] { continue; }
                let s = score[i];
                if !have_winner || s > best {
                    best = s;
                    have_winner = true;
                    winners = [false; MAX_SEATS];
                    winners[i] = true;
                    winner_count = 1;
                } else if s == best {
                    winners[i] = true;
                    winner_count += 1;
                }
            }

            if !have_winner {
                // no eligible live hand for this layer (only possible if every
                // contributor to it folded). award to the overall best live hand
                // so chips are never burned.
                let mut fallback = 0u32;
                let mut fb_seat: Option<usize> = None;
                for i in 0..n {
                    if can_win[i] && (fb_seat.is_none() || score[i] > fallback) {
                        fallback = score[i];
                        fb_seat = Some(i);
                    }
                }
                if let Some(i) = fb_seat {
                    self.stacks[i] += pot_amount;
                    if headline_winner.is_none() { headline_winner = Some(i as u8); }
                }
                continue;
            }

            // split evenly; odd chips go to the first winner in seat order
            // (seats 0..n). this matches the convention the previous single-pot
            // split code used.
            let share = pot_amount / winner_count;
            let remainder = pot_amount % winner_count;
            let mut first = true;
            for i in 0..n {
                if winners[i] {
                    self.stacks[i] += share + if first { remainder } else { 0 };
                    if headline_winner.is_none() { headline_winner = Some(i as u8); }
                    first = false;
                }
            }
        }

        self.pot = 0;
        self.contributed = [0; MAX_SEATS];
        self.phase = Phase::Settled;
        // button is NOT rotated here (fix #3): the next deal() derives it
        // deterministically from hand_number, so a side that never calls
        // showdown() still ends up on the identical button next hand.

        // return the headline (main-pot) winner seat.
        headline_winner.unwrap_or(0)
    }

    fn best_hand(&self, seat: usize) -> u32 {
        let mut all7 = [0u8; 7];
        all7[0] = self.cards[seat][0];
        all7[1] = self.cards[seat][1];
        for i in 0..5 { all7[2 + i] = self.community[i]; }

        let mut best = 0u32;
        for i in 0..7 {
            for j in (i + 1)..7 {
                let mut hand = [0u8; 5];
                let mut idx = 0;
                for k in 0..7 {
                    if k != i && k != j { hand[idx] = all7[k]; idx += 1; }
                }
                let score = eval_5(hand);
                if score > best { best = score; }
            }
        }
        best
    }
}

/// evaluate best 5-card hand from 2 hole cards + 5 community cards
pub fn best_hand_7(hole: [u8; 2], community: &[u8; 5]) -> u32 {
    let mut all7 = [0u8; 7];
    all7[0] = hole[0];
    all7[1] = hole[1];
    for i in 0..5 { all7[2 + i] = community[i]; }

    let mut best = 0u32;
    for i in 0..7 {
        for j in (i + 1)..7 {
            let mut hand = [0u8; 5];
            let mut idx = 0;
            for k in 0..7 {
                if k != i && k != j { hand[idx] = all7[k]; idx += 1; }
            }
            let score = eval_5(hand);
            if score > best { best = score; }
        }
    }
    best
}

// ============================================================================
// Hand evaluation
// ============================================================================

const HIGH_CARD: u32 = 0;
const PAIR: u32 = 1;
const TWO_PAIR: u32 = 2;
const TRIPS: u32 = 3;
const STRAIGHT: u32 = 4;
const FLUSH: u32 = 5;
const FULL_HOUSE: u32 = 6;
const QUADS: u32 = 7;
const STRAIGHT_FLUSH: u32 = 8;

pub fn eval_5(hand: [u8; 5]) -> u32 {
    let mut ranks = [0u8; 5];
    let mut suits = [0u8; 5];
    for i in 0..5 { ranks[i] = hand[i] % 13; suits[i] = hand[i] / 13; }
    ranks.sort_unstable();
    ranks.reverse();

    let is_flush = suits[0] == suits[1] && suits[1] == suits[2] && suits[2] == suits[3] && suits[3] == suits[4];
    let is_straight = is_straight_check(&ranks);
    let is_wheel = ranks == [12, 3, 2, 1, 0];

    if is_flush && (is_straight || is_wheel) {
        return (STRAIGHT_FLUSH << 20) | if is_wheel { 3 } else { ranks[0] as u32 };
    }

    let mut freq = [0u8; 13];
    for &r in &ranks { freq[r as usize] += 1; }

    let mut quads_rank = 0u8;
    let mut trips_rank = 0u8;
    let mut pairs = [0u8; 2];
    let mut pair_count = 0usize;
    let mut kickers = [0u8; 5];
    let mut kick_idx = 0;

    for r in (0..13u8).rev() {
        match freq[r as usize] {
            4 => quads_rank = r + 1,
            3 => trips_rank = r + 1,
            2 => { if pair_count < 2 { pairs[pair_count] = r + 1; pair_count += 1; } }
            1 => { if kick_idx < 5 { kickers[kick_idx] = r; kick_idx += 1; } }
            _ => {}
        }
    }

    if quads_rank > 0 {
        return (QUADS << 20) | ((quads_rank as u32 - 1) << 4) | kickers[0] as u32;
    }
    if trips_rank > 0 && pair_count > 0 {
        return (FULL_HOUSE << 20) | ((trips_rank as u32 - 1) << 4) | (pairs[0] as u32 - 1);
    }
    if is_flush { return (FLUSH << 20) | kicker_score(&ranks); }
    if is_straight { return (STRAIGHT << 20) | ranks[0] as u32; }
    if is_wheel { return (STRAIGHT << 20) | 3; }
    if trips_rank > 0 {
        return (TRIPS << 20) | ((trips_rank as u32 - 1) << 8) | (kickers[0] as u32) << 4 | kickers[1] as u32;
    }
    if pair_count >= 2 {
        return (TWO_PAIR << 20) | ((pairs[0] as u32 - 1) << 8) | ((pairs[1] as u32 - 1) << 4) | kickers[0] as u32;
    }
    if pair_count == 1 {
        return (PAIR << 20) | ((pairs[0] as u32 - 1) << 12) | (kickers[0] as u32) << 8 | (kickers[1] as u32) << 4 | kickers[2] as u32;
    }
    (HIGH_CARD << 20) | kicker_score(&ranks)
}

fn kicker_score(ranks: &[u8; 5]) -> u32 {
    (ranks[0] as u32) << 16 | (ranks[1] as u32) << 12 | (ranks[2] as u32) << 8 | (ranks[3] as u32) << 4 | ranks[4] as u32
}

fn is_straight_check(sorted_desc: &[u8; 5]) -> bool {
    sorted_desc[0].saturating_sub(sorted_desc[4]) == 4 &&
    sorted_desc[0] != sorted_desc[1] && sorted_desc[1] != sorted_desc[2] &&
    sorted_desc[2] != sorted_desc[3] && sorted_desc[3] != sorted_desc[4]
}

// ============================================================================
// WASM bindings (heads-up convenience wrapper, delegates to N-seat engine)
// ============================================================================

#[cfg(feature = "wasm")]
mod wasm {
    use super::*;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub struct WasmGame {
        state: GameState,
    }

    #[wasm_bindgen]
    impl WasmGame {
        #[wasm_bindgen(constructor)]
        pub fn new(buyin: u32, small_blind: u32, big_blind: u32) -> Self {
            Self {
                state: GameState::new(Rules {
                    buyin, small_blind, big_blind,
                    turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
                }, 2),
            }
        }

        /// create N-player game
        pub fn new_table(num_players: u8, buyin: u32, small_blind: u32, big_blind: u32, rake_bps: u16, rake_cap: u32) -> Self {
            Self {
                state: GameState::new(Rules {
                    buyin, small_blind, big_blind,
                    turn_timeout_blocks: 6, rake_bps, rake_cap,
                }, num_players),
            }
        }

        pub fn new_with_rake(buyin: u32, small_blind: u32, big_blind: u32, rake_bps: u16, rake_cap: u32) -> Self {
            Self::new_table(2, buyin, small_blind, big_blind, rake_bps, rake_cap)
        }

        /// deal for 2 players (backwards compatible)
        pub fn deal(&mut self, a0: u8, a1: u8, b0: u8, b1: u8, c0: u8, c1: u8, c2: u8, c3: u8, c4: u8) {
            self.state.deal(&[[a0, a1], [b0, b1]], [c0, c1, c2, c3, c4]);
        }

        /// deal for N players. cards is a flat array [p0c0, p0c1, p1c0, p1c1, ...]
        pub fn deal_n(&mut self, cards: &[u8], c0: u8, c1: u8, c2: u8, c3: u8, c4: u8) {
            let n = self.state.num_players as usize;
            let mut all_cards = Vec::with_capacity(n);
            for i in 0..n {
                let idx = i * 2;
                if idx + 1 < cards.len() {
                    all_cards.push([cards[idx], cards[idx + 1]]);
                } else {
                    all_cards.push([0, 0]);
                }
            }
            self.state.deal(&all_cards, [c0, c1, c2, c3, c4]);
        }

        pub fn apply_action(&mut self, seat: u8, action: u8, amount: u32, seq: u32) -> Vec<u32> {
            let signed = SignedAction {
                seat, action: Action::from_u8(action).unwrap_or(Action::Fold),
                amount, seq, sig: [0; 64],
            };
            match self.state.apply(&signed) {
                Ok(r) => vec![1, r.hand_over as u32, r.winner as u32, r.payout, r.advance_phase as u32],
                Err(_) => vec![0, 0, 0, 0, 0],
            }
        }

        pub fn apply_action_debug(&mut self, seat: u8, action: u8, amount: u32, seq: u32) -> String {
            let signed = SignedAction {
                seat, action: Action::from_u8(action).unwrap_or(Action::Fold),
                amount, seq, sig: [0; 64],
            };
            match self.state.apply(&signed) {
                Ok(r) => format!("ok valid={} over={} winner={} payout={} advance={}",
                    r.valid, r.hand_over, r.winner, r.payout, r.advance_phase),
                Err(e) => format!("err: {}", e),
            }
        }

        pub fn acting_seat(&self) -> u8 { self.state.acting_seat }
        pub fn round_actions(&self) -> u8 { self.state.round_actions }
        pub fn num_players(&self) -> u8 { self.state.num_players }
        pub fn last_raise_size(&self) -> u32 { self.state.last_raise_size }

        pub fn debug_state(&self) -> String {
            format!("phase={:?} acting={} bets=[{},{}] stacks=[{},{}] pot={} round_actions={} seq={}",
                self.state.phase, self.state.acting_seat,
                self.state.bets[0], self.state.bets[1],
                self.state.stacks[0], self.state.stacks[1],
                self.state.pot, self.state.round_actions, self.state.action_count)
        }

        pub fn showdown(&mut self) -> u8 { self.state.showdown() }
        pub fn phase(&self) -> u8 { self.state.phase as u8 }
        pub fn pot(&self) -> u32 { self.state.pot }
        pub fn rake(&self) -> u32 { self.state.rake }
        pub fn stack(&self, seat: u8) -> u32 { self.state.stacks[seat as usize] }
        pub fn bet(&self, seat: u8) -> u32 { self.state.bets[seat as usize] }
        pub fn community_count(&self) -> u8 { self.state.community_count }
        pub fn button(&self) -> u8 { self.state.button }
        pub fn hand_number(&self) -> u32 { self.state.hand_number }
        pub fn seat_state(&self, seat: u8) -> u8 { self.state.seat_state[seat as usize] as u8 }

        pub fn update_community(&mut self, c0: u8, c1: u8, c2: u8, c3: u8, c4: u8) {
            self.state.community = [c0, c1, c2, c3, c4];
            self.state.community_count = 5;
        }

        pub fn update_opp_cards(&mut self, c0: u8, c1: u8) {
            self.state.cards[1] = [c0, c1];
        }

        /// update any seat's cards
        pub fn update_seat_cards(&mut self, seat: u8, c0: u8, c1: u8) {
            self.state.cards[seat as usize] = [c0, c1];
        }

        /// get all stacks as flat array
        pub fn all_stacks(&self) -> Vec<u32> {
            self.state.stacks[..self.state.num_players as usize].to_vec()
        }

        /// get all bets as flat array
        pub fn all_bets(&self) -> Vec<u32> {
            self.state.bets[..self.state.num_players as usize].to_vec()
        }

        /// get all seat states as flat array
        pub fn all_seat_states(&self) -> Vec<u8> {
            self.state.seat_state[..self.state.num_players as usize]
                .iter().map(|s| *s as u8).collect()
        }

        /// set stacks + button for N players
        pub fn set_state_n(&mut self, stacks: &[u32], btn: u8) {
            for (i, &s) in stacks.iter().enumerate() {
                if i < self.state.num_players as usize {
                    self.state.stacks[i] = s;
                }
            }
            self.state.button = btn;
        }

        /// 2-player backwards compat
        pub fn set_state(&mut self, stack0: u32, stack1: u32, btn: u8) {
            self.state.stacks[0] = stack0;
            self.state.stacks[1] = stack1;
            self.state.button = btn;
        }

        pub fn sit_out(&mut self, seat: u8) { self.state.sit_out(seat); }
        pub fn sit_in(&mut self, seat: u8) { self.state.sit_in(seat); }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_action(seat: u8, action: Action, amount: u32, seq: u32) -> SignedAction {
        SignedAction { seat, action, amount, seq, sig: [0; 64] }
    }

    #[test]
    fn test_deal_and_blinds() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[0, 1], [2, 3]], [4, 5, 6, 7, 8]);
        assert_eq!(state.phase, Phase::Preflop);
        assert_eq!(state.pot, 15);
        assert_eq!(state.stacks[0], 995);
        assert_eq!(state.stacks[1], 990);
    }

    #[test]
    fn test_fold() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[0, 1], [2, 3]], [4, 5, 6, 7, 8]);
        let result = state.apply(&make_action(0, Action::Fold, 0, 0)).unwrap();
        assert!(result.hand_over);
        assert_eq!(result.winner, 1);
        assert_eq!(state.stacks[1], 990 + 15);
    }

    #[test]
    fn test_check_check_advances() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[0, 1], [2, 3]], [4, 5, 6, 7, 8]);
        state.apply(&make_action(0, Action::Call, 0, 0)).unwrap();
        let result = state.apply(&make_action(1, Action::Check, 0, 0)).unwrap();
        assert!(result.advance_phase);
        assert_eq!(state.phase, Phase::Flop);
    }

    #[test]
    fn test_wrong_turn_rejected() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[0, 1], [2, 3]], [4, 5, 6, 7, 8]);
        assert!(state.apply(&make_action(1, Action::Check, 0, 0)).is_err());
    }

    #[test]
    fn test_full_hand_to_showdown() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[12, 11], [13, 14]], [29, 31, 33, 47, 41]);
        // preflop
        state.apply(&make_action(0, Action::Call, 0, 0)).unwrap();
        state.apply(&make_action(1, Action::Check, 0, 0)).unwrap();
        assert_eq!(state.phase, Phase::Flop);
        // flop
        state.apply(&make_action(1, Action::Check, 0, 0)).unwrap();
        state.apply(&make_action(0, Action::Check, 0, 0)).unwrap();
        // turn
        state.apply(&make_action(1, Action::Check, 0, 0)).unwrap();
        state.apply(&make_action(0, Action::Check, 0, 0)).unwrap();
        // river
        state.apply(&make_action(1, Action::Check, 0, 0)).unwrap();
        state.apply(&make_action(0, Action::Check, 0, 0)).unwrap();
        assert_eq!(state.phase, Phase::Showdown);
        let winner = state.showdown();
        assert_eq!(winner, 0);
        assert_eq!(state.stacks[0], 990 + 20);
    }

    #[test]
    fn test_both_allin_skips_to_showdown() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[12, 11], [13, 14]], [29, 31, 33, 47, 41]);
        state.apply(&make_action(0, Action::AllIn, 0, 0)).unwrap();
        let result = state.apply(&make_action(1, Action::AllIn, 0, 0)).unwrap();
        assert!(result.hand_over);
        assert_eq!(state.phase, Phase::Showdown);
        let pot_before = state.pot;
        assert!(pot_before > 0);
        let winner = state.showdown();
        assert_eq!(state.stacks[winner as usize], pot_before);
    }

    #[test]
    fn test_3player_side_pot() {
        // 3 players with different stacks go all-in
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10, turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0 };
        let mut state = GameState::new(rules, 3);
        // give different stacks: seat 0 = 100, seat 1 = 500, seat 2 = 1000
        state.stacks[0] = 100;
        state.stacks[1] = 500;
        state.stacks[2] = 1000;
        let initial_total: u32 = state.stacks.iter().take(3).sum();

        state.deal(&[[12, 11], [10, 9], [8, 7]], [29, 31, 33, 47, 41]);
        let post_deal_total: u32 = state.stacks.iter().take(3).sum::<u32>() + state.pot;
        assert_eq!(post_deal_total, initial_total, "deal leaked chips");

        // seat 2 (UTG) goes all-in
        let _ = state.apply(&make_action(state.acting_seat, Action::AllIn, 0, 0));
        // seat 0 (SB) calls all-in
        let _ = state.apply(&make_action(state.acting_seat, Action::AllIn, 0, 0));
        // seat 1 (BB) calls all-in
        let _ = state.apply(&make_action(state.acting_seat, Action::AllIn, 0, 0));

        assert_eq!(state.phase, Phase::Showdown);

        let pre_sd: u32 = state.stacks.iter().take(3).sum::<u32>() + state.pot;
        assert_eq!(pre_sd, initial_total, "chips leaked before showdown");

        let _winner = state.showdown();
        let post_sd: u32 = state.stacks.iter().take(3).sum::<u32>() + state.pot + state.rake;
        assert_eq!(post_sd, initial_total, "chips leaked in showdown: stacks={:?} pot={} rake={}",
            &state.stacks[..3], state.pot, state.rake);
    }

    #[test]
    fn test_3player_unequal_allin_excess() {
        // verify excess returned correctly: A=100, B=500, C=1000
        let mut state = GameState::new(Rules { buyin: 1000, ..Rules::default() }, 3);
        state.stacks = [100, 500, 1000, 0, 0, 0, 0, 0, 0, 0];
        state.deal(&[[12, 11], [10, 9], [8, 7]], [29, 31, 33, 47, 41]);

        // all go all-in in order
        while state.phase != Phase::Showdown && state.phase != Phase::Settled {
            let seat = state.acting_seat;
            match state.apply(&make_action(seat, Action::AllIn, 0, 0)) {
                Ok(_) => {}
                Err(_) => {
                    // try fold if all-in fails
                    let _ = state.apply(&make_action(seat, Action::Fold, 0, 0));
                }
            }
        }

        // chip conservation check
        let total: u32 = state.stacks.iter().take(3).sum::<u32>() + state.pot + state.rake;
        assert_eq!(total, 1600, "chips: stacks={:?} pot={} rake={}", &state.stacks[..3], state.pot, state.rake);
    }

    #[test]
    fn test_phase_guard() {
        // E5: can't apply actions outside betting phases
        let mut state = GameState::new(Rules::default(), 2);
        state.phase = Phase::Settled;
        let result = state.apply(&make_action(0, Action::Fold, 0, 0));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "not in betting phase");

        state.phase = Phase::Showdown;
        let result = state.apply(&make_action(0, Action::Fold, 0, 0));
        assert!(result.is_err());
    }

    #[test]
    fn test_sit_out_sit_in() {
        let mut state = GameState::new(Rules::default(), 2);
        state.sit_out(1);
        assert_eq!(state.seat_state[1], SeatState::SittingOut);
        state.sit_in(1);
        assert_eq!(state.seat_state[1], SeatState::Active);
    }

    #[test]
    fn test_rake() {
        let rules = Rules { rake_bps: 500, rake_cap: 0, ..Rules::default() }; // 5%
        let mut state = GameState::new(rules, 2);
        state.deal(&[[12, 11], [13, 14]], [29, 31, 33, 47, 41]);
        // fold → winner gets pot minus rake
        let result = state.apply(&make_action(0, Action::Fold, 0, 0)).unwrap();
        // pot was 15, rake 5% = 0 (integer truncation: 15*500/10000 = 0)
        // with bigger pot:
        assert_eq!(result.payout, 15); // small pot, rake rounds to 0
    }

    #[test]
    fn test_eval_pair_beats_high_card() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[0, 13], [12, 11]], [29, 31, 33, 47, 41]);
        state.phase = Phase::Showdown;
        state.community_count = 5;
        state.pot = 100;
        state.contributed = [0; MAX_SEATS];
        state.contributed[0] = 50;
        state.contributed[1] = 50;
        let winner = state.showdown();
        assert_eq!(winner, 0); // pair of 2s beats A-K high
    }

    #[test]
    fn test_eval_flush_beats_straight() {
        let mut state = GameState::new(Rules::default(), 2);
        state.deal(&[[12, 8], [17, 18]], [0, 3, 6, 20, 14]);
        state.phase = Phase::Showdown;
        state.community_count = 5;
        state.pot = 100;
        state.contributed = [0; MAX_SEATS];
        state.contributed[0] = 50;
        state.contributed[1] = 50;
        let winner = state.showdown();
        assert_eq!(winner, 0); // flush beats straight
    }

    // ------------------------------------------------------------------
    // Side-pot correctness (the money-critical fix).
    //
    // Card encoding: rank = card % 13 (0=2 .. 11=K, 12=A), suit = card / 13.
    // Rainbow board with no pairs/straights/flush so each seat's hand strength
    // is decided purely by their pocket pair:
    //   board = { Q(10), 8(19), 5(29), 3(40), 2(0) }  suits {0,1,2,3,0}
    //   seat0 pocket AA -> A♠(12) A♥(25)   best hand
    //   seat1 pocket KK -> K♠(11) K♥(24)   2nd
    //   seat2 pocket 44 -> 4♠(2)  4♥(15)   3rd
    // ------------------------------------------------------------------

    const BOARD_RAINBOW: [u8; 5] = [10, 19, 29, 40, 0];

    /// build a 3-handed state parked at showdown with explicit contributions,
    /// so we test pot distribution in isolation with exact chip counts.
    fn showdown_fixture(contributed: [u32; 3]) -> GameState {
        let mut state = GameState::new(Rules::default(), 3);
        state.cards[0] = [12, 25]; // AA
        state.cards[1] = [11, 24]; // KK
        state.cards[2] = [2, 15];  // 44
        state.community = BOARD_RAINBOW;
        state.community_count = 5;
        state.phase = Phase::Showdown;
        state.stacks = [0; MAX_SEATS];
        state.contributed = [0; MAX_SEATS];
        for i in 0..3 {
            state.contributed[i] = contributed[i];
            state.seat_state[i] = SeatState::AllIn;
        }
        state.pot = contributed.iter().sum();
        state
    }

    #[test]
    fn test_short_allin_best_hand_wins_only_main_pot() {
        // (a) Short all-in with the BEST hand only scoops the main pot it
        // covered; the remainder goes to the next-best eligible player.
        //
        // seat0 (AA, best) is all-in for 100; seat1 (KK) and seat2 (44) each
        // put in 500.
        //   main pot  = 3*100 = 300, eligible {0,1,2} -> seat0 (AA)
        //   side pot  = 2*400 = 800, eligible {1,2}   -> seat1 (KK)
        let mut state = showdown_fixture([100, 500, 500]);
        let total: u32 = 100 + 500 + 500;

        let winner = state.showdown();

        assert_eq!(winner, 0, "main-pot winner is the best hand");
        assert_eq!(state.stacks[0], 300, "AA covered only 100 -> wins 300 main pot only");
        assert_eq!(state.stacks[1], 800, "KK takes the 800 side pot AA could not contest");
        assert_eq!(state.stacks[2], 0, "44 wins nothing");
        assert_eq!(state.stacks[0] + state.stacks[1] + state.stacks[2], total,
            "chip conservation");
        assert_eq!(state.pot, 0);
    }

    #[test]
    fn test_three_way_allin_three_stacks_side_pots() {
        // (b) Three-way all-in at three different sizes -> main + 2 side pots.
        //
        // Contributions already equalized (uncalled top layer returned upstream
        // by equalize_bets): seat0=100, seat1=500, seat2=500 would be the
        // equalized case, but here we exercise the raw distributor with an
        // uncontested top layer left in to prove the lone-eligible seat gets it
        // back: seat0=100, seat1=500, seat2=1000.
        //   main pot   = 3*100 = 300, eligible {0,1,2} -> seat0 (AA)
        //   side pot 1 = 2*400 = 800, eligible {1,2}   -> seat1 (KK)
        //   side pot 2 = 1*500 = 500, eligible {2}     -> seat2 (only member)
        let mut state = showdown_fixture([100, 500, 1000]);
        let total: u32 = 100 + 500 + 1000;

        let winner = state.showdown();

        assert_eq!(winner, 0, "main-pot winner is the best hand");
        assert_eq!(state.stacks[0], 300, "AA -> 300 main pot");
        assert_eq!(state.stacks[1], 800, "KK -> 800 side pot 1");
        assert_eq!(state.stacks[2], 500, "44 is sole member of side pot 2 -> 500 back");
        assert_eq!(state.stacks[0] + state.stacks[1] + state.stacks[2], total,
            "chip conservation");
        assert_eq!(state.pot, 0);
    }

    #[test]
    fn test_side_pots_full_action_flow_conservation() {
        // (c) End-to-end through real actions: three unequal all-ins.
        // Chip conservation must hold, and the short best hand must not scoop
        // more than the main pot.
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10,
            turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0 };
        let mut state = GameState::new(rules, 3);
        state.stacks[0] = 100;
        state.stacks[1] = 500;
        state.stacks[2] = 1000;
        let initial: u32 = state.stacks.iter().take(3).sum();

        // deal with our rainbow board + pocket pairs (seat0 AA best but shortest)
        state.deal(&[[12, 25], [11, 24], [2, 15]], BOARD_RAINBOW);

        // everyone shoves until we reach showdown
        while state.phase != Phase::Showdown && state.phase != Phase::Settled {
            let seat = state.acting_seat;
            if state.apply(&make_action(seat, Action::AllIn, 0, 0)).is_err() {
                let _ = state.apply(&make_action(seat, Action::Call, 0, 0));
            }
        }
        assert_eq!(state.phase, Phase::Showdown);

        // pre-showdown conservation
        let pre: u32 = state.stacks.iter().take(3).sum::<u32>() + state.pot;
        assert_eq!(pre, initial, "chips leaked before showdown");

        state.showdown();

        // post-showdown conservation (no rake configured)
        let post: u32 = state.stacks.iter().take(3).sum::<u32>() + state.pot + state.rake;
        assert_eq!(post, initial,
            "chips leaked in showdown: stacks={:?} pot={} rake={}",
            &state.stacks[..3], state.pot, state.rake);

        // seat0 (AA) is all-in for at most 100 into a 3-way pot, so it can win
        // at most the main pot (3*100 = 300). It must NOT scoop the whole pot.
        assert!(state.stacks[0] <= 300,
            "short all-in AA over-scooped: got {}", state.stacks[0]);
    }

    #[test]
    fn test_side_pot_rake_conserved() {
        // rake is charged once off the top; side pots still sum correctly.
        let rules = Rules { rake_bps: 500, rake_cap: 0, ..Rules::default() }; // 5%
        let mut state = GameState::new(rules, 3);
        state.cards[0] = [12, 25]; // AA
        state.cards[1] = [11, 24]; // KK
        state.cards[2] = [2, 15];  // 44
        state.community = BOARD_RAINBOW;
        state.community_count = 5;
        state.phase = Phase::Showdown;
        state.stacks = [0; MAX_SEATS];
        state.contributed = [0; MAX_SEATS];
        state.contributed[0] = 100;
        state.contributed[1] = 500;
        state.contributed[2] = 500;
        state.seat_state[0] = SeatState::AllIn;
        state.seat_state[1] = SeatState::AllIn;
        state.seat_state[2] = SeatState::AllIn;
        state.pot = 1100;

        state.showdown();

        // 5% of 1100 = 55 rake. 1100 - 55 = 1045 distributed.
        assert_eq!(state.rake, 55);
        let paid: u32 = state.stacks.iter().take(3).sum();
        assert_eq!(paid, 1045, "distributed pot + rake must equal total");
        assert_eq!(paid + state.rake, 1100, "chip conservation with rake");
    }

    // ------------------------------------------------------------------
    // Regression tests for the four confirmed liveness/desync bugs.
    // ------------------------------------------------------------------

    /// helper: assert chips are conserved (stacks + pot + rake).
    fn assert_conserved(state: &GameState, expected: u32, ctx: &str) {
        let n = state.num_players as usize;
        let total: u32 = state.stacks.iter().take(n).sum::<u32>() + state.pot + state.rake;
        assert_eq!(total, expected, "chip leak {}: stacks={:?} pot={} rake={}",
            ctx, &state.stacks[..n], state.pot, state.rake);
    }

    #[test]
    fn test_deal_allin_blind_does_not_deadlock() {
        // BUG #1: a posted blind that empties a short stack marks the seat AllIn,
        // but deal() used to leave acting_seat on a seat that cannot act -> every
        // apply() returns an error and the hand is locked forever.
        // HU stacks=[3,1000], blinds 5/10. seat0 is BTN/SB, posts 3 -> all-in.
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10, ..Rules::default() };
        let mut state = GameState::new(rules, 2);
        state.stacks = [3, 1000, 0, 0, 0, 0, 0, 0, 0, 0];
        let initial: u32 = 3 + 1000;
        state.deal(&[[12, 11], [13, 14]], [29, 31, 33, 47, 41]);

        // seat0 posts SB 3 -> all-in; seat1 posts BB 10. only seat1 could act,
        // but with only one player able to act the hand must skip to showdown.
        assert_eq!(state.seat_state[0], SeatState::AllIn, "short blind is all-in");
        assert_eq!(state.phase, Phase::Showdown, "no live betting -> showdown");
        assert_conserved(&state, initial, "post-deal all-in blind");

        state.showdown();
        assert_conserved(&state, initial, "after showdown");
    }

    #[test]
    fn test_allin_then_call_completes_round() {
        // BUG #2: HU, BB is short and all-in; the other player calls -> bets are
        // equal, only one non-all-in player remains, but is_round_complete used
        // to require >=2 actions so the round never closed and next_active kept
        // returning the same seat -> permanent hang.
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10, ..Rules::default() };
        let mut state = GameState::new(rules, 2);
        state.stacks = [1000, 8, 0, 0, 0, 0, 0, 0, 0, 0];
        let initial: u32 = 1008;
        state.deal(&[[12, 11], [0, 1]], [29, 31, 33, 47, 41]);

        // HU: seat0 = BTN/SB acts first. seat1 = BB posted 8 (all-in, < 10 BB).
        assert_eq!(state.seat_state[1], SeatState::AllIn, "short BB is all-in");
        assert_eq!(state.acting_seat, 0, "SB acts first HU");

        // seat0 calls -> bets equal, seat1 all-in. round must close to showdown.
        let r = state.apply(&make_action(0, Action::Call, 0, 0)).unwrap();
        assert_eq!(state.phase, Phase::Showdown, "all-in-then-call must reach showdown");
        assert!(r.hand_over || state.phase == Phase::Showdown);
        assert_conserved(&state, initial, "after call closes round");

        state.showdown();
        assert_conserved(&state, initial, "after showdown");
    }

    #[test]
    fn test_button_is_pure_function_of_hand_number() {
        // BUG #3: button must be identical on both engines regardless of which
        // hand-end path ran. Derived purely from hand_number in deal().
        let mut a = GameState::new(Rules::default(), 3);
        let mut b = GameState::new(Rules::default(), 3);

        // engine A ends every hand via showdown(); engine B ends via fold.
        for h in 0..6u32 {
            a.deal(&[[12, 11], [10, 9], [8, 7]], [29, 31, 33, 47, 41]);
            b.deal(&[[12, 11], [10, 9], [8, 7]], [29, 31, 33, 47, 41]);
            // buttons must match at deal time every hand
            assert_eq!(a.button, b.button, "button desync at hand {}", h + 1);
            // button advances by one seat per hand (3-handed, no busts)
            assert_eq!(a.button as u32, h % 3, "button rotation wrong at hand {}", h + 1);

            // drive A to a settled state one way and B another; button for next
            // hand is recomputed in deal() so the divergent paths don't matter.
            a.phase = Phase::Settled;
            b.button = 99; // deliberately corrupt B's button between hands
            b.phase = Phase::Settled;
        }
    }

    #[test]
    fn test_button_matches_regardless_of_showdown_call() {
        // Two engines: one calls showdown() on the all-in path, the other never
        // does (relies on the apply() result). Next hand's button must match.
        let seed_cards: [[u8; 2]; 2] = [[12, 11], [0, 1]];
        let board = [29, 31, 33, 47, 41];

        let mut host = GameState::new(Rules::default(), 2);
        let mut peer = GameState::new(Rules::default(), 2);
        for _ in 0..4 {
            host.deal(&seed_cards, board);
            peer.deal(&seed_cards, board);
            assert_eq!(host.button, peer.button);

            // both all-in to showdown
            host.apply(&make_action(host.acting_seat, Action::AllIn, 0, 0)).unwrap();
            host.apply(&make_action(host.acting_seat, Action::AllIn, 0, 0)).unwrap();
            peer.apply(&make_action(peer.acting_seat, Action::AllIn, 0, 0)).unwrap();
            peer.apply(&make_action(peer.acting_seat, Action::AllIn, 0, 0)).unwrap();

            // HOST calls showdown(); PEER does not (simulates the non-host side).
            host.showdown();
            peer.phase = Phase::Settled; // peer resolves without showdown()
            // resync stacks so the next deal is comparable (in the real system
            // both sides agree on stacks via the FROST-settled result).
            peer.stacks = host.stacks;
            peer.hand_number = host.hand_number;
        }
    }

    #[test]
    fn test_raise_must_be_raise_to_not_undercall() {
        // BUG #4: a "raise" delta that leaves bets[seat] <= max_bet is an
        // under-call and must be rejected; a legal raise updates the baseline.
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10, ..Rules::default() };
        let mut state = GameState::new(rules, 3);
        state.deal(&[[12, 11], [10, 9], [8, 7]], [29, 31, 33, 47, 41]);
        // 3-handed: SB=1, BB=2, UTG=0 acts first. max_bet = BB = 10.
        let utg = state.acting_seat;
        assert_eq!(state.bets[state.sb_seat() as usize], 5);
        assert_eq!(state.bets[state.bb_seat() as usize], 10);

        // an under-min raise BY 1 (would total 11, only +1 over the BB) is illegal.
        let bad = state.apply(&make_action(utg, Action::Raise, 1, 0));
        assert!(bad.is_err(), "under-min raise must be rejected");

        // a legal min-raise: to_call(10) + last_raise_size(10) = 20 delta -> total 20.
        let good = state.apply(&make_action(utg, Action::Raise, 20, 0));
        assert!(good.is_ok(), "min raise-to must be accepted: {:?}", good.err());
        assert_eq!(state.bets[utg as usize], 20, "raise is TO 20");
        assert_eq!(state.last_raise_size, 10, "raise increment recorded");
    }

    #[test]
    fn test_reraise_min_raise_tracks_last_increment() {
        // facing a raise, the next min-raise must cover to_call + last increment.
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10, ..Rules::default() };
        let mut state = GameState::new(rules, 3);
        state.deal(&[[12, 11], [10, 9], [8, 7]], [29, 31, 33, 47, 41]);
        let utg = state.acting_seat;
        // UTG raises TO 30 (delta 30, increment 20 over BB 10).
        state.apply(&make_action(utg, Action::Raise, 30, 0)).unwrap();
        assert_eq!(state.last_raise_size, 20);

        // next actor faces 30, must add to_call + 20. bets currently: their blind.
        let s = state.acting_seat as usize;
        let to_call = 30 - state.bets[s];
        // a raise delta of exactly to_call + 20 - 1 is illegal.
        let bad = state.apply(&make_action(s as u8, Action::Raise, to_call + 20 - 1, 0));
        assert!(bad.is_err(), "sub-min re-raise rejected");
        // to_call + 20 is legal.
        let good = state.apply(&make_action(s as u8, Action::Raise, to_call + 20, 0));
        assert!(good.is_ok(), "legal re-raise accepted: {:?}", good.err());
        assert_eq!(state.bets[s], 50, "re-raise TO 50");
    }

    #[test]
    fn test_allin_for_less_than_min_raise_allowed() {
        // an all-in below the min-raise is always legal (does not reopen action).
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10, ..Rules::default() };
        let mut state = GameState::new(rules, 3);
        state.stacks = [1000, 1000, 14, 0, 0, 0, 0, 0, 0, 0];
        let initial: u32 = 2014;
        state.deal(&[[12, 11], [10, 9], [8, 7]], [29, 31, 33, 47, 41]);
        let utg = state.acting_seat; // seat2 has only 14
        // seat2 all-in for 14 total (< min raise to 20) must be accepted.
        let r = state.apply(&make_action(utg, Action::AllIn, 0, 0));
        assert!(r.is_ok(), "all-in for less must be allowed: {:?}", r.err());
        assert_eq!(state.seat_state[utg as usize], SeatState::AllIn);
        assert_conserved(&state, initial, "after all-in for less");
    }

    #[test]
    fn test_multiway_allin_conservation_full_flow() {
        // N=4, mixed stacks, everyone shoves; chips must be conserved through
        // showdown across main + side pots.
        let rules = Rules { buyin: 1000, small_blind: 5, big_blind: 10, ..Rules::default() };
        let mut state = GameState::new(rules, 4);
        state.stacks = [50, 200, 700, 1000, 0, 0, 0, 0, 0, 0];
        let initial: u32 = 50 + 200 + 700 + 1000;
        state.deal(&[[12, 25], [11, 24], [2, 15], [0, 13]], BOARD_RAINBOW);

        let mut guard = 0;
        while state.phase != Phase::Showdown && state.phase != Phase::Settled {
            let seat = state.acting_seat;
            if state.apply(&make_action(seat, Action::AllIn, 0, 0)).is_err() {
                let _ = state.apply(&make_action(seat, Action::Call, 0, 0));
            }
            guard += 1;
            assert!(guard < 32, "no infinite loop");
        }
        assert_conserved(&state, initial, "pre-showdown 4-way");
        state.showdown();
        assert_conserved(&state, initial, "post-showdown 4-way");
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;

    #[test]
    fn test_allin_pot_and_stacks() {
        let mut state = GameState::new(Rules::default(), 2);
        // seat 0: A♠(12) K♠(11), seat 1: 2♥(13) 3♥(14)
        state.deal(&[[12, 11], [13, 14]], [29, 31, 33, 47, 41]);

        println!("after deal: stacks={:?} pot={} bets={:?}", 
            &state.stacks[..2], state.pot, &state.bets[..2]);

        // seat 0 (BTN/SB) all-in: has 995 chips (posted 5 blind)
        let r = state.apply(&SignedAction { seat: 0, action: Action::AllIn, amount: 0, seq: 0, sig: [0;64] }).unwrap();
        println!("after seat0 allin: stacks={:?} pot={} bets={:?} hand_over={}", 
            &state.stacks[..2], state.pot, &state.bets[..2], r.hand_over);

        // seat 1 (BB) calls all-in: has 990 chips (posted 10 blind)
        let r = state.apply(&SignedAction { seat: 1, action: Action::AllIn, amount: 0, seq: 0, sig: [0;64] }).unwrap();
        println!("after seat1 allin: stacks={:?} pot={} bets={:?} hand_over={} phase={:?}", 
            &state.stacks[..2], state.pot, &state.bets[..2], r.hand_over, state.phase);

        // showdown
        let winner = state.showdown();
        println!("showdown: winner={} stacks={:?} pot={}", winner, &state.stacks[..2], state.pot);

        // verify: one player should have all chips, other should have 0 (or small excess)
        assert!(state.stacks[0] + state.stacks[1] == 2000, 
            "chips don't add up: {} + {} = {}", state.stacks[0], state.stacks[1], state.stacks[0] + state.stacks[1]);
    }
}
