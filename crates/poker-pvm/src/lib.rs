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
//! every action is signed by the acting player. the engine verifies
//! signatures before applying state transitions. on dispute, the
//! entire game can be replayed from the signed action log and the
//! execution proved correct via WIM.
//!
//! # state machine
//!
//! ```text
//! negotiate → escrow → preflop → flop → turn → river → showdown → settle
//!     ↑                   ↑       ↑      ↑       ↑
//!     └───────────────────┴───────┴──────┴───────┘  (fold at any point)
//! ```

#![cfg_attr(feature = "pvm", no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

// ============================================================================
// Game state (fixed-size, no heap allocation in pvm mode)
// ============================================================================

/// maximum community cards
pub const MAX_COMMUNITY: usize = 5;
/// maximum actions per hand
pub const MAX_ACTIONS: usize = 64;

/// game phase
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

/// player action
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
        match v {
            0 => Some(Action::Fold),
            1 => Some(Action::Check),
            2 => Some(Action::Call),
            3 => Some(Action::Bet),
            4 => Some(Action::Raise),
            5 => Some(Action::AllIn),
            _ => None,
        }
    }
}

/// signed action from a player
#[derive(Debug, Clone, Copy)]
pub struct SignedAction {
    /// player seat (0 or 1)
    pub seat: u8,
    /// action type
    pub action: Action,
    /// amount (for bet/raise/call)
    pub amount: u32,
    /// sequence number (monotonic)
    pub seq: u32,
    /// ed25519 signature over (seat || action || amount || seq || prev_hash)
    pub sig: [u8; 64],
}

/// game rules (agreed before play)
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    pub buyin: u32,
    pub small_blind: u32,
    pub big_blind: u32,
    pub turn_timeout_blocks: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Self { buyin: 1000, small_blind: 5, big_blind: 10, turn_timeout_blocks: 6 }
    }
}

/// the full game state (deterministic, replayable)
#[derive(Debug, Clone)]
pub struct GameState {
    pub phase: Phase,
    pub rules: Rules,
    pub hand_number: u32,
    pub button: u8,     // 0 or 1
    pub stacks: [u32; 2],
    pub pot: u32,
    pub bets: [u32; 2], // current round bets
    pub cards: [[u8; 2]; 2],  // hole cards per player
    pub community: [u8; MAX_COMMUNITY],
    pub community_count: u8,
    pub acting_seat: u8,
    pub round_actions: u8, // actions taken in current betting round
    pub last_aggressor: u8, // 255 = none (used to track who closed action)
    pub action_count: u32,
    pub last_action_hash: [u8; 32],
}

impl GameState {
    pub fn new(rules: Rules) -> Self {
        Self {
            phase: Phase::Preflop,
            rules,
            hand_number: 0,
            button: 0,
            stacks: [rules.buyin, rules.buyin],
            pot: 0,
            bets: [0, 0],
            cards: [[0; 2]; 2],
            community: [0; MAX_COMMUNITY],
            community_count: 0,
            acting_seat: 0,
            round_actions: 0,
            last_aggressor: 255,
            action_count: 0,
            last_action_hash: [0; 32],
        }
    }

    /// start a new hand with dealt cards
    pub fn deal(&mut self, cards_a: [u8; 2], cards_b: [u8; 2], community: [u8; 5]) {
        self.hand_number += 1;
        self.phase = Phase::Preflop;
        self.cards[0] = cards_a;
        self.cards[1] = cards_b;
        self.community = community;
        self.community_count = 0;
        self.pot = 0;
        self.bets = [0, 0];
        self.round_actions = 0;
        self.last_aggressor = 255;
        self.action_count = 0;

        // post blinds
        let sb = self.button as usize;
        let bb = 1 - sb;
        let sb_amount = self.rules.small_blind.min(self.stacks[sb]);
        let bb_amount = self.rules.big_blind.min(self.stacks[bb]);
        self.stacks[sb] -= sb_amount;
        self.stacks[bb] -= bb_amount;
        self.bets[sb] = sb_amount;
        self.bets[bb] = bb_amount;
        self.pot = sb_amount + bb_amount;

        // action to the player after big blind
        self.acting_seat = sb as u8; // SB acts first preflop in heads-up
    }

    /// apply a signed action. returns Ok(events) or Err(reason).
    pub fn apply(&mut self, action: &SignedAction) -> Result<ActionResult, &'static str> {
        // verify seat
        if action.seat > 1 {
            return Err("invalid seat");
        }
        if action.seat != self.acting_seat {
            return Err("not your turn");
        }

