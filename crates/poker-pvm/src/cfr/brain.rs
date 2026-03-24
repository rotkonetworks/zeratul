//! The Brain: unified decision engine combining all layers.
//!
//! Integrates:
//!   - Blueprint (CFR strategy table)
//!   - Real-time search (depth-limited CFR)
//!   - Range tracking (Bayesian inference over opponent hands)
//!   - Player profiling (VPIP/PFR/AF classification)
//!   - MoE-CTM leaf evaluation (future: via WASM bridge)
//!
//! Decision flow:
//!   1. Update opponent range from last action (Bayes)
//!   2. Update opponent profile stats
//!   3. Compute search with range-weighted opponent sampling
//!   4. Blend with profile-based adjustment (1-15%)
//!   5. Return final action distribution
//!
//! Safety: defaults to pure Nash (no profiling) unless
//! exploitation mode is explicitly enabled.

use std::collections::HashMap;
use crate::*;
use super::abstraction::*;
use super::strategy::import_strategy;
use super::search::{PokerBot, SearchConfig, SearchResult};
use super::range::{Range, action_likelihoods};

/// exploitation mode
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExploitMode {
    /// pure Nash — no profiling, no adaptation, unexploitable
    Nash,
    /// exploit detected weaknesses (1-15% deviation from Nash)
    Exploit,
}

/// the unified poker brain
pub struct Brain {
    /// blueprint + search bot
    pub bot: PokerBot,
    /// opponent ranges (per seat)
    pub ranges: [Option<Range>; MAX_SEATS],
    /// opponent profiles (persistent across hands)
    pub profiles: TableProfiles,
    /// exploitation mode
    pub mode: ExploitMode,
    /// action history for range updates (reset per hand)
    history: Vec<u8>,
    /// current hand number (for range reset)
    hand_number: u32,
}

impl Brain {
    pub fn new(strategy_data: &[u8]) -> Self {
        Self {
            bot: PokerBot::from_strategy_bytes(strategy_data),
            ranges: Default::default(),
            profiles: TableProfiles::default(),
            mode: ExploitMode::Nash,
            history: Vec::new(),
            hand_number: 0,
        }
    }

    pub fn with_mode(mut self, mode: ExploitMode) -> Self {
        self.mode = mode;
        self
    }

    /// call when a new hand starts
    pub fn new_hand(&mut self, state: &GameState) {
        self.hand_number = state.hand_number;
        self.history.clear();
        self.profiles.new_hand(state.num_players);

        // reset ranges for all active players
        let n = state.num_players as usize;
        let mut dead_cards: Vec<u8> = Vec::new();

        // we know our own hole cards (hero = acting seat at time of call)
        // but ranges are initialized fresh each hand
        for i in 0..n {
            let mut range = Range::uniform();
            // remove known cards (community + our hole cards will be removed as revealed)
            self.ranges[i] = Some(range);
        }
    }

    /// call when hero's hole cards are known (removes from opponent ranges)
    pub fn set_hero_cards(&mut self, hero_seat: u8, cards: [u8; 2], state: &GameState) {
        let n = state.num_players as usize;
        for i in 0..n {
            if i == hero_seat as usize { continue; }
            if let Some(ref mut range) = self.ranges[i] {
                range.remove_cards(&cards);
            }
        }
    }

    /// call when community cards are revealed
    pub fn reveal_community(&mut self, cards: &[u8], state: &GameState) {
        let n = state.num_players as usize;
        for i in 0..n {
            if let Some(ref mut range) = self.ranges[i] {
                range.remove_cards(cards);
            }
        }
    }

    /// observe an opponent's action — updates range + profile
    pub fn observe_action(
        &mut self,
        seat: u8,
        action: Action,
        amount: u32,
        state: &GameState,
    ) {
        let n = state.num_players as usize;
        let s = seat as usize;

        // 1. update profile
        let max_bet = state.bets.iter().take(n).copied().max().unwrap_or(0);
        let is_facing_raise = state.bets[s] < max_bet;
        self.profiles.observe(seat, action, state.phase, is_facing_raise);

        // 2. update range (Bayesian)
        if let Some(ref mut range) = self.ranges[s] {
            let street = match state.phase {
                Phase::Preflop => 0,
                Phase::Flop => 1,
                Phase::Turn => 2,
                Phase::River => 3,
                _ => 0,
            };

            let action_bucket = abstract_action(action, amount, state.pot, state.stacks[s]);

            // compute likelihoods from blueprint
            let community = &state.community[..state.community_count as usize];
            let blueprint = &self.bot.config.blueprint;
            let mut rng_state = 0xBAD_C0FFEEu64;
            let mut rng = || -> u32 {
                rng_state ^= rng_state << 13;
                rng_state ^= rng_state >> 7;
                rng_state ^= rng_state << 17;
                (rng_state >> 16) as u32
            };
            let likelihoods = action_likelihoods(
                blueprint,
                community,
                street,
                &self.history,
                action_bucket as usize,
                &mut rng,
            );

            range.update(&likelihoods);
        }

        // 3. update action history
        let abs = abstract_action(action, amount, state.pot, state.stacks[s]);
        self.history.push(abs);
    }

