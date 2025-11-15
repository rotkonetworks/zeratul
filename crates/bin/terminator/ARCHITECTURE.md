# Terminator Architecture - Crux Philosophy

## Design Goal: Platform-Agnostic Core

Following [Crux](https://redbadger.github.io/crux/) architecture:
- **Core**: Pure business logic (no UI dependencies)
- **Shell**: Interchangeable frontends (TUI/GUI/Web)

This lets us easily add:
- Desktop GUI (egui)
- Web interface (WASM)
- Mobile apps (iOS/Android)
- CLI tools (for automation)

## Directory Structure

```
crates/bin/terminator/
├── src/
│   ├── core/              # Platform-agnostic business logic
│   │   ├── mod.rs
│   │   ├── app.rs         # Core app state & logic
│   │   ├── trading.rs     # Position management
│   │   ├── market_data.rs # Order book, trades
│   │   └── penumbra.rs    # Blockchain integration
│   │
│   ├── shell/             # UI implementations
│   │   ├── mod.rs
│   │   ├── tui/           # Terminal UI (ratatui)
│   │   │   ├── mod.rs
│   │   │   ├── main.rs
│   │   │   └── panels/
│   │   │
│   │   └── gui/           # Desktop GUI (egui) - future
│   │       ├── mod.rs
│   │       └── main.rs
│   │
│   ├── capabilities/      # Side effects (Crux pattern)
│   │   ├── http.rs        # gRPC client
│   │   ├── storage.rs     # Wallet/config
│   │   └── time.rs        # Clock
│   │
│   └── main.rs            # Shell selector
```

## Core - Business Logic

### No UI Dependencies!

```rust
// crates/bin/terminator/src/core/app.rs

use serde::{Serialize, Deserialize};

/// Core application state - pure data
#[derive(Clone, Serialize, Deserialize)]
pub struct AppCore {
    pub order_book: OrderBook,
    pub recent_trades: Vec<Trade>,
    pub positions: Vec<Position>,
    pub balances: Vec<Balance>,
    pub interaction: Option<Interaction>,
}

/// User interactions - pure events
#[derive(Clone, Serialize, Deserialize)]
pub enum Event {
    // Market data
    OrderBookUpdated(OrderBook),
    TradeExecuted(Trade),
    
    // User actions
    ChartClicked { price: f64, y_coord: f64 },
    PositionDragged { id: String, new_price: f64 },
    SliderMoved { position: f64 },
    
    // Position management
    CreatePosition { side: Side, price: f64, size: f64 },
    CancelPosition { id: String },
    
    // Navigation
    PanelFocused(PanelType),
}

/// Effects to request from shell
#[derive(Clone, Serialize, Deserialize)]
pub enum Effect {
    // Render updates
    Render(ViewModel),
    
    // Penumbra operations
    SubmitPosition { phi: TradingFunction, reserves: Reserves },
    ClosePosition { id: String },
    
    // Data fetching
    StreamOrderBook { pair: TradingPair },
    FetchTrades { start_height: u64 },
    
    // UI feedback
    ShowNotification { message: String, level: NotificationLevel },
}

/// Pure update function - no side effects!
impl AppCore {
    pub fn update(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::ChartClicked { price, .. } => {
                // Detect buy/sell based on current market price
                let current_price = self.order_book.mid_price();
                let side = if price > current_price {
                    Side::Sell
                } else {
                    Side::Buy
                };
                
                // Enter position creation mode
                self.interaction = Some(Interaction::CreatingPosition {
                    price,
                    side,
                    size: 0.1, // Default
                });
                
                // Request render update
                vec![Effect::Render(self.view_model())]
            }
            
            Event::SliderMoved { position } => {
                if let Some(Interaction::CreatingPosition { size, .. }) = &mut self.interaction {
                    // Logarithmic slider
                    *size = slider_to_size(position, 0.1, 100.0);
                    vec![Effect::Render(self.view_model())]
                } else {
                    vec![]
                }
            }
            
            Event::CreatePosition { side, price, size } => {
                // Build trading function
                let phi = self.build_trading_function(side, price);
                let reserves = self.calculate_reserves(side, size);
                
                vec![
                    Effect::SubmitPosition { phi, reserves },
                    Effect::ShowNotification {
                        message: format!("Creating {} position at ${}", side, price),
                        level: NotificationLevel::Info,
                    },
                ]
            }
            
            // ... other events
            _ => vec![]
        }
    }
    
    /// Convert state to view model
    fn view_model(&self) -> ViewModel {
        ViewModel {
            order_book: self.order_book.clone(),
            recent_trades: self.recent_trades.clone(),
            positions: self.positions.clone(),
            interaction: self.interaction.clone(),
            // ... other fields
        }
    }
}

/// View model - data optimized for rendering
#[derive(Clone, Serialize, Deserialize)]
pub struct ViewModel {
    pub order_book: OrderBook,
    pub recent_trades: Vec<Trade>,
    pub positions: Vec<Position>,
    pub interaction: Option<Interaction>,
    // Computed fields for convenience
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub spread: Option<f64>,
}
```

## Shell - UI Layer

### TUI Implementation

```rust
// crates/bin/terminator/src/shell/tui/main.rs

use crate::core::{AppCore, Event, Effect, ViewModel};

struct TuiShell {
    core: AppCore,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    penumbra_client: PenumbraClient,
}

impl TuiShell {
    async fn run(&mut self) -> Result<()> {
        loop {
            // Render current view model
            let view_model = self.core.view_model();
            self.terminal.draw(|f| self.render(f, &view_model))?;
            
            // Handle events
            if event::poll(Duration::from_millis(100))? {
                let event = self.map_terminal_event(event::read()?)?;
                let effects = self.core.update(event);
                
                // Execute effects
                for effect in effects {
                    self.execute_effect(effect).await?;
                }
            }
            
            // Poll for network updates
            self.poll_penumbra_updates().await?;
        }
    }
    
    fn map_terminal_event(&self, event: crossterm::event::Event) -> Result<Event> {
        match event {
            crossterm::event::Event::Mouse(mouse) => {
                if let Some(chart_rect) = self.get_chart_rect() {
                    if chart_rect.contains(mouse.column, mouse.row) {
                        let price = self.y_to_price_log(mouse.row, chart_rect);
                        return Ok(Event::ChartClicked {
                            price,
                            y_coord: mouse.row as f64,
                        });
                    }
                }
                Ok(Event::Ignored)
            }
            // ... other mappings
        }
    }
    
    async fn execute_effect(&mut self, effect: Effect) -> Result<()> {
        match effect {
            Effect::SubmitPosition { phi, reserves } => {
                self.penumbra_client.submit_position(phi, reserves).await?;
            }
            Effect::StreamOrderBook { pair } => {
                self.penumbra_client.stream_order_book(pair).await?;
            }
            Effect::ShowNotification { message, level } => {
                // Show in status bar or popup
                self.show_notification(message, level);
            }
            _ => {}
        }
        Ok(())
    }
}
```

### Future: GUI Implementation

```rust
// crates/bin/terminator/src/shell/gui/main.rs

use crate::core::{AppCore, Event, Effect, ViewModel};
use egui::*;

struct GuiShell {
    core: AppCore,
    penumbra_client: PenumbraClient,
}

impl eframe::App for GuiShell {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let view_model = self.core.view_model();
        
        CentralPanel::default().show(ctx, |ui| {
            // Render chart with click detection
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(800.0, 600.0),
                Sense::click_and_drag(),
            );
            
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let price = y_to_price_log(pos.y, rect);
                    let effects = self.core.update(Event::ChartClicked {
                        price,
                        y_coord: pos.y as f64,
                    });
                    self.execute_effects(effects);
                }
            }
            
            // Draw order book
            self.render_order_book(ui, &view_model);
            
            // Logarithmic slider (native in egui!)
            if let Some(Interaction::CreatingPosition { size, .. }) = &view_model.interaction {
                ui.add(Slider::new(&mut size, 0.1..=100.0).logarithmic(true));
            }
        });
    }
}
```

## Benefits of This Architecture

### ✅ Easy to Add Platforms
```bash
# Add web frontend
cargo new --lib terminator-web
wasm-pack build

# Add mobile
cargo new --lib terminator-mobile
# Use Crux FFI bindings for Swift/Kotlin
```

### ✅ Testable Core
```rust
#[test]
fn test_chart_click_creates_position() {
    let mut core = AppCore::new();
    
    let effects = core.update(Event::ChartClicked {
        price: 3000.0,
        y_coord: 100.0,
    });
    
    assert!(matches!(core.interaction, Some(Interaction::CreatingPosition { .. })));
    assert!(effects.contains(&Effect::Render(_)));
}
```

### ✅ Serializable State
```rust
// Save session
let state = serde_json::to_string(&core)?;
std::fs::write("session.json", state)?;

// Resume later
let core: AppCore = serde_json::from_str(&std::fs::read_to_string("session.json")?)?;
```

### ✅ Network Protocol
```rust
// Core events are serializable - can send over network!
// Future: Web UI connects to desktop core via WebSocket
let event_json = serde_json::to_string(&Event::ChartClicked { price: 3000.0, y_coord: 100.0 })?;
websocket.send(event_json)?;
```

## Current Migration Plan

1. **Phase 1**: Refactor existing code to separate core/shell ✅ (Do this now)
2. **Phase 2**: Complete TUI implementation
3. **Phase 3**: Add egui shell (same core!)
4. **Phase 4**: Add Web shell (WASM)
5. **Phase 5**: Add mobile shells (iOS/Android)

## Naming

- **Terminator** = TUI shell
- **Twilight** = GUI shell (egui)
- **terminator-core** = Shared business logic

All powered by the same battle-tested Penumbra integration!
