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
    pub seat_state: [SeatState; MAX_SEATS],
    pub cards: [[u8; 2]; MAX_SEATS],
    pub community: [u8; MAX_COMMUNITY],
    pub community_count: u8,
    pub acting_seat: u8,
    pub round_actions: u8,
    pub last_aggressor: u8,
    pub action_count: u32,
    pub last_action_hash: [u8; 32],
    /// rake collected this hand
    pub rake: u32,
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
            seat_state,
            cards: [[0; 2]; MAX_SEATS],
            community: [0; MAX_COMMUNITY],
            community_count: 0,
            acting_seat: 0,
            round_actions: 0,
            last_aggressor: 255,
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

        // post blinds
        let sb = self.sb_seat() as usize;
        let bb = self.bb_seat() as usize;
        let sb_amount = self.rules.small_blind.min(self.stacks[sb]);
        let bb_amount = self.rules.big_blind.min(self.stacks[bb]);
        self.stacks[sb] -= sb_amount;
        self.stacks[bb] -= bb_amount;
        self.bets[sb] = sb_amount;
        self.bets[bb] = bb_amount;
        self.pot = sb_amount + bb_amount;

        self.acting_seat = self.first_preflop();
    }

    /// apply a signed action
    pub fn apply(&mut self, action: &SignedAction) -> Result<ActionResult, &'static str> {
        if action.seat as usize >= self.num_players as usize {
            return Err("invalid seat");
        }
        if action.seat != self.acting_seat {
            return Err("not your turn");
        }
        if !matches!(self.seat_state[action.seat as usize], SeatState::Active) {
            return Err("seat not active");
        }
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
                    self.phase = Phase::Settled;
                    self.button = self.next_active(self.button);
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
                self.pot += actual;
            }

            Action::Bet | Action::Raise => {
                if action.amount == 0 { return Err("bet amount must be > 0"); }
                let amount = action.amount.min(self.stacks[seat]);
                if amount < self.rules.big_blind && amount < self.stacks[seat] {
                    return Err("raise below minimum");
                }
                self.stacks[seat] -= amount;
                self.bets[seat] += amount;
                self.pot += amount;
            }

            Action::AllIn => {
                let amount = self.stacks[seat];
                self.stacks[seat] = 0;
                self.bets[seat] += amount;
                self.pot += amount;
                self.seat_state[seat] = SeatState::AllIn;
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
        let max_bet = self.bets.iter().take(self.num_players as usize).copied().max().unwrap_or(0);
        let all_equal = (0..self.num_players as usize)
            .filter(|&i| self.seat_state[i] == SeatState::Active)
            .all(|i| self.bets[i] == max_bet);
        let was_passive = matches!(last_action.action, Action::Check | Action::Call);
        all_equal && was_passive && self.round_actions >= self.active_count()
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

    /// equalize bets into pot, return excess to bigger bettors
    fn equalize_bets(&mut self) {
        // find effective all-in amounts and create side pots
        // simplified: return excess over the smallest all-in
        let min_allin: u32 = (0..self.num_players as usize)
            .filter(|&i| matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn))
            .map(|i| self.bets[i])
            .min()
            .unwrap_or(0);

        for i in 0..self.num_players as usize {
            if self.bets[i] > min_allin {
                let excess = self.bets[i] - min_allin;
                self.stacks[i] += excess;
                self.pot -= excess;
            }
        }
        self.bets = [0; MAX_SEATS];
    }

    fn advance_phase(&mut self) {
        self.bets = [0; MAX_SEATS];
        self.round_actions = 0;
        self.last_aggressor = 255;
        self.acting_seat = self.first_postflop();
        // skip to next active player who has chips
        if self.seat_state[self.acting_seat as usize] != SeatState::Active
            || self.stacks[self.acting_seat as usize] == 0
        {
            self.acting_seat = self.next_active_in_round(self.acting_seat.wrapping_sub(1));
        }

        match self.phase {
            Phase::Preflop => { self.phase = Phase::Flop; self.community_count = 3; }
            Phase::Flop => { self.phase = Phase::Turn; self.community_count = 4; }
            Phase::Turn => { self.phase = Phase::River; self.community_count = 5; }
            Phase::River => { self.phase = Phase::Showdown; }
            _ => {}
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
        let mut best_score = 0u32;
        let mut winners: [bool; MAX_SEATS] = [false; MAX_SEATS];
        let mut winner_count = 0u8;

        for i in 0..self.num_players as usize {
            if !matches!(self.seat_state[i], SeatState::Active | SeatState::AllIn) { continue; }
            let score = self.best_hand(i);
            if score > best_score {
                best_score = score;
                winners = [false; MAX_SEATS];
                winners[i] = true;
                winner_count = 1;
            } else if score == best_score {
                winners[i] = true;
                winner_count += 1;
            }
        }

        let payout = self.collect_rake();

        if winner_count > 1 {
            // split pot
            let share = payout / winner_count as u32;
            let remainder = payout % winner_count as u32;
            let mut first = true;
            for i in 0..self.num_players as usize {
                if winners[i] {
                    self.stacks[i] += share + if first { remainder } else { 0 };
                    first = false;
                }
            }
        } else {
            for i in 0..self.num_players as usize {
                if winners[i] { self.stacks[i] += payout; }
            }
        }

        self.pot = 0;
        self.phase = Phase::Settled;
        // rotate button to next active player
        self.button = self.next_active(self.button);

        // return first winner seat
        (0..self.num_players as usize).find(|&i| winners[i]).unwrap_or(0) as u8
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

fn eval_5(hand: [u8; 5]) -> u32 {
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

        pub fn new_with_rake(buyin: u32, small_blind: u32, big_blind: u32, rake_bps: u16, rake_cap: u32) -> Self {
            Self {
                state: GameState::new(Rules {
                    buyin, small_blind, big_blind,
                    turn_timeout_blocks: 6, rake_bps, rake_cap,
                }, 2),
            }
        }

        pub fn deal(&mut self, a0: u8, a1: u8, b0: u8, b1: u8, c0: u8, c1: u8, c2: u8, c3: u8, c4: u8) {
            self.state.deal(&[[a0, a1], [b0, b1]], [c0, c1, c2, c3, c4]);
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
        let winner = state.showdown();
        assert_eq!(winner, 0); // flush beats straight
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
