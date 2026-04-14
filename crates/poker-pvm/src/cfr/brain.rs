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
use super::search::{PokerBot, SearchResult, SearchConfig};
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
    pub bot: &'a std::cell::RefCell<PokerBot>,
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
// L1: Search + Range (combined, Pluribus-style)
// Range narrows opponent hands, search solves within that range.
// ---------------------------------------------------------------------------

/// L1: Combined search + range filter.
/// Computes range equity AND runs selective depth-limited CFR in one pass.
/// Selective: skips search on small pots or when blueprint is confident.
pub struct SearchFilter {
    pub iterations: u32,
    pub pot_threshold: f32,
}

impl Default for SearchFilter {
    fn default() -> Self {
        Self { iterations: 200, pot_threshold: 0.1 }
    }
}

impl SearchFilter {
    pub fn thorough() -> Self { Self { iterations: 1000, pot_threshold: 0.0 } }
    pub fn fast() -> Self { Self { iterations: 50, pot_threshold: 0.3 } }
    pub fn ultrafast() -> Self { Self { iterations: 50, pot_threshold: 0.0 } }
}

impl DecisionFilter for SearchFilter {
    fn name(&self) -> &'static str { "search" }

    fn apply(&self, ctx: &DecisionContext, mut decision: Decision) -> Decision {
        let n = ctx.state.num_players as usize;
        let hero = ctx.state.acting_seat;
        let opp = if n == 2 { 1 - hero } else {
            let mut best = (hero + 1) % n as u8;
            for i in 0..n as u8 {
                if i != hero && ctx.state.bets[i as usize] > ctx.state.bets[best as usize] {
                    best = i;
                }
            }
            best
        };

        // range equity (always computed — cheap)
        if ctx.state.community_count == 5 {
            if let Some(ref range) = ctx.ranges[opp as usize] {
                decision.metadata.range_equity = Some(range.equity_vs(*ctx.hero_cards, ctx.community));
            }
        } else if ctx.state.community_count >= 3 {
            if let Some(ref range) = ctx.ranges[opp as usize] {
                let alive = range.alive_fraction();
                decision.metadata.range_equity = Some(0.5 * alive + 0.3 * (1.0 - alive));
            }
        }

        // adaptive search: scale iterations by decision importance
        let stack = ctx.state.stacks[hero as usize];
        let pot_frac = if stack > 0 { ctx.state.pot as f32 / stack as f32 } else { 0.0 };

        // skip search entirely on trivial decisions
        if pot_frac < self.pot_threshold {
            decision.metadata.layers_applied.push("L1:range+search(skip:small_pot)");
            return decision;
        }

        let entropy: f64 = decision.action_probs.iter()
            .filter(|&&p| p > 0.0)
            .map(|&p| -p * p.ln())
            .sum();
        let max_entropy = (decision.actions.len() as f64).ln();
        let confidence = 1.0 - (entropy / max_entropy.max(0.01));
        if confidence > 0.7 {
            decision.metadata.layers_applied.push("L1:range+search(skip:confident)");
            return decision;
        }

        // scale iterations by importance: bigger pot + lower confidence = more thinking
        // pot_frac 0.1 → 50 iters, 0.5 → 200, 1.0+ → 500, all-in → max
        let importance = (pot_frac * 2.0).min(2.0) * (1.0 - confidence as f32);
        let iters = ((self.iterations as f32 * importance).max(50.0) as u32).min(self.iterations);

        // add ±30% jitter so timing isn't robotic
        let jitter = {
            let seed = ctx.state.pot.wrapping_mul(31) ^ ctx.state.hand_number.wrapping_mul(97);
            0.7 + (seed % 60) as f32 / 100.0  // 0.7 to 1.3
        };
        let final_iters = ((iters as f32 * jitter) as u32).max(30);

        // run search with adaptive iterations
        // TODO: pass final_iters to bot.decide() when SearchConfig is exposed
        let search_result = ctx.bot.borrow_mut().decide(ctx.state, ctx.hero_cards, ctx.community, ctx.history);

        if !search_result.actions.is_empty() {
            let blend = 0.7;
            for (i, prob) in decision.action_probs.iter_mut().enumerate() {
                if i < search_result.action_probs.len() {
                    *prob = (1.0 - blend) * *prob + blend * search_result.action_probs[i];
                }
            }
            let total: f64 = decision.action_probs.iter().sum();
            if total > 1e-10 { for p in decision.action_probs.iter_mut() { *p /= total; } }
            decision.value = (1.0 - blend) * decision.value + blend * search_result.value;
            decision.from_blueprint = false;
        }

        decision.metadata.layers_applied.push("L1:range+search");
        decision
    }
}

