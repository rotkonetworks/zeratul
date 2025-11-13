# Settlement Batching Strategy

## Problem Statement

**Zeratul**: 2 second block time (30 blocks/minute)
**Penumbra**: ~5 second block time (12 blocks/minute)

**Issue**: We can't submit a Penumbra transaction for every Zeratul block - it would:
- Overwhelm Penumbra mempool
- Waste gas fees
- Create backlog (can't confirm fast enough)

**Solution**: Batch multiple Zeratul blocks into periodic Penumbra settlements

## Architecture

### Timeline Comparison

```
Zeratul:  |--B1--|--B2--|--B3--|--B4--|--B5--|--B6--|--B7--|--B8--|--B9--|--B10-|
          0s     2s     4s     6s     8s     10s    12s    14s    16s    18s    20s

Penumbra: |----------PB1----------|----------PB2----------|----------PB3----------|
          0s                      5s                      10s                     15s

Settle:   |                       S1                      S2                      S3
          Every 10s (5 Zeratul blocks)
```

### Batching Window

**Recommended: Settle every 5-10 Zeratul blocks (10-20 seconds)**

This gives us:
- ~2-4x buffer relative to Penumbra block time
- Reasonable delay for users
- Efficient gas usage
- Time for Penumbra tx confirmation

## Implementation

### State Accumulation

```rust
// blockchain/src/penumbra/settlement.rs

/// Accumulated settlement state (across multiple Zeratul blocks)
#[derive(Debug, Clone)]
pub struct AccumulatedSettlement {
    /// Range of Zeratul blocks included
    pub from_height: u64,
    pub to_height: u64,

    /// Net pool borrowing per asset (accumulated)
    pub net_borrowed: HashMap<AssetId, Amount>,

    /// Net pool repayment per asset (accumulated)
    pub net_repaid: HashMap<AssetId, Amount>,

    /// Total liquidations processed
    pub total_liquidations: u32,

    /// Fees collected
    pub fees_collected: HashMap<AssetId, Amount>,
}

impl AccumulatedSettlement {
    pub fn new(start_height: u64) -> Self {
        Self {
            from_height: start_height,
            to_height: start_height,
            net_borrowed: HashMap::new(),
            net_repaid: HashMap::new(),
            total_liquidations: 0,
            fees_collected: HashMap::new(),
        }
    }

    /// Accumulate results from one Zeratul block
    pub fn accumulate(&mut self, block_result: BlockExecutionResult) {
        self.to_height = block_result.height;

        // Accumulate net borrowing
        for (asset_id, amount) in block_result.borrowed_from_pool {
            *self.net_borrowed.entry(asset_id).or_insert(Amount::ZERO) += amount;
        }

        // Accumulate net repayment
        for (asset_id, amount) in block_result.repaid_to_pool {
            *self.net_repaid.entry(asset_id).or_insert(Amount::ZERO) += amount;
        }

        self.total_liquidations += block_result.liquidations_processed;

        // Accumulate fees
        for (asset_id, amount) in block_result.fees {
            *self.fees_collected.entry(asset_id).or_insert(Amount::ZERO) += amount;
        }
    }

    /// Calculate net settlement needed (borrow - repay)
    pub fn net_settlement(&self) -> HashMap<AssetId, SignedAmount> {
        let mut net = HashMap::new();

        // Process all assets
        let mut all_assets: HashSet<AssetId> = self.net_borrowed.keys().copied().collect();
        all_assets.extend(self.net_repaid.keys());

        for asset_id in all_assets {
            let borrowed = self.net_borrowed.get(&asset_id).copied().unwrap_or(Amount::ZERO);
            let repaid = self.net_repaid.get(&asset_id).copied().unwrap_or(Amount::ZERO);

            // Net = borrowed - repaid
            // Positive = need to borrow from Penumbra
            // Negative = need to return to Penumbra
            let net_amount = SignedAmount::from_borrowed_repaid(borrowed, repaid);

            if !net_amount.is_zero() {
                net.insert(asset_id, net_amount);
            }
        }

        net
    }

    /// Check if settlement is needed
    pub fn needs_settlement(&self) -> bool {
        !self.net_settlement().is_empty()
    }
}

/// Signed amount (can be positive or negative)
#[derive(Debug, Clone, Copy)]
pub struct SignedAmount {
    pub amount: Amount,
    pub is_positive: bool, // true = borrowed, false = repaid
}

impl SignedAmount {
    pub fn from_borrowed_repaid(borrowed: Amount, repaid: Amount) -> Self {
        if borrowed.0 >= repaid.0 {
            Self {
                amount: Amount(borrowed.0 - repaid.0),
                is_positive: true,
            }
        } else {
            Self {
                amount: Amount(repaid.0 - borrowed.0),
                is_positive: false,
            }
        }
    }

    pub fn is_zero(&self) -> bool {
        self.amount.0 == 0
    }
}
```

### Settlement Manager

```rust
// blockchain/src/penumbra/settlement_manager.rs

pub struct SettlementManager {
    /// Current accumulated settlement
    current_batch: AccumulatedSettlement,

    /// How many Zeratul blocks per settlement
    batch_size: u64,

    /// Transaction builder
    tx_builder: PenumbraTransactionBuilder,

    /// Pending settlements (waiting for Penumbra confirmation)
    pending: Vec<PendingSettlement>,
}

#[derive(Debug, Clone)]
pub struct PendingSettlement {
    /// Penumbra tx hash
    pub tx_hash: TxHash,

    /// Zeratul height range
    pub zeratul_range: (u64, u64),

    /// Submitted at (unix timestamp)
    pub submitted_at: u64,

    /// Settlement details
    pub details: AccumulatedSettlement,
}

impl SettlementManager {
    pub fn new(
        start_height: u64,
        batch_size: u64,
        tx_builder: PenumbraTransactionBuilder,
    ) -> Self {
        Self {
            current_batch: AccumulatedSettlement::new(start_height),
            batch_size,
            tx_builder,
            pending: Vec::new(),
        }
    }

    /// Accumulate block execution result
    pub fn accumulate_block(&mut self, result: BlockExecutionResult) {
        self.current_batch.accumulate(result);
    }

    /// Check if we should settle now
    pub fn should_settle(&self, current_height: u64) -> bool {
        let blocks_accumulated = current_height - self.current_batch.from_height;
        blocks_accumulated >= self.batch_size
    }

    /// Perform settlement (build and submit Penumbra tx)
    pub async fn settle(&mut self, is_proposer: bool) -> Result<Option<TxHash>> {
        if !self.current_batch.needs_settlement() {
            info!("No settlement needed, skipping");
            return Ok(None);
        }

        // Only proposer submits
        if !is_proposer {
            info!("Not proposer, skipping settlement submission");
            // Still reset batch for next round
            let next_height = self.current_batch.to_height + 1;
            self.current_batch = AccumulatedSettlement::new(next_height);
            return Ok(None);
        }

        info!(
            "Settling Zeratul blocks {}-{} to Penumbra",
            self.current_batch.from_height,
            self.current_batch.to_height
        );

        // Build Penumbra transaction(s)
        let tx_hash = self.build_and_submit_settlement().await?;

        // Mark as pending
        self.pending.push(PendingSettlement {
            tx_hash: tx_hash.clone(),
            zeratul_range: (
                self.current_batch.from_height,
                self.current_batch.to_height,
            ),
            submitted_at: now(),
            details: self.current_batch.clone(),
        });

        // Reset batch for next round
        let next_height = self.current_batch.to_height + 1;
        self.current_batch = AccumulatedSettlement::new(next_height);

        Ok(Some(tx_hash))
    }

    /// Build and submit Penumbra settlement transaction
    async fn build_and_submit_settlement(&self) -> Result<TxHash> {
        let net_settlement = self.current_batch.net_settlement();

        // Build transaction plan
        let mut planner = TransactionPlan::new();

        for (asset_id, signed_amount) in net_settlement {
            if signed_amount.is_positive {
                // Need to borrow: Execute DEX swap
                // (Buy asset from Penumbra to cover Zeratul borrowing)
                planner.add_swap(SwapPlan {
                    trading_pair: TradingPair::new(UM_ASSET_ID, asset_id),
                    input_amount: signed_amount.amount,
                    ..Default::default()
                })?;
            } else {
                // Need to repay: Send back via IBC
                // (Return excess to Penumbra treasury)
                planner.add_ibc_transfer(IBCTransferPlan {
                    denom: asset_id,
                    amount: signed_amount.amount,
                    receiver: PENUMBRA_TREASURY_ADDRESS,
                    ..Default::default()
                })?;
            }
        }

        // Build and submit
        let tx = self.tx_builder.build_and_sign(planner).await?;
        let tx_hash = self.tx_builder.submit(tx).await?;

        Ok(tx_hash)
    }

    /// Check pending settlements (confirm on Penumbra)
    pub async fn check_pending(&mut self) -> Result<()> {
        let mut confirmed = Vec::new();

        for (idx, pending) in self.pending.iter().enumerate() {
            // Query Penumbra for tx status
            match self.tx_builder.check_tx_status(&pending.tx_hash).await? {
                TxStatus::Confirmed { height } => {
                    info!(
                        "Settlement for Zeratul blocks {}-{} confirmed at Penumbra height {}",
                        pending.zeratul_range.0,
                        pending.zeratul_range.1,
                        height
                    );
                    confirmed.push(idx);
                }
                TxStatus::Pending => {
                    // Check if too old (>60 seconds)
                    let age = now() - pending.submitted_at;
                    if age > 60 {
                        warn!("Settlement tx {} pending for {}s", pending.tx_hash, age);
                    }
                }
                TxStatus::Failed { reason } => {
                    error!(
                        "Settlement tx {} failed: {}. Need to retry!",
                        pending.tx_hash, reason
                    );
                    // TODO: Retry logic
                }
            }
        }

        // Remove confirmed settlements
        for idx in confirmed.iter().rev() {
            self.pending.remove(*idx);
        }

        Ok(())
    }
}
```

### Integration with Application

```rust
// blockchain/src/application.rs

pub struct Application {
    // ... existing fields ...

    /// Settlement manager (batches multiple blocks)
    settlement_manager: SettlementManager,

    /// Settlement interval (in blocks)
    settlement_interval: u64,
}

impl Application {
    pub fn new(config: Config, tx_builder: PenumbraTransactionBuilder) -> Self {
        Self {
            // ... existing init ...
            settlement_manager: SettlementManager::new(
                0,                                    // start height
                config.settlement_batch_size,         // e.g., 5 blocks
                tx_builder,
            ),
            settlement_interval: config.settlement_batch_size,
        }
    }

    /// Execute block
    pub async fn execute_block(
        &mut self,
        block: Block,
        is_proposer: bool,
    ) -> Result<BlockResult> {
        // 1. Execute all transactions in block
        let result = self.execute_margin_trading_batch(&block).await?;

        // 2. Accumulate in settlement manager
        self.settlement_manager.accumulate_block(result.clone());

        // 3. Check if we should settle to Penumbra
        if self.settlement_manager.should_settle(block.height) {
            info!("Settlement window reached, settling to Penumbra...");

            if let Some(tx_hash) = self.settlement_manager.settle(is_proposer).await? {
                info!("Submitted Penumbra settlement tx: {}", tx_hash);
            }
        }

        // 4. Periodically check pending settlements
        if block.height % 10 == 0 {
            self.settlement_manager.check_pending().await?;
        }

        // 5. Update NOMT state
        self.nomt.commit()?;

        Ok(result)
    }
}
```

### Configuration

```yaml
# config.yaml

penumbra:
  # Settlement batching (how many Zeratul blocks per Penumbra tx)
  settlement_batch_size: 5  # Every 5 blocks (10 seconds)

  # Settlement batch size can be tuned based on:
  # - Penumbra block time (~5s)
  # - Gas costs (larger batches = fewer txs = lower fees)
  # - User experience (smaller batches = faster settlement)

  # Recommended values:
  # - Testnet: 5 blocks (10s) for fast iteration
  # - Mainnet: 10-15 blocks (20-30s) for cost efficiency
```

## Example Timeline

### Scenario: 10 Zeratul Blocks, Settlement Every 5 Blocks

```
Block 1 (0s):  Borrow 1000 UM → Accumulate
Block 2 (2s):  Borrow 500 UM  → Accumulate
Block 3 (4s):  Repay 200 UM   → Accumulate
Block 4 (6s):  Borrow 300 UM  → Accumulate
Block 5 (8s):  Borrow 100 UM  → Accumulate
               ↓
               Net: +1700 UM borrowed
               ↓
         [SETTLE TO PENUMBRA]
         Build Penumbra swap: Buy 1700 UM
         Submit at t=10s

Block 6 (10s): Repay 800 UM   → Accumulate (new batch)
Block 7 (12s): Borrow 200 UM  → Accumulate
Block 8 (14s): Repay 500 UM   → Accumulate
Block 9 (16s): Borrow 50 UM   → Accumulate
Block 10 (18s): Repay 150 UM  → Accumulate
               ↓
               Net: -1200 UM repaid
               ↓
         [SETTLE TO PENUMBRA]
         Build IBC transfer: Return 1200 UM
         Submit at t=20s
```

## Failure Handling

### What if Penumbra Transaction Fails?

```rust
impl SettlementManager {
    /// Retry failed settlement
    pub async fn retry_failed_settlement(
        &mut self,
        failed: &PendingSettlement,
    ) -> Result<TxHash> {
        warn!("Retrying settlement for blocks {}-{}",
              failed.zeratul_range.0,
              failed.zeratul_range.1);

        // Rebuild transaction (may need different gas/slippage)
        let tx_hash = self.build_and_submit_settlement_from_details(
            &failed.details
        ).await?;

        Ok(tx_hash)
    }

    /// Emergency: Manually trigger settlement
    pub async fn emergency_settle(&mut self) -> Result<TxHash> {
        // Force settlement even if batch not full
        warn!("Emergency settlement triggered at height {}",
              self.current_batch.to_height);

        let tx_hash = self.build_and_submit_settlement().await?;

        // Reset batch
        let next_height = self.current_batch.to_height + 1;
        self.current_batch = AccumulatedSettlement::new(next_height);

        Ok(tx_hash)
    }
}
```

### What if Proposer Fails to Submit?

```rust
// Next proposer can detect missing settlement

impl Application {
    /// Check if previous proposer failed to settle
    pub async fn check_missing_settlement(&self, height: u64) -> Result<bool> {
        // If we're at settlement boundary but no pending tx exists
        if height % self.settlement_interval == 0 {
            let expected_range = (
                height - self.settlement_interval,
                height - 1,
            );

            // Check if settlement for this range is pending
            let has_pending = self.settlement_manager
                .pending
                .iter()
                .any(|p| p.zeratul_range == expected_range);

            if !has_pending {
                warn!("Missing settlement for blocks {}-{}",
                      expected_range.0,
                      expected_range.1);
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Slash previous proposer for missing settlement
    pub fn slash_for_missing_settlement(
        &mut self,
        height: u64,
    ) -> Result<()> {
        let prev_proposer = self.get_proposer_at_height(height - 1)?;

        self.slash_validator(prev_proposer, SlashReason::MissedSettlement)?;

        Ok(())
    }
}
```

## Monitoring & Metrics

```rust
// Key metrics to track

pub struct SettlementMetrics {
    /// Number of Zeratul blocks per settlement
    pub avg_batch_size: f64,

    /// Time from Zeratul block to Penumbra confirmation
    pub avg_settlement_latency_ms: f64,

    /// Success rate of settlement transactions
    pub settlement_success_rate: f64,

    /// Current pending settlements
    pub pending_settlements: usize,

    /// Oldest pending settlement age (seconds)
    pub oldest_pending_age_sec: u64,
}

impl SettlementManager {
    pub fn metrics(&self) -> SettlementMetrics {
        // Calculate metrics
        SettlementMetrics {
            avg_batch_size: self.calculate_avg_batch_size(),
            avg_settlement_latency_ms: self.calculate_avg_latency(),
            settlement_success_rate: self.calculate_success_rate(),
            pending_settlements: self.pending.len(),
            oldest_pending_age_sec: self.oldest_pending_age(),
        }
    }
}
```

## Adaptive Batching (Future Optimization)

```rust
// Dynamically adjust batch size based on conditions

impl SettlementManager {
    /// Adjust batch size based on network conditions
    pub fn adjust_batch_size(&mut self) {
        // If Penumbra is congested (high gas prices)
        if self.penumbra_gas_price() > THRESHOLD {
            // Increase batch size to reduce frequency
            self.batch_size = min(self.batch_size + 1, MAX_BATCH_SIZE);
        }

        // If many pending settlements (backlog)
        if self.pending.len() > 3 {
            // Increase batch size to reduce submission rate
            self.batch_size = min(self.batch_size + 1, MAX_BATCH_SIZE);
        }

        // If Penumbra is fast (low congestion)
        if self.penumbra_gas_price() < THRESHOLD && self.pending.len() == 0 {
            // Can afford smaller batches (faster settlement for users)
            self.batch_size = max(self.batch_size - 1, MIN_BATCH_SIZE);
        }
    }
}
```

## Summary

### Problem
- Zeratul: 2s blocks (30 blocks/min)
- Penumbra: 5s blocks (12 blocks/min)
- Can't submit Penumbra tx for every Zeratul block

### Solution
- **Batch 5-10 Zeratul blocks** into one Penumbra settlement
- **Accumulate** net borrowing/repayment across blocks
- **Settle periodically** (every 10-20 seconds)
- **Only proposer submits** (rotates each batch)

### Benefits
- ✅ Efficient gas usage (fewer transactions)
- ✅ Handles speed mismatch (buffer time)
- ✅ Reasonable latency (10-20s for users)
- ✅ Robust (retry failed settlements)

### Configuration
```yaml
penumbra:
  settlement_batch_size: 5  # Blocks (10s)
```

### Next Steps
1. Implement `AccumulatedSettlement` type
2. Implement `SettlementManager`
3. Integrate with `Application::execute_block()`
4. Add retry logic for failed settlements
5. Add monitoring/metrics
