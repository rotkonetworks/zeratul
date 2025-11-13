# Validator CLI Design

## Validator Binary Configuration

### CLI Flags

```bash
zeratul-validator run \
  --config /etc/zeratul/config.yaml \
  --penumbra-grpc-url https://grpc.testnet.penumbra.zone:443 \
  --penumbra-storage /var/lib/zeratul/penumbra.db \
  --penumbra-fvk <optional-full-viewing-key> \
  --home /var/lib/zeratul \
  --p2p-bind 0.0.0.0:26656 \
  --rpc-bind 0.0.0.0:26657 \
  --log-level info
```

### Configuration File

```yaml
# /etc/zeratul/config.yaml

# Validator identity
validator:
  pubkey: "0x123..." # Validator public key
  privkey_path: "/etc/zeratul/validator.key" # For signing proposals

# Penumbra integration
penumbra:
  # gRPC endpoint of Penumbra full node (can be shared or local)
  grpc_url: "https://grpc.testnet.penumbra.zone:443"

  # Local storage for embedded ViewServer
  storage_path: "/var/lib/zeratul/penumbra.db"

  # Full Viewing Key (optional, for oracle-only mode use null)
  fvk: null  # or "penumbra1fvk..."

  # Trading pairs to monitor for oracle prices
  oracle_pairs:
    - ["penumbra.core.asset.v1.Asset/...", "penumbra.core.asset.v1.Asset/..."]  # UM/gm
    - ["penumbra.core.asset.v1.Asset/...", "penumbra.core.asset.v1.Asset/..."]  # UM/gn

  # How often to update oracle prices (in Zeratul blocks)
  oracle_update_interval: 10

# Consensus
consensus:
  block_time_ms: 2000  # 2 second blocks
  num_validators: 7

# P2P networking
p2p:
  bind_address: "0.0.0.0:26656"
  seeds:
    - "validator1.zeratul.zone:26656"
    - "validator2.zeratul.zone:26656"

# RPC
rpc:
  bind_address: "0.0.0.0:26657"

# Storage
storage:
  data_dir: "/var/lib/zeratul"
  nomt_db: "/var/lib/zeratul/nomt.db"

# Logging
logging:
  level: "info"
  format: "json"
```

### Environment Variables (Alternative)

```bash
# For containerized deployments
export ZERATUL_PENUMBRA_GRPC_URL="https://grpc.testnet.penumbra.zone:443"
export ZERATUL_PENUMBRA_STORAGE="/data/penumbra.db"
export ZERATUL_HOME="/data"
export ZERATUL_VALIDATOR_KEY="/secrets/validator.key"

zeratul-validator run
```

## Transaction Building Strategy

### Key Question: Who Submits Transactions to Penumbra?

**Answer: The current block proposer (leader) submits batch settlement transactions**

### Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                     Zeratul Block N                          │
│                                                              │
│  - 100 margin trades executed                                │
│  - Net result: +5M UM borrowed from pool                     │
│  - Oracle price: 1.05 gm/UM                                  │
│                                                              │
│  Proposer: Validator #3                                      │
└──────────────────────┬───────────────────────────────────────┘
                       │
                       │ (Proposer builds Penumbra tx)
                       │
                       ▼
         ┌─────────────────────────────┐
         │  Penumbra Transaction       │
         │                             │
         │  Type: DEX Swap             │
         │  Trading Pair: UM/gm        │
         │  Amount: 5M UM              │
         │  Source: Zeratul IBC addr   │
         │                             │
         │  Signed by: Validator #3    │
         └─────────────┬───────────────┘
                       │
                       │ (Broadcast to Penumbra)
                       │
                       ▼
         ┌─────────────────────────────┐
         │    Penumbra Mempool         │
         │                             │
         │  Tx included in next block  │
         └─────────────────────────────┘
```

### Why Proposer Submits?

**Advantages:**
1. ✅ **Natural role**: Proposer already assembling block, has all data
2. ✅ **Incentive alignment**: Proposer can earn tx fees
3. ✅ **Single submission**: Avoids duplicate txs from all validators
4. ✅ **Rotation**: Leadership rotates, so all validators take turns

**Alternatives (rejected):**
- ❌ **All validators submit**: Duplicates, wastes gas, conflicts
- ❌ **External relayer**: Single point of failure, trust issues
- ❌ **Designated validator**: Centralization risk if that validator is down

### Transaction Building Flow

```rust
// In application.rs (block execution)

impl Application {
    /// Execute block and build Penumbra settlement transaction
    pub async fn execute_block(
        &mut self,
        block: Block,
        is_proposer: bool,
    ) -> Result<BlockResult> {
        // 1. Execute all Zeratul transactions
        let batch_result = self.execute_margin_trading_batch(&block).await?;

        // 2. Update NOMT state
        self.nomt.commit()?;

        // 3. If we're the proposer, build Penumbra settlement tx
        if is_proposer {
            self.build_penumbra_settlement(batch_result).await?;
        }

        Ok(BlockResult { /* ... */ })
    }