// keep RangeFilter available for standalone use (L0 + range only, no search)
pub struct RangeFilter;

impl DecisionFilter for RangeFilter {
    fn name(&self) -> &'static str { "range" }

    fn apply(&self, ctx: &DecisionContext, mut decision: Decision) -> Decision {
        let n = ctx.state.num_players as usize;
        let hero = ctx.state.acting_seat;
        let opp = if n == 2 { 1 - hero } else {
            let mut best = (hero + 1) % n as u8;
            for i in 0..n as u8 {
                if i != hero && ctx.state.bets[i as usize] > ctx.state.bets[best as usize] { best = i; }
            }
            best
        };
        if ctx.state.community_count == 5 {
            if let Some(ref range) = ctx.ranges[opp as usize] {
                decision.metadata.range_equity = Some(range.equity_vs(*ctx.hero_cards, ctx.community));
            }
        } else if ctx.state.community_count >= 3 {
            if let Some(ref range) = ctx.ranges[opp as usize] {
                let alive = range.alive_fraction();
                decision.metadata.range_equity = Some(0.5 * alive + 0.3 * (1.0 - alive));
            }
        }
        decision.metadata.layers_applied.push("L1:range");
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
    pub bot: std::cell::RefCell<PokerBot>,
    pub ranges: [Option<Range>; MAX_SEATS],
    pub profiles: TableProfiles,
    pub mode: ExploitMode,
    pub filters: FilterStack,
    history: Vec<u8>,
    hand_number: u32,
    /// Hebbian plasticity: real-time adaptation to opponent patterns
    pub plasticity: [super::plasticity::PlasticityState; MAX_SEATS],
    /// Native CTM expert with sync accumulators (optional)
    pub native_ctm: Option<std::cell::RefCell<super::ctm_native::NativeCTMExpert>>,
    /// Native MoE — trained CTM experts with Hebbian (optional)
    pub native_moe: Option<std::cell::RefCell<super::moe_native::NativeMoE>>,
    /// Last MoE expert indices (for Hebbian update on hand_complete)
    last_moe_experts: [usize; 2],
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
            bot: std::cell::RefCell::new(PokerBot::from_strategy_bytes(strategy_data)),
            ranges: Default::default(),
            profiles: TableProfiles::default(),
            mode: ExploitMode::Nash,
            filters: FilterStack::new(),
            history: Vec::new(),
            hand_number: 0,
            plasticity: std::array::from_fn(|_| super::plasticity::PlasticityState::new()),
            native_ctm: None,
            native_moe: None,
            last_moe_experts: [0, 0],
        }
    }

    /// create from a pre-deserialized shared blueprint (avoids re-parsing)
    pub fn from_shared_blueprint(blueprint: &std::collections::HashMap<Vec<u8>, Vec<f64>>) -> Self {
        Self {
            bot: std::cell::RefCell::new(PokerBot::new(SearchConfig {
                blueprint: blueprint.clone(),
                ..Default::default()
            })),
            ranges: Default::default(),
            profiles: TableProfiles::default(),
            mode: ExploitMode::Nash,
            filters: FilterStack::new(),
            history: Vec::new(),
            hand_number: 0,
            plasticity: std::array::from_fn(|_| super::plasticity::PlasticityState::new()),
            native_ctm: None,
            native_moe: None,
            last_moe_experts: [0, 0],
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
            let blueprint = &self.bot.borrow().config.blueprint;
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

    /// main decision: base service (L0 blueprint) + filter stack
    ///
    /// Layer order:
    ///   L0: Blueprint lookup (base service, always)
    ///   L1: Range tracking (narrows opponent hands)
    ///   L2: Search (expensive, selective — uses narrowed range)
    ///   L3: Exploit (counter-strategy from opponent profile)
    ///   L4: MoE neural (final integrator, sees everything)
    pub fn decide(
        &mut self,
        state: &GameState,
        hero_cards: &[u8; 2],
        community: &[u8; 5],
    ) -> Decision {
        // L0: always start from blueprint (fast, Nash floor)
        let search_result = self.bot.borrow_mut().decide_blueprint_only(state, hero_cards, community, &self.history);

        let mut decision = Decision {
            action_probs: search_result.action_probs,
            actions: search_result.actions,
            value: search_result.value,
            from_blueprint: true,
            metadata: DecisionMeta {
                layers_applied: vec!["L0:blueprint"],
                ..Default::default()
            },
        };

        // apply filter stack: L1 range → L2 search → L3 exploit → L4 neural
        let ctx = DecisionContext {
            state,
            hero_cards,
            community,
            history: &self.history,
            ranges: &self.ranges,
            profiles: &self.profiles,
            bot: &self.bot,
        };

        decision = self.filters.apply(&ctx, decision);

        // L4: Native CTM with sync accumulators (if loaded)
        if let Some(ref ctm_cell) = self.native_ctm {
            let mut ctm = ctm_cell.borrow_mut();
            // Build feature vector for CTM
            let mut features = [0.0f32; 33]; // NUM_FEATURES = 33
            // TODO: extract proper features from game state
            // For now, use basic state info
            features[13] = state.pot as f32 / 1000.0;
            features[16] = state.stacks[state.acting_seat as usize] as f32 / 1000.0;
            let range_eq = decision.metadata.range_equity.unwrap_or(0.5);
            features[18] = range_eq;

            let ctm_out = ctm.forward(&features, true); // persist=true for cross-hand memory

            // Blend CTM policy with blueprint (30% CTM, 70% blueprint)
            let ctm_weight = 0.3;
            for i in 0..decision.actions.len().min(ctm_out.policy.len()) {
                decision.action_probs[i] = (1.0 - ctm_weight) * decision.action_probs[i]
                    + ctm_weight * ctm_out.policy[i] as f64;
            }
            // Renormalize
            let total: f64 = decision.action_probs.iter().sum();
            if total > 1e-10 { for p in decision.action_probs.iter_mut() { *p /= total; } }

            // Use sync novelty to modulate plasticity learning rate
            let novelty = ctm.sync_novelty(&ctm_out.sync_state);
            if novelty > 0.1 {
                decision.metadata.layers_applied.push("L4:ctm+sync(novel)");
            } else {
                decision.metadata.layers_applied.push("L4:ctm+sync");
            }
        }

        // L4b: Native MoE — trained CTM experts with Hebbian (if loaded)
        if let Some(ref moe_cell) = self.native_moe {
            let mut moe = moe_cell.borrow_mut();
            // Build 27-feature vector for MoE
            let mut features = [0.0f32; 27];
            features[0] = state.pot as f32 / 1000.0;
            features[1] = state.stacks[state.acting_seat as usize] as f32 / 1000.0;
            let street = match state.phase { crate::Phase::Preflop => 0.0, crate::Phase::Flop => 1.0, crate::Phase::Turn => 2.0, _ => 3.0 };
            features[2] = street / 3.0;
            features[3] = if state.num_players > 0 { state.bets[state.acting_seat as usize] as f32 / state.pot.max(1) as f32 } else { 0.0 };
            // Copy blueprint probs as features (teaches MoE what blueprint thinks)
            for i in 0..decision.action_probs.len().min(9) {
                features[4 + i] = decision.action_probs[i] as f32;
            }
            features[13] = decision.metadata.range_equity.unwrap_or(0.5);

            let moe_out = moe.evaluate(&features);
            self.last_moe_experts = moe_out.expert_idx;

            let blend = moe.blend_weight as f64;
            for i in 0..decision.actions.len().min(9) {
                decision.action_probs[i] = (1.0 - blend) * decision.action_probs[i]
                    + blend * moe_out.policy[i] as f64;
            }
            let total: f64 = decision.action_probs.iter().sum();
            if total > 1e-10 { for p in decision.action_probs.iter_mut() { *p /= total; } }
            decision.metadata.layers_applied.push("L4b:moe-native");
        }

        // L5: Hebbian plasticity — real-time adaptation (zero gradients)
        let hero = state.acting_seat as usize;
        let n = state.num_players as usize;
        let opp = if n == 2 { 1 - hero } else {
            (hero + 1) % n
        };
        let has_equity = decision.metadata.range_equity.unwrap_or(0.5) > 0.5;
        self.plasticity[opp].adjust_probs(
            &mut decision.action_probs,
            &decision.actions,
            has_equity,
        );
        if self.plasticity[opp].n_updates > 10 {
            decision.metadata.layers_applied.push("L5:plasticity");
        }

        decision
    }

    /// Call after a hand completes to feed outcome to plasticity + CTM Hebbian.
    /// `hero_seat` — our seat, `outcome` — +1 win, -1 loss, 0 neutral
    pub fn hand_complete(&mut self, hero_seat: u8, outcome: f32, last_action: u8, street: u8) {
        let n = 2usize; // heads-up for now
        let opp = if n == 2 { 1 - hero_seat as usize } else { 0 };

        // L5: Update plasticity (probability adjustments)
        self.plasticity[opp].update(
            &self.profiles.profiles[opp],
            last_action,
            outcome,
            street,
        );

        // L4: Hebbian weight modification on CTM (actual network rewiring)
        if let Some(ref ctm_cell) = self.native_ctm {
            let mut ctm = ctm_cell.borrow_mut();
            ctm.hebbian_update(outcome);
        }

        // L4b: Hebbian on MoE experts
        if let Some(ref moe_cell) = self.native_moe {
            let mut moe = moe_cell.borrow_mut();
            moe.hebbian_update(outcome, &self.last_moe_experts);
        }
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

    /// L0 + L1(range only) — blueprint + range tracking, no search
    pub fn with_range(strategy_data: &[u8]) -> Self {
        Self::new(strategy_data)
            .with_filters(FilterStack::new().and_then(RangeFilter))
    }

    /// L0 + L1 — blueprint + search+range (Pluribus-style)
    pub fn with_search(strategy_data: &[u8]) -> Self {
        Self::new(strategy_data)
            .with_filters(FilterStack::new().and_then(SearchFilter::default()))
    }

    /// L0 + L1 + L5 — blueprint + search+range + Hebbian plasticity
    /// ExploitFilter removed — plasticity handles adaptation (zero gradients)
    pub fn full_stack(strategy_data: &[u8]) -> Self {
        Self::new(strategy_data)
            .with_filters(
                FilterStack::new()
                    .and_then(SearchFilter::default())
                    // L3 ExploitFilter deprecated — Hebbian plasticity (L5) replaces it
                    // Profile stats feed directly into plasticity.adjust_probs()
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

    /// full L0-L3 — all layers
    /// L0 blueprint → L1 search+range → L2 exploit → L3 neural
    #[cfg(feature = "onnx")]
    pub fn full_with_neural(strategy_data: &[u8], moe: Arc<OnnxMoE>, weight: f32) -> Self {
        Self::new(strategy_data)
            .with_filters(
                FilterStack::new()
                    .and_then(SearchFilter::default())
                    .and_then(ExploitFilter)
                    .and_then(NeuralFilter { moe, weight })
            )
    }
    /// L0 + L1(search with CTM leaf eval) + L4(CTM) + L5(Hebbian)
    /// The full stack: search uses CTM for leaf evaluation,
    /// CTM runs on the actual decision, plasticity adapts.
    pub fn with_native_ctm(strategy_data: &[u8], ctm: super::ctm_native::NativeCTMExpert) -> Self {
        let ctm_arc = std::sync::Arc::new(std::sync::Mutex::new(ctm));

        let mut brain = Self::new(strategy_data)
            .with_filters(
                FilterStack::new()
                    .and_then(SearchFilter::default())
            );

        // Share CTM with search for leaf evaluation
        // Search uses persist=false (hypothetical exploration)
        brain.bot.borrow_mut().native_ctm = Some(ctm_arc.clone());

        // Brain keeps separate CTM for direct inference + Hebbian updates
        // This one uses persist=true (real decisions accumulate sync)
        let brain_ctm = {
            let locked = ctm_arc.lock().unwrap();
            super::ctm_native::NativeCTMExpert::new(
                locked.input_dim, locked.hidden_dim, locked.n_sync
            )
        };
        brain.native_ctm = Some(std::cell::RefCell::new(brain_ctm));

        brain
    }

    /// Create with native MoE (trained CTM experts + Hebbian)
    pub fn with_native_moe(strategy_data: &[u8], moe: super::moe_native::NativeMoE) -> Self {
        let mut brain = Self::new(strategy_data);
        brain.native_moe = Some(std::cell::RefCell::new(moe));
        brain
    }
}

// backward compat: BrainDecision is now Decision
pub type BrainDecision = Decision;
