# Terminator Architecture

Following pcli's clean separation of UI and logic.

## Directory Structure

```
terminator/
├── src/
│   ├── command/           # Business logic (like pcli)
│   │   ├── query/        # Query commands
│   │   │   ├── dex.rs    # DEX queries (order book, positions)
│   │   │   ├── candles.rs # Historical price data
│   │   │   └── mod.rs
│   │   └── tx/           # Transaction commands
│   │       ├── swap.rs   # Submit swaps
│   │       ├── position.rs # Manage liquidity positions
│   │       └── mod.rs
│   │
│   ├── network/          # Network clients
│   │   ├── penumbra/     # Penumbra gRPC client
│   │   └── zeratul/      # Zeratul P2P client (future)
│   │
│   ├── ui/               # TUI rendering (presentation layer)
│   │   ├── panels/       # Panel renderers
│   │   │   ├── order_book_visual.rs
│   │   │   ├── chart_candles.rs
│   │   │   ├── order_entry.rs
│   │   │   └── positions.rs
│   │   └── mod.rs        # Main UI coordinator
│   │
│   ├── state/            # Application state
│   │   ├── app.rs        # Main app state
│   │   └── panel.rs      # Panel management
│   │
│   └── main.rs           # Entry point
```

## Separation of Concerns (Following pcli)

### 1. Command Layer (Business Logic)

Like pcli's `command/query/dex.rs`:

```rust
// src/command/query/dex.rs
pub async fn query_order_book(
    client: &mut DexQueryServiceClient<Channel>,
    pair: &TradingPair,
) -> Result<OrderBookData> {
    // Pure business logic
    // No UI concerns
    // Returns structured data
}
```

### 2. Data Structures (Serializable)

Like pcli's JSON types:

```rust
#[derive(Serialize, Debug, Clone)]
pub struct OrderBookData {
    pub pair: TradingPair,
    pub bids: Vec<LevelData>,
    pub asks: Vec<LevelData>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Serialize, Debug, Clone)]
pub struct LevelData {
    pub price: f64,
    pub size: f64,
    pub total: f64,
}
```

### 3. UI Layer (Presentation)

Like pcli's `utils::render_positions`:

```rust
// src/ui/panels/order_book_visual.rs
pub fn render_order_book(
    f: &mut Frame,
    data: &OrderBookData,  // Takes data structure
    rect: Rect,
    style: Style,
) {
    // Pure presentation
    // No business logic
    // Just rendering
}
```

### 4. Network Layer

Adapted from pcli's gRPC clients:

```rust
// src/network/penumbra/client.rs
pub struct PenumbraClient {
    dex_client: DexQueryServiceClient<Channel>,
    // ...
}

impl PenumbraClient {
    pub async fn stream_order_book(&mut self) -> Result<impl Stream<Item = OrderBookData>> {
        // Network communication only
    }
}
```

## Data Flow

```
User Input (keyboard/mouse)
    ↓
App State (state/app.rs)
    ↓
Command Layer (command/query/*.rs)
    ↓
Network Layer (network/penumbra/*.rs)
    ↓
Penumbra gRPC
    ↓
Data Structures (OrderBookData, etc.)
    ↓
UI Layer (ui/panels/*.rs)
    ↓
Terminal Display
```

## Key Principles from pcli

1. **Commands are async functions** - Not methods on App
2. **Data structures are serializable** - Can be JSON, can be saved
3. **UI takes data structures** - No direct network access
4. **App coordinates** - Calls commands, updates state, triggers UI

## Example: Fetching Order Book

### Bad (Current)

```rust
// In AppState
pub async fn update_market_data(&mut self) {
    // Business logic mixed with state management
    let update = self.grpc_client.fetch_order_book().await?;
    self.order_book = update;
}
```

### Good (Following pcli)

```rust
// In command/query/dex.rs
pub async fn query_order_book(
    client: &mut DexQueryServiceClient<Channel>,
    pair: &TradingPair,
) -> Result<OrderBookData> {
    let request = LiquidityPositionsRequest { /* ... */ };
    let response = client.liquidity_positions(request).await?;
    positions_to_order_book(&response.positions, pair)
}

// In main.rs event loop
loop {
    // Command execution
    if let Some(client) = &mut app.penumbra_client {
        let order_book = command::query::dex::query_order_book(
            client,
            &app.current_pair
        ).await?;
        app.order_book_data = Some(order_book);
    }

    // UI rendering
    terminal.draw(|f| ui::render(f, &app))?;
}
```

## Benefits

1. **Testability** - Commands can be tested without UI
2. **Reusability** - Same commands work in CLI and TUI
3. **Clarity** - Each layer has one responsibility
4. **Maintainability** - Easy to find and fix bugs

---

*Inspired by pcli's excellent architecture* 🎯
