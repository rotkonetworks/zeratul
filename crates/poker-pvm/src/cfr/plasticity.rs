//! Hebbian plasticity for real-time opponent adaptation.
//!
//! Core mechanism (proven at 0.193 plasticity score in Python bench):
//!   1. Track baseline opponent behavior (expected action frequencies)
//!   2. Compute sync novelty: how much recent actions deviate from baseline
//!   3. Weight update: delta = lr * novelty * outcome
//!   4. Apply to decision adjustments — no gradients needed
//!
//! This lets the bot learn "this player folds to river bets" within 10-20 hands
//! and start bluffing more on the river, without any retraining.

use crate::PlayerProfile;

/// Plasticity state — tracks deviation from expected behavior
#[derive(Clone, Debug)]
pub struct PlasticityState {
    /// baseline expectations (running average of opponent stats)
    baseline_vpip: f32,
    baseline_pfr: f32,
    baseline_af: f32,
    baseline_wtsd: f32,
    baseline_fold_to_bet: f32,

    /// weight adjustments learned from experience
    /// these shift the decision probabilities
    pub bluff_adjustment: f32,      // positive = bluff more, negative = bluff less
    pub value_bet_adjustment: f32,  // positive = value bet thinner
    pub fold_adjustment: f32,       // positive = fold more to aggression
    pub call_adjustment: f32,       // positive = call lighter (they're bluffing)

    /// learning rate (decays over time)
    lr: f32,
    /// decay factor for baseline updates
    baseline_decay: f32,
    /// number of updates applied
    pub n_updates: u32,
}

impl PlasticityState {
    pub fn new() -> Self {
        Self {
            // start with "average" player baseline
            baseline_vpip: 0.30,
            baseline_pfr: 0.20,
            baseline_af: 1.0,
            baseline_wtsd: 0.30,
            baseline_fold_to_bet: 0.40,

            bluff_adjustment: 0.0,
            value_bet_adjustment: 0.0,
            fold_adjustment: 0.0,
            call_adjustment: 0.0,

            lr: 0.05,            // initial learning rate
            baseline_decay: 0.95, // exponential moving average
            n_updates: 0,
        }
    }

    /// Compute sync novelty: how surprising is the opponent's current behavior?
    /// Returns a value [0, 1] where 0 = exactly as expected, 1 = maximally surprising.
    pub fn novelty(&self, profile: &PlayerProfile) -> f32 {
        if profile.hands_seen < 3 { return 0.0; } // need data first

        let vpip_dev = (profile.vpip() - self.baseline_vpip).abs();
        let pfr_dev = (profile.pfr() - self.baseline_pfr).abs();
        let af_dev = (profile.aggression_factor().min(5.0) / 5.0
                     - self.baseline_af.min(5.0) / 5.0).abs();
        let wtsd_dev = (profile.wtsd() - self.baseline_wtsd).abs();

        // Average deviation, clamped to [0, 1]
        ((vpip_dev + pfr_dev + af_dev + wtsd_dev) / 4.0).min(1.0)
    }

    /// Update after observing a hand outcome.
    ///
    /// `profile` — current opponent profile (updated with latest actions)
    /// `our_action` — what we did (fold/check/bet/raise/allin)
    /// `outcome` — result: +1.0 (won pot), -1.0 (lost pot), 0.0 (folded/no showdown)
    /// `street` — which street (0=preflop, 1=flop, 2=turn, 3=river)
    pub fn update(&mut self, profile: &PlayerProfile, our_action: u8, outcome: f32, street: u8) {
        if profile.hands_seen < 5 { return; } // wait for minimal data

        let novelty = self.novelty(profile);
        let effective_lr = self.lr * (1.0 + novelty); // learn faster when surprised

        // Update baseline (exponential moving average)
        self.baseline_vpip = self.baseline_decay * self.baseline_vpip
            + (1.0 - self.baseline_decay) * profile.vpip();
        self.baseline_pfr = self.baseline_decay * self.baseline_pfr
            + (1.0 - self.baseline_decay) * profile.pfr();
        self.baseline_af = self.baseline_decay * self.baseline_af
            + (1.0 - self.baseline_decay) * profile.aggression_factor().min(5.0);
        self.baseline_wtsd = self.baseline_decay * self.baseline_wtsd
            + (1.0 - self.baseline_decay) * profile.wtsd();

        // Compute fold-to-bet ratio (key exploitability signal)
        let fold_to_bet = if profile.postflop_bets + profile.postflop_calls + profile.postflop_folds > 0 {
            profile.postflop_folds as f32
                / (profile.postflop_bets + profile.postflop_calls + profile.postflop_folds) as f32
        } else { 0.4 };
        self.baseline_fold_to_bet = self.baseline_decay * self.baseline_fold_to_bet
            + (1.0 - self.baseline_decay) * fold_to_bet;

        // Hebbian weight updates based on what we observe:
        //
        // "opponent folds a lot" + "our bluffs succeeded" → bluff more
        // "opponent calls everything" + "our bluffs failed" → stop bluffing
        // "opponent is aggressive" + "we folded and they showed bluff" → call more
        // "opponent is passive" + "we called and lost" → fold more

        if fold_to_bet > 0.50 {
            // Opponent folds too much → increase bluffing
            self.bluff_adjustment += effective_lr * 0.1;
        } else if fold_to_bet < 0.25 {
            // Opponent rarely folds → decrease bluffing, increase value betting
            self.bluff_adjustment -= effective_lr * 0.1;
            self.value_bet_adjustment += effective_lr * 0.05;
        }

        if profile.aggression_factor() > 2.5 {
            // Opponent very aggressive → they might be bluffing → call lighter
            self.call_adjustment += effective_lr * 0.05;
        } else if profile.aggression_factor() < 0.5 {
            // Opponent very passive → their bets are real → fold more to aggression
            self.fold_adjustment += effective_lr * 0.05;
        }

        // Outcome feedback: reinforce successful adjustments
        if outcome > 0.0 {
            // We won — reinforce current adjustments slightly
            self.bluff_adjustment *= 1.01;
            self.value_bet_adjustment *= 1.01;
        } else if outcome < 0.0 {
            // We lost — dampen current adjustments slightly
            self.bluff_adjustment *= 0.99;
            self.fold_adjustment *= 1.01;
        }

        // Clamp all adjustments to [-0.3, 0.3]
        self.bluff_adjustment = self.bluff_adjustment.clamp(-0.3, 0.3);
        self.value_bet_adjustment = self.value_bet_adjustment.clamp(-0.3, 0.3);
        self.fold_adjustment = self.fold_adjustment.clamp(-0.3, 0.3);
        self.call_adjustment = self.call_adjustment.clamp(-0.3, 0.3);

        // Decay learning rate slowly
        self.lr *= 0.999;
        self.lr = self.lr.max(0.005); // floor

        self.n_updates += 1;
    }

