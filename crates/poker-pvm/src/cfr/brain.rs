//! The Brain: unified decision engine as a composable filter stack.
//!
//! Follows the "Your Server as a Function" (Eriksen 2013) pattern:
//!   - **Service**: `GameState → Decision` (the base computation)
//!   - **Filter**: transforms a service into a new service
//!   - **andThen**: composes filters into a pipeline
//!
//! Layer stack (each is an independent filter):
//!   L0: Blueprint  — CFR strategy table lookup (the Nash floor)
//!   L1: Search     — depth-limited real-time CFR refinement
//!   L2: Range      — Bayesian opponent hand tracking
//!   L3: Exploit    — player profiling + counter-strategy
//!   L4: Neural     — CTM-MoE value/policy evaluation
//!
//! Filters compose:
//!   blueprint.and_then(search).and_then(range).and_then(exploit).and_then(neural)
//!
//! Each filter can be independently enabled/disabled/swapped.

use crate::*;
use super::abstraction::*;
use super::search::{PokerBot, SearchResult};
use super::range::{Range, action_likelihoods};
#[cfg(feature = "std")]
use super::ctm;
#[cfg(feature = "onnx")]
use super::inference::OnnxMoE;
#[cfg(feature = "onnx")]
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Decision: the "Rep" type (response) in Eriksen's model
// ---------------------------------------------------------------------------

/// decision output — the response type flowing through the filter stack
#[derive(Debug, Clone)]
pub struct Decision {
    pub action_probs: Vec<f64>,
    pub actions: Vec<(Action, u32)>,
    pub value: f64,
    pub from_blueprint: bool,
    pub metadata: DecisionMeta,
}

/// metadata accumulated as the decision flows through filters
#[derive(Debug, Clone, Default)]
pub struct DecisionMeta {
    pub opponent_type: PlayerType,
    pub profile_weight: f32,
    pub range_equity: Option<f32>,
    pub moe_blend: f32,
    pub layers_applied: Vec<&'static str>,
}

impl Decision {
    pub fn sample(&self, rng_val: f64) -> Option<(Action, u32)> {
        if self.actions.is_empty() { return None; }
        let mut cumul = 0.0;
        for (i, &p) in self.action_probs.iter().enumerate() {
            cumul += p;
            if rng_val < cumul { return Some(self.actions[i]); }
        }
        Some(*self.actions.last().unwrap())
    }

    fn renormalize(&mut self) {
        let total: f64 = self.action_probs.iter().sum();
        if total > 1e-10 {
            for p in self.action_probs.iter_mut() { *p /= total; }
        }
    }
}

// ---------------------------------------------------------------------------
// Context: the "Req" type flowing into each filter
// ---------------------------------------------------------------------------

/// all context needed for a decision — passed through the filter chain
pub struct DecisionContext<'a> {
    pub state: &'a GameState,
    pub hero_cards: &'a [u8; 2],
    pub community: &'a [u8; 5],
    pub history: &'a [u8],
    pub ranges: &'a [Option<Range>; MAX_SEATS],
    pub profiles: &'a TableProfiles,
}

// ---------------------------------------------------------------------------
// Filter trait: (Req, Service) → Decision
// ---------------------------------------------------------------------------

/// a filter transforms a decision produced by an inner service.
/// `apply(ctx, inner_decision) → filtered_decision`
///
/// this is Eriksen's Filter[Req, Rep]:
///   type Filter = (Req, Service[Req, Rep]) => Future[Rep]
/// but synchronous (no futures needed for CPU-bound poker decisions).
pub trait DecisionFilter: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, ctx: &DecisionContext, decision: Decision) -> Decision;
}

// ---------------------------------------------------------------------------
// L0: Blueprint filter (identity — the base service)
// ---------------------------------------------------------------------------

/// produces the initial decision from blueprint lookup.
/// this is the "Service" in Eriksen's model — the innermost layer.
pub struct BlueprintService {
    // blueprint is accessed via PokerBot in Brain
}

// ---------------------------------------------------------------------------
// L1: Search filter — refines blueprint via real-time CFR
// ---------------------------------------------------------------------------

pub struct SearchFilter;

