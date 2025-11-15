//! ZSwap: Privacy-preserving DEX based on Penumbra's design
//!
//! ## Architecture (from Penumbra)
//!
//! 1. **Batch swaps**: All swaps in a block execute as single batch (no MEV)
//! 2. **Burn-mint model**: Users burn input assets publicly, mint output privately
//! 3. **No user interaction**: Chain computes clearing price, users transmute assets
//! 4. **Concentrated liquidity**: LP positions with specific price ranges
//!
//! ## How it works
//!
//! ```text
//! Block N:
//!   1. Users submit swap intents (burn 100 DOT, want KSM)
//!   2. All burns aggregated: Σ(DOT burns) = 1000 DOT
//!   3. Chain computes clearing price from CFMM
//!   4. Users prove burn and mint output at clearing price
//! ```
//!
//! ## Privacy
//!
//! - Input amounts: PUBLIC (for now, encrypted in future upgrade)
//! - Output amounts: PRIVATE (ZK proof of correct minting)
//! - LP positions: Amounts visible, identity hidden
//!
//! ## Integration with PolkaVM
//!
//! - Swap logic runs in PolkaVM (deterministic execution)
//! - Proof generated with Ligerito (512μs verification)
//! - State transitions proven cryptographically

use serde::{Deserialize, Serialize};
use crate::consensus::BlockNumber;
use std::collections::HashMap;

/// Asset identifier (e.g., DOT, KSM, USDT)
pub type AssetId = [u8; 32];

/// Native token: ZT (Zeratul Token)
/// Used for staking, governance, and trading fees
pub const ZT_ASSET_ID: AssetId = [0u8; 32]; // All zeros = native token

/// Minimum stake required to participate in consensus
pub const MIN_STAKE_ZT: u64 = 100_000_000; // 100 ZT (assuming 6 decimals)

/// Total supply of ZT tokens
pub const TOTAL_SUPPLY_ZT: u64 = 100_000_000_000_000; // 100M ZT

/// Trading pair
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradingPair {
    pub asset_1: AssetId,
    pub asset_2: AssetId,
}

impl TradingPair {
    /// Canonical ordering (asset_1 < asset_2)
    pub fn new(a: AssetId, b: AssetId) -> Self {
        if a < b {
            Self { asset_1: a, asset_2: b }
        } else {
            Self { asset_1: b, asset_2: a }
        }
    }