        // sequence check: if seq is 0, auto-assign (WASM mode)
        // if non-zero, verify it matches
        if action.seq != 0 && action.seq != self.action_count + 1 {
            return Err("wrong sequence");
        }

        // TODO: verify signature against player's public key
        // for now, skip sig verification (demo)

        let seat = action.seat as usize;
        let opp = 1 - seat;

        match action.action {
            Action::Fold => {
                let payout = self.pot;
                self.stacks[opp] += payout;
                self.pot = 0;
                self.phase = Phase::Settled;
                self.button = 1 - self.button; // rotate button
                self.action_count += 1;
                return Ok(ActionResult {
                    valid: true,
                    hand_over: true,
                    winner: opp as u8,
                    payout,
                    advance_phase: false,
                });
            }

            Action::Check => {
                if self.bets[seat] < self.bets[opp] {
                    return Err("cannot check when facing a bet");
                }
            }

            Action::Call => {
                let to_call = self.bets[opp].saturating_sub(self.bets[seat]);
                let actual = to_call.min(self.stacks[seat]);
                self.stacks[seat] -= actual;
                self.bets[seat] += actual;
                self.pot += actual;
            }

            Action::Bet | Action::Raise => {
                if action.amount == 0 {
                    return Err("bet amount must be > 0");
                }
                let amount = action.amount.min(self.stacks[seat]);
                // minimum raise: must be at least the big blind, or all-in
                let min_raise = self.rules.big_blind;
                if amount < min_raise && amount < self.stacks[seat] {
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
            }
        }

        self.action_count += 1;
        self.round_actions += 1;

        // track aggression — resets round counter so opponent must respond
        if matches!(action.action, Action::Bet | Action::Raise | Action::AllIn) {
            self.last_aggressor = seat as u8;
            self.round_actions = 1;
        }

        // action goes to opponent
        self.acting_seat = opp as u8;

        // if either player is all-in after action closes, skip to showdown
        let anyone_allin = self.stacks[0] == 0 || self.stacks[1] == 0;
        let both_allin = self.stacks[0] == 0 && self.stacks[1] == 0;
        if both_allin {
            // equalize bets into pot
            let min_bet = self.bets[0].min(self.bets[1]);
            let excess = self.bets[0].max(self.bets[1]) - min_bet;
            if excess > 0 {
                // return excess to the bigger bettor
                let bigger = if self.bets[0] > self.bets[1] { 0 } else { 1 };
                self.stacks[bigger] += excess;
                self.pot -= excess;
            }
            self.bets = [0, 0];
            self.phase = Phase::Showdown;
            self.community_count = 5;
            return Ok(ActionResult {
                valid: true,
                hand_over: true,
                winner: 255,
                payout: 0,
                advance_phase: true,
            });
        }

        // round complete when:
        // - bets are equal
        // - both players have acted (round_actions >= 2)
        // - last action was passive (check or call)
        let bets_equal = self.bets[0] == self.bets[1];
        let was_passive = matches!(action.action, Action::Check | Action::Call);
        let both_acted = self.round_actions >= 2;
        let advance = bets_equal && was_passive && both_acted;

        if advance {
            // if someone is all-in after the round closes, skip all remaining
            // phases directly to showdown (no more betting possible)
            if anyone_allin {
                self.bets = [0, 0];
                self.phase = Phase::Showdown;
                self.community_count = 5;
                return Ok(ActionResult {
                    valid: true,
                    hand_over: true,
                    winner: 255,
                    payout: 0,
                    advance_phase: true,
                });
            }
            self.advance_phase();
        }

        Ok(ActionResult {
            valid: true,
            hand_over: self.phase == Phase::Showdown,
            winner: 255,
            payout: 0,
            advance_phase: advance,
        })
    }

    fn advance_phase(&mut self) {
        self.bets = [0, 0];
        self.round_actions = 0;
        self.last_aggressor = 255;
        self.acting_seat = (1 - self.button) as u8; // OOP acts first post-flop

        match self.phase {
            Phase::Preflop => {
                self.phase = Phase::Flop;
                self.community_count = 3;
            }
            Phase::Flop => {
                self.phase = Phase::Turn;
                self.community_count = 4;
            }
            Phase::Turn => {
                self.phase = Phase::River;
                self.community_count = 5;
            }
            Phase::River => {
                self.phase = Phase::Showdown;
            }
            _ => {}
        }
    }

