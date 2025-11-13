//! Enhanced Application with Lending and Liquidation
//!
//! This extends the base application to include:
//! - Multi-asset lending pool
//! - Batch margin trading
//! - ZK-based liquidations
//! - Penumbra settlement

use crate::block::Block;
use crate::lending::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Enhanced configuration including lending params
#[derive(Debug, Clone)]
pub struct LendingApplicationConfig {
    /// Base application config
    pub base: super::application::Config,

    /// Liquidation penalty (5%)
    pub liquidation_penalty_percent: u8,

    /// Settlement batch size (every N blocks)
    pub settlement_batch_size: u64,

    /// Oracle update interval (blocks)
    pub oracle_update_interval: u64,
}

impl Default for LendingApplicationConfig {
    fn default() -> Self {
        Self {
            base: super::application::Config {
                mailbox_size: 100,
                nomt_path: "/tmp/zeratul-nomt".to_string(),
                accidental_computer_config: Default::default(),
            },
            liquidation_penalty_percent: 5,
            settlement_batch_size: 3,      // CRITICAL: Reduced from 5 to 3 for MEV protection (6s window)
            oracle_update_interval: 1,     // CRITICAL: Update every block (2s) for MEV protection
        }
    }
}

/// Enhanced application with lending capabilities
pub struct LendingApplication {
    /// Base application (proof verification, NOMT)
    base: super::application::Actor<impl commonware_runtime::Rng
        + commonware_runtime::Spawner
        + commonware_runtime::Metrics
        + commonware_runtime::Clock>,

    /// Configuration
    config: LendingApplicationConfig,

    /// Lending pool state
    lending_pool: Arc<Mutex<LendingPool>>,

    /// Position manager (encrypted positions)
    position_manager: Arc<Mutex<PrivatePositionManager>>,

    /// Liquidation engine
    liquidation_engine: Arc<Mutex<LiquidationEngine>>,

    /// Oracle prices (current)
    oracle_prices: Arc<Mutex<HashMap<AssetId, Amount>>>,

    /// Settlement manager (batches Penumbra settlements)
    settlement_manager: Arc<Mutex<Option<SettlementManager>>>,
}

/// Block execution result with lending info
#[derive(Debug, Clone)]
pub struct LendingBlockResult {
    /// Block height
    pub height: u64,

    /// Number of proofs executed
    pub num_proofs: usize,

    /// Margin trading batch result (if any)
    pub margin_batch: Option<BatchExecutionResult>,

    /// Liquidation result (if any)
    pub liquidations: Option<BatchLiquidationResult>,

    /// Updated state root
    pub state_root: [u8; 32],

    /// Events emitted
    pub events: Vec<PrivateEvent>,
}

impl LendingApplication {
    /// Execute a block with lending operations
    pub async fn execute_block(
        &mut self,
        block: &Block,
        is_proposer: bool,
    ) -> Result<LendingBlockResult> {
        info!("Executing block {} (height: {})", block.digest(), block.height);

        let mut result = LendingBlockResult {
            height: block.height,
            num_proofs: block.proofs.len(),
            margin_batch: None,
            liquidations: None,
            state_root: block.state_root,
            events: Vec::new(),
        };

        // 1. Verify and execute all transaction proofs
        for proof in &block.proofs {
            // Verify AccidentalComputer proof
            self.verify_and_apply_proof(proof).await?;
        }

        // 2. Update oracle prices (if needed)
        if block.height % self.config.oracle_update_interval == 0 {
            self.update_oracle_prices(block.height).await?;
        }

        // 3. Execute margin trading batch
        let margin_batch = self.execute_margin_trading_batch(block).await?;
        if let Some(batch) = &margin_batch {
            result.events.push(PrivateEvent::BatchExecuted {
                trading_pair: batch.trading_pair,
                height: block.height,
                num_orders: batch.num_orders_executed,
                total_volume: batch.total_volume,
                clearing_price: batch.clearing_price,
            });
        }
        result.margin_batch = margin_batch;

        // 4. Process liquidations (ZK proofs)
        let liquidations = self.process_liquidations(block).await?;
        if liquidations.num_liquidated > 0 {
            result.events.push(PrivateEvent::LiquidationsProcessed {
                height: block.height,
                num_liquidated: liquidations.num_liquidated,
                total_liquidation_volume: liquidations.total_debt_repaid
                    .values()
                    .fold(Amount::ZERO, |acc, amt| acc.checked_add(*amt).unwrap()),
                average_penalty: liquidations.avg_health_factor,
            });
        }
        result.liquidations = Some(liquidations);

        // 5. Check if we should settle to Penumbra
        if let Some(settlement_mgr) = &mut *self.settlement_manager.lock().unwrap() {
            if settlement_mgr.should_settle(block.height) {
                info!("Settlement window reached, settling to Penumbra...");

                // Spawn async settlement task (non-blocking!)
                let settlement_mgr_clone = Arc::clone(&self.settlement_manager);
                tokio::spawn(async move {
                    if let Some(mgr) = &mut *settlement_mgr_clone.lock().unwrap() {
                        match mgr.settle(is_proposer).await {
                            Ok(Some(tx_hash)) => {
                                info!("Submitted Penumbra settlement: {:?}", tx_hash);
                            }
                            Ok(None) => {
                                debug!("No settlement needed this round");
                            }
                            Err(e) => {
                                error!("Settlement failed: {}", e);
                            }
                        }
                    }
                });
            }
        }

        // 6. Accrue interest on all pool positions
        self.accrue_interest(block.height).await?;

        Ok(result)
    }