impl DecisionFilter for SearchFilter {
    fn name(&self) -> &'static str { "search" }

    fn apply(&self, _ctx: &DecisionContext, mut decision: Decision) -> Decision {
        // search is handled at the base level (PokerBot::decide vs decide_blueprint_only)
        // this filter is a marker — the actual search happens in Brain::base_decision()
        decision.metadata.layers_applied.push("L1:search");
        decision
    }
}

// ---------------------------------------------------------------------------
// L2: Range filter — Bayesian equity estimation
// ---------------------------------------------------------------------------

pub struct RangeFilter;

impl DecisionFilter for RangeFilter {
    fn name(&self) -> &'static str { "range" }

    fn apply(&self, ctx: &DecisionContext, mut decision: Decision) -> Decision {
        let n = ctx.state.num_players as usize;
        let hero = ctx.state.acting_seat;
        let opp = if n == 2 { 1 - hero } else {
            // multiplayer: pick the most active opponent (highest bet)
            let mut best = (hero + 1) % n as u8;
            for i in 0..n as u8 {
                if i != hero && ctx.state.bets[i as usize] > ctx.state.bets[best as usize] {
                    best = i;
                }
            }
            best
        };

        if ctx.state.community_count == 5 {
            // river: exact equity vs range
            if let Some(ref range) = ctx.ranges[opp as usize] {
                let equity = range.equity_vs(*ctx.hero_cards, ctx.community);
                decision.metadata.range_equity = Some(equity);
            }
        } else if ctx.state.community_count >= 3 {
            // flop/turn: approximate equity from range weight (how much of their range is left)
            if let Some(ref range) = ctx.ranges[opp as usize] {
                let alive = range.alive_fraction();
                // tighter range = stronger hands → lower equity for us
                // uniform range (~1.0) → 0.5 equity, narrow range (~0.1) → 0.3 equity
                let equity = 0.5 * alive + 0.3 * (1.0 - alive);
                decision.metadata.range_equity = Some(equity);
            }
        }
        // preflop: leave as None (default 0.5)

        decision.metadata.layers_applied.push("L2:range");
        decision
    }
}

// ---------------------------------------------------------------------------
// L3: Exploit filter — player profiling + counter-strategy adjustments
// ---------------------------------------------------------------------------

pub struct ExploitFilter;

impl DecisionFilter for ExploitFilter {
    fn name(&self) -> &'static str { "exploit" }

