# Penumbra SDK Integration Guide

## Overview

This guide explains how to integrate Penumbra SDK directly into Zeratul validators instead of running `pclientd` as a separate process.

## Key Insight

**pclientd is just a thin wrapper around ViewServer**

Looking at the Penumbra codebase:
- `pclientd` = CLI args parser + gRPC server + ViewServer initialization
- `ViewServer` = The actual light client logic (sync, storage, queries)

**We can embed ViewServer directly** and skip the separate process entirely.

## Required Penumbra Dependencies

Add to `blockchain/Cargo.toml`:

```toml
[dependencies]
# Core view service (light client)
penumbra-view = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# Cryptography and keys
penumbra-keys = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }
penumbra-tct = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# DEX types for batch swaps
penumbra-dex = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# Asset metadata
penumbra-asset = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# IBC integration
penumbra-ibc = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }
ibc-types = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# Protobuf and gRPC
penumbra-proto = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# Compact block format
penumbra-compact-block = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# Storage
penumbra-storage = { git = "https://github.com/penumbra-zone/penumbra", branch = "main" }

# Database
r2d2 = "0.8"
r2d2_sqlite = "0.22"
```

## Architecture

### In Validator Process

```
┌─────────────────────────────────────────┐
│         Zeratul Validator               │
│                                         │
│  ┌──────────────┐   ┌───────────────┐ │
│  │ Zeratul Node │◄──┤ ViewServer    │ │
│  │              │   │ (Embedded)    │ │
│  │ - Consensus  │   │               │ │
│  │ - NOMT       │   │ - SQLite DB   │ │
│  │ - Lending    │   │ - SCT Tree    │ │
│  └──────────────┘   │ - Sync Worker │ │
│                     └───────┬───────┘ │
│                             │ gRPC    │
└─────────────────────────────┼─────────┘
                              │
                              ▼
                     ┌─────────────────┐
                     │ Penumbra Node   │
                     │ (pd)            │
                     └─────────────────┘
```

### ViewServer Responsibilities

1. **Background Sync Worker**
   - Fetches compact blocks from Penumbra node
   - Scans for relevant notes/swaps
   - Updates State Commitment Tree (SCT)
   - Stores to SQLite

2. **Storage Layer**
   - SQLite database (~1GB)
   - Stores:
     - Spendable notes
     - Swap records
     - Nullifiers
     - Asset metadata
     - Transaction history

3. **State Commitment Tree**
   - In-memory tiered Merkle tree
   - Tracks note commitments
   - Enables witness generation
   - Prunes irrelevant commitments

4. **Query Interface**
   - Get batch swap output data (PUBLIC)
   - Query asset balances
   - Verify IBC proofs
   - Check transaction status

## Implementation Example

### 1. Initialize ViewServer

```rust
use penumbra_view::{ViewServer, Storage};
use penumbra_keys::FullViewingKey;
use url::Url;

// In validator startup code
pub async fn start_validator(config: ValidatorConfig) -> Result<()> {
    // 1. Initialize embedded Penumbra ViewServer
    let fvk = if let Some(fvk_hex) = &config.penumbra.fvk_hex {
        FullViewingKey::from_str(fvk_hex)?
    } else {
        // Use dummy FVK for oracle-only mode
        FullViewingKey::dummy()
    };

    let view_server = ViewServer::load_or_initialize(
        Some(&config.penumbra.storage_path),
        None, // asset registry
        &fvk,
        Url::parse(&config.penumbra.node_url)?,
    ).await?;

    // ViewServer spawns background sync worker automatically

    // 2. Start Zeratul consensus engine
    let engine = Engine::new(
        config,
        view_server, // Pass to engine
    ).await?;

    engine.run().await
}
```

### 2. Query DEX Oracle Prices

```rust
use penumbra_proto::core::component::compact_block::v1::CompactBlock;

impl OracleManager {
    pub async fn fetch_penumbra_prices(
        &self,
        view_server: &ViewServer,
        trading_pairs: Vec<TradingPair>,
    ) -> Result<HashMap<TradingPair, Price>> {
        let mut prices = HashMap::new();

        // Get latest synced height
        let status = view_server.status().await?;
        let height = status.full_sync_height;

        // Fetch compact block for this height
        let compact_block = view_server
            .compact_block_range(CompactBlockRangeRequest {
                start_height: height,
                end_height: height + 1,
                ..Default::default()
            })
            .await?
            .next()
            .await?;

        // Extract batch swap outputs (PUBLIC data)
        for trading_pair in trading_pairs {
            if let Some(batch_output) = compact_block.swap_outputs.get(&trading_pair) {
                // Calculate clearing price from batch data
                let price = calculate_price(
                    batch_output.delta_1,
                    batch_output.delta_2,
                );

                prices.insert(trading_pair, price);
            }
        }

        Ok(prices)
    }
}
```

### 3. Verify IBC Packets