    /// Get direction of swap
    pub fn direction(&self, from: AssetId) -> SwapDirection {
        if from == self.asset_1 {
            SwapDirection::OneToTwo
        } else {
            SwapDirection::TwoToOne
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapDirection {
    /// Swap asset_1 → asset_2
    OneToTwo,
    /// Swap asset_2 → asset_1
    TwoToOne,
}

/// Swap intent: user burns input, wants output
///
/// ## Privacy levels:
///
/// ### V1 (current): Semi-private
/// - burn_amount: PUBLIC (for now)
/// - output: PRIVATE (commitment)
///
/// ### V2 (future): Fully private
/// - burn_amount: ENCRYPTED (Pedersen commitment)
/// - output: PRIVATE (commitment)
/// - Use range proofs to prove amounts are valid
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapIntent {
    /// Trading pair
    pub pair: TradingPair,
    /// Direction of swap
    pub direction: SwapDirection,
    /// Amount to burn (PUBLIC in V1, will be commitment in V2)
    pub burn_amount: u64,
    /// Minimum output amount (slippage protection)
    pub min_output: u64,
    /// User's commitment (private output address)
    pub output_commitment: [u8; 32],
    /// Block number when submitted
    pub block_number: BlockNumber,
}

/// Private swap intent (V2 - future upgrade)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateSwapIntent {
    /// Trading pair (PUBLIC)
    pub pair: TradingPair,
    /// Direction (PUBLIC)
    pub direction: SwapDirection,
    /// Encrypted burn amount (PRIVATE)
    pub burn_commitment: crate::privacy::PedersenCommitment,
    /// Range proof (proves amount is reasonable)
    pub burn_proof: crate::privacy::RangeProof,
    /// Blinding factor (kept secret by user)
    #[serde(skip)]
    pub blinding: Option<curve25519_dalek::scalar::Scalar>,
    /// Output commitment (PRIVATE)
    pub output_commitment: [u8; 32],
    /// Block number
    pub block_number: BlockNumber,
}

/// Batch swap output data (Penumbra model)
///
/// ## How it works:
///
/// 1. All swaps aggregated to:
///    - delta_1: total amount of asset_1 swapping for asset_2
///    - delta_2: total amount of asset_2 swapping for asset_1
///
/// 2. Execute aggregate swaps against liquidity:
///    - delta_1 → lambda_2 output (asset_2)
///    - delta_2 → lambda_1 output (asset_1)
///
/// 3. Users claim pro-rata:
///    - If you contributed X% of delta_1, you get X% of lambda_2
///    - Clearing price: p = lambda_2 / delta_1
///
/// This is how Penumbra eliminates MEV!
#[derive(Debug, Clone)]
pub struct BatchSwapOutputData {
    /// Trading pair
    pub trading_pair: TradingPair,
    /// Block height
    pub height: BlockNumber,
    /// Total input: asset_1 → asset_2
    pub delta_1: u64,
    /// Total input: asset_2 → asset_1
    pub delta_2: u64,
    /// Total output: asset_1 (from delta_2 swaps)
    pub lambda_1: u64,
    /// Total output: asset_2 (from delta_1 swaps)
    pub lambda_2: u64,
    /// Unfilled amount of asset_1 (insufficient liquidity)
    pub unfilled_1: u64,
    /// Unfilled amount of asset_2 (insufficient liquidity)
    pub unfilled_2: u64,
}

impl BatchSwapOutputData {
    /// Get effective clearing price for asset_1 → asset_2
    /// Returns asset_2 per asset_1
    pub fn clearing_price_1_for_2(&self) -> f64 {
        if self.delta_1 == 0 {
            return 0.0;
        }
        self.lambda_2 as f64 / self.delta_1 as f64
    }

    /// Get effective clearing price for asset_2 → asset_1
    /// Returns asset_1 per asset_2
    pub fn clearing_price_2_for_1(&self) -> f64 {
        if self.delta_2 == 0 {
            return 0.0;
        }
        self.lambda_1 as f64 / self.delta_2 as f64
    }

    /// Calculate output for individual swap (pro-rata claim)
    ///
    /// User contributed `input_amount` to the batch,
    /// gets proportional share of total output.
    pub fn pro_rata_output(&self, input_amount: u64, direction: SwapDirection) -> u64 {
        match direction {
            SwapDirection::OneToTwo => {
                if self.delta_1 == 0 {
                    return 0;
                }
                // User gets (input_amount / delta_1) * lambda_2
                ((input_amount as f64 / self.delta_1 as f64) * self.lambda_2 as f64) as u64
            }
            SwapDirection::TwoToOne => {
                if self.delta_2 == 0 {
                    return 0;
                }
                ((input_amount as f64 / self.delta_2 as f64) * self.lambda_1 as f64) as u64
            }
        }
    }
}

/// Concentrated liquidity position (like Uniswap v3)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidityPosition {
    /// Position ID (hash of position params)
    pub id: [u8; 32],
    /// Trading pair
    pub pair: TradingPair,
    /// Amount of asset_1
    pub reserves_1: u64,
    /// Amount of asset_2
    pub reserves_2: u64,
    /// Price range lower bound
    pub price_lower: f64,
    /// Price range upper bound
    pub price_upper: f64,
    /// Fee tier (basis points, e.g., 30 = 0.3%)
    pub fee_bps: u16,
}

impl LiquidityPosition {
    /// Check if price is within range
    pub fn is_active(&self, price: f64) -> bool {
        price >= self.price_lower && price <= self.price_upper
    }