    fn apply(&self, ctx: &DecisionContext, mut decision: Decision) -> Decision {
        let n = ctx.state.num_players as usize;
        let hero = ctx.state.acting_seat;
        let opp = if n == 2 { 1 - hero } else {
            // multiplayer: pick most active opponent (highest bet or most hands)
            let mut best = (hero as usize + 1) % n;
            for i in 0..n {
                if i != hero as usize && ctx.profiles.profiles[i].hands_seen > ctx.profiles.profiles[best].hands_seen {
                    best = i;
                }
            }
            best as u8
        };
        let profile = &ctx.profiles.profiles[opp as usize];
        let opp_type = profile.classify();
        let conf = profile.confidence_weight();

        decision.metadata.opponent_type = opp_type;
        decision.metadata.profile_weight = conf;

        if conf < 0.01 {
            decision.metadata.layers_applied.push("L3:exploit(skip:low_conf)");
            return decision;
        }

        let actions = &decision.actions;
        let mut adjusted = decision.action_probs.clone();
        let range_eq = decision.metadata.range_equity;

        match opp_type {
            PlayerType::Rock | PlayerType::Nit => {
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Bet | Action::Raise => adjusted[i] *= 1.3,
                        Action::Fold => {
                            let max_bet = ctx.state.bets.iter().take(n).copied().max().unwrap_or(0);
                            if ctx.state.bets[hero as usize] < max_bet {
                                adjusted[i] *= 1.5;
                            }
                        }
                        _ => {}
                    }
                }
            }
            PlayerType::CallingStation => {
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Bet | Action::Raise => {
                            if range_eq.unwrap_or(0.5) > 0.5 {
                                adjusted[i] *= 1.4;
                            } else {
                                adjusted[i] *= 0.5;
                            }
                        }
                        _ => {}
                    }
                }
            }
            PlayerType::Maniac => {
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Call => adjusted[i] *= 1.3,
                        Action::Fold => adjusted[i] *= 0.7,
                        _ => {}
                    }
                }
            }
            PlayerType::LAG => {
                for (i, &(action, _)) in actions.iter().enumerate() {
                    match action {
                        Action::Call => adjusted[i] *= 1.2,
                        Action::Check => adjusted[i] *= 1.1,
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        // blend: (1-conf) * current + conf * adjusted
        let adj_total: f64 = adjusted.iter().sum();
        if adj_total > 1e-10 {
            for p in adjusted.iter_mut() { *p /= adj_total; }
        }
        let w = conf as f64;
        for i in 0..actions.len() {
            decision.action_probs[i] = (1.0 - w) * decision.action_probs[i] + w * adjusted[i];
        }

        decision.metadata.layers_applied.push("L3:exploit");
        decision
    }
}

// ---------------------------------------------------------------------------
// L4: Neural filter — CTM-MoE value/policy blend
// ---------------------------------------------------------------------------

#[cfg(feature = "onnx")]
pub struct NeuralFilter {
    pub moe: Arc<OnnxMoE>,
    pub weight: f32,
}

#[cfg(feature = "onnx")]
impl DecisionFilter for NeuralFilter {
    fn name(&self) -> &'static str { "neural" }

    fn apply(&self, ctx: &DecisionContext, mut decision: Decision) -> Decision {
        let board_slice = &ctx.community[..ctx.state.community_count as usize];
        let n = ctx.state.num_players as usize;
        let hero = ctx.state.acting_seat;
        let opp = if n == 2 { 1 - hero } else {
            // multiplayer: pick opponent with highest bet (most relevant for feature extraction)
            let mut best = (hero as usize + 1) % n;
            for i in 0..n {
                if i != hero as usize && ctx.state.bets[i] > ctx.state.bets[best] {
                    best = i;
                }
            }
            best as u8
        };

        let features = ctm::extract_all(
            board_slice,
            ctx.state.pot,
            ctx.state.stacks[hero as usize],
            ctx.state.stacks[opp as usize],
            ctx.state.bets[hero as usize],
            ctx.state.rules.big_blind,
            decision.metadata.range_equity.unwrap_or(0.5),
            0.3, 0.1, 0.1,
            hero == ctx.state.button,
        );

        if let Ok(moe_out) = self.moe.evaluate(&features.features) {
            let w = self.weight as f64;

            // blend value
            decision.value = (1.0 - w) * decision.value + w * moe_out.value as f64;

            // blend policy
            let moe_mapped = map_moe_to_actions(&decision.actions, &moe_out.action_probs, ctx.state.pot);
            for i in 0..decision.actions.len() {
                decision.action_probs[i] = (1.0 - w) * decision.action_probs[i] + w * moe_mapped[i];
            }

            decision.metadata.moe_blend = self.weight;
        }

        decision.metadata.layers_applied.push("L4:neural");
        decision
    }
}

// ---------------------------------------------------------------------------
// Filter stack: andThen composition
// ---------------------------------------------------------------------------

/// a composed filter stack — filters applied in order.
/// this is the `andThen` combinator from Eriksen's paper:
///   recordHandletime andThen traceRequest andThen parseRequest andThen ...
pub struct FilterStack {
    filters: Vec<Box<dyn DecisionFilter>>,
}

impl FilterStack {
    pub fn new() -> Self {
        Self { filters: Vec::new() }
    }

    /// andThen: compose a filter onto the stack
    pub fn and_then(mut self, filter: impl DecisionFilter + 'static) -> Self {
        self.filters.push(Box::new(filter));
        self
    }

    /// apply all filters in sequence
    pub fn apply(&self, ctx: &DecisionContext, mut decision: Decision) -> Decision {
        for filter in &self.filters {
            decision = filter.apply(ctx, decision);
        }
        decision.renormalize();
        decision
    }

    pub fn layer_names(&self) -> Vec<&'static str> {
        self.filters.iter().map(|f| f.name()).collect()
    }
}

// ---------------------------------------------------------------------------
// Brain: owns state, composes filters, delegates decisions
// ---------------------------------------------------------------------------

