//! bot - CTM-MoE poker bot that plays as a participant
//!
//! loads the blueprint strategy + optional ONNX MoE models.
//! when it's the bot's turn, Brain.decide() picks an action.

use poker_p2p::engine::*;
use poker_p2p::protocol::ActionType;

use poker_pvm::cfr::brain::{Brain, ExploitMode};
use poker_pvm::cfr::strategy::import_strategy;

/// a poker bot that can decide actions
pub struct Bot {
    brain: Brain,
    hero_seat: u8,
    hero_cards: Option<[u8; 2]>,
}

impl Bot {
    /// create bot from a strategy file
    pub fn from_strategy(strategy_data: &[u8], seat: u8) -> Self {
        Self {
            brain: Brain::new(strategy_data).with_mode(ExploitMode::Exploit),
            hero_seat: seat,
            hero_cards: None,
        }
    }

    /// notify bot of new hand
    pub fn new_hand(&mut self, state: &BotGameState) {
        // reset for new hand
        self.hero_cards = None;
    }

    /// set bot's hole cards
    pub fn set_cards(&mut self, cards: [u8; 2]) {
        self.hero_cards = Some(cards);
    }

    /// decide an action given valid options
    pub fn decide(&mut self, valid_actions: &[ValidAction], game_state: &BotGameState) -> ActionType {
        // if we have hole cards + game state, use the brain
        if let Some(cards) = self.hero_cards {
            let state = to_pvm_state(game_state, self.hero_seat);
            let community = game_state.community_as_indices();

            let decision = self.brain.decide(&state, &cards, &community);

            // map brain output to engine action
            if let Some((action, amount)) = decision.sample(rand::random::<f64>()) {
                return pvm_action_to_engine(action, amount, valid_actions);
            }
        }

        // fallback: pick simplest valid action
        fallback_action(valid_actions)
    }
}

/// simplified game state for the bot
pub struct BotGameState {
    pub stacks: Vec<u64>,
    pub pot: u64,
    pub community_cards: Vec<zk_shuffle::poker::Card>,
    pub bets: Vec<u64>,
    pub num_players: u8,
    pub phase: Phase,
    pub acting_seat: u8,
    pub hand_number: u32,
}

impl BotGameState {
    pub fn community_as_indices(&self) -> [u8; 5] {
        let mut result = [0u8; 5];
        for (i, card) in self.community_cards.iter().enumerate().take(5) {
            result[i] = card_to_index(card);
        }
        result
    }
}

fn card_to_index(card: &zk_shuffle::poker::Card) -> u8 {
    use zk_shuffle::poker::{Rank, Suit};
    let rank = match card.rank {
        Rank::Two => 0, Rank::Three => 1, Rank::Four => 2,
        Rank::Five => 3, Rank::Six => 4, Rank::Seven => 5,
        Rank::Eight => 6, Rank::Nine => 7, Rank::Ten => 8,
        Rank::Jack => 9, Rank::Queen => 10, Rank::King => 11,
        Rank::Ace => 12,
    };
    let suit = match card.suit {
        Suit::Clubs => 0, Suit::Diamonds => 1, Suit::Hearts => 2, Suit::Spades => 3,
    };
    rank + suit * 13
}

fn to_pvm_state(gs: &BotGameState, hero_seat: u8) -> poker_pvm::GameState {
    let phase = match gs.phase {
        Phase::Preflop => poker_pvm::Phase::Preflop,
        Phase::Flop => poker_pvm::Phase::Flop,
        Phase::Turn => poker_pvm::Phase::Turn,
        Phase::River => poker_pvm::Phase::River,
        _ => poker_pvm::Phase::Preflop,
    };

    let mut stacks = [0u32; poker_pvm::MAX_SEATS];
    let mut bets = [0u32; poker_pvm::MAX_SEATS];
    let mut seat_state = [poker_pvm::SeatState::Empty; poker_pvm::MAX_SEATS];
    for (i, &s) in gs.stacks.iter().enumerate().take(poker_pvm::MAX_SEATS) {
        stacks[i] = s as u32;
        if s > 0 { seat_state[i] = poker_pvm::SeatState::Active; }
    }
    for (i, &b) in gs.bets.iter().enumerate().take(poker_pvm::MAX_SEATS) {
        bets[i] = b as u32;
    }

    let community = gs.community_as_indices();

    let community_count = match gs.phase {
        Phase::Preflop => 0,
        Phase::Flop => 3,
        Phase::Turn => 4,
        Phase::River | Phase::Showdown => 5,
        _ => 0,
    };

    poker_pvm::GameState {
        stacks,
        bets,
        pot: gs.pot as u32,
        community,
        community_count,
        phase,
        acting_seat: gs.acting_seat,
        num_players: gs.num_players,
        hand_number: gs.hand_number,
        button: 0,
        seat_state,
        cards: [[0u8; 2]; poker_pvm::MAX_SEATS],
        round_actions: 0,
        last_aggressor: 0,
        action_count: 0,
        last_action_hash: [0u8; 32],
        rake: 0,
        rules: poker_pvm::Rules {
            buyin: 1000, small_blind: 5, big_blind: 10,
            turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
        },
    }
}

fn pvm_action_to_engine(action: poker_pvm::Action, amount: u32, valid: &[ValidAction]) -> ActionType {
    match action {
        poker_pvm::Action::Fold => {
            if valid.iter().any(|v| v.kind == ActionKind::Fold) {
                ActionType::Fold
            } else {
                ActionType::Check // can't fold if nothing to call
            }
        }
        poker_pvm::Action::Check => ActionType::Check,
        poker_pvm::Action::Call => ActionType::Call,
        poker_pvm::Action::Bet => {
            if let Some(va) = valid.iter().find(|v| v.kind == ActionKind::Bet) {
                let clamped = (amount as u64).max(va.min_amount).min(va.max_amount);
                ActionType::Bet(clamped as u128)
            } else if let Some(va) = valid.iter().find(|v| v.kind == ActionKind::Raise) {
                let clamped = (amount as u64).max(va.min_amount).min(va.max_amount);
                ActionType::Raise(clamped as u128)
            } else {
                ActionType::Call
            }
        }
        poker_pvm::Action::Raise => {
            if let Some(va) = valid.iter().find(|v| v.kind == ActionKind::Raise) {
                let clamped = (amount as u64).max(va.min_amount).min(va.max_amount);
                ActionType::Raise(clamped as u128)
            } else {
                ActionType::Call
            }
        }
        poker_pvm::Action::AllIn => ActionType::AllIn,
    }
}

fn fallback_action(valid: &[ValidAction]) -> ActionType {
    // prefer check > call > fold
    if valid.iter().any(|v| v.kind == ActionKind::Check) {
        ActionType::Check
    } else if valid.iter().any(|v| v.kind == ActionKind::Call) {
        ActionType::Call
    } else if valid.iter().any(|v| v.kind == ActionKind::Fold) {
        ActionType::Fold
    } else {
        ActionType::Check
    }
}
