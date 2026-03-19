//! Monte Carlo Counterfactual Regret Minimization (MCCFR) solver.
//!
//! Computes a Nash equilibrium strategy for heads-up NL Hold'em
//! through self-play on the poker-pvm engine.
//!
//! Architecture:
//!   1. Abstract the game into information sets (hand buckets + action history)
//!   2. Run external sampling MCCFR traversals
//!   3. Store cumulative regrets and strategy sums
//!   4. After convergence, extract average strategy → strategy table
//!   5. Export as compact binary for WASM bot
//!
//! Based on: "Monte Carlo Sampling for Regret Minimization in Extensive Games"
//! (Lanctot et al., 2009) + Pluribus blueprint strategy approach.

pub mod abstraction;
pub mod solver;
pub mod strategy;