/// the unified poker brain.
///
/// Brain owns the mutable state (ranges, profiles, history) and the
/// base service (PokerBot). The filter stack is a separate, composable
/// pipeline that transforms the base decision.
pub struct Brain {
    pub bot: PokerBot,
    pub ranges: [Option<Range>; MAX_SEATS],
    pub profiles: TableProfiles,
    pub mode: ExploitMode,
    pub filters: FilterStack,
    history: Vec<u8>,
    hand_number: u32,
    use_search: bool,
}

/// exploitation mode
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExploitMode {
    Nash,
    Exploit,
}

impl Brain {
    /// create with default filter stack (L0 blueprint only)
    pub fn new(strategy_data: &[u8]) -> Self {
        Self {
            bot: PokerBot::from_strategy_bytes(strategy_data),
            ranges: Default::default(),
            profiles: TableProfiles::default(),
            mode: ExploitMode::Nash,
            filters: FilterStack::new(),
            history: Vec::new(),
            hand_number: 0,
            use_search: false,
        }
    }

    /// builder: set exploitation mode
    pub fn with_mode(mut self, mode: ExploitMode) -> Self {
        self.mode = mode;
        self
    }

    /// builder: set filter stack
    ///
    /// usage (Eriksen-style composition):
    /// ```ignore
    /// Brain::new(&strategy)
    ///     .with_filters(
    ///         FilterStack::new()
    ///             .and_then(SearchFilter)
    ///             .and_then(RangeFilter)
    ///             .and_then(ExploitFilter)
    ///     )
    /// ```
    pub fn with_filters(mut self, filters: FilterStack) -> Self {
        // detect if search filter is in the stack
        self.use_search = filters.filters.iter().any(|f| f.name() == "search");
        if filters.filters.iter().any(|f| f.name() == "exploit") {
            self.mode = ExploitMode::Exploit;
        }
        self.filters = filters;
        self
    }

    #[cfg(feature = "onnx")]
    pub fn with_moe(mut self, moe: Arc<OnnxMoE>, weight: f32) -> Self {
        self.filters = self.filters.and_then(NeuralFilter { moe, weight });
        self
    }

    /// call when a new hand starts
    pub fn new_hand(&mut self, state: &GameState) {
        self.hand_number = state.hand_number;
        self.history.clear();
        self.profiles.new_hand(state.num_players);

        let n = state.num_players as usize;
        for i in 0..n {
            self.ranges[i] = Some(Range::uniform());
        }
    }

    /// call when hero's hole cards are known
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

        // update profile
        let max_bet = state.bets.iter().take(n).copied().max().unwrap_or(0);
        let is_facing_raise = state.bets[s] < max_bet;
        self.profiles.observe(seat, action, state.phase, is_facing_raise);

        // push action to history FIRST — Bayesian update needs full history
        let abs = abstract_action(action, amount, state.pot, state.stacks[s]);
        self.history.push(abs);

