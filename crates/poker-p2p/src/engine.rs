//! engine - deterministic poker game state machine
//!
//! pure rust, no bevy, no async. usable by both P2P host and offline client.
//! same (rules, stacks, button, deck, actions) → same state.

use crate::protocol::{ActionType, TableRules};
use zk_shuffle::poker::{Card, showdown_holdem, ShowdownResult};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GameEngine {
    pub rules: TableRules,
    /// chips per seat (0 = empty seat)
    stacks: Vec<u64>,
    hand: Option<HandState>,
}

#[derive(Clone, Debug)]
pub struct HandState {
    pub hand_number: u64,
    pub phase: Phase,
    pub seats: Vec<SeatState>,
    pub community_cards: Vec<Card>,
    pub pots: Vec<Pot>,
    pub betting: BettingState,
    pub button: u8,
    /// full deck passed in at hand start (indices 0..51)
    deck: Vec<Card>,
    /// next index to deal from deck
    deck_idx: usize,
}

#[derive(Clone, Debug)]
pub struct SeatState {
    pub seat: u8,
    pub chips: u64,
    pub hole_cards: Option<[Card; 2]>,
    pub status: SeatStatus,
    pub bet_this_round: u64,
    pub total_bet: u64,
    pub has_acted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatStatus {
    Active,
    Folded,
    AllIn,
    SittingOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preflop,
    Flop,
    Turn,
    River,
    Showdown,
    Complete,
}

#[derive(Clone, Debug)]
pub struct BettingState {
    pub current_bet: u64,
    pub min_raise: u64,
    pub last_raise_size: u64,
    /// seat index (not seat number) whose turn it is
    pub action_on: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Pot {
    pub amount: u64,
    /// seat indices eligible for this pot
    pub eligible: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum EngineEvent {
    HandStarted { hand_number: u64, button: u8 },
    BlindsPosted { small_blind: (u8, u64), big_blind: (u8, u64) },
    HoleCardsDealt { seat: u8, cards: [Card; 2] },
    ActionRequired { seat: u8, valid_actions: Vec<ValidAction> },
    PlayerActed { seat: u8, action: ActionType, new_stack: u64 },
    PhaseChanged { phase: Phase, new_cards: Vec<Card> },
    PotUpdated { pots: Vec<Pot> },
    Showdown { result: ShowdownResult },
    PotAwarded { seat: u8, amount: u64, pot_index: usize },
    HandComplete { stacks: Vec<u64> },
}

#[derive(Clone, Debug)]
pub struct ValidAction {
    pub kind: ActionKind,
    pub min_amount: u64,
    pub max_amount: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Fold,
    Check,
    Call,
    Bet,
    Raise,
    AllIn,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("not your turn")]
    NotYourTurn,
    #[error("invalid action")]
    InvalidAction,
    #[error("bet too small (min {min})")]
    BetTooSmall { min: u64 },
    #[error("cannot check when there is an outstanding bet")]
    CannotCheck,
    #[error("no hand in progress")]
    NoHandInProgress,
    #[error("hand already in progress")]
    HandAlreadyInProgress,
    #[error("not enough players (need at least 2)")]
    NotEnoughPlayers,
    #[error("invalid seat {0}")]
    InvalidSeat(u8),
    #[error("seat {0} is occupied")]
    SeatOccupied(u8),
    #[error("seat {0} is empty")]
    SeatEmpty(u8),
    #[error("buy-in {amount} not in range [{min}, {max}]")]
    InvalidBuyIn { amount: u64, min: u64, max: u64 },
    #[error("deck too small")]
    DeckTooSmall,
    #[error("player is {0:?}, cannot act")]
    PlayerCannotAct(SeatStatus),
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl GameEngine {
    pub fn new(rules: TableRules, num_seats: u8) -> Result<Self, EngineError> {
        Ok(Self {
            stacks: vec![0; num_seats as usize],
            hand: None,
            rules,
        })
    }

    pub fn seat_player(&mut self, seat: u8, buy_in: u64) -> Result<(), EngineError> {
        let idx = seat as usize;
        if idx >= self.stacks.len() {
            return Err(EngineError::InvalidSeat(seat));
        }
        if self.stacks[idx] > 0 {
            return Err(EngineError::SeatOccupied(seat));
        }
        let min = self.rules.min_buy_in as u64;
        let max = if self.rules.max_buy_in == 0 { u64::MAX } else { self.rules.max_buy_in as u64 };
        if buy_in < min || buy_in > max {
            return Err(EngineError::InvalidBuyIn { amount: buy_in, min, max });
        }
        self.stacks[idx] = buy_in;
        Ok(())
    }

    pub fn unseat_player(&mut self, seat: u8) -> Result<u64, EngineError> {
        let idx = seat as usize;
        if idx >= self.stacks.len() {
            return Err(EngineError::InvalidSeat(seat));
        }
        if self.hand.is_some() {
            // can't leave mid-hand — would need to fold first
            return Err(EngineError::HandAlreadyInProgress);
        }
        let chips = self.stacks[idx];
        self.stacks[idx] = 0;
        Ok(chips)
    }

    pub fn stacks(&self) -> &[u64] {
        &self.stacks
    }

    pub fn hand_state(&self) -> Option<&HandState> {
        self.hand.as_ref()
    }

    /// start a new hand. `deck` must have enough cards for hole cards + 5 community.
    pub fn new_hand(&mut self, button: u8, deck: &[Card]) -> Result<Vec<EngineEvent>, EngineError> {
        if self.hand.is_some() {
            return Err(EngineError::HandAlreadyInProgress);
        }

        // collect active seats (have chips)
        let active: Vec<u8> = self.stacks.iter().enumerate()
            .filter(|(_, &s)| s > 0)
            .map(|(i, _)| i as u8)
            .collect();
        if active.len() < 2 {
            return Err(EngineError::NotEnoughPlayers);
        }

        let needed = active.len() * 2 + 5;
        if deck.len() < needed {
            return Err(EngineError::DeckTooSmall);
        }

        if !active.contains(&button) {
            return Err(EngineError::InvalidSeat(button));
        }
        let button_idx = button;

        // build seat states
        let seats: Vec<SeatState> = (0..self.stacks.len() as u8).map(|i| {
            let chips = self.stacks[i as usize];
            SeatState {
                seat: i,
                chips,
                hole_cards: None,
                status: if chips > 0 { SeatStatus::Active } else { SeatStatus::SittingOut },
                bet_this_round: 0,
                total_bet: 0,
                has_acted: false,
            }
        }).collect();

        let hand_number = 1; // caller can set via returned state
        let mut hand = HandState {
            hand_number,
            phase: Phase::Preflop,
            seats,
            community_cards: Vec::new(),
            pots: Vec::new(),
            betting: BettingState {
                current_bet: 0,
                min_raise: 0,
                last_raise_size: 0,
                action_on: None,
            },
            button: button_idx,
            deck: deck.to_vec(),
            deck_idx: 0,
        };

        let mut events = Vec::new();
        events.push(EngineEvent::HandStarted { hand_number, button: button_idx });

        // post blinds
        let blind_events = post_blinds(&mut hand, &self.rules);
        events.extend(blind_events);

        // deal hole cards
        let active_seats: Vec<u8> = hand.seats.iter()
            .filter(|s| s.status == SeatStatus::Active || s.status == SeatStatus::AllIn)
            .map(|s| s.seat)
            .collect();
        for &seat in &active_seats {
            let c1 = hand.deck[hand.deck_idx];
            let c2 = hand.deck[hand.deck_idx + 1];
            hand.deck_idx += 2;
            hand.seats[seat as usize].hole_cards = Some([c1, c2]);
            events.push(EngineEvent::HoleCardsDealt { seat, cards: [c1, c2] });
        }

        // set first to act preflop (UTG, or SB in heads-up)
        set_first_to_act_preflop(&mut hand);

        if let Some(idx) = hand.betting.action_on {
            let seat = hand.seats[idx].seat;
            let va = compute_valid_actions(&hand, idx);
            events.push(EngineEvent::ActionRequired { seat, valid_actions: va });
        }

        self.hand = Some(hand);
        Ok(events)
    }

    pub fn apply_action(&mut self, seat: u8, action: ActionType) -> Result<Vec<EngineEvent>, EngineError> {
        let hand = self.hand.as_mut().ok_or(EngineError::NoHandInProgress)?;

        let action_idx = hand.betting.action_on.ok_or(EngineError::NotYourTurn)?;
        if hand.seats[action_idx].seat != seat {
            return Err(EngineError::NotYourTurn);
        }

        let seat_state = &hand.seats[action_idx];
        if seat_state.status != SeatStatus::Active {
            return Err(EngineError::PlayerCannotAct(seat_state.status));
        }

        // validate and apply
        let mut events = Vec::new();
        let valid = compute_valid_actions(hand, action_idx);

        match &action {
            ActionType::Fold => {
                if !valid.iter().any(|v| v.kind == ActionKind::Fold) {
                    return Err(EngineError::InvalidAction);
                }
                hand.seats[action_idx].status = SeatStatus::Folded;
                hand.seats[action_idx].has_acted = true;
            }
            ActionType::Check => {
                if !valid.iter().any(|v| v.kind == ActionKind::Check) {
                    return Err(EngineError::CannotCheck);
                }
                hand.seats[action_idx].has_acted = true;
            }
            ActionType::Call => {
                let call_action = valid.iter().find(|v| v.kind == ActionKind::Call)
                    .ok_or(EngineError::InvalidAction)?;
                let call_amount = call_action.min_amount;
                apply_bet(hand, action_idx, call_amount);
                hand.seats[action_idx].has_acted = true;
            }
            ActionType::Bet(amount) => {
                let amount = *amount as u64;
                let bet_action = valid.iter().find(|v| v.kind == ActionKind::Bet)
                    .ok_or(EngineError::InvalidAction)?;
                if amount < bet_action.min_amount || amount > bet_action.max_amount {
                    return Err(EngineError::BetTooSmall { min: bet_action.min_amount });
                }
                let raise_size = amount; // first bet, raise size = bet amount
                hand.betting.last_raise_size = raise_size;
                hand.betting.current_bet = amount;
                hand.betting.min_raise = amount + raise_size;
                apply_bet(hand, action_idx, amount);
                hand.seats[action_idx].has_acted = true;
                // reset has_acted for others
                reset_acted_except(hand, action_idx);
            }
            ActionType::Raise(amount) => {
                let amount = *amount as u64;
                let raise_action = valid.iter().find(|v| v.kind == ActionKind::Raise)
                    .ok_or(EngineError::InvalidAction)?;
                if amount < raise_action.min_amount || amount > raise_action.max_amount {
                    return Err(EngineError::BetTooSmall { min: raise_action.min_amount });
                }
                let raise_size = amount - hand.betting.current_bet;
                hand.betting.last_raise_size = raise_size;
                hand.betting.current_bet = amount;
                hand.betting.min_raise = amount + raise_size;
                let needed = amount - hand.seats[action_idx].bet_this_round;
                apply_bet(hand, action_idx, needed);
                hand.seats[action_idx].has_acted = true;
                reset_acted_except(hand, action_idx);
            }
            ActionType::AllIn => {
                if !valid.iter().any(|v| v.kind == ActionKind::AllIn) {
                    return Err(EngineError::InvalidAction);
                }
                let remaining = hand.seats[action_idx].chips;
                let total_bet = hand.seats[action_idx].bet_this_round + remaining;
                // if all-in is a raise, update betting state
                if total_bet > hand.betting.current_bet {
                    let raise_size = total_bet - hand.betting.current_bet;
                    // only update min_raise if this is a full raise
                    if raise_size >= hand.betting.last_raise_size {
                        hand.betting.last_raise_size = raise_size;
                        hand.betting.min_raise = total_bet + raise_size;
                    }
                    hand.betting.current_bet = total_bet;
                    reset_acted_except(hand, action_idx);
                }
                apply_bet(hand, action_idx, remaining);
                hand.seats[action_idx].status = SeatStatus::AllIn;
                hand.seats[action_idx].has_acted = true;
            }
        }

        events.push(EngineEvent::PlayerActed {
            seat,
            action: action.clone(),
            new_stack: hand.seats[action_idx].chips,
        });

        // check if only one player remains (everyone else folded)
        let active_count = hand.seats.iter()
            .filter(|s| s.status == SeatStatus::Active || s.status == SeatStatus::AllIn)
            .count();

        if active_count <= 1 {
            // hand over — award pot to last player
            recalculate_pots(hand);
            events.push(EngineEvent::PotUpdated { pots: hand.pots.clone() });
            let winner = hand.seats.iter()
                .find(|s| s.status == SeatStatus::Active || s.status == SeatStatus::AllIn)
                .map(|s| s.seat)
                .unwrap_or(0);
            let mut total_awarded = 0u64;
            for (i, pot) in hand.pots.iter().enumerate() {
                events.push(EngineEvent::PotAwarded { seat: winner, amount: pot.amount, pot_index: i });
                total_awarded += pot.amount;
            }
            hand.seats[winner as usize].chips += total_awarded;
            // write back stacks
            for s in &hand.seats {
                self.stacks[s.seat as usize] = s.chips;
            }
            hand.phase = Phase::Complete;
            events.push(EngineEvent::HandComplete { stacks: self.stacks.clone() });
            self.hand = None;
            return Ok(events);
        }

        // check if betting round is complete
        if is_betting_round_complete(hand) {
            recalculate_pots(hand);
            events.push(EngineEvent::PotUpdated { pots: hand.pots.clone() });

            // check if we can still have action (more than 1 active non-allin)
            let can_act = hand.seats.iter()
                .filter(|s| s.status == SeatStatus::Active)
                .count();

            let next_phase = match hand.phase {
                Phase::Preflop => Phase::Flop,
                Phase::Flop => Phase::Turn,
                Phase::Turn => Phase::River,
                Phase::River => Phase::Showdown,
                _ => Phase::Complete,
            };

            if next_phase == Phase::Showdown || (can_act <= 1 && next_phase != Phase::Showdown) {
                // run out remaining community cards if needed, then showdown
                let cards_needed = 5 - hand.community_cards.len();
                if cards_needed > 0 {
                    // deal remaining cards in proper phases
                    let mut phase = hand.phase;
                    while hand.community_cards.len() < 5 {
                        phase = match phase {
                            Phase::Preflop => Phase::Flop,
                            Phase::Flop => Phase::Turn,
                            Phase::Turn => Phase::River,
                            _ => break,
                        };
                        let new_cards = deal_community(hand, phase);
                        hand.phase = phase;
                        events.push(EngineEvent::PhaseChanged { phase, new_cards });
                    }
                }
                hand.phase = Phase::Showdown;
                let showdown_events = run_showdown(hand);
                events.extend(showdown_events);
                // write back stacks
                for s in &hand.seats {
                    self.stacks[s.seat as usize] = s.chips;
                }
                hand.phase = Phase::Complete;
                events.push(EngineEvent::HandComplete { stacks: self.stacks.clone() });
                self.hand = None;
            } else {
                // advance to next phase
                let new_cards = deal_community(hand, next_phase);
                hand.phase = next_phase;
                // reset for new betting round
                for s in hand.seats.iter_mut() {
                    if s.status == SeatStatus::Active {
                        s.bet_this_round = 0;
                        s.has_acted = false;
                    }
                }
                hand.betting.current_bet = 0;
                hand.betting.min_raise = self.rules.big_blind as u64;
                hand.betting.last_raise_size = self.rules.big_blind as u64;

                events.push(EngineEvent::PhaseChanged { phase: next_phase, new_cards });

                set_first_to_act_postflop(hand);

                if let Some(idx) = hand.betting.action_on {
                    let seat = hand.seats[idx].seat;
                    let va = compute_valid_actions(hand, idx);
                    events.push(EngineEvent::ActionRequired { seat, valid_actions: va });
                } else {
                    // no one can act — shouldn't happen if can_act > 1
                }
            }
        } else {
            // advance to next player
            advance_action(hand);
            if let Some(idx) = hand.betting.action_on {
                let seat = hand.seats[idx].seat;
                let va = compute_valid_actions(hand, idx);
                events.push(EngineEvent::ActionRequired { seat, valid_actions: va });
            }
        }

        Ok(events)
    }

    /// auto-fold for timeout
    pub fn timeout(&mut self) -> Result<Vec<EngineEvent>, EngineError> {
        let hand = self.hand.as_ref().ok_or(EngineError::NoHandInProgress)?;
        let action_idx = hand.betting.action_on.ok_or(EngineError::NoHandInProgress)?;
        let seat = hand.seats[action_idx].seat;
        self.apply_action(seat, ActionType::Fold)
    }

    pub fn valid_actions(&self) -> Result<Vec<ValidAction>, EngineError> {
        let hand = self.hand.as_ref().ok_or(EngineError::NoHandInProgress)?;
        let idx = hand.betting.action_on.ok_or(EngineError::NoHandInProgress)?;
        Ok(compute_valid_actions(hand, idx))
    }

    /// compute a hash of the current engine state for determinism verification
    pub fn state_hash(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        // hash stacks
        for &s in &self.stacks {
            hasher.update(&s.to_le_bytes());
        }
        // hash hand state if present
        if let Some(hand) = &self.hand {
            hasher.update(&hand.hand_number.to_le_bytes());
            hasher.update(&[hand.phase as u8]);
            hasher.update(&[hand.button]);
            for s in &hand.seats {
                hasher.update(&[s.seat, s.status as u8]);
                hasher.update(&s.chips.to_le_bytes());
                hasher.update(&s.bet_this_round.to_le_bytes());
                hasher.update(&s.total_bet.to_le_bytes());
                hasher.update(&[s.has_acted as u8]);
            }
            hasher.update(&hand.betting.current_bet.to_le_bytes());
            hasher.update(&hand.betting.min_raise.to_le_bytes());
            hasher.update(&hand.betting.last_raise_size.to_le_bytes());
            // include action_on — two states with different players to act must differ
            let action_on_byte = hand.betting.action_on.map(|i| i as u8 + 1).unwrap_or(0);
            hasher.update(&[action_on_byte]);
            for pot in &hand.pots {
                hasher.update(&pot.amount.to_le_bytes());
            }
        }
        *hasher.finalize().as_bytes()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// find active seat indices in clockwise order starting after `start_seat`
fn active_seats_from(hand: &HandState, start_seat: u8) -> Vec<usize> {
    let n = hand.seats.len();
    let mut result = Vec::new();
    for offset in 1..=n {
        let idx = ((start_seat as usize) + offset) % n;
        let s = &hand.seats[idx];
        if s.status == SeatStatus::Active || s.status == SeatStatus::AllIn {
            result.push(idx);
        }
    }
    result
}

/// get active (can-still-bet) seat indices from a starting seat
fn actable_seats_from(hand: &HandState, start_seat: u8) -> Vec<usize> {
    let n = hand.seats.len();
    let mut result = Vec::new();
    for offset in 1..=n {
        let idx = ((start_seat as usize) + offset) % n;
        if hand.seats[idx].status == SeatStatus::Active {
            result.push(idx);
        }
    }
    result
}

fn post_blinds(hand: &mut HandState, rules: &TableRules) -> Vec<EngineEvent> {
    let mut events = Vec::new();
    let sb_amount = rules.small_blind as u64;
    let bb_amount = rules.big_blind as u64;

    let active = active_seats_from(hand, hand.button);
    if active.len() < 2 { return events; }

    let is_heads_up = active.len() == 2;

    let (sb_idx, bb_idx) = if is_heads_up {
        // heads-up: button = SB
        let sb = hand.seats.iter().position(|s| s.seat == hand.button).unwrap();
        let bb = active.iter().find(|&&i| i != sb).copied().unwrap();
        (sb, bb)
    } else {
        // 3+: first after button = SB, second = BB
        (active[0], active[1])
    };

    // post small blind
    let sb_actual = sb_amount.min(hand.seats[sb_idx].chips);
    apply_bet(hand, sb_idx, sb_actual);
    if hand.seats[sb_idx].chips == 0 {
        hand.seats[sb_idx].status = SeatStatus::AllIn;
    }
    let sb_seat = hand.seats[sb_idx].seat;

    // post big blind
    let bb_actual = bb_amount.min(hand.seats[bb_idx].chips);
    apply_bet(hand, bb_idx, bb_actual);
    if hand.seats[bb_idx].chips == 0 {
        hand.seats[bb_idx].status = SeatStatus::AllIn;
    }
    let bb_seat = hand.seats[bb_idx].seat;

    // set betting state
    hand.betting.current_bet = bb_amount;
    hand.betting.min_raise = bb_amount * 2;
    hand.betting.last_raise_size = bb_amount;

    events.push(EngineEvent::BlindsPosted {
        small_blind: (sb_seat, sb_actual),
        big_blind: (bb_seat, bb_actual),
    });

    events
}

fn set_first_to_act_preflop(hand: &mut HandState) {
    let active = active_seats_from(hand, hand.button);
    let is_heads_up = active.len() == 2;

    if is_heads_up {
        // heads-up: button/SB acts first preflop
        let btn_idx = hand.seats.iter().position(|s| s.seat == hand.button).unwrap();
        if hand.seats[btn_idx].status == SeatStatus::Active {
            hand.betting.action_on = Some(btn_idx);
        }
    } else {
        // find BB seat, then UTG is first actable after BB
        let bb = {
            let active_from_btn = active_seats_from(hand, hand.button);
            active_from_btn[1] // second active after button = BB
        };
        let bb_seat = hand.seats[bb].seat;
        let actable = actable_seats_from(hand, bb_seat);
        hand.betting.action_on = actable.first().copied();
    }
}

fn set_first_to_act_postflop(hand: &mut HandState) {
    // first active seat after button
    let actable = actable_seats_from(hand, hand.button);
    hand.betting.action_on = actable.first().copied();
}

fn advance_action(hand: &mut HandState) {
    if let Some(current) = hand.betting.action_on {
        let current_seat = hand.seats[current].seat;
        let actable = actable_seats_from(hand, current_seat);
        hand.betting.action_on = actable.first().copied();
    }
}

fn apply_bet(hand: &mut HandState, seat_idx: usize, amount: u64) {
    let s = &mut hand.seats[seat_idx];
    let actual = amount.min(s.chips);
    s.chips -= actual;
    s.bet_this_round += actual;
    s.total_bet += actual;
}

fn reset_acted_except(hand: &mut HandState, except: usize) {
    for (i, s) in hand.seats.iter_mut().enumerate() {
        if i != except && s.status == SeatStatus::Active {
            s.has_acted = false;
        }
    }
}

fn is_betting_round_complete(hand: &HandState) -> bool {
    for s in &hand.seats {
        if s.status != SeatStatus::Active {
            continue;
        }
        if !s.has_acted {
            return false;
        }
        if s.bet_this_round != hand.betting.current_bet {
            return false;
        }
    }
    true
}

fn deal_community(hand: &mut HandState, phase: Phase) -> Vec<Card> {
    let count = match phase {
        Phase::Flop => 3,
        Phase::Turn => 1,
        Phase::River => 1,
        _ => 0,
    };
    let cards: Vec<Card> = hand.deck[hand.deck_idx..hand.deck_idx + count].to_vec();
    hand.deck_idx += count;
    hand.community_cards.extend_from_slice(&cards);
    cards
}

fn recalculate_pots(hand: &mut HandState) {
    // collect all contributions from non-sitting-out players
    let mut contributions: Vec<(u8, u64)> = hand.seats.iter()
        .filter(|s| s.status != SeatStatus::SittingOut)
        .map(|s| (s.seat, s.total_bet))
        .filter(|(_, bet)| *bet > 0)
        .collect();

    if contributions.is_empty() {
        hand.pots = Vec::new();
        return;
    }

    // sort by contribution amount
    contributions.sort_by_key(|(_, bet)| *bet);

    let mut pots = Vec::new();
    let mut prev_level = 0u64;

    // get unique contribution levels
    let levels: Vec<u64> = {
        let mut lvls: Vec<u64> = contributions.iter().map(|(_, b)| *b).collect();
        lvls.dedup();
        lvls
    };

    for &level in &levels {
        let layer = level.checked_sub(prev_level).expect("pot level underflow");
        if layer == 0 { continue; }

        // everyone who contributed at least this much
        let contributors: Vec<u8> = contributions.iter()
            .filter(|(_, bet)| *bet >= level)
            .map(|(seat, _)| *seat)
            .collect();

        let amount = layer.checked_mul(contributors.len() as u64).expect("pot overflow");

        // eligible = contributors who haven't folded
        let eligible: Vec<u8> = contributors.iter()
            .filter(|&&seat| {
                let s = &hand.seats[seat as usize];
                s.status != SeatStatus::Folded
            })
            .copied()
            .collect();

        pots.push(Pot { amount, eligible });
        prev_level = level;
    }

    hand.pots = pots;
}

fn run_showdown(hand: &mut HandState) -> Vec<EngineEvent> {
    let mut events = Vec::new();

    // build hole cards for active/all-in players
    let hole_cards: Vec<(usize, [Card; 2])> = hand.seats.iter()
        .filter(|s| (s.status == SeatStatus::Active || s.status == SeatStatus::AllIn) && s.hole_cards.is_some())
        .map(|s| (s.seat as usize, s.hole_cards.unwrap()))
        .collect();

    if hand.community_cards.len() < 5 {
        // shouldn't happen — community should be fully dealt before showdown
        return events;
    }

    let community: [Card; 5] = [
        hand.community_cards[0],
        hand.community_cards[1],
        hand.community_cards[2],
        hand.community_cards[3],
        hand.community_cards[4],
    ];

    let result = showdown_holdem(&hole_cards, &community);
    events.push(EngineEvent::Showdown { result: result.clone() });

    // award each pot
    for (pot_idx, pot) in hand.pots.iter().enumerate() {
        if pot.eligible.is_empty() {
            continue;
        }

        // find best hand among eligible seats
        let eligible_hands: Vec<(usize, [Card; 2])> = hole_cards.iter()
            .filter(|(id, _)| pot.eligible.contains(&(*id as u8)))
            .cloned()
            .collect();

        if eligible_hands.is_empty() {
            // give to first eligible (shouldn't happen normally)
            let winner = pot.eligible[0];
            hand.seats[winner as usize].chips += pot.amount;
            events.push(EngineEvent::PotAwarded { seat: winner, amount: pot.amount, pot_index: pot_idx });
            continue;
        }

        let pot_result = showdown_holdem(&eligible_hands, &community);
        let winners = &pot_result.winners;
        let share = pot.amount / winners.len() as u64;
        let remainder = pot.amount % winners.len() as u64;

        for (i, &winner_id) in winners.iter().enumerate() {
            let award = share + if i == 0 { remainder } else { 0 };
            hand.seats[winner_id].chips += award;
            events.push(EngineEvent::PotAwarded {
                seat: winner_id as u8,
                amount: award,
                pot_index: pot_idx,
            });
        }
    }

    events
}

fn compute_valid_actions(hand: &HandState, seat_idx: usize) -> Vec<ValidAction> {
    let s = &hand.seats[seat_idx];
    let mut actions = Vec::new();

    if s.status != SeatStatus::Active {
        return actions;
    }

    let chips = s.chips;
    let to_call = hand.betting.current_bet.saturating_sub(s.bet_this_round);

    // fold is always valid (unless there's nothing to call)
    if to_call > 0 {
        actions.push(ValidAction { kind: ActionKind::Fold, min_amount: 0, max_amount: 0 });
    }

    // check: valid when no outstanding bet to us
    if to_call == 0 {
        actions.push(ValidAction { kind: ActionKind::Check, min_amount: 0, max_amount: 0 });
    }

    // call: valid when there's a bet to match
    if to_call > 0 && chips > 0 {
        let call_amount = to_call.min(chips);
        actions.push(ValidAction { kind: ActionKind::Call, min_amount: call_amount, max_amount: call_amount });
    }

    // bet: valid when no outstanding bet (current_bet == 0 or we already match it)
    if hand.betting.current_bet == 0 && chips > 0 {
        let min_bet = hand.betting.min_raise.min(chips); // min_raise holds BB at start of round
        actions.push(ValidAction { kind: ActionKind::Bet, min_amount: min_bet, max_amount: chips });
    }

    // raise: valid when there's a bet to raise
    if hand.betting.current_bet > 0 && to_call < chips {
        let min_raise_to = hand.betting.min_raise;
        let needed_for_min_raise = min_raise_to.saturating_sub(s.bet_this_round);
        if chips >= needed_for_min_raise {
            // can make at least a min raise
            let max_raise_to = s.bet_this_round + chips;
            actions.push(ValidAction {
                kind: ActionKind::Raise,
                min_amount: min_raise_to,
                max_amount: max_raise_to,
            });
        }
    }

    // all-in: always valid when you have chips
    if chips > 0 {
        actions.push(ValidAction { kind: ActionKind::AllIn, min_amount: chips, max_amount: chips });
    }

    actions
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zk_shuffle::poker::{Rank, Suit};

    fn card(rank: Rank, suit: Suit) -> Card {
        Card { rank, suit }
    }

    fn make_deck() -> Vec<Card> {
        let mut deck = Vec::new();
        for &suit in &[Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades] {
            for rank_val in 2..=14u8 {
                let rank = match rank_val {
                    2 => Rank::Two, 3 => Rank::Three, 4 => Rank::Four,
                    5 => Rank::Five, 6 => Rank::Six, 7 => Rank::Seven,
                    8 => Rank::Eight, 9 => Rank::Nine, 10 => Rank::Ten,
                    11 => Rank::Jack, 12 => Rank::Queen, 13 => Rank::King,
                    14 => Rank::Ace, _ => unreachable!(),
                };
                deck.push(card(rank, suit));
            }
        }
        deck
    }

    fn training_rules(seats: u8) -> TableRules {
        TableRules {
            small_blind: 5,
            big_blind: 10,
            ante: 0,
            min_buy_in: 0,
            max_buy_in: 0,
            seats,
            tier: crate::protocol::SecurityTier::Training,
            allow_spectators: true,
            max_spectators: 10,
            time_bank: 60,
            action_timeout: 30,
        }
    }

    fn setup_game(num_seats: u8, buy_ins: &[(u8, u64)]) -> GameEngine {
        let rules = training_rules(num_seats);
        let mut engine = GameEngine::new(rules, num_seats).unwrap();
        for &(seat, amount) in buy_ins {
            engine.seat_player(seat, amount).unwrap();
        }
        engine
    }

    fn total_chips(engine: &GameEngine) -> u64 {
        engine.stacks().iter().sum::<u64>()
            + engine.hand_state().map(|h| {
                h.seats.iter().map(|s| s.chips + s.total_bet).sum::<u64>()
            }).unwrap_or(0)
    }

    fn total_chips_engine_only(engine: &GameEngine) -> u64 {
        if engine.hand_state().is_some() {
            let h = engine.hand_state().unwrap();
            h.seats.iter().map(|s| s.chips + s.total_bet).sum::<u64>()
        } else {
            engine.stacks().iter().sum::<u64>()
        }
    }

    fn find_action_required(events: &[EngineEvent]) -> Option<u8> {
        events.iter().find_map(|e| match e {
            EngineEvent::ActionRequired { seat, .. } => Some(*seat),
            _ => None,
        })
    }

    // === Position tests ===

    #[test]
    fn test_heads_up_positions() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();

        // heads-up: button(0) = SB, seat 1 = BB
        // button/SB acts first preflop
        let first = find_action_required(&events).unwrap();
        assert_eq!(first, 0, "button/SB should act first preflop in heads-up");
    }

    #[test]
    fn test_three_player_positions() {
        let mut engine = setup_game(3, &[(0, 1000), (1, 1000), (2, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();

        // button=0, SB=1, BB=2, UTG=0 acts first preflop
        let first = find_action_required(&events).unwrap();
        assert_eq!(first, 0, "UTG (button in 3-way) should act first preflop");
    }

    #[test]
    fn test_six_player_positions() {
        let mut engine = setup_game(6, &[(0, 1000), (1, 1000), (2, 1000), (3, 1000), (4, 1000), (5, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();

        // button=0, SB=1, BB=2, UTG=3
        let first = find_action_required(&events).unwrap();
        assert_eq!(first, 3, "UTG should act first preflop in 6-max");
    }

    #[test]
    fn test_nine_player_positions() {
        let mut engine = setup_game(9, &[
            (0, 1000), (1, 1000), (2, 1000), (3, 1000), (4, 1000),
            (5, 1000), (6, 1000), (7, 1000), (8, 1000),
        ]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();

        // button=0, SB=1, BB=2, UTG=3
        let first = find_action_required(&events).unwrap();
        assert_eq!(first, 3, "UTG should act first preflop in 9-max");
    }

    #[test]
    fn test_positions_with_empty_seats() {
        // seats: 0(empty), 1(active), 2(empty), 3(active), 4(active)
        let mut engine = setup_game(5, &[(1, 1000), (3, 1000), (4, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(1, &deck).unwrap();

        // button=1, SB=3, BB=4, UTG=1
        let first = find_action_required(&events).unwrap();
        assert_eq!(first, 1, "UTG wraps around to button in 3-player with gaps");
    }

    // === Pot tests ===

    #[test]
    fn test_main_pot_only() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();
        let initial_total = 2000u64;

        // button/SB calls, BB checks
        let first = find_action_required(&events).unwrap();
        let events = engine.apply_action(first, ActionType::Call).unwrap();
        let next = find_action_required(&events).unwrap();
        let events = engine.apply_action(next, ActionType::Check).unwrap();

        // should advance to flop
        assert!(events.iter().any(|e| matches!(e, EngineEvent::PhaseChanged { phase: Phase::Flop, .. })));

        // conservation
        assert_eq!(total_chips_engine_only(&engine), initial_total);
    }

    #[test]
    fn test_two_way_allin_unequal() {
        let mut engine = setup_game(2, &[(0, 500), (1, 1000)]);
        let deck = make_deck();
        let initial_total = 1500u64;
        engine.new_hand(0, &deck).unwrap();

        // seat 0 (SB/button) goes all-in
        engine.apply_action(0, ActionType::AllIn).unwrap();
        // seat 1 (BB) calls
        let events = engine.apply_action(1, ActionType::Call).unwrap();

        // hand should complete
        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
        // conservation
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
    }

    #[test]
    fn test_three_way_side_pots() {
        let mut engine = setup_game(3, &[(0, 100), (1, 300), (2, 500)]);
        let deck = make_deck();
        let initial_total = 900u64;

        engine.new_hand(0, &deck).unwrap();

        // UTG (seat 0) goes all-in for 100
        engine.apply_action(0, ActionType::AllIn).unwrap();
        // seat 1 goes all-in for 300
        engine.apply_action(1, ActionType::AllIn).unwrap();
        // seat 2 calls 300
        let events = engine.apply_action(2, ActionType::Call).unwrap();

        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
    }

    // === Validation tests ===

    #[test]
    fn test_not_your_turn() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();
        let first = find_action_required(&events).unwrap();
        let other = if first == 0 { 1 } else { 0 };

        let result = engine.apply_action(other, ActionType::Fold);
        assert_eq!(result.unwrap_err(), EngineError::NotYourTurn);
    }

    #[test]
    fn test_cannot_check_with_bet() {
        let mut engine = setup_game(3, &[(0, 1000), (1, 1000), (2, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();

        // UTG cannot check preflop (BB is a bet)
        let first = find_action_required(&events).unwrap();
        let result = engine.apply_action(first, ActionType::Check);
        assert_eq!(result.unwrap_err(), EngineError::CannotCheck);
    }

    #[test]
    fn test_min_raise_chain() {
        let mut engine = setup_game(3, &[(0, 10000), (1, 10000), (2, 10000)]);
        let deck = make_deck();
        // blinds 5/10
        engine.new_hand(0, &deck).unwrap();

        // UTG raises to 20 (min raise = 2x BB)
        engine.apply_action(0, ActionType::Raise(20)).unwrap();
        // SB raises to 30 (min reraise = 20 + 10 = 30)
        engine.apply_action(1, ActionType::Raise(30)).unwrap();
        // BB raises to 40 (min reraise = 30 + 10 = 40)
        engine.apply_action(2, ActionType::Raise(40)).unwrap();

        // UTG min reraise should be 50 (40 + 10)
        let valid = engine.valid_actions().unwrap();
        let raise = valid.iter().find(|v| v.kind == ActionKind::Raise).unwrap();
        assert_eq!(raise.min_amount, 50);
    }

    #[test]
    fn test_allin_below_min_raise() {
        // player with very few chips can still go all-in even below min raise
        let mut engine = setup_game(3, &[(0, 15), (1, 1000), (2, 1000)]);
        let deck = make_deck();
        engine.new_hand(0, &deck).unwrap();

        // UTG (seat 0, 15 chips after posting) goes all-in
        let result = engine.apply_action(0, ActionType::AllIn);
        assert!(result.is_ok());
    }

    // === Full hand tests ===

    #[test]
    fn test_preflop_fold() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        let events = engine.new_hand(0, &deck).unwrap();
        let initial_total = 2000u64;

        let first = find_action_required(&events).unwrap();
        let events = engine.apply_action(first, ActionType::Fold).unwrap();

        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
        assert!(engine.hand_state().is_none());
    }

    #[test]
    fn test_check_to_showdown() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        let initial_total = 2000u64;

        engine.new_hand(0, &deck).unwrap();

        // preflop: SB/button calls, BB checks
        engine.apply_action(0, ActionType::Call).unwrap();
        engine.apply_action(1, ActionType::Check).unwrap();

        // flop: check check
        engine.apply_action(1, ActionType::Check).unwrap();
        engine.apply_action(0, ActionType::Check).unwrap();

        // turn: check check
        engine.apply_action(1, ActionType::Check).unwrap();
        engine.apply_action(0, ActionType::Check).unwrap();

        // river: check check → showdown
        engine.apply_action(1, ActionType::Check).unwrap();
        let events = engine.apply_action(0, ActionType::Check).unwrap();

        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
    }

    #[test]
    fn test_bet_call_raise_sequence() {
        let mut engine = setup_game(3, &[(0, 1000), (1, 1000), (2, 1000)]);
        let deck = make_deck();
        let initial_total = 3000u64;

        engine.new_hand(0, &deck).unwrap();

        // preflop: UTG calls, SB calls, BB checks
        engine.apply_action(0, ActionType::Call).unwrap();
        engine.apply_action(1, ActionType::Call).unwrap();
        engine.apply_action(2, ActionType::Check).unwrap();

        // flop: SB bets 20, BB raises to 50, UTG folds, SB calls
        engine.apply_action(1, ActionType::Bet(20)).unwrap();
        engine.apply_action(2, ActionType::Raise(50)).unwrap();
        engine.apply_action(0, ActionType::Fold).unwrap();
        engine.apply_action(1, ActionType::Call).unwrap();

        // turn: SB checks, BB bets 100, SB calls
        engine.apply_action(1, ActionType::Check).unwrap();
        engine.apply_action(2, ActionType::Bet(100)).unwrap();
        engine.apply_action(1, ActionType::Call).unwrap();

        // river: SB checks, BB checks → showdown
        engine.apply_action(1, ActionType::Check).unwrap();
        let events = engine.apply_action(2, ActionType::Check).unwrap();

        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
    }

    #[test]
    fn test_allin_preflop_runout() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        let initial_total = 2000u64;

        engine.new_hand(0, &deck).unwrap();

        // SB/button goes all-in
        engine.apply_action(0, ActionType::AllIn).unwrap();
        // BB calls
        let events = engine.apply_action(1, ActionType::Call).unwrap();

        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
    }

    // === Edge cases ===

    #[test]
    fn test_split_pot() {
        // give both players identical hole cards by crafting deck
        // deck: [Ac, Ac, Ad, Ad, ...community...]
        // wait — can't have duplicate cards. Instead rely on community making the best hand.
        // give both weak hole cards, community has a straight that plays
        let mut deck = Vec::new();
        // player 0 hole cards: 2c, 3c (weak)
        deck.push(card(Rank::Two, Suit::Clubs));
        deck.push(card(Rank::Three, Suit::Clubs));
        // player 1 hole cards: 2d, 3d (weak)
        deck.push(card(Rank::Two, Suit::Diamonds));
        deck.push(card(Rank::Three, Suit::Diamonds));
        // community: T-J-Q-K-A (broadway straight on board)
        deck.push(card(Rank::Ten, Suit::Hearts));
        deck.push(card(Rank::Jack, Suit::Hearts));
        deck.push(card(Rank::Queen, Suit::Spades));
        deck.push(card(Rank::King, Suit::Spades));
        deck.push(card(Rank::Ace, Suit::Hearts));

        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let initial_total = 2000u64;
        engine.new_hand(0, &deck).unwrap();

        // all-in
        engine.apply_action(0, ActionType::AllIn).unwrap();
        let events = engine.apply_action(1, ActionType::Call).unwrap();

        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
        // both should have same chips (split)
        assert_eq!(engine.stacks()[0], 1000);
        assert_eq!(engine.stacks()[1], 1000);
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
    }

    #[test]
    fn test_timeout_auto_fold() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        engine.new_hand(0, &deck).unwrap();

        let events = engine.timeout().unwrap();
        assert!(events.iter().any(|e| matches!(e, EngineEvent::HandComplete { .. })));
    }

    // === Conservation ===

    #[test]
    fn test_chip_conservation_multi_hand() {
        let mut engine = setup_game(3, &[(0, 1000), (1, 1000), (2, 1000)]);
        let deck = make_deck();
        let initial_total = 3000u64;

        // hand 1: everyone folds to BB
        engine.new_hand(0, &deck).unwrap();
        engine.apply_action(0, ActionType::Fold).unwrap();
        engine.apply_action(1, ActionType::Fold).unwrap();
        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);

        // hand 2: button=1, SB=2, BB=0, UTG=1
        let events = engine.new_hand(1, &deck).unwrap();
        let first = find_action_required(&events).unwrap();
        // UTG=1 calls, then SB=2 calls, then BB=0 checks
        engine.apply_action(first, ActionType::Call).unwrap();
        // find who's next each time
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Call).unwrap();
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        // flop: postflop order is first active after button(1) = seat 2
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        // turn
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        // river
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();
        let h = engine.hand_state().unwrap();
        let next = h.seats[h.betting.action_on.unwrap()].seat;
        engine.apply_action(next, ActionType::Check).unwrap();

        assert_eq!(engine.stacks().iter().sum::<u64>(), initial_total);
    }

    // === Determinism ===

    #[test]
    fn test_determinism() {
        let deck = make_deck();

        let run = || -> [u8; 32] {
            let mut engine = setup_game(3, &[(0, 1000), (1, 1000), (2, 1000)]);
            engine.new_hand(0, &deck).unwrap();
            engine.apply_action(0, ActionType::Call).unwrap();
            engine.apply_action(1, ActionType::Call).unwrap();
            engine.apply_action(2, ActionType::Check).unwrap();
            engine.state_hash()
        };

        assert_eq!(run(), run());
    }

    #[test]
    fn test_seat_unseat() {
        let mut engine = setup_game(4, &[(0, 500), (2, 500)]);
        assert_eq!(engine.stacks()[0], 500);
        assert_eq!(engine.stacks()[1], 0);

        engine.seat_player(1, 300).unwrap();
        assert_eq!(engine.stacks()[1], 300);

        assert_eq!(engine.seat_player(0, 100), Err(EngineError::SeatOccupied(0)));

        let chips = engine.unseat_player(1).unwrap();
        assert_eq!(chips, 300);
        assert_eq!(engine.stacks()[1], 0);
    }

    #[test]
    fn test_postflop_action_order() {
        // verify SB acts first postflop in 3+ player game
        let mut engine = setup_game(3, &[(0, 1000), (1, 1000), (2, 1000)]);
        let deck = make_deck();
        engine.new_hand(0, &deck).unwrap();

        // preflop: everyone calls/checks
        engine.apply_action(0, ActionType::Call).unwrap(); // UTG
        engine.apply_action(1, ActionType::Call).unwrap(); // SB
        let events = engine.apply_action(2, ActionType::Check).unwrap(); // BB

        // flop: first to act should be seat 1 (SB, first active after button=0)
        let first_flop = find_action_required(&events).unwrap();
        assert_eq!(first_flop, 1, "SB should act first postflop");
    }

    #[test]
    fn test_heads_up_postflop_order() {
        let mut engine = setup_game(2, &[(0, 1000), (1, 1000)]);
        let deck = make_deck();
        engine.new_hand(0, &deck).unwrap();

        // preflop: button/SB calls, BB checks
        engine.apply_action(0, ActionType::Call).unwrap();
        let events = engine.apply_action(1, ActionType::Check).unwrap();

        // flop: BB acts first postflop in heads-up
        let first_flop = find_action_required(&events).unwrap();
        assert_eq!(first_flop, 1, "BB should act first postflop in heads-up");
    }
}
