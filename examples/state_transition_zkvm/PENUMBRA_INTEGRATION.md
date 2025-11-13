# Penumbra Integration Architecture

## Overview

This document describes how Zeratul validators integrate with Penumbra to:
1. Get oracle price data from Penumbra's batch swaps
2. Lock/unlock assets via IBC
3. Settle profits back to Penumbra
4. Maintain security without compromising privacy

## Design Decision: Embedded ViewServer

**Answer: Yes, validators embed Penumbra ViewServer directly in validator process**

### Why Embedded ViewServer, Not Separate Process?

| Approach | Pros | Cons | Decision |
|----------|------|------|----------|
| **Full Nodes** | Complete data | Very heavy (100GB+), expensive | ❌ Too expensive |
| **Separate pclientd** | Standard tool | Extra process, IPC overhead | ❌ Unnecessary complexity |
| **Embedded ViewServer** | Efficient, no IPC, same memory | Requires Penumbra SDK deps | ✅ **CHOSEN** |
| **External Oracle** | Simple | Single point of failure, trust issues | ❌ Centralization risk |
| **IBC Relayers Only** | Standard pattern | No price data access | ❌ Insufficient |

**Decision: Each validator embeds Penumbra ViewServer as a library**

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 Zeratul Validator Process                   │
│                                                             │
│  ┌──────────────────┐         ┌──────────────────┐        │
│  │  Zeratul Node    │◄────────┤  Embedded        │        │
│  │  (Consensus)     │  Oracle │  ViewServer      │        │
│  │                  │  Prices │  (Penumbra SDK)  │        │
│  │  - Block Exec    │         │  - Light Client  │        │
│  │  - NOMT State    │         │  - SCT Tree      │        │
│  │  - Margin Trade  │         │  - SQLite DB     │        │
│  └────────┬─────────┘         └────────┬─────────┘        │
│           │                            │                   │
│           │ Propose Block              │ gRPC to          │
│           │ with Oracle Data           │ Penumbra Node    │
│           │                            │                   │
└───────────┼────────────────────────────┼───────────────────┘
            │                            │
            ▼                            ▼
    ┌───────────────┐          ┌─────────────────┐
    │  Zeratul      │◄─────────┤  Penumbra       │
    │  Network      │   IBC    │  Network        │
    │  (BFT)        │  Relayer │  (Tendermint)   │
    └───────────────┘          └─────────────────┘
```

## Component 1: Embedded ViewServer

### What Each Validator Runs

```rust
// blockchain/src/penumbra/light_client.rs

use penumbra_view::{ViewServer, Storage};
use penumbra_keys::FullViewingKey;
use penumbra_tct::Tree as StateCommitmentTree;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct EmbeddedPenumbraClient {
    /// Embedded ViewServer (runs in same process)
    view_server: Arc<ViewServer>,

    /// Configuration
    config: PenumbraClientConfig,

    /// Current sync height
    current_height: Arc<RwLock<u64>>,
}

impl EmbeddedPenumbraClient {
    /// Start embedded ViewServer
    pub async fn start(config: PenumbraClientConfig) -> Result<Self> {
        // 1. Load or create Full Viewing Key
        let fvk = if let Some(fvk_hex) = &config.fvk_hex {
            FullViewingKey::from_str(fvk_hex)?
        } else {
            // Use dummy FVK for oracle-only mode
            // (won't decrypt notes, but can query public DEX data)
            FullViewingKey::dummy()
        };

        // 2. Initialize ViewServer with SQLite storage
        let view_server = ViewServer::load_or_initialize(
            Some(&config.storage_path),  // SQLite database path
            None,                         // Optional asset registry
            &fvk,
            url::Url::parse(&config.node_url)?,
        ).await?;

        // ViewServer spawns background sync worker automatically
        // that fetches compact blocks and updates state

        Ok(Self {
            view_server: Arc::new(view_server),
            config,
            current_height: Arc::new(RwLock::new(0)),
        })
    }