    /// Build and submit Penumbra transaction for batch settlement
    async fn build_penumbra_settlement(
        &self,
        batch_result: BatchExecutionResult,
    ) -> Result<()> {
        // Calculate net pool borrowing
        let net_borrowed = batch_result.total_borrowed_from_pool;

        if net_borrowed.is_zero() {
            return Ok(()); // No settlement needed
        }

        // Build Penumbra DEX swap to hedge pool exposure
        let penumbra_tx = self.build_penumbra_swap(
            batch_result.trading_pair,
            net_borrowed,
        ).await?;

        // Submit to Penumbra
        self.penumbra_client.submit_transaction(penumbra_tx).await?;

        Ok(())
    }
}
```

### Transaction Custody Model

**Option 1: Shared Custody (Multisig)**

```
Zeratul validators control a Penumbra multisig address:
- Threshold: 5-of-7 validators
- Each proposer builds tx, collects signatures via consensus
- Broadcasts when threshold reached

Advantages:
✅ Decentralized control
✅ No single point of failure

Disadvantages:
❌ Complex: Need signature collection protocol
❌ Slow: Extra round of communication
```

**Option 2: Hot Wallet Per Validator** ⭐ **RECOMMENDED**

```
Each validator has its own Penumbra spend key:
- Zeratul chain maintains a Penumbra "treasury" address
- Validators periodically rebalance using IBC
- Proposer signs and broadcasts immediately

Advantages:
✅ Simple: No coordination needed
✅ Fast: Proposer broadcasts immediately
✅ Rotation: Different validators take turns

Disadvantages:
⚠️ Trust: Proposer could misbehave (slashing needed)
⚠️ Key management: Each validator needs secure key storage
```

**Recommendation: Option 2 (Hot Wallet) with slashing**

### Implementation: Transaction Builder

```rust
// blockchain/src/penumbra/transaction_builder.rs

use penumbra_transaction::TransactionPlan;
use penumbra_keys::SpendKey;
use penumbra_dex::swap::SwapPlaintext;

pub struct PenumbraTransactionBuilder {
    /// Our spend key (for signing)
    spend_key: SpendKey,

    /// Embedded view server (for balance queries)
    view_server: Arc<ViewServer>,

    /// Penumbra chain app parameters
    app_params: AppParameters,
}

impl PenumbraTransactionBuilder {
    /// Build DEX swap transaction
    pub async fn build_swap(
        &self,
        trading_pair: TradingPair,
        input_amount: Amount,
        min_output: Amount,
    ) -> Result<Transaction> {
        // 1. Create swap plaintext
        let swap = SwapPlaintext::new(
            &mut OsRng,
            trading_pair,
            input_amount,
            Fee::default(),
            ClaimAddress::from(self.spend_key.full_viewing_key().payment_address(0.into())),
        );

        // 2. Create transaction plan
        let mut planner = Planner::new(&mut OsRng);
        planner.set_gas_prices(self.app_params.gas_prices.clone());

        // Add swap
        planner.swap(swap)?;

        let plan = planner.plan(
            self.view_server.as_ref(),
            AddressIndex::new(0),
        ).await?;

        // 3. Authorize and build transaction
        let auth_data = plan.authorize(&mut OsRng, &self.spend_key)?;
        let witness_data = self.view_server.witness(&plan).await?;

        let tx = plan.build(&self.spend_key.full_viewing_key(), witness_data, auth_data)?;

        Ok(tx)
    }

    /// Submit transaction to Penumbra
    pub async fn submit_transaction(&self, tx: Transaction) -> Result<TxHash> {
        // Broadcast via gRPC
        let mut client = penumbra_proto::core::component::compact_block::v1::query_service_client::QueryServiceClient::connect(
            self.view_server.node_url.clone()
        ).await?;

        let response = client.broadcast_transaction(BroadcastTransactionRequest {
            transaction: Some(tx.into()),
            await_detection: true,
        }).await?;

        Ok(response.into_inner().id.try_into()?)
    }
}
```

### Proposer Selection & Transaction Timing

```rust
// In consensus engine

impl Engine {
    /// Determine if we're the proposer for this height
    fn are_we_proposer(&self, height: u64) -> bool {
        // Round-robin or weighted by stake
        let proposer_idx = (height % self.validator_set.len() as u64) as usize;
        self.validator_set[proposer_idx].pubkey == self.our_pubkey
    }

