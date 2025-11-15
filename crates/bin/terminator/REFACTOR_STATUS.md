# Crux Architecture Refactor - STATUS

## ✅ Phase 1: Core Module Created

### What We Built

#### 1. **`core/` Module** - Platform-Agnostic Business Logic
```
src/core/
├── mod.rs           # Module exports
├── event.rs         # All possible events (30+ variants)
├── effect.rs        # Side effects to execute
├── types.rs         # Pure data structures
└── app.rs           # Core update logic
```

#### 2. **Event System** (`core/event.rs`)
- **Market Data Events**: `OrderBookUpdated`, `TradeExecuted`, `CandleUpdated`
- **User Interactions**: `ChartClicked`, `PositionDragged`, `SliderMoved`
- **Position Management**: `CreatePosition`, `CancelPosition`, `ClosePosition`
- **Navigation**: `PanelFocused`, `NextPanel`, `ResizeModeToggled`
- **Keyboard**: `LimitOrderAtCursor`, `MarketOrder`, `CancelAction`, `Quit`
- **System**: `Tick`, `BalancesUpdated`

All events are:
- ✅ Serializable (can send over network)
- ✅ Cloneable (can store in history)
- ✅ Pure data (no logic)

#### 3. **Effect System** (`core/effect.rs`)
- **Rendering**: `Render(ViewModel)`
- **Penumbra Ops**: `SubmitPosition`, `ClosePosition`, `WithdrawPosition`
- **Data Fetching**: `StreamOrderBook`, `FetchTrades`, `FetchCandles`, `RefreshBalances`
- **UI Feedback**: `ShowNotification`, `ShowConfirmation`, `SetCursor`
- **System**: `Exit`, `None`

Helper methods:
```rust
Effect::success("Position created!")
Effect::error("Failed to submit")
Effect::info("Loading...")
```

#### 4. **Core Types** (`core/types.rs`)
Pure data structures with no UI dependencies:
- `OrderBook` - Bids/asks with `mid_price()` and `spread()`
- `Trade` - Execution with block height
- `Position` - User's LP positions
- `Interaction` - Current UI interaction state
- `ViewModel` - Optimized for rendering

#### 5. **App Core** (`core/app.rs`)
Pure update function:
```rust
impl AppCore {
    pub fn update(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::ChartClicked { price, .. } => {
                // Pure logic - no side effects!
                self.interaction = Interaction::CreatingPosition { ... };
                vec![Effect::Render(self.view_model())]
            }
            // ... 30+ more events
        }
    }
}
```

## Key Features

### ✅ Testable Core
```rust
#[test]
fn test_chart_click() {
    let mut core = AppCore::new();
    let effects = core.update(Event::ChartClicked { price: 3000.0, .. });
    assert!(matches!(core.interaction, Interaction::CreatingPosition { .. }));
}
```

### ✅ Logarithmic Slider Built-In
```rust
fn slider_to_size(position: f64, min: f64, max: f64) -> f64 {
    let log_min = min.ln();
    let log_max = max.ln();
    (log_min + position * (log_max - log_min)).exp()
}
// position=0.0 → 0.1
// position=0.5 → 3.16
// position=1.0 → 100.0
```

### ✅ Auto Buy/Sell Detection
```rust
Event::ChartClicked { price, .. } => {
    let mid_price = self.order_book.mid_price().unwrap_or(price);
    let side = if price > mid_price {
        Side::Sell  // 🔴 Above mid = sell
    } else {
        Side::Buy   // 🟢 Below mid = buy
    };
    // ...
}
```

### ✅ Serializable State
```rust
let state_json = serde_json::to_string(&core)?;
// Save session, send over network, etc.
```

## Next Steps

### Phase 2: TUI Shell ✅ COMPLETE
Created TUI shell implementation:
- ✅ Created `shell/tui/mod.rs` - Main TUI shell with event loop
- ✅ Created `shell/tui/mapper.rs` - Maps terminal events to core Events
- ✅ Created `shell/tui/executor.rs` - Executes Effects (submit positions, fetch data)
- ✅ Created `shell/tui/renderer.rs` - Renders ViewModel to terminal
- [ ] Adapt `main.rs` to use TuiShell (next step)
- [ ] Test complete Event → Core → Effect → Executor flow