    /// Get liquidity at price (constant product formula)
    pub fn get_liquidity(&self, price: f64) -> f64 {
        if !self.is_active(price) {
            return 0.0;
        }

        // L = sqrt(x * y) for constant product
        (self.reserves_1 as f64 * self.reserves_2 as f64).sqrt()
    }
}

/// Staking position for consensus participation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakePosition {
    /// Validator's public key (Ed25519)
    pub validator_pubkey: [u8; 32],
    /// Amount of ZT staked
    pub stake_amount: u64,
    /// Block when stake was deposited
    pub staked_at_block: BlockNumber,
    /// Delegators (for light clients)
    pub delegators: Vec<Delegation>,
}

/// Delegation to a validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    /// Delegator's public key
    pub delegator_pubkey: [u8; 32],
    /// Amount delegated
    pub amount: u64,
}

impl StakePosition {
    /// Total stake including delegations
    pub fn total_stake(&self) -> u64 {
        self.stake_amount + self.delegators.iter().map(|d| d.amount).sum::<u64>()
    }
}

/// DEX state (all liquidity positions + staking)
#[derive(Debug, Clone)]
pub struct DexState {
    /// All liquidity positions by pair
    positions: HashMap<TradingPair, Vec<LiquidityPosition>>,
    /// Pending swaps for current block
    pending_swaps: HashMap<TradingPair, Vec<SwapIntent>>,
    /// Active stakes for consensus
    stakes: HashMap<[u8; 32], StakePosition>,
}