    /// Get latest batch swap prices from Penumbra DEX
    pub async fn get_oracle_prices(
        &mut self,
        trading_pairs: Vec<(AssetId, AssetId)>,
    ) -> Result<HashMap<(AssetId, AssetId), Price>> {
        let mut prices = HashMap::new();

        for (base, quote) in trading_pairs {
            // Query latest batch swap output
            let batch_data = self.query_latest_batch_swap(base, quote).await?;

            // Extract clearing price from batch swap
            let clearing_price = calculate_price_from_batch(&batch_data)?;

            prices.insert((base, quote), clearing_price);
        }

        Ok(prices)
    }

    /// Query batch swap output data (this is PUBLIC on Penumbra)
    async fn query_latest_batch_swap(
        &mut self,
        base: AssetId,
        quote: AssetId,
    ) -> Result<BatchSwapOutputData> {
        // Use view client to query DEX component
        let request = QueryBatchSwapRequest {
            trading_pair: Some(TradingPair {
                asset_1: base.into(),
                asset_2: quote.into(),
            }),
            height: 0, // Latest
        };

        let response = self.view_client
            .query_batch_swap(request)
            .await?
            .into_inner();

        Ok(response.data.unwrap())
    }
}
```

### Validator Setup

Each validator needs:

```yaml
# validator-config.yaml

penumbra:
  # RPC endpoint of Penumbra node (can be shared)
  rpc_endpoint: "https://grpc.testnet.penumbra.zone:443"

  # Local pclientd data directory
  pclientd_home: "/var/lib/zeratul/pclientd"

  # Trading pairs to monitor for oracle prices
  oracle_pairs:
    - ["UM", "gm"]
    - ["UM", "gn"]
    - ["gm", "gn"]

  # Oracle update frequency (blocks)
  oracle_update_interval: 10
```

**Hardware Requirements:**
- +1GB storage for pclientd state
- +512MB RAM for light client
- Outbound connection to Penumbra RPC

## Component 2: Oracle Price Feed

### How Validators Agree on Prices

**Problem**: Each validator's light client sees the same Penumbra chain, but we need consensus on which price to use.

**Solution: Price Commitment + Median**

```rust
// blockchain/src/penumbra/oracle.rs

/// Oracle price proposal (included in block proposal)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleProposal {
    /// Penumbra block height where prices were observed
    pub penumbra_height: u64,

    /// Trading pair
    pub trading_pair: (AssetId, AssetId),

    /// Price observed by this validator
    pub price: Price,

    /// Signature from validator (proves they actually observed this)
    pub signature: Signature,
}

/// Consensus oracle update
pub struct OracleConsensus {
    /// Proposals from all validators
    proposals: Vec<OracleProposal>,
}

impl OracleConsensus {
    /// Aggregate oracle proposals into consensus price
    pub fn compute_consensus_price(&self) -> Result<Price> {
        // Require 2/3+ validators to submit proposals
        let min_proposals = (self.total_validators() * 2) / 3;
        if self.proposals.len() < min_proposals {
            bail!("insufficient oracle proposals");
        }

        // Take median price (resistant to outliers)
        let mut prices: Vec<Price> = self.proposals
            .iter()
            .map(|p| p.price)
            .collect();

        prices.sort();
        Ok(prices[prices.len() / 2])
    }

    /// Verify all proposals reference the same Penumbra height
    pub fn verify_proposals(&self) -> Result<()> {
        let first_height = self.proposals[0].penumbra_height;

        for proposal in &self.proposals {
            // All must reference same Penumbra block
            if proposal.penumbra_height != first_height {
                bail!("inconsistent Penumbra heights");
            }

            // Verify validator signatures
            verify_signature(proposal)?;
        }

        Ok(())
    }
}
```

### Oracle Update Flow

```
Block N:
1. Validator 1 proposes: UM/gm = 1.05 @ Penumbra height 12345
2. Validator 2 proposes: UM/gm = 1.04 @ Penumbra height 12345
3. Validator 3 proposes: UM/gm = 1.06 @ Penumbra height 12345
4. Validator 4 proposes: UM/gm = 1.05 @ Penumbra height 12345

Consensus:
- 4/4 validators submitted proposals ✓
- All reference Penumbra height 12345 ✓
- Median price: 1.05 ✓
- Consensus oracle price = 1.05

