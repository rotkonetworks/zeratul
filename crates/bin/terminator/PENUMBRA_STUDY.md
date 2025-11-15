# Penumbra Architecture Study

## Overview

Penumbra is a privacy-focused blockchain with:
- Shielded transactions (like Zcash)
- Decentralized exchange (DEX with concentrated liquidity)
- Staking & governance
- IBC (Inter-Blockchain Communication)

## Directory Structure

```
penumbra/
├── crates/
│   ├── bin/          # Binaries
│   │   ├── pcli      # CLI wallet (like our Terminator!)
│   │   ├── pclientd  # Client daemon
│   │   ├── pd        # Full node
│   │   └── pindexer  # Indexer for historical data
│   │
│   ├── core/         # Core blockchain logic
│   │   ├── asset/    # Asset types & registry
│   │   ├── keys/     # Cryptographic keys (FVK, spending keys)
│   │   ├── num/      # Numeric types (amounts, balances)
│   │   ├── transaction/  # Transaction building
│   │   └── component/    # State machine components
│   │       ├── dex/      # DEX logic
│   │       ├── stake/    # Staking
│   │       ├── governance/  # Governance
│   │       └── shielded-pool/  # Private transfers
│   │
│   ├── crypto/       # Cryptography
│   │   ├── decaf377/       # Curve25519 variant
│   │   ├── decaf377-frost/ # Threshold signatures
│   │   ├── proof-params/   # Groth16 parameters
│   │   └── tct/            # Tiered commitment tree
│   │
│   ├── view/         # View service (wallet state)
│   ├── custody/      # Key management & signing
│   ├── proto/        # Protobuf definitions
│   └── wallet/       # Wallet logic
```

## Key Components for Terminator

### 1. `pcli` - Command Line Interface

**Location:** `crates/bin/pcli/`

**Purpose:** Reference implementation for wallet operations

**Key files:**
- `src/main.rs` - Entry point, command routing
- `src/command/query/dex.rs` - DEX queries (ORDER BOOK!)
- `src/command/tx/liquidity_position.rs` - LP management
- `src/dex_utils/` - DEX utilities

**How it works:**
```rust
// pcli loads config
let config = PcliConfig::load(home_dir)?;
let fvk = config.full_viewing_key;

// Creates ViewService from sqlite
let view = ViewServer::load_or_initialize(
    pcli-view.sqlite,
    fvk,
    grpc_url
)?;

// Queries via gRPC
let client = DexQueryServiceClient::connect(grpc_url)?;
let positions = client.liquidity_positions(request)?;
```

### 2. View Service - Wallet State

**Location:** `crates/view/`

**Purpose:** Tracks wallet state, scans blockchain, stores in sqlite

**Key concepts:**
- Scans blocks for notes owned by FVK
- Stores in `pcli-view.sqlite`
- Provides query interface via gRPC

**Database schema:**
```
tables:
- notes: Spendable notes
- assets: Asset metadata
- transactions: Transaction history
- balances: Current balances per asset
```

**API:**
```rust
// Query balances
let request = BalancesRequest { .. };
let mut stream = view_client.balances(request).await?;
while let Some(response) = stream.next().await {
    let balance: Value = response.balance?;
}
```

### 3. DEX Component

**Location:** `crates/core/component/dex/`

**Purpose:** Decentralized exchange logic

**Key types:**
```rust
// Trading pair
pub struct TradingPair {
    asset_1: AssetId,
    asset_2: AssetId,
}

// Liquidity position
pub struct Position {
    pub phi: TradingFunction,  // Pricing function
    pub reserves: Reserves,     // Current reserves
    pub state: PositionState,   // Open/Closed/Withdrawn
}

// Concentrated liquidity
pub struct TradingFunction {
    pub pair: TradingPair,
    pub fee_bps: u32,          // Fee in basis points
    pub p: Amount,              // Price parameters
    pub q: Amount,
}
```

**Batch auction process:**
1. Users submit swaps
2. Validator collects all swaps for the block
3. Batch execution: all swaps executed at same price
4. Fills distributed pro-rata
5. LP fees distributed