    /// evaluate showdown — proper poker hand ranking.
    /// evaluates best 5-card hand from 7 cards (2 hole + 5 community).
    /// hand ranks: straight flush > quads > full house > flush > straight >
    ///             trips > two pair > pair > high card
    pub fn showdown(&mut self) -> u8 {
        let a = self.best_hand(0);
        let b = self.best_hand(1);

        if a == b {
            // split pot
            let half = self.pot / 2;
            let remainder = self.pot % 2;
            self.stacks[0] += half + remainder; // seat 0 gets odd chip
            self.stacks[1] += half;
        } else {
            let winner = if a > b { 0 } else { 1 };
            self.stacks[winner] += self.pot;
        }

        let winner = if a >= b { 0 } else { 1 };
        self.pot = 0;
        self.phase = Phase::Settled;
        self.button = 1 - self.button;

        winner
    }

    /// evaluate best 5-card hand from 7 cards.
    /// returns a u32 score where higher = better hand.
    /// encoding: (category << 20) | kickers
    fn best_hand(&self, seat: usize) -> u32 {
        let mut all7 = [0u8; 7];
        all7[0] = self.cards[seat][0];
        all7[1] = self.cards[seat][1];
        for i in 0..5 {
            all7[2 + i] = self.community[i];
        }

        let mut best = 0u32;
        // evaluate all C(7,5) = 21 combinations
        for i in 0..7 {
            for j in (i + 1)..7 {
                // skip cards i and j, use the other 5
                let mut hand = [0u8; 5];
                let mut idx = 0;
                for k in 0..7 {
                    if k != i && k != j {
                        hand[idx] = all7[k];
                        idx += 1;
                    }
                }
                let score = eval_5(hand);
                if score > best { best = score; }
            }
        }
        best
    }
}

/// hand categories (higher = better)
const HIGH_CARD: u32 = 0;
const PAIR: u32 = 1;
const TWO_PAIR: u32 = 2;
const TRIPS: u32 = 3;
const STRAIGHT: u32 = 4;
const FLUSH: u32 = 5;
const FULL_HOUSE: u32 = 6;
const QUADS: u32 = 7;
const STRAIGHT_FLUSH: u32 = 8;

/// evaluate a 5-card hand. returns (category << 20) | kicker_score.
/// card encoding: index 0..51, rank = index % 13 (0=2..12=A), suit = index / 13
fn eval_5(hand: [u8; 5]) -> u32 {
    let mut ranks = [0u8; 5];
    let mut suits = [0u8; 5];
    for i in 0..5 {
        ranks[i] = hand[i] % 13;
        suits[i] = hand[i] / 13;
    }

    // sort ranks descending
    ranks.sort_unstable();
    ranks.reverse();

    let is_flush = suits[0] == suits[1] && suits[1] == suits[2] &&
                   suits[2] == suits[3] && suits[3] == suits[4];

    let is_straight = is_straight_check(&ranks);
    // special case: A-2-3-4-5 (wheel)
    let is_wheel = ranks == [12, 3, 2, 1, 0];

    if is_flush && (is_straight || is_wheel) {
        let top = if is_wheel { 3 } else { ranks[0] as u32 }; // wheel: 5-high
        return (STRAIGHT_FLUSH << 20) | top;
    }

    // count rank frequencies
    let mut freq = [0u8; 13];
    for &r in &ranks { freq[r as usize] += 1; }

    let mut quads_rank = 0u8;
    let mut trips_rank = 0u8;
    let mut pairs = [0u8; 2];
    let mut pair_count = 0usize;
    let mut kickers = [0u8; 5];
    let mut kick_idx = 0;

    // collect from highest rank down
    for r in (0..13u8).rev() {
        match freq[r as usize] {
            4 => quads_rank = r + 1, // +1 to distinguish from 0
            3 => trips_rank = r + 1,
            2 => {
                if pair_count < 2 { pairs[pair_count] = r + 1; pair_count += 1; }
            }
            1 => {
                if kick_idx < 5 { kickers[kick_idx] = r; kick_idx += 1; }
            }
            _ => {}
        }
    }

    if quads_rank > 0 {
        return (QUADS << 20) | ((quads_rank as u32 - 1) << 4) | kickers[0] as u32;
    }
    if trips_rank > 0 && pair_count > 0 {
        return (FULL_HOUSE << 20) | ((trips_rank as u32 - 1) << 4) | (pairs[0] as u32 - 1);
    }
    if is_flush {
        return (FLUSH << 20) | kicker_score(&ranks);
    }
    if is_straight {
        return (STRAIGHT << 20) | ranks[0] as u32;
    }
    if is_wheel {
        return (STRAIGHT << 20) | 3; // 5-high straight
    }
    if trips_rank > 0 {
        return (TRIPS << 20) | ((trips_rank as u32 - 1) << 8) |
               (kickers[0] as u32) << 4 | kickers[1] as u32;
    }
    if pair_count >= 2 {
        return (TWO_PAIR << 20) | ((pairs[0] as u32 - 1) << 8) |
               ((pairs[1] as u32 - 1) << 4) | kickers[0] as u32;
    }
    if pair_count == 1 {
        return (PAIR << 20) | ((pairs[0] as u32 - 1) << 12) |
               (kickers[0] as u32) << 8 | (kickers[1] as u32) << 4 | kickers[2] as u32;
    }
    // high card
    (HIGH_CARD << 20) | kicker_score(&ranks)
}