```rust
use penumbra_ibc::component::proof_verification::verify_merkle_proof;

impl IBCHandler {
    pub async fn verify_packet(
        &self,
        view_server: &ViewServer,
        packet: &IBCPacket,
        proof_height: u64,
        merkle_proof: &MerkleProof,
    ) -> Result<bool> {
        // Get trusted consensus state at proof height
        let consensus_state = view_server
            .get_consensus_state(proof_height)
            .await?;

        // Verify Merkle proof against trusted root
        verify_merkle_proof(
            &proof_specs,
            &merkle_prefix,
            merkle_proof,
            &consensus_state.root,
            IbcPath::packet_commitment(
                &packet.source_channel,
                packet.sequence,
            ),
            compute_packet_commitment(packet).to_vec(),
        )?;

        Ok(true)
    }
}
```

## Minimal vs Full Integration

### Minimal (Oracle-Only)

For just getting DEX prices:

```rust
// Minimal deps
penumbra-view
penumbra-keys
penumbra-dex
penumbra-compact-block
penumbra-proto
```

**Use Case**: Only need oracle prices, don't care about IBC transfers yet.

**Storage**: ~1GB SQLite database

**Memory**: ~512MB for SCT + view state

### Full (Oracle + IBC)

For both prices and IBC transfers:

```rust
// Full deps
penumbra-view
penumbra-keys
penumbra-dex
penumbra-ibc
penumbra-compact-block
penumbra-proto
ibc-types
```

**Use Case**: Production deployment with cross-chain transfers

**Storage**: ~1GB SQLite database

**Memory**: ~1GB for SCT + view state + IBC state

## Benefits of Embedded Approach

### 1. Performance
- **No IPC overhead**: Direct function calls instead of gRPC
- **Shared memory**: ViewServer and consensus in same process
- **Lower latency**: No network round-trips

### 2. Simplicity
- **No separate process**: One binary to manage
- **No port conflicts**: No need for local gRPC ports
- **Easier deployment**: Fewer moving parts

### 3. Resource Efficiency
- **Shared memory**: SCT, storage connections, caches
- **Single process**: Lower OS overhead
- **Better scheduling**: OS schedules one process

### 4. Development
- **Type safety**: Direct Rust types, no protobuf serialization
- **Better errors**: Stack traces across boundaries
- **Easier debugging**: Single process to debug

## Trade-offs

### Cons of Embedded Approach

1. **Dependency bloat**: Need Penumbra SDK as dependency
2. **Coupling**: Tighter coupling to Penumbra versions
3. **Binary size**: Larger validator binary
4. **Compilation time**: Longer builds

### Mitigation

- Use specific Penumbra git tags (not `branch = "main"`)
- Pin versions for reproducible builds
- Consider dynamic linking for Penumbra libs (future)

## Deployment Comparison

### Separate pclientd (Old Way)

```bash
# Two processes to manage
systemctl start pclientd
systemctl start zeratul-validator

# Two configs to maintain
/etc/pclientd/config.toml
/etc/zeratul/config.yaml

# Two logs to monitor
journalctl -u pclientd
journalctl -u zeratul-validator
```

### Embedded ViewServer (New Way)

```bash
# One process
systemctl start zeratul-validator

# One config
/etc/zeratul/config.yaml

# One log
journalctl -u zeratul-validator
```

## Migration Path

### Phase 1: Prototype (Current)
- Mock ViewServer interface
- Placeholder implementations
- Focus on architecture

### Phase 2: Real Integration
1. Add Penumbra SDK dependencies
2. Replace mocks with real ViewServer
3. Test against Penumbra testnet
4. Verify oracle prices match

### Phase 3: Production
1. Pin Penumbra versions
2. Comprehensive error handling
3. Monitoring and metrics
4. Graceful degradation if Penumbra unavailable

## Testing Strategy

### Unit Tests
```rust
#[tokio::test]
async fn test_oracle_prices() {
    let view_server = MockViewServer::new();
    let oracle = OracleManager::new(view_server);

    let prices = oracle.fetch_prices().await.unwrap();
    assert!(prices.len() > 0);
}
```

### Integration Tests
```rust
#[tokio::test]
async fn test_against_real_penumbra() {
    let view_server = ViewServer::connect_testnet().await?;

    // Query real batch swap data
    let prices = fetch_oracle_prices(&view_server).await?;

    // Verify reasonable prices
    for (pair, price) in prices {
        assert!(price.0 > 0.0);
        assert!(price.0 < 1000.0);
    }
}
```

## Monitoring

### Key Metrics

```rust
// Penumbra sync lag
let sync_height = view_server.status().await?.full_sync_height;
let chain_height = view_server.status().await?.chain_height;
let lag = chain_height - sync_height;

if lag > 100 {
    warn!("Penumbra sync lagging: {} blocks behind", lag);
}

// Oracle update frequency
let last_update = oracle_manager.last_update_time();
let age = now() - last_update;

if age > Duration::from_secs(60) {
    warn!("Oracle prices stale: {}s old", age.as_secs());
}
```

## Conclusion

**Embedding ViewServer directly** is superior to running pclientd separately:

✅ **Better Performance**: No IPC overhead, shared memory
✅ **Simpler Deployment**: One process, one config, one log
✅ **Type Safety**: Direct Rust types, no protobuf serialization
✅ **Easier Development**: Single process to debug

**Minor downsides** (dependency size, coupling) are worth it for the benefits.

**Next Steps**:
1. Add Penumbra SDK dependencies to Cargo.toml
2. Replace MockViewServer with real ViewServer
3. Test against Penumbra testnet
4. Verify oracle prices work correctly
