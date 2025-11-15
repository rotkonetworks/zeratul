# Interactive Trading UX Design

## Core Interaction: Click-to-Trade

### Visual Layout
```
┌─────────────────────────────────────────────────────────┐
│ Chart Panel (Logarithmic Price Scale)                   │
│                                                          │
│  $3200 ── [Sell Order] ─────────────────── 🔴          │
│  $3100 ── [Sell Order] ─────────────────── 🔴          │
│  $3000 ── Current Price ─────────────────── ●          │
│  $2900 ── [Buy Order]  ─────────────────── 🟢          │
│  $2800 ── [Buy Order]  ─────────────────── 🟢          │
│                                                          │
│  [Click anywhere to create position]                    │
└─────────────────────────────────────────────────────────┘
```

## Interaction Flow

### 1. Click Price Level
```rust
User clicks at (x, y) on chart
  ↓
Calculate price from y-coordinate (logarithmic scale)
  ↓
Compare with current market price:
  - Above current → Auto-select "Sell" (Red)
  - Below current → Auto-select "Buy" (Green)
  ↓
Show context menu at click position
```

### 2. Context Menu
```
┌─────────────────────┐
│ Price: $2,950       │
├─────────────────────┤
│ 🟢 BUY Limit Order  │  ← Auto-selected (below market)
│ 🔴 SELL Limit Order │
├─────────────────────┤
│ Size: [====|    ]   │  ← Logarithmic slider
│       0.1 → 100 BTC │
├─────────────────────┤
│ [Confirm] [Cancel]  │
└─────────────────────┘
```

### 3. Logarithmic Slider
```rust
// Maps slider position (0.0 → 1.0) to size
fn slider_to_size(position: f64, min: f64, max: f64) -> f64 {
    // Logarithmic scale for better control across ranges
    let log_min = min.ln();
    let log_max = max.ln();
    (log_min + position * (log_max - log_min)).exp()
}

// Examples:
// position=0.0 → 0.1 BTC
// position=0.5 → 3.16 BTC
// position=1.0 → 100 BTC
```

### 4. Drag Existing Positions
```
User clicks and drags existing position marker
  ↓
Show ghost/preview at new price level
  ↓
Release → Confirm adjustment dialog
  ↓
Submit updated position to Penumbra
```

## State Management

### New State Fields
```rust
pub struct AppState {
    // ... existing fields ...
    
    /// Active interaction state
    pub interaction: Option<InteractionState>,
    
    /// Current market price (for auto-detecting buy/sell)
    pub current_price: Option<Decimal>,
}

pub enum InteractionState {
    /// Creating new position
    CreatingPosition {
        price: Decimal,
        side: Side, // Auto-detected
        size: Decimal, // From slider
        click_pos: (u16, u16),
    },
    
    /// Dragging existing position
    DraggingPosition {
        order_id: String,
        original_price: Decimal,
        new_price: Decimal,
        preview_pos: (u16, u16),
    },
}
```

## Mouse Event Handling

### Chart Panel Mouse Events
```rust
match mouse_event {
    MouseEventKind::Down(MouseButton::Left) => {
        if is_on_existing_position(x, y) {
            // Start dragging position
            state.interaction = Some(InteractionState::DraggingPosition { ... });
        } else {
            // Create new position
            let price = y_to_price_log(y, chart_rect);
            let side = if price > current_price { Side::Sell } else { Side::Buy };
            state.interaction = Some(InteractionState::CreatingPosition {
                price,
                side,
                size: Decimal::new(1, 1), // 0.1 default
                click_pos: (x, y),
            });
        }
    }
    
    MouseEventKind::Drag(_) => {
        if let Some(InteractionState::DraggingPosition { .. }) = state.interaction {
            // Update preview position
            let new_price = y_to_price_log(y, chart_rect);
            // Update state...
        }
    }
    
    MouseEventKind::Up(_) => {
        // Finalize interaction
        match state.interaction.take() {
            Some(InteractionState::CreatingPosition { price, side, size, .. }) => {
                // Show confirmation / submit to Penumbra
            }
            Some(InteractionState::DraggingPosition { order_id, new_price, .. }) => {
                // Confirm position adjustment
            }
            _ => {}
        }
    }
}
```

## Logarithmic Price Scale