### 4. Proto Definitions

**Location:** `crates/proto/`

**Purpose:** gRPC service definitions

**Key services:**
```protobuf
// DEX queries
service QueryService {
  rpc LiquidityPositions(LiquidityPositionsRequest)
    returns (stream LiquidityPositionsResponse);

  rpc LiquidityPositionById(LiquidityPositionByIdRequest)
    returns (LiquidityPositionByIdResponse);

  rpc BatchSwapOutputData(BatchSwapOutputDataRequest)
    returns (BatchSwapOutputDataResponse);
}

// View service
service ViewService {
  rpc Balances(BalancesRequest) returns (stream BalancesResponse);
  rpc Notes(NotesRequest) returns (stream NotesResponse);
  rpc TransactionInfo(TransactionInfoRequest) returns (TransactionInfoResponse);
  rpc Status(StatusRequest) returns (stream StatusResponse);
}
```

### 5. Keys & Addresses

**Location:** `crates/core/keys/`

**Key hierarchy:**
```
Seed Phrase (24 words)
    ↓
Spend Seed
    ↓
Spend Key (private)
    ↓
Full Viewing Key (FVK) - Can see all transactions
    ↓
Address - Public receiving address
```

**Code:**
```rust
pub struct FullViewingKey {
    // Can:
    // - See all incoming/outgoing transactions
    // - Derive addresses
    // - Query balances
    // Cannot:
    // - Spend funds (need Spend Key for that)
}
```

## How pcli Works (Simplified)

### Startup
```rust
// 1. Load config
let home = ~/.local/share/pcli/
let config = read(home/config.toml)?;
let fvk = config.full_viewing_key;
let grpc_url = config.grpc_url;

// 2. Load view database
let db = sqlite::open(home/pcli-view.sqlite)?;
let view = ViewServer::new(db, fvk, grpc_url)?;

// 3. Sync (if needed)
view.sync().await?;  // Scans new blocks

// 4. Execute command
match command {
    Query::Balance => view.balances().await?,
    Query::Dex::Positions => dex_client.liquidity_positions().await?,
    Tx::Swap => build_swap_tx(...),
}
```

### Querying Order Book

```rust
// From pcli/src/command/query/dex.rs

pub async fn query_order_book(
    pair: &TradingPair,
    client: &mut DexQueryServiceClient,
) -> Result<Vec<Position>> {
    let request = LiquidityPositionsRequest {
        // Can filter by:
        // - trading pair
        // - position state (Open/Closed)
        // - price range
    };

    let mut stream = client.liquidity_positions(request).await?;
    let mut positions = Vec::new();

    while let Some(response) = stream.next().await {
        let position: Position = response.position?.try_into()?;
        positions.push(position);
    }

    // Convert concentrated liquidity to order book
    let (bids, asks) = positions_to_order_book(&positions, pair);

    Ok((bids, asks))
}
```

### Converting Positions to Order Book

Penumbra uses **concentrated liquidity** (like Uniswap v3), not traditional order books:

```rust
// Position has:
// - phi: TradingFunction (defines price curve)
// - reserves: (r1, r2) current reserves

// To get price at this position:
let price = phi.p / phi.q;

// To get liquidity depth:
let depth = reserves.r1 + reserves.r2;

// Create order book level:
OrderBookLevel {
    price,
    size: depth,
    total: cumulative_depth,
}
```

## Integration Points for Terminator

### 1. Use pcli's Config & Database

✅ **Already done!** We load from `~/.local/share/pcli/`

```rust
// terminator/src/wallet/mod.rs
let home = pcli_home();  // ~/.local/share/pcli/
let config = PcliConfig::load(home.join("config.toml"))?;
let view = ViewServer::load_or_initialize(
    home.join("pcli-view.sqlite"),
    config.full_viewing_key,
    config.grpc_url,
)?;
```

### 2. Query DEX Data

```rust
// Connect to same gRPC endpoint
let client = DexQueryServiceClient::connect(config.grpc_url)?;

// Get liquidity positions
let positions = client.liquidity_positions(request).await?;

// Convert to order book for UI
let order_book = positions_to_order_book(&positions);
```