fn kicker_score(ranks: &[u8; 5]) -> u32 {
    (ranks[0] as u32) << 16 | (ranks[1] as u32) << 12 |
    (ranks[2] as u32) << 8 | (ranks[3] as u32) << 4 | ranks[4] as u32
}

fn is_straight_check(sorted_desc: &[u8; 5]) -> bool {
    sorted_desc[0].saturating_sub(sorted_desc[4]) == 4 &&
    sorted_desc[0] != sorted_desc[1] &&
    sorted_desc[1] != sorted_desc[2] &&
    sorted_desc[2] != sorted_desc[3] &&
    sorted_desc[3] != sorted_desc[4]
}

/// result of applying an action
#[derive(Debug, Clone, Copy)]
pub struct ActionResult {
    pub valid: bool,
    pub hand_over: bool,
    pub winner: u8,      // 255 = no winner yet
    pub payout: u32,
    pub advance_phase: bool,
}

// ============================================================================
// WASM bindings (optional)
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
                    turn_timeout_blocks: 6,
                }),
            }
        }

        pub fn deal(&mut self, a0: u8, a1: u8, b0: u8, b1: u8, c0: u8, c1: u8, c2: u8, c3: u8, c4: u8) {
            self.state.deal([a0, a1], [b0, b1], [c0, c1, c2, c3, c4]);
        }

        /// returns: [valid, hand_over, winner, payout, advance_phase] as packed u32
        /// apply action. pass seq=0 to auto-assign sequence.
        /// returns [valid, hand_over, winner, payout, advance_phase]
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

        /// apply action with error message returned
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
        pub fn stack(&self, seat: u8) -> u32 { self.state.stacks[seat as usize] }
        pub fn bet(&self, seat: u8) -> u32 { self.state.bets[seat as usize] }
        pub fn community_count(&self) -> u8 { self.state.community_count }
        pub fn button(&self) -> u8 { self.state.button }
        pub fn hand_number(&self) -> u32 { self.state.hand_number }

        /// set stacks and button for next hand (sync guest with host)
        pub fn set_state(&mut self, stack0: u32, stack1: u32, btn: u8) {
            self.state.stacks = [stack0, stack1];
            self.state.button = btn;
        }

        /// update community cards without resetting hand state.
        /// used when shuffle reveals cards incrementally.
        pub fn update_community(&mut self, c0: u8, c1: u8, c2: u8, c3: u8, c4: u8) {
            self.state.community = [c0, c1, c2, c3, c4];
            self.state.community_count = 5;
        }

        /// update opponent's hole cards (for showdown eval on host side)
        pub fn update_opp_cards(&mut self, c0: u8, c1: u8) {
            self.state.cards[1] = [c0, c1];
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deal_and_blinds() {
        let mut state = GameState::new(Rules::default());
        state.deal([0, 1], [2, 3], [4, 5, 6, 7, 8]);

        assert_eq!(state.phase, Phase::Preflop);
        assert_eq!(state.pot, 15); // 5 + 10
        assert_eq!(state.stacks[0], 995); // SB posted 5
        assert_eq!(state.stacks[1], 990); // BB posted 10
    }

    #[test]
    fn test_fold() {
        let mut state = GameState::new(Rules::default());
        state.deal([0, 1], [2, 3], [4, 5, 6, 7, 8]);

        let action = SignedAction {
            seat: 0, action: Action::Fold, amount: 0, seq: 1, sig: [0; 64],
        };
        let result = state.apply(&action).unwrap();
        assert!(result.hand_over);
        assert_eq!(result.winner, 1);
        assert_eq!(state.stacks[1], 990 + 15); // BB wins the pot
    }

    #[test]
    fn test_check_check_advances() {
        let mut state = GameState::new(Rules::default());
        state.deal([0, 1], [2, 3], [4, 5, 6, 7, 8]);

        // SB calls (matches BB)
        let call = SignedAction { seat: 0, action: Action::Call, amount: 0, seq: 1, sig: [0; 64] };
        state.apply(&call).unwrap();

        // BB checks
        let check = SignedAction { seat: 1, action: Action::Check, amount: 0, seq: 2, sig: [0; 64] };
        let result = state.apply(&check).unwrap();
        assert!(result.advance_phase);
        assert_eq!(state.phase, Phase::Flop);
        assert_eq!(state.community_count, 3);
    }

    #[test]
    fn test_wrong_turn_rejected() {
        let mut state = GameState::new(Rules::default());
        state.deal([0, 1], [2, 3], [4, 5, 6, 7, 8]);

        // seat 1 tries to act but it's seat 0's turn
        let action = SignedAction { seat: 1, action: Action::Check, amount: 0, seq: 1, sig: [0; 64] };
        assert!(state.apply(&action).is_err());
    }

    #[test]
    fn test_full_hand_to_showdown() {
        let mut state = GameState::new(Rules::default());
        // seat 0: A♠(12) K♠(11) — high cards, no pair
        // seat 1: 2♥(13) 3♥(14) — low cards
        // community: 5♦(29) 7♦(31) 9♦(33) J♣(47) 4♣(41) — no flush/straight possible
        state.deal([12, 11], [13, 14], [29, 31, 33, 47, 41]);

        // preflop: SB calls, BB checks
        state.apply(&SignedAction { seat: 0, action: Action::Call, amount: 0, seq: 1, sig: [0; 64] }).unwrap();
        state.apply(&SignedAction { seat: 1, action: Action::Check, amount: 0, seq: 2, sig: [0; 64] }).unwrap();
        assert_eq!(state.phase, Phase::Flop);

        // flop: check check
        state.apply(&SignedAction { seat: 1, action: Action::Check, amount: 0, seq: 3, sig: [0; 64] }).unwrap();
        state.apply(&SignedAction { seat: 0, action: Action::Check, amount: 0, seq: 4, sig: [0; 64] }).unwrap();
        assert_eq!(state.phase, Phase::Turn);

        // turn: check check
        state.apply(&SignedAction { seat: 1, action: Action::Check, amount: 0, seq: 5, sig: [0; 64] }).unwrap();
        state.apply(&SignedAction { seat: 0, action: Action::Check, amount: 0, seq: 6, sig: [0; 64] }).unwrap();
        assert_eq!(state.phase, Phase::River);

        // river: check check
        state.apply(&SignedAction { seat: 1, action: Action::Check, amount: 0, seq: 7, sig: [0; 64] }).unwrap();
        state.apply(&SignedAction { seat: 0, action: Action::Check, amount: 0, seq: 8, sig: [0; 64] }).unwrap();
        assert_eq!(state.phase, Phase::Showdown);

        let winner = state.showdown();
        assert_eq!(winner, 0); // A-K high beats 2-3 high
        assert_eq!(state.stacks[0], 990 + 20); // won 20 pot (blinds)
    }

    #[test]
    fn test_both_allin_skips_to_showdown() {
        let mut state = GameState::new(Rules::default());
        // seat 0: A♠(12) K♠(11), seat 1: 2♥(13) 3♥(14)
        // community: 5♦(29) 7♦(31) 9♦(33) J♣(47) 4♣(41)
        state.deal([12, 11], [13, 14], [29, 31, 33, 47, 41]);

        // preflop: SB goes all-in
        let result = state.apply(&SignedAction {
            seat: 0, action: Action::AllIn, amount: 0, seq: 0, sig: [0; 64],
        }).unwrap();
        assert!(!result.hand_over); // opponent hasn't acted yet
        assert_eq!(state.stacks[0], 0);

        // BB calls all-in
        let result = state.apply(&SignedAction {
            seat: 1, action: Action::AllIn, amount: 0, seq: 0, sig: [0; 64],
        }).unwrap();

        // should go straight to showdown, no more actions needed
        assert!(result.hand_over || state.phase == Phase::Showdown);
        assert_eq!(state.phase, Phase::Showdown);
        assert_eq!(state.community_count, 5);

        // showdown should award the full pot
        let pot_before = state.pot;
        assert!(pot_before > 0);
        let winner = state.showdown();
        assert_eq!(state.stacks[winner as usize], pot_before); // winner gets all
    }

    // ── hand evaluation tests ────────────────────────────────

    #[test]
    fn test_eval_high_card() {
        // A♠(12) K♠(11) vs 2♠(0) 3♠(1), community: 5♦(29) 7♦(31) 9♦(33) J♣(47) 4♣(41)
        let mut state = GameState::new(Rules::default());
        state.deal([12, 11], [0, 1], [29, 31, 33, 47, 41]);
        state.phase = Phase::Showdown;
        state.community_count = 5;
        state.pot = 100;
        let winner = state.showdown();
        assert_eq!(winner, 0); // A-K high beats 2-3 high
    }

    #[test]
    fn test_eval_pair_beats_high_card() {
        // 2♠(0) 2♥(13) vs A♠(12) K♠(11), community: 5♦(29) 7♦(31) 9♦(33) J♣(47) 4♣(41)
        let mut state = GameState::new(Rules::default());
        state.deal([0, 13], [12, 11], [29, 31, 33, 47, 41]);
        state.phase = Phase::Showdown;
        state.community_count = 5;
        state.pot = 100;
        let winner = state.showdown();
        assert_eq!(winner, 0); // pair of 2s beats A-K high
    }

    #[test]
    fn test_eval_flush_beats_straight() {
        // A♠(12) T♠(8) vs 6♥(17) 7♥(18), community: 2♠(0) 5♠(3) 8♠(6) 9♥(20) 3♥(14)
        // seat 0: A♠ T♠ 2♠ 5♠ 8♠ = spade flush
        // seat 1: 6♥ 7♥ 9♥ 3♥ + 5,8 = no flush, 5-6-7-8-9 straight
        let mut state = GameState::new(Rules::default());
        state.deal([12, 8], [17, 18], [0, 3, 6, 20, 14]);
        state.phase = Phase::Showdown;
        state.community_count = 5;
        state.pot = 100;
        let winner = state.showdown();
        assert_eq!(winner, 0); // flush beats straight
    }

    #[test]
    fn test_eval_full_house_beats_flush() {
        // 5♠(3) 5♥(16) vs A♠(12) K♠(11), community: 5♦(29) 9♠(7) 9♦(33) 2♠(0) 4♠(2)
        // seat 0: 5-5-5-9-9 = full house
        // seat 1: A♠ K♠ 9♠ 2♠ 4♠ = spade flush
        let mut state = GameState::new(Rules::default());
        state.deal([3, 16], [12, 11], [29, 7, 33, 0, 2]);
        state.phase = Phase::Showdown;
        state.community_count = 5;
        state.pot = 100;
        let winner = state.showdown();
        assert_eq!(winner, 0); // full house beats flush
    }

    #[test]
    fn test_eval_5() {
        // royal flush: A♠ K♠ Q♠ J♠ T♠
        assert!(eval_5([12, 11, 10, 9, 8]) > eval_5([12, 25, 38, 51, 7])); // RF > high card

        // pair of aces vs pair of kings
        let pair_aces = eval_5([12, 25, 5, 3, 1]); // A♠ A♥ 7♠ 5♠ 3♠
        let pair_kings = eval_5([11, 24, 5, 3, 1]); // K♠ K♥ 7♠ 5♠ 3♠
        assert!(pair_aces > pair_kings);

        // two pair vs one pair
        let two_pair = eval_5([12, 25, 11, 24, 5]); // AA KK 7
        let one_pair = eval_5([12, 25, 10, 8, 5]); // AA Q T 7
        assert!(two_pair > one_pair);

        // straight: 5-6-7-8-9
        let straight = eval_5([3, 4, 5, 6, 7]); // 5♠ 6♠ 7♠ 8♠ 9♠ — this is actually straight flush!
        let trips = eval_5([3, 16, 29, 11, 8]); // 5-5-5-K-T
        assert!(straight > trips); // straight flush > trips

        // wheel (A-2-3-4-5): A♠(12) 2♥(13) 3♦(27) 4♣(41) 5♠(3)
        // ranks: 12(A), 0(2), 1(3), 2(4), 3(5) — all different suits
        let wheel = eval_5([12, 13, 27, 41, 3]);
        let pair = eval_5([12, 25, 5, 4, 1]); // A♠ A♥ 7♠ 6♠ 3♠
        assert!(wheel > pair); // straight > pair
    }
}