Block N+1:
- Use oracle price 1.05 for margin trading batch
- Execute all trades at this fair price
```

**Security Properties:**
- ✅ Byzantine resistant (2/3+ honest validators needed)
- ✅ No single point of failure (median of all proposals)
- ✅ Penumbra finality (all reference same Penumbra block)
- ✅ Verifiable (signatures prove observation)

## Component 3: IBC Integration

### Asset Locking (Penumbra → Zeratul)

**User Flow:**
```
1. User sends UM to Zeratul's IBC address on Penumbra
2. IBC packet relayed to Zeratul
3. Zeratul validators verify packet (via light client)
4. Credit user's account on Zeratul
5. User can now supply to lending pool or trade
```

**Implementation:**

```rust
// blockchain/src/penumbra/ibc.rs

use ibc::core::ics04_channel::packet::Packet;

pub struct IBCHandler {
    /// Embedded Penumbra client (verifies IBC proofs)
    penumbra_client: EmbeddedPenumbraClient,
}

impl IBCHandler {
    /// Verify and process incoming IBC packet from Penumbra
    pub async fn handle_incoming_packet(
        &mut self,
        packet: Packet,
    ) -> Result<IBCTransfer> {
        // 1. Verify IBC proof against Penumbra light client
        let proof_height = packet.proof_height;
        let verified = self.penumbra_client
            .verify_ibc_packet(&packet, proof_height)
            .await?;

        if !verified {
            bail!("invalid IBC proof");
        }

        // 2. Decode transfer data
        let transfer_data = decode_fungible_token_packet(&packet.data)?;

        // 3. Credit user on Zeratul
        Ok(IBCTransfer {
            sender: transfer_data.sender,
            receiver: transfer_data.receiver,
            asset_id: transfer_data.denom.parse()?,
            amount: Amount(transfer_data.amount.parse()?),
        })
    }

    /// Send IBC packet to Penumbra (unlock assets)
    pub async fn send_to_penumbra(
        &mut self,
        receiver: String,  // Penumbra address
        asset_id: AssetId,
        amount: Amount,
    ) -> Result<Packet> {
        // 1. Create IBC transfer packet
        let packet = create_fungible_token_packet(
            "zeratul-channel-0",  // Our channel
            "penumbra-channel-X", // Penumbra's channel
            asset_id.to_string(),
            amount.0.to_string(),
            receiver,
        )?;

        // 2. Validators will relay this packet
        Ok(packet)
    }
}
```

### IBC Relayer Setup

**Option 1: Hermes Relayer (Standard)**
```bash
# Run Hermes relayer alongside validator
hermes start \
  --config /etc/hermes/config.toml

# config.toml
[[chains]]
id = "zeratul-1"
rpc_addr = "http://localhost:26657"
grpc_addr = "http://localhost:9090"
websocket_addr = "ws://localhost:26657/websocket"

[[chains]]
id = "penumbra-testnet-X"
rpc_addr = "https://rpc.testnet.penumbra.zone:443"
grpc_addr = "https://grpc.testnet.penumbra.zone:443"
```

**Option 2: Embedded Relayer (Future)**
- Validators could run relayer as part of node software
- More automated, less setup

## Component 4: Profit Settlement

### Settling Profits Back to Penumbra

```rust
// blockchain/src/lending/settlement.rs