### 3. Real-time Updates

```rust
// Background task
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let positions = client.liquidity_positions(request).await?;
        update_tx.send(positions).await?;
    }
});

// Main UI loop
while let Ok(positions) = update_rx.try_recv() {
    app.order_book = positions_to_order_book(&positions);
    render_ui(&app);
}
```

### 4. Submit Swaps

```rust
// Build swap transaction
let swap = SwapPlaintext {
    trading_pair,
    delta_1_i: input_amount,
    delta_2_i: output_amount,
    claim_fee: Fee::default(),
    claim_address: my_address,
};

// Sign with custody service
let auth_data = custody_client.authorize(swap_plan).await?;
let tx = Transaction::build(swap, auth_data)?;

// Broadcast
let result = client.broadcast_tx_sync(tx).await?;
```

## Key Lessons for Terminator

### 1. **Same Data Structures**

Use Penumbra's types directly:
- `TradingPair`
- `Position`
- `Value` (amounts with asset ID)
- `Address`

### 2. **Streaming gRPC**

Most Penumbra APIs return streams, not single responses:
```rust
let mut stream = client.balances(request).await?;
while let Some(response) = stream.next().await {
    // Handle each balance
}
```

### 3. **Concentrated Liquidity ≠ Order Book**

Penumbra uses Uniswap-v3 style concentrated liquidity. To show an "order book", we need to:
- Query all positions for a pair
- Calculate effective price at each position
- Aggregate into price levels
- Show as bids/asks

### 4. **Privacy-First**

All transactions are shielded by default. We can only see:
- Our own transactions (via FVK)
- Public chain state (total liquidity, trading pairs)
- Cannot see other users' orders/balances

## Example: Building Terminator's Order Book View

```rust
// 1. Query positions
let request = LiquidityPositionsRequest {
    // Filter for our trading pair
};
let mut stream = dex_client.liquidity_positions(request).await?;

// 2. Collect positions
let mut positions = Vec::new();
while let Some(response) = stream.next().await? {
    positions.push(response.position?);
}

// 3. Convert to order book
let mut bids = Vec::new();
let mut asks = Vec::new();

for position in positions {
    let price = calculate_position_price(&position);
    let size = calculate_position_liquidity(&position);

    // Positions selling asset_1 for asset_2 = asks
    // Positions buying asset_1 with asset_2 = bids
    if is_selling_asset_1(&position) {
        asks.push(Level { price, size });
    } else {
        bids.push(Level { price, size });
    }
}

// 4. Sort
bids.sort_by(|a, b| b.price.cmp(&a.price));  // Highest first
asks.sort_by(|a, b| a.price.cmp(&b.price));  // Lowest first

// 5. Display in TUI
render_order_book(&bids, &asks);
```

## Next Steps for Terminator

1. ✅ **Use pcli's wallet** - Done!
2. ✅ **Stream liquidity positions** - Done!
3. ⏳ **Convert positions to order book** - TODO
4. ⏳ **Submit swap transactions** - TODO
5. ⏳ **LP position management** - TODO

## Useful pcli Commands for Reference

```bash
# View DEX positions
pcli query dex lp list

# Get order book simulation
pcli query dex lp list --trading-pair penumbra:gm

# View balances
pcli view balance

# Submit swap
pcli tx swap 100penumbra --into gm

# Open LP position
pcli tx position open \
  --pair penumbra:gm \
  --fee 30 \  # 0.30%
  --position "1000penumbra@2.0gm"
```

## Summary

Penumbra architecture:
- **pcli** = Reference wallet (what we're building a TUI version of)
- **ViewService** = Local wallet state (sqlite database)
- **DEX** = Concentrated liquidity (not traditional order book)
- **gRPC** = Communication protocol
- **Proto** = API definitions

Terminator integrates by:
- Sharing pcli's config & database
- Connecting to same gRPC endpoint
- Querying DEX positions
- Rendering as traditional order book UI
- Eventually submitting transactions

We're on the right track! 🎯
