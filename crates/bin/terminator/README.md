# Terminator 🚀

**High-performance TUI trading terminal for Zeratul blockchain**

Full-screen, mouse-controlled trading experience with resizable/movable panels.

## Features

✅ **Mouse-Controlled Panels**
- Drag panels to move
- Resize from edges and corners
- Click to select active panel

✅ **Real-Time Market Data**
- Live order book display
- Price chart (coming soon)
- Recent trades feed

✅ **Order Management**
- Quick buy/sell order entry
- Position tracking
- Fill history

✅ **Professional Layout**
- 4-panel default layout
- Customizable panel arrangement
- Clean, information-dense UI

## Quick Start

### 1. Prerequisites

```bash
# Install pcli and initialize wallet
pcli init --grpc-url https://penumbra.rotko.net

# Sync wallet
pcli view sync

# Check balance
pcli view balance
```

### 2. Build Terminator

```bash
# Use the build script (handles rocksdb)
./build.sh

# Or manually:
export ROCKSDB_LIB_DIR=/usr/lib64
cargo build --release
```

### 3. Run

```bash
./target/release/terminator
```

## Keyboard Controls

| Key | Action |
|-----|--------|
| `Q` | Quit |
| `R` | Toggle resize mode |
| `Tab` | Switch active panel |

## Mouse Controls

**Normal Mode:**
- Click: Select panel

**Resize Mode (press `R`):**
- Drag panel: Move it
- Drag right edge: Resize width
- Drag bottom edge: Resize height
- Drag corner: Resize both dimensions

## Panels

### 1. Order Book
- Real-time bid/ask levels
- Size and price display
- Color-coded (green bids, red asks)

### 2. Price Chart
- Historical price data
- Coming soon: Candlestick charts

### 3. Order Entry
- Quick buy/sell forms
- Price and amount input
- One-click order submission

### 4. Positions & Fills
- Active orders
- Fill history
- P&L tracking (coming soon)

## Architecture

```
terminator/
├── src/
│   ├── main.rs           # App entry and event loop
│   ├── state/            # Application state
│   │   ├── mod.rs        # Main state with order book, orders, fills
│   │   └── panel.rs      # Panel system (resize/move)
│   ├── ui/               # Rendering
│   │   └── mod.rs        # Main render function
│   └── panels/           # Panel implementations
│       ├── order_book.rs # Order book display
│       ├── chart.rs      # Price chart
│       ├── order_entry.rs # Buy/sell forms
│       ├── positions.rs  # Positions & fills
│       └── recent_trades.rs # Trade feed
```

## Integration with Zeratul

### Phase 1 (Current)
- [x] TUI interface
- [x] Mock market data
- [x] Panel system

### Phase 2 (Next)
- [ ] Connect to Zeratul P2P network
- [ ] Submit orders to batch executor
- [ ] Receive fill notifications
- [ ] Real-time order book from validators

### Phase 3 (Future)
- [ ] Privacy layer (encrypted orders)
- [ ] Advanced charting (TradingView-style)
- [ ] Portfolio analytics
- [ ] Multi-market support

## The Vision 🌸

Terminator is the **user-facing interface** for Zeratul's batch auction blockchain:

```
User (Terminator) → P2P Network → Validators
                                      ↓
                                 Batch Executor (PCVM)
                                      ↓
                                 Ligerito Proof
                                      ↓
                                 Consensus
                                      ↓
                                 Fills → User
```

**Every 1 second:**
1. Users submit orders via Terminator
2. Validators collect and batch orders
3. Batch auction executes in PCVM
4. Ligerito proves execution (67ms)
5. Consensus commits fills
6. Terminator shows results

**Privacy:** Orders batched together, pro-rata fills hide individual trades.

**Speed:** 67ms proof generation, 15ms verification per batch.

**MEV-Proof:** Batch auction prevents frontrunning.

## Dependencies

- `ratatui` - TUI framework with mouse support
- `crossterm` - Terminal control
- `tokio` - Async runtime
- `rust_decimal` - Precise decimal math for prices

## Contributing

Terminator is part of the Zeratul blockchain project. See the main repo for contribution guidelines.

---

*"The best trading interface is the one that gets out of your way."* 🎯