        // update range (Bayesian) — uses history INCLUDING this action
        if let Some(ref mut range) = self.ranges[s] {
            let street = match state.phase {
                Phase::Preflop => 0, Phase::Flop => 1, Phase::Turn => 2, Phase::River => 3, _ => 0,
            };
            let action_bucket = abs;
            let community = &state.community[..state.community_count as usize];
            let blueprint = &self.bot.config.blueprint;
            // seed from hand + history length for varied bucketing
            let mut rng_state = 0xBAD_C0FFEEu64 ^ (self.hand_number as u64 * 31) ^ (self.history.len() as u64 * 97);
            let mut rng = || -> u32 {
                rng_state ^= rng_state << 13;
                rng_state ^= rng_state >> 7;
                rng_state ^= rng_state << 17;
                (rng_state >> 16) as u32
            };
            let likelihoods = action_likelihoods(
                blueprint, community, street, &self.history, action_bucket as usize, &mut rng,
            );
            range.update(&likelihoods);
        }
    }

    /// main decision: base service + filter stack
    ///
    /// equivalent to Eriksen's:
    ///   blueprint andThen search andThen range andThen exploit andThen neural
    pub fn decide(
        &mut self,
        state: &GameState,
        hero_cards: &[u8; 2],
        community: &[u8; 5],
    ) -> Decision {
        // base service: L0 (+ L1 if search filter present)
        let search_result = if self.use_search {
            self.bot.decide(state, hero_cards, community, &self.history)
        } else {
            self.bot.decide_blueprint_only(state, hero_cards, community, &self.history)
        };

        let mut decision = Decision {
            action_probs: search_result.action_probs,
            actions: search_result.actions,
            value: search_result.value,
            from_blueprint: search_result.from_blueprint,
            metadata: DecisionMeta {
                layers_applied: vec![if self.use_search { "L0+L1:blueprint+search" } else { "L0:blueprint" }],
                ..Default::default()
            },
        };

        // apply filter stack
        let ctx = DecisionContext {
            state,
            hero_cards,
            community,
            history: &self.history,
            ranges: &self.ranges,
            profiles: &self.profiles,
        };

        decision = self.filters.apply(&ctx, decision);
        decision
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// map MoE's 9-action probs [fold, check, call, bet_25, bet_50, bet_75, bet_100, bet_200, allin]
/// onto the search bot's variable action list (which includes concrete sizing)
fn map_moe_to_actions(actions: &[(Action, u32)], moe_probs: &[f32; ctm::NUM_ACTIONS], pot: u32) -> Vec<f64> {
    let mut mapped = vec![0.0f64; actions.len()];
    for (i, &(action, amount)) in actions.iter().enumerate() {
        mapped[i] = match action {
            Action::Fold => moe_probs[0] as f64,
            Action::Check => moe_probs[1] as f64,
            Action::Call => moe_probs[2] as f64,
            Action::Bet | Action::Raise => {
                let frac = if pot > 0 { amount as f64 / pot as f64 } else { 1.0 };
                if frac <= 0.375 { moe_probs[3] as f64 }      // bet_25
                else if frac <= 0.625 { moe_probs[4] as f64 }  // bet_50
                else if frac <= 0.875 { moe_probs[5] as f64 }  // bet_75
                else if frac <= 1.5 { moe_probs[6] as f64 }    // bet_100
                else { moe_probs[7] as f64 }                    // bet_200
            }
            Action::AllIn => moe_probs[8] as f64,
        };
    }
    let total: f64 = mapped.iter().sum();
    if total > 1e-10 { for p in mapped.iter_mut() { *p /= total; } }
    mapped
}

// ---------------------------------------------------------------------------
// Convenience constructors (common filter stacks)
// ---------------------------------------------------------------------------

impl Brain {
    /// L0 only — fastest, pure blueprint lookup
    pub fn blueprint_only(strategy_data: &[u8]) -> Self {
        Self::new(strategy_data)
    }

    /// L0 + L1 — blueprint + search
    pub fn with_search(strategy_data: &[u8]) -> Self {
        Self::new(strategy_data)
            .with_filters(FilterStack::new().and_then(SearchFilter))
    }

    /// full L0-L3 stack — blueprint + search + range + exploit
    pub fn full_stack(strategy_data: &[u8]) -> Self {
        Self::new(strategy_data)
            .with_filters(
                FilterStack::new()
                    .and_then(SearchFilter)
                    .and_then(RangeFilter)
                    .and_then(ExploitFilter)
            )
    }

    /// L0 + L4 — blueprint + neural (fast, no search)
    #[cfg(feature = "onnx")]
    pub fn blueprint_neural(strategy_data: &[u8], moe: Arc<OnnxMoE>, weight: f32) -> Self {
        Self::new(strategy_data)
            .with_filters(
                FilterStack::new()
                    .and_then(NeuralFilter { moe, weight })
            )
    }

    /// full L0-L4 — all layers
    #[cfg(feature = "onnx")]
    pub fn full_with_neural(strategy_data: &[u8], moe: Arc<OnnxMoE>, weight: f32) -> Self {
        Self::new(strategy_data)
            .with_filters(
                FilterStack::new()
                    .and_then(SearchFilter)
                    .and_then(RangeFilter)
                    .and_then(ExploitFilter)
                    .and_then(NeuralFilter { moe, weight })
            )
    }
}

// backward compat: BrainDecision is now Decision
pub type BrainDecision = Decision;