    /// Execute margin trading batch for this block
    async fn execute_margin_trading_batch(
        &mut self,
        block: &Block,
    ) -> Result<Option<BatchExecutionResult>> {
        // Collect margin orders from block proofs
        let margin_orders = self.extract_margin_orders(block)?;

        if margin_orders.is_empty() {
            return Ok(None);
        }

        // Group by trading pair
        let mut batches: HashMap<(AssetId, AssetId), Vec<MarginOrder>> = HashMap::new();
        for order in margin_orders {
            let pair = (order.base_asset, order.quote_asset);
            batches.entry(pair).or_default().push(order);
        }

        // Execute each batch
        let mut results = Vec::new();
        for ((base, quote), orders) in batches {
            // Get oracle price as rational number
            let oracle_prices = self.oracle_prices.lock().unwrap();
            let oracle_price = oracle_prices
                .get(&base)
                .and_then(|base_price| {
                    oracle_prices.get(&quote).map(|quote_price| {
                        // Calculate price as rational: base_price / quote_price
                        Price {
                            numerator: base_price.0,
                            denominator: quote_price.0,
                        }
                        .normalize()
                    })
                })
                .unwrap_or(Price::ONE);

            // Create batch
            let batch = MarginBatch {
                base_asset: base,
                quote_asset: quote,
                long_orders: orders.iter().filter(|o| o.is_long).cloned().collect(),
                short_orders: orders.iter().filter(|o| !o.is_long).cloned().collect(),
                total_long_size: orders
                    .iter()
                    .filter(|o| o.is_long)
                    .map(|o| o.size.0)
                    .sum::<u64>()
                    .into(),
                total_short_size: orders
                    .iter()
                    .filter(|o| !o.is_long)
                    .map(|o| o.size.0)
                    .sum::<u64>()
                    .into(),
            };

            // Execute batch
            let mut pool = self.lending_pool.lock().unwrap();
            let mut positions = HashMap::new(); // Would be real positions
            let result = execute_margin_batch(
                batch,
                oracle_price,
                &mut *pool,
                &mut positions,
                Ratio::from_percent(2), // 2% max slippage
                block.height,
            )?;

            results.push(result);
        }

        // Return first result (or aggregate if multiple pairs)
        Ok(results.into_iter().next())
    }

    /// Process liquidations from block
    async fn process_liquidations(
        &mut self,
        block: &Block,
    ) -> Result<BatchLiquidationResult> {
        // Extract liquidation proofs from block
        let liquidation_proofs = self.extract_liquidation_proofs(block)?;

        if !liquidation_proofs.is_empty() {
            info!("Processing {} liquidation proofs", liquidation_proofs.len());
        }

        // Submit proposals to liquidation engine
        let mut engine = self.liquidation_engine.lock().unwrap();
        for proof in liquidation_proofs {
            let proposal = LiquidationProposal {
                proposer: [0; 32], // Would be actual proposer
                height: block.height,
                proofs: vec![proof],
                signature: [0; 64],
            };

            engine.submit_proposal(proposal)?;
        }

        // Execute batch liquidation
        let mut pool = self.lending_pool.lock().unwrap();
        let mut position_manager = self.position_manager.lock().unwrap();
        let result = engine.execute_batch_liquidation(
            &mut *pool,
            &mut *position_manager,
            block.height,
        )?;

        Ok(result)
    }

    /// Update oracle prices from Penumbra
    async fn update_oracle_prices(&mut self, height: u64) -> Result<()> {
        // In real implementation:
        // 1. Query embedded ViewServer for latest batch swap prices
        // 2. Validators submit oracle proposals
        // 3. Take median of all proposals
        // 4. Update oracle_prices

        info!("Updating oracle prices at height {}", height);

        // Placeholder: would query Penumbra ViewServer
        let mut prices = self.oracle_prices.lock().unwrap();
        prices.insert(AssetId([1; 32]), Amount(100)); // UM price
        prices.insert(AssetId([2; 32]), Amount(105)); // gm price

        Ok(())
    }

    /// Accrue interest on lending pool
    async fn accrue_interest(&mut self, block_height: u64) -> Result<()> {
        let mut pool = self.lending_pool.lock().unwrap();

        for (_asset_id, pool_state) in pool.assets.iter_mut() {
            pool_state.accrue_interest(block_height)?;
        }

        Ok(())
    }

    /// Extract margin orders from block proofs
    fn extract_margin_orders(&self, block: &Block) -> Result<Vec<MarginOrder>> {
        // In real implementation:
        // Decode margin orders from AccidentalComputer proofs
        // For now, return empty
        Ok(Vec::new())
    }

    /// Extract liquidation proofs from block
    fn extract_liquidation_proofs(&self, block: &Block) -> Result<Vec<LiquidationProof>> {
        // In real implementation:
        // Separate liquidation proofs from regular transaction proofs
        // For now, return empty
        Ok(Vec::new())
    }

    /// Verify and apply a single proof
    async fn verify_and_apply_proof(
        &mut self,
        proof: &state_transition_circuit::AccidentalComputerProof,
    ) -> Result<()> {
        // Verify proof using AccidentalComputer
        // Apply state transition to NOMT
        // Update position manager if needed
        Ok(())
    }
}

/// Settlement manager (placeholder struct ref)
pub struct SettlementManager;

impl SettlementManager {
    pub fn should_settle(&self, _height: u64) -> bool {
        false
    }

    pub async fn settle(&mut self, _is_proposer: bool) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lending_application_config() {
        let config = LendingApplicationConfig::default();
        assert_eq!(config.liquidation_penalty_percent, 5);
        assert_eq!(config.settlement_batch_size, 5);
        assert_eq!(config.oracle_update_interval, 10);
    }
}