/// User wants to withdraw profits back to Penumbra
pub async fn settle_to_penumbra(
    position: &Position,
    ibc_handler: &mut IBCHandler,
    penumbra_address: String,
) -> Result<()> {
    // 1. Close position
    let pnl = calculate_pnl(position)?;

    if pnl.is_negative() {
        bail!("position has losses, cannot settle");
    }

    // 2. Calculate amounts to return
    let initial_collateral = position.initial_collateral();
    let profits = pnl.as_positive();
    let total_return = initial_collateral.checked_add(profits)?;

    // 3. Send IBC transfer back to Penumbra
    for (asset_id, amount) in total_return {
        ibc_handler
            .send_to_penumbra(
                penumbra_address.clone(),
                asset_id,
                amount,
            )
            .await?;
    }

    Ok(())
}
```

## Security Considerations

### Light Client Trust Model

**What Validators Trust:**
- Penumbra's validator set (via light client verification)
- 2/3+ of Penumbra validators are honest
- IBC proofs are valid

**What We Don't Trust:**
- Any single Zeratul validator (BFT consensus)
- Any single price source (median of all)
- External RPC endpoints (light client verifies all data)

### Attack Scenarios

**Attack 1: Fake Oracle Prices**
```
Attacker: Single validator reports fake price
Defense: Median of all validators, outliers ignored
Result: ✅ Attack fails (need 2/3+ validators)
```

**Attack 2: Stale Prices**
```
Attacker: Validator reports old Penumbra price
Defense: All validators must agree on Penumbra height
Result: ✅ Attack fails (height mismatch detected)
```

**Attack 3: IBC Packet Forgery**
```
Attacker: Try to fake IBC transfer from Penumbra
Defense: Light client verifies Merkle proofs
Result: ✅ Attack fails (invalid proof rejected)
```

## Deployment Guide

### Validator Setup Steps

1. **Install Penumbra Client**
```bash
# Install pclientd
cargo install --git https://github.com/penumbra-zone/penumbra pclientd

# Initialize client
pclientd init --grpc-url https://grpc.testnet.penumbra.zone:443

# Sync to latest
pclientd sync
```

2. **Configure Zeratul Validator**
```yaml
# /etc/zeratul/config.yaml
penumbra:
  pclientd_home: "/var/lib/zeratul/pclientd"
  rpc_endpoint: "https://grpc.testnet.penumbra.zone:443"
  oracle_pairs:
    - ["penumbra.core.asset.v1.Asset/penumbra1um...", "penumbra.core.asset.v1.Asset/penumbra1gm..."]
```

3. **Start Services**
```bash
# Start pclientd (as systemd service)
systemctl start zeratul-pclientd

# Start Zeratul validator
zeratul-validator run --config /etc/zeratul/config.yaml
```

4. **Setup IBC Relayer**
```bash
# Configure Hermes
hermes config auto \
  --chain-a zeratul-1 \
  --chain-b penumbra-testnet-X

# Create IBC channel
hermes create channel \
  --a-chain zeratul-1 \
  --b-chain penumbra-testnet-X \
  --a-port transfer \
  --b-port transfer

# Start relaying
hermes start
```

## Performance Characteristics

### Latency
- **Oracle updates**: ~10-30 seconds (Penumbra block time)
- **IBC transfers**: ~1-2 minutes (includes finality wait)
- **Light client sync**: ~1 minute for 100 blocks

### Resource Usage
- **Storage**: +1GB for pclientd state
- **Memory**: +512MB for light client
- **CPU**: Minimal (only verification)
- **Bandwidth**: ~10MB/day for light client sync

## Future Improvements

### Phase 2: Optimizations
1. **Checkpoint Sync**: Faster light client initial sync
2. **Compact Block Filters**: Reduce bandwidth
3. **Batch IBC**: Aggregate multiple transfers

### Phase 3: Advanced Features
1. **Multi-Chain Oracle**: Support prices from multiple sources
2. **Privacy-Preserving IBC**: Hide transfer amounts
3. **Optimistic Relaying**: Faster confirmation with fraud proofs

## Conclusion

**Answer to Original Question:**

> "do our validator software run pclientd lightclients?"

**Yes, each Zeratul validator runs an embedded Penumbra light client (`pclientd`) that:**
1. Connects to Penumbra network via RPC
2. Maintains ~1GB of verified state
3. Queries batch swap prices for oracle feed
4. Verifies IBC packet proofs
5. Enables trustless cross-chain integration

**This design is:**
- ✅ Decentralized (no central oracle)
- ✅ Secure (light client verification + BFT consensus)
- ✅ Efficient (light client not full node)
- ✅ Standard (uses IBC protocol)

The validators reach consensus on oracle prices by:
1. Each querying their own light client
2. Proposing price + Penumbra height + signature
3. Taking median of all proposals (Byzantine resistant)
4. Using consensus price for margin trading batch