impl DexState {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            pending_swaps: HashMap::new(),
            stakes: HashMap::new(),
        }
    }

    /// Add or update stake position
    pub fn add_stake(&mut self, stake: StakePosition) {
        self.stakes.insert(stake.validator_pubkey, stake);
    }

    /// Remove stake (unstake)
    pub fn remove_stake(&mut self, validator_pubkey: &[u8; 32]) -> Option<StakePosition> {
        self.stakes.remove(validator_pubkey)
    }

    /// Get total staked ZT across all validators
    pub fn total_stake(&self) -> u64 {
        self.stakes.values().map(|s| s.total_stake()).sum()
    }

    /// Get validator's stake
    pub fn get_stake(&self, validator_pubkey: &[u8; 32]) -> Option<&StakePosition> {
        self.stakes.get(validator_pubkey)
    }

    /// Add liquidity position
    pub fn add_position(&mut self, pos: LiquidityPosition) {
        self.positions
            .entry(pos.pair)
            .or_insert_with(Vec::new)
            .push(pos);
    }

    /// Add swap intent to current batch
    pub fn add_swap(&mut self, swap: SwapIntent) {
        self.pending_swaps
            .entry(swap.pair)
            .or_insert_with(Vec::new)
            .push(swap);
    }

    /// Execute batch swaps for a trading pair (Penumbra exact model)
    ///
    /// ## How Penumbra does it:
    ///
    /// 1. **Aggregate all swaps** (order doesn't matter!)
    ///    - delta_1 = Σ all asset_1 → asset_2 burns
    ///    - delta_2 = Σ all asset_2 → asset_1 burns
    ///
    /// 2. **Execute aggregates against liquidity**
    ///    - delta_1 → lambda_2 output
    ///    - delta_2 → lambda_1 output
    ///
    /// 3. **Users claim pro-rata**
    ///    - Contributed X% of delta → get X% of lambda
    ///    - Clearing price: p = lambda / delta
    ///
    /// This is MEV-proof because:
    /// - Leader can reorder swaps however they want
    /// - Doesn't matter! All aggregate to same delta
    /// - Everyone gets same clearing price
    /// - No frontrunning, no sandwich attacks possible
    pub fn execute_batch(&mut self, pair: TradingPair, block: BlockNumber) -> Option<BatchSwapOutputData> {
        let swaps = self.pending_swaps.remove(&pair)?;
        if swaps.is_empty() {
            return None;
        }

        // Step 1: Aggregate to delta_1 and delta_2 (Penumbra!)
        let mut delta_1 = 0u64;
        let mut delta_2 = 0u64;

        for swap in &swaps {
            match swap.direction {
                SwapDirection::OneToTwo => delta_1 += swap.burn_amount,
                SwapDirection::TwoToOne => delta_2 += swap.burn_amount,
            }
        }

        // Get active liquidity positions
        let positions = self.positions.get(&pair)?;

        // Total reserves
        let total_reserves_1: u64 = positions.iter().map(|p| p.reserves_1).sum();
        let total_reserves_2: u64 = positions.iter().map(|p| p.reserves_2).sum();

        if total_reserves_1 == 0 || total_reserves_2 == 0 {
            return None;
        }

        // Step 2: Execute delta_1 and delta_2 against liquidity
        //
        // Simplified constant product AMM (Penumbra uses routing across
        // concentrated liquidity positions, but concept is same)

        // Execute delta_1 (asset_1 → asset_2) to get lambda_2
        let k = total_reserves_1 as f64 * total_reserves_2 as f64;
        let new_reserves_1_after_delta1 = total_reserves_1 + delta_1;
        let new_reserves_2_after_delta1 = (k / new_reserves_1_after_delta1 as f64) as u64;
        let lambda_2 = total_reserves_2.saturating_sub(new_reserves_2_after_delta1);

        // Execute delta_2 (asset_2 → asset_1) to get lambda_1
        // Starting from state after delta_1 execution
        let k2 = new_reserves_1_after_delta1 as f64 * new_reserves_2_after_delta1 as f64;
        let new_reserves_2_final = new_reserves_2_after_delta1 + delta_2;
        let new_reserves_1_final = (k2 / new_reserves_2_final as f64) as u64;
        let lambda_1 = new_reserves_1_after_delta1.saturating_sub(new_reserves_1_final);

        // For this simple version, assume everything fills
        // (Penumbra handles unfilled amounts when liquidity is insufficient)
        let unfilled_1 = 0;
        let unfilled_2 = 0;

        Some(BatchSwapOutputData {
            trading_pair: pair,
            height: block,
            delta_1,
            delta_2,
            lambda_1,
            lambda_2,
            unfilled_1,
            unfilled_2,
        })
    }

    /// Clear all pending swaps (called at end of block)
    pub fn finalize_block(&mut self, block: BlockNumber) -> Vec<BatchSwapOutputData> {
        let pairs: Vec<_> = self.pending_swaps.keys().copied().collect();

        pairs.iter()
            .filter_map(|&pair| self.execute_batch(pair, block))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trading_pair_canonical() {
        let dot = [1u8; 32];
        let ksm = [2u8; 32];

        let pair1 = TradingPair::new(dot, ksm);
        let pair2 = TradingPair::new(ksm, dot);

        assert_eq!(pair1, pair2); // Same pair regardless of order
    }

    #[test]
    fn test_batch_swap_execution() {
        let mut dex = DexState::new();

        let dot = [1u8; 32];
        let ksm = [2u8; 32];
        let pair = TradingPair::new(dot, ksm);

        // Add liquidity: 1000 DOT, 2000 KSM (price = 2.0 KSM/DOT)
        dex.add_position(LiquidityPosition {
            id: [0; 32],
            pair,
            reserves_1: 1000,
            reserves_2: 2000,
            price_lower: 1.0,
            price_upper: 3.0,
            fee_bps: 30,
        });

        // User 1: Sell 100 DOT for KSM
        dex.add_swap(SwapIntent {
            pair,
            direction: SwapDirection::OneToTwo,
            burn_amount: 100,
            min_output: 150,
            output_commitment: [1; 32],
            block_number: 1,
        });

        // Execute batch (Penumbra model!)
        let batch = dex.execute_batch(pair, 1).unwrap();

        println!("\nPenumbra batch execution:");
        println!("delta_1 (input): {} DOT", batch.delta_1);
        println!("lambda_2 (output): {} KSM", batch.lambda_2);
        println!("Clearing price: {} KSM/DOT", batch.clearing_price_1_for_2());

        // User claims pro-rata
        let user_output = batch.pro_rata_output(100, SwapDirection::OneToTwo);
        println!("User receives: {} KSM", user_output);

        assert_eq!(batch.delta_1, 100);
        assert!(batch.lambda_2 > 150);
        assert!(batch.clearing_price_1_for_2() > 0.0);
        assert_eq!(user_output, batch.lambda_2); // Only user in batch, gets 100%
    }

    #[test]
    fn test_batch_swap_pro_rata_distribution() {
        let mut dex = DexState::new();

        let dot = [1u8; 32];
        let ksm = [2u8; 32];
        let pair = TradingPair::new(dot, ksm);

        // Add liquidity: 10000 DOT, 20000 KSM (price = 2.0 KSM/DOT)
        dex.add_position(LiquidityPosition {
            id: [0; 32],
            pair,
            reserves_1: 10000,
            reserves_2: 20000,
            price_lower: 1.0,
            price_upper: 3.0,
            fee_bps: 30,
        });

        // Alice: Sell 100 DOT (10% of batch)
        dex.add_swap(SwapIntent {
            pair,
            direction: SwapDirection::OneToTwo,
            burn_amount: 100,
            min_output: 0,
            output_commitment: [1; 32],
            block_number: 1,
        });

        // Bob: Sell 400 DOT (40% of batch)
        dex.add_swap(SwapIntent {
            pair,
            direction: SwapDirection::OneToTwo,
            burn_amount: 400,
            min_output: 0,
            output_commitment: [2; 32],
            block_number: 1,
        });

        // Charlie: Sell 500 DOT (50% of batch)
        dex.add_swap(SwapIntent {
            pair,
            direction: SwapDirection::OneToTwo,
            burn_amount: 500,
            min_output: 0,
            output_commitment: [3; 32],
            block_number: 1,
        });

        // Execute batch (Penumbra!)
        let batch = dex.execute_batch(pair, 1).unwrap();

        println!("\n=== Penumbra Pro-rata Distribution ===");
        println!("Total input (delta_1): {} DOT", batch.delta_1);
        println!("Total output (lambda_2): {} KSM", batch.lambda_2);
        println!("Clearing price: {} KSM/DOT", batch.clearing_price_1_for_2());

        // Users claim pro-rata based on their contribution
        let alice_output = batch.pro_rata_output(100, SwapDirection::OneToTwo);
        let bob_output = batch.pro_rata_output(400, SwapDirection::OneToTwo);
        let charlie_output = batch.pro_rata_output(500, SwapDirection::OneToTwo);

        println!("\nPro-rata claims:");
        println!("Alice (100 DOT, 10%): {} KSM", alice_output);
        println!("Bob (400 DOT, 40%): {} KSM", bob_output);
        println!("Charlie (500 DOT, 50%): {} KSM", charlie_output);

        // Check pro-rata: Bob should get ~4x Alice, Charlie ~5x Alice
        let alice_per_dot = alice_output as f64 / 100.0;
        let bob_per_dot = bob_output as f64 / 400.0;
        let charlie_per_dot = charlie_output as f64 / 500.0;

        println!("\nEffective price per DOT:");
        println!("Alice: {}", alice_per_dot);
        println!("Bob: {}", bob_per_dot);
        println!("Charlie: {}", charlie_per_dot);

        // All should get EXACT same price (clearing price!)
        assert!((alice_per_dot - bob_per_dot).abs() < 0.01);
        assert!((alice_per_dot - charlie_per_dot).abs() < 0.01);
        assert!((bob_per_dot - charlie_per_dot).abs() < 0.01);

        // Total outputs should equal lambda_2
        assert_eq!(alice_output + bob_output + charlie_output, batch.lambda_2);

        println!("\n✅ MEV-PROOF:");
        println!("   - Leader can reorder swaps however");
        println!("   - All aggregate to same delta_1 = {}", batch.delta_1);
        println!("   - All get same clearing price = {:.4}", batch.clearing_price_1_for_2());
        println!("   - No frontrunning possible!");
    }

    #[test]
    fn test_concentrated_liquidity_range() {
        let dot = [1u8; 32];
        let ksm = [2u8; 32];

        let pos = LiquidityPosition {
            id: [0; 32],
            pair: TradingPair::new(dot, ksm),
            reserves_1: 1000,
            reserves_2: 2000,
            price_lower: 1.5,
            price_upper: 2.5,
            fee_bps: 30,
        };

        assert!(pos.is_active(2.0));
        assert!(!pos.is_active(1.0));
        assert!(!pos.is_active(3.0));
    }
}