    /// Execute block
    async fn execute_block(&mut self, block: Block) -> Result<()> {
        let is_proposer = self.are_we_proposer(block.height);

        // All validators execute block
        let result = self.application.execute_block(block, is_proposer).await?;

        // Only proposer builds Penumbra tx
        if is_proposer {
            info!("We are proposer for height {}, building Penumbra settlement", block.height);
        }

        Ok(())
    }
}
```

### Failure Handling

**What if proposer fails to submit Penumbra tx?**

```rust
impl Application {
    async fn build_penumbra_settlement(
        &self,
        batch_result: BatchExecutionResult,
    ) -> Result<()> {
        // Try to submit
        match self.penumbra_tx_builder.submit_transaction(tx).await {
            Ok(tx_hash) => {
                info!("Submitted Penumbra tx: {}", tx_hash);
                // Store tx hash in next Zeratul block
                self.pending_penumbra_txs.insert(tx_hash);
                Ok(())
            }
            Err(e) => {
                error!("Failed to submit Penumbra tx: {}", e);
                // Mark settlement as pending
                // Next proposer will retry
                self.pending_settlements.push(batch_result);
                Ok(())
            }
        }
    }

    /// Next proposer can pick up failed settlements
    async fn retry_pending_settlements(&self) -> Result<()> {
        for pending in &self.pending_settlements {
            // Try to submit again
            self.build_penumbra_settlement(pending.clone()).await?;
        }
        Ok(())
    }
}
```

### Security: Slashing Malicious Proposers

```rust
// If proposer builds invalid Penumbra tx or steals funds

pub struct MisbehaviorProof {
    /// Height where proposer misbehaved
    pub height: u64,

    /// Proposer's pubkey
    pub proposer: PublicKey,

    /// Evidence (e.g., invalid Penumbra tx hash)
    pub evidence: Vec<u8>,

    /// Signatures from 2/3+ validators confirming misbehavior
    pub signatures: Vec<Signature>,
}

impl Application {
    /// Slash validator for misbehavior
    pub fn slash_validator(&mut self, proof: MisbehaviorProof) -> Result<()> {
        // Verify proof
        if !self.verify_misbehavior_proof(&proof)? {
            bail!("invalid misbehavior proof");
        }

        // Slash proposer's stake
        self.validator_set.slash(proof.proposer, SlashAmount::Percent(10))?;

        // Remove from validator set if stake too low
        if self.validator_set.get_stake(proof.proposer) < MIN_STAKE {
            self.validator_set.remove(proof.proposer)?;
        }

        Ok(())
    }
}
```

## Complete Validator Startup

```rust
// bin/zeratul-validator/src/main.rs

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Parse CLI args
    let args = CliArgs::parse();

    // 2. Load config
    let config: ValidatorConfig = load_config(&args.config)?;

    // 3. Initialize Penumbra ViewServer
    let view_server = ViewServer::load_or_initialize(
        Some(&config.penumbra.storage_path),
        None,
        &SpendKey::from_file(&args.penumbra_key_path)?.full_viewing_key(),
        Url::parse(&config.penumbra.grpc_url)?,
    ).await?;

    info!("Penumbra ViewServer initialized, syncing...");

    // 4. Initialize transaction builder
    let tx_builder = PenumbraTransactionBuilder::new(
        SpendKey::from_file(&args.penumbra_key_path)?,
        view_server.clone(),
    );

    // 5. Initialize Zeratul application
    let application = Application::new(
        config.clone(),
        view_server.clone(),
        tx_builder,
    )?;

    // 6. Initialize consensus engine
    let engine = Engine::new(
        config,
        application,
    ).await?;

    info!("Starting Zeratul validator...");

    // 7. Run forever
    engine.run().await?;

    Ok(())
}
```

## Summary

### Validator CLI Flags

```bash
zeratul-validator run \
  --config /etc/zeratul/config.yaml \
  --penumbra-grpc-url https://grpc.testnet.penumbra.zone:443 \
  --penumbra-storage /var/lib/zeratul/penumbra.db \
  --penumbra-key /secrets/penumbra-spend.key \
  --home /var/lib/zeratul
```

### Transaction Building Strategy

**Proposer submits Penumbra transactions:**
- ✅ Natural role (already building block)
- ✅ Single submission (no duplicates)
- ✅ Rotation (leadership rotates)
- ⚠️ Requires slashing for misbehavior

### Key Management

**Each validator needs:**
1. **Zeratul validator key**: For consensus (signing blocks, proposals)
2. **Penumbra spend key**: For submitting txs to Penumbra

**Storage:**
```
/secrets/
  validator.key      # Zeratul consensus key
  penumbra-spend.key # Penumbra spend key
```

### Next Steps

1. Implement `PenumbraTransactionBuilder`
2. Add proposer selection logic to consensus
3. Implement settlement tx building
4. Add failure recovery (retry pending settlements)
5. Design slashing mechanism for misbehaving proposers