    /// main decision: what should hero do?
    pub fn decide(
        &mut self,
        state: &GameState,
        hero_cards: &[u8; 2],
        community: &[u8; 5],
    ) -> BrainDecision {
        let hero_seat = state.acting_seat;
        let n = state.num_players as usize;

        // 1. base decision from search (pure Nash)
        let search_result = self.bot.decide(state, hero_cards, community, &self.history);

        if self.mode == ExploitMode::Nash {
            // pure Nash: return search result as-is
            return BrainDecision {
                action_probs: search_result.action_probs.clone(),
                actions: search_result.actions.clone(),
                value: search_result.value,
                mode: ExploitMode::Nash,
                profile_weight: 0.0,
                opponent_type: PlayerType::Unknown,
                range_equity: None,
                from_blueprint: search_result.from_blueprint,
            };
        }

        // 2. exploitation mode: compute adjustments
        let opp_seat = if n == 2 { 1 - hero_seat } else { hero_seat }; // TODO: multi-way
        let profile = &self.profiles.profiles[opp_seat as usize];
        let opp_type = profile.classify();
        let conf_weight = profile.confidence_weight();

        // 3. range-based equity (if we have enough community cards)
        let range_equity = if state.community_count == 5 {
            self.ranges[opp_seat as usize].as_ref().map(|r| {
                r.equity_vs(*hero_cards, community)
            })
        } else {
            None
        };

        // 4. compute exploitative adjustment based on opponent type
        let mut adjusted_probs = search_result.action_probs.clone();
        let actions = &search_result.actions;

        match opp_type {
            PlayerType::Rock | PlayerType::Nit => {
                // vs rock: steal more (increase bet), fold to their aggression
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Bet | Action::Raise => adjusted_probs[i] *= 1.3,
                        Action::Fold => {
                            // fold MORE when they bet (they always have it)
                            let max_bet = state.bets.iter().take(n).copied().max().unwrap_or(0);
                            if state.bets[hero_seat as usize] < max_bet {
                                adjusted_probs[i] *= 1.5;
                            }
                        }
                        _ => {}
                    }
                }
            }
            PlayerType::CallingStation => {
                // vs station: value bet big, never bluff
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Bet | Action::Raise => {
                            // only increase betting with decent equity
                            if range_equity.unwrap_or(0.5) > 0.5 {
                                adjusted_probs[i] *= 1.4;
                            } else {
                                adjusted_probs[i] *= 0.5; // don't bluff stations
                            }
                        }
                        _ => {}
                    }
                }
            }
            PlayerType::Maniac => {
                // vs maniac: call wider, don't fold light
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Call => adjusted_probs[i] *= 1.3,
                        Action::Fold => adjusted_probs[i] *= 0.7,
                        _ => {}
                    }
                }
            }
            PlayerType::LAG => {
                // vs LAG: call down lighter, trap more
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Call => adjusted_probs[i] *= 1.2,
                        Action::Check => adjusted_probs[i] *= 1.1, // trap
                        _ => {}
                    }
                }
            }
            _ => {} // TAG and Unknown: no adjustment (stay Nash)
        }

        // 5. blend: (1 - weight) * search + weight * exploitative
        let search_probs = &search_result.action_probs;
        let mut final_probs = vec![0.0; actions.len()];

        // renormalize adjusted
        let adj_total: f64 = adjusted_probs.iter().sum();
        if adj_total > 1e-10 {
            for p in adjusted_probs.iter_mut() { *p /= adj_total; }
        }

        for i in 0..actions.len() {
            final_probs[i] = (1.0 - conf_weight as f64) * search_probs[i]
                + conf_weight as f64 * adjusted_probs[i];
        }

        // renormalize final
        let total: f64 = final_probs.iter().sum();
        if total > 1e-10 {
            for p in final_probs.iter_mut() { *p /= total; }
        }

        BrainDecision {
            action_probs: final_probs,
            actions: actions.clone(),
            value: search_result.value,
            mode: self.mode,
            profile_weight: conf_weight,
            opponent_type: opp_type,
            range_equity,
            from_blueprint: search_result.from_blueprint,
        }
    }
}

/// decision output with full context
#[derive(Debug)]
pub struct BrainDecision {
    pub action_probs: Vec<f64>,
    pub actions: Vec<(Action, u32)>,
    pub value: f64,
    pub mode: ExploitMode,
    pub profile_weight: f32,
    pub opponent_type: PlayerType,
    pub range_equity: Option<f32>,
    pub from_blueprint: bool,
}

impl BrainDecision {
    /// sample an action from the distribution
    pub fn sample(&self, rng_val: f64) -> Option<(Action, u32)> {
        if self.actions.is_empty() { return None; }
        let mut cumul = 0.0;
        for (i, &p) in self.action_probs.iter().enumerate() {
            cumul += p;
            if rng_val < cumul {
                return Some(self.actions[i]);
            }
        }
        Some(*self.actions.last().unwrap())
    }
}