### Phase 3: GUI Shell (Future - Twilight)
```
src/shell/gui/
├── main.rs          # egui app
├── chart.rs         # Pixel-perfect chart
├── slider.rs        # Native logarithmic slider
└── panels.rs        # GUI panels
```

### Phase 4: Web Shell (Future)
```
terminator-web/
└── src/lib.rs       # WASM + yew/leptos
```

## Benefits Achieved

1. **Platform Agnostic** - Core has zero UI dependencies
2. **Easily Testable** - Pure functions, no mocking needed
3. **Reusable** - Same core for TUI/GUI/Web/Mobile
4. **Serializable** - Can save/load state, send over network
5. **Future Proof** - Add new shells without touching core
6. **Type Safe** - All events/effects are strongly typed

## File Organization

```
crates/bin/terminator/
├── src/
│   ├── core/                # ✅ DONE - Platform-agnostic
│   │   ├── mod.rs
│   │   ├── event.rs
│   │   ├── effect.rs
│   │   ├── types.rs
│   │   └── app.rs
│   │
│   ├── shell/               # ✅ DONE - UI implementations
│   │   └── tui/             # Terminal UI (mapper, executor, renderer)
│   │       ├── mod.rs
│   │       ├── mapper.rs
│   │       ├── executor.rs
│   │       └── renderer.rs
│   │
│   ├── network/             # ✅ Keep - Penumbra client
│   ├── wallet/              # ✅ Keep - Wallet integration
│   ├── panels/              # → Move to shell/tui/panels/
│   ├── ui/                  # → Move to shell/tui/ui/
│   ├── state/               # → Deprecated (use core/)
│   └── main.rs              # → Adapt to use core
```

## Example Usage (Future)

### TUI Shell
```rust
let mut core = AppCore::new();
let event = map_terminal_event(crossterm_event);
let effects = core.update(event);
for effect in effects {
    execute_effect(effect).await?;
}
```

### GUI Shell (egui)
```rust
let mut core = AppCore::new();
if response.clicked() {
    let event = Event::ChartClicked { price, x, y };
    let effects = core.update(event);
    for effect in effects {
        execute_effect(effect);
    }
}
```

### Web Shell (WASM)
```rust
let core = use_state(|| AppCore::new());
let event_json = ws.recv_string()?;
let event: Event = serde_json::from_str(&event_json)?;
core.update(event);
```

## Testing

Run tests:
```bash
cargo test --lib
```

Current tests:
- ✅ Chart click creates interaction
- ✅ Logarithmic slider math
- ✅ Auto buy/sell detection

## Migration Notes

The existing code is still functional. Core refactor is **additive**, not breaking. We can migrate incrementally:

1. Keep existing code running
2. Add core module (done!)
3. Gradually adapt shell to use core
4. Remove old state module when complete

---

**Status**: Phase 2 Complete ✅
**Next**: Adapt `main.rs` to use the new TuiShell and test the complete flow

## Latest Updates (2025-11-15)

### ✅ Fixed Order Book Parsing
Successfully adapted to new Penumbra protobuf structure:
- `phi.component` now contains `BareTradingFunction` with p, q, fee
- `position.state` is `Option<i32>` where 1 = Opened
- Price calculation: `p/q` from `component.p` and `component.q`
- Reserves parsing: `r1` and `r2` from Amount types (hi/lo u64)

### ✅ Added RPC Configuration
- Reads `PENUMBRA_RPC_URL` environment variable
- Defaults to `https://penumbra.rotko.net` (Rotko infrastructure)
- Can override: `PENUMBRA_RPC_URL=https://other-rpc.com ./terminator`

### Ready for Testing
Once binary builds (rocksdb linking resolved), the app will:
1. Connect to Rotko RPC by default
2. Stream real liquidity positions
3. Parse order book from concentrated liquidity
4. Display in TUI with proper bid/ask levels