    /// Apply plasticity adjustments to a decision's action probabilities.
    ///
    /// `probs` — mutable action probabilities from the brain
    /// `actions` — corresponding actions
    /// `has_equity` — do we have a strong hand? (range equity > 0.5)
    pub fn adjust_probs(
        &self,
        probs: &mut [f64],
        actions: &[(crate::Action, u32)],
        has_equity: bool,
    ) {
        if self.n_updates < 10 { return; } // don't adjust until we have enough data

        for (i, (action, _amount)) in actions.iter().enumerate() {
            match action {
                crate::Action::Fold => {
                    // Adjust fold frequency based on opponent aggression
                    probs[i] *= (1.0 + self.fold_adjustment as f64);
                    // If we think they're bluffing, fold less
                    if self.call_adjustment > 0.05 {
                        probs[i] *= (1.0 - self.call_adjustment as f64 * 0.5);
                    }
                }
                crate::Action::Check => {
                    // Check more against aggressive opponents (trap)
                    if self.call_adjustment > 0.1 && has_equity {
                        probs[i] *= 1.1;
                    }
                }
                crate::Action::Call => {
                    // Call more if we think opponent is bluffing
                    probs[i] *= (1.0 + self.call_adjustment as f64);
                }
                crate::Action::Bet | crate::Action::Raise => {
                    if has_equity {
                        // Value bet adjustment
                        probs[i] *= (1.0 + self.value_bet_adjustment as f64);
                    } else {
                        // Bluff adjustment
                        probs[i] *= (1.0 + self.bluff_adjustment as f64);
                    }
                }
                crate::Action::AllIn => {
                    // Don't adjust all-in much — too high variance
                    if has_equity && self.value_bet_adjustment > 0.1 {
                        probs[i] *= 1.05;
                    }
                }
            }
        }

        // Renormalize
        let total: f64 = probs.iter().sum();
        if total > 1e-10 {
            for p in probs.iter_mut() { *p /= total; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plasticity_adapts_to_folder() {
        let mut state = PlasticityState::new();
        let mut profile = PlayerProfile::default();

        // Simulate a player who folds a lot
        for _ in 0..30 {
            profile.hands_seen += 1;
            profile.postflop_folds += 1;
            state.update(&profile, 0, 0.5, 1);
        }

        assert!(state.bluff_adjustment > 0.0, "should learn to bluff more vs folder");
        assert!(state.n_updates > 20);
    }

    #[test]
    fn test_plasticity_adapts_to_caller() {
        let mut state = PlasticityState::new();
        let mut profile = PlayerProfile::default();

        // Simulate a calling station
        for _ in 0..30 {
            profile.hands_seen += 1;
            profile.vpip_count += 1;
            profile.postflop_calls += 1;
            state.update(&profile, 0, -0.5, 1);
        }

        assert!(state.bluff_adjustment < 0.0, "should learn to stop bluffing vs caller");
        assert!(state.value_bet_adjustment > 0.0, "should value bet thinner");
    }

    #[test]
    fn test_novelty_detection() {
        let state = PlasticityState::new();

        // Average player — low novelty
        let mut avg = PlayerProfile::default();
        avg.hands_seen = 20;
        avg.vpip_count = 6;  // 30%
        avg.pfr_count = 4;   // 20%
        assert!(state.novelty(&avg) < 0.2);

        // Maniac — high novelty
        let mut maniac = PlayerProfile::default();
        maniac.hands_seen = 20;
        maniac.vpip_count = 18;  // 90%
        maniac.pfr_count = 14;   // 70%
        maniac.postflop_bets = 30;
        maniac.postflop_calls = 2;
        assert!(state.novelty(&maniac) > 0.3);
    }
}