### Y-Coordinate ↔ Price Conversion
```rust
/// Convert y-coordinate to price (logarithmic scale)
fn y_to_price_log(y: u16, rect: Rect, price_range: (f64, f64)) -> Decimal {
    let (min_price, max_price) = price_range;
    
    // Normalize y to 0.0 → 1.0 (inverted: top = high price)
    let normalized = 1.0 - (y - rect.y) as f64 / rect.height as f64;
    
    // Apply logarithmic scale
    let log_min = min_price.ln();
    let log_max = max_price.ln();
    let log_price = log_min + normalized * (log_max - log_min);
    
    Decimal::from_f64_retain(log_price.exp()).unwrap()
}

/// Convert price to y-coordinate (inverse)
fn price_to_y_log(price: Decimal, rect: Rect, price_range: (f64, f64)) -> u16 {
    let (min_price, max_price) = price_range;
    let price_f64 = price.to_f64().unwrap();
    
    let log_min = min_price.ln();
    let log_max = max_price.ln();
    let log_price = price_f64.ln();
    
    let normalized = (log_price - log_min) / (log_max - log_min);
    let y = rect.y + ((1.0 - normalized) * rect.height as f64) as u16;
    
    y.clamp(rect.y, rect.y + rect.height - 1)
}
```

## Visual Feedback

### Position Markers
```rust
// Render positions on chart
for order in &state.orders {
    let y = price_to_y_log(order.price, chart_rect, price_range);
    let color = match order.side {
        Side::Buy => Color::Green,
        Side::Sell => Color::Red,
    };
    
    // Draw horizontal line at price level
    render_horizontal_line(f, y, chart_rect, color);
    
    // Draw marker circle
    render_marker(f, chart_rect.x + 2, y, color);
    
    // Show size as bar width
    let bar_width = (order.size.to_f64().unwrap() * 10.0) as u16;
    render_size_bar(f, y, bar_width, color);
}

// Render ghost/preview during drag
if let Some(InteractionState::DraggingPosition { new_price, .. }) = &state.interaction {
    let y = price_to_y_log(*new_price, chart_rect, price_range);
    render_horizontal_line(f, y, chart_rect, Color::DarkGray);
    // Dashed line style for preview
}
```

## Right-Side Panel: Route Depth

### Visual Design
```
┌─────────────────────────┐
│ Route Depth             │
├─────────────────────────┤
│ Direct:  50.2 BTC       │
│ ████████████▌           │
│                         │
│ 1-hop:   32.1 BTC       │
│ ████████▍               │
│                         │
│ 2-hop:   12.5 BTC       │
│ ███▏                    │
│                         │
│ Total: 94.8 BTC         │
├─────────────────────────┤
│ Best Route:             │
│ BTC → USDC → ETH        │
│ Fee: 0.3%               │
└─────────────────────────┘
```

## Implementation Priority

1. ✅ **Order book streaming** (Done)
2. **Chart rendering with log scale**
3. **Mouse click → price detection**
4. **Context menu for position creation**
5. **Logarithmic size slider**
6. **Drag existing positions**
7. **Submit positions to Penumbra**
8. **Route depth visualization**

## Penumbra Integration

### Creating LP Position
```rust
// When user confirms position creation
async fn create_lp_position(
    wallet: &Wallet,
    side: Side,
    price: Decimal,
    size: Decimal,
) -> Result<PositionId> {
    // Convert to Penumbra trading function
    let (asset_1, asset_2) = get_trading_pair();
    
    let (reserves_1, reserves_2) = match side {
        Side::Buy => (0, size), // Offering asset_2 to buy asset_1
        Side::Sell => (size, 0), // Offering asset_1 to sell for asset_2
    };
    
    // Build position with trading function φ(p, q)
    let position = Position {
        phi: TradingFunction {
            pair: TradingPair { asset_1, asset_2 },
            fee_bps: 30, // 0.30% fee
            p: (price * 10000).to_u128(), // Price numerator
            q: 10000, // Price denominator
        },
        reserves: Reserves {
            r1: reserves_1,
            r2: reserves_2,
        },
        close_on_fill: true, // Limit order behavior
        ..Default::default()
    };
    
    // Submit via custody service
    wallet.submit_position(position).await
}
```

## UX Polish

### Keyboard Shortcuts
- `L` → Create limit order at current mouse position
- `M` → Market order (instant fill)
- `Esc` → Cancel current interaction
- `Delete` → Cancel selected position
- `Arrow Up/Down` → Adjust size slider
- `Enter` → Confirm action

### Visual Polish
- Smooth animations for dragging
- Haptic-like feedback on clicks (visual pulse)
- Price level snapping (snap to $10/$100 increments)
- Show P&L for each position in real-time
- Highlight position on hover with details tooltip

