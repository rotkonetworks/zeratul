# The Terminator UX Meditation 🌸

*After studying Veil, pcli, and the nature of terminal trading*

## The Three Truths of Terminal Trading

### 1. **Simplicity is Speed**
```
Web UI:  Click menu → Select pair → Click buy → Enter amount → Confirm → Success
         (6 steps, ~10 seconds)

Terminal: b 100 [Enter]
         (1 step, <1 second)
```

**The Way:** Minimize clicks, maximize keyboard.

### 2. **Context is Everything**
```
Trader needs:
1. Current price (Am I getting a good deal?)
2. Order book depth (Will my order fill?)
3. Recent trades (What's the momentum?)
4. My positions (What's my exposure?)
```

**The Way:** All critical info always visible.

### 3. **Flow State**
```
Bad UX: Think → Navigate → Click → Confirm → Wait
Good UX: See → Think → Act → Done
```

**The Way:** Remove friction between thought and action.

## Veil Analysis (What to Learn)

### ✅ What Veil Does Well

1. **Responsive layouts** - Adapts to screen size
2. **Route info** - Shows liquidity routing
3. **Trade history** - Both personal and market
4. **Pair selection** - Easy asset switching

### ❌ What Doesn't Fit Terminal

1. **Too many panels** - Web can show 6+ panels, terminal should show 2-3
2. **Mouse required** - Charts need clicking, terminal should be keyboard-first
3. **Complex forms** - Multi-step flows, terminal should be single-line
4. **Animations** - Nice on web, unnecessary in terminal

## The Terminal Advantage

### What Terminal Does Better:

1. **Speed**
   - No loading spinners
   - No render delays
   - Instant feedback

2. **Focus**
   - No distractions
   - No ads
   - No animations

3. **Power User Features**
   - Vim-style navigation
   - Command mode
   - Macros/aliases

4. **SSH-able**
   - Trade from server
   - Low bandwidth
   - Works everywhere

## The Optimal Layouts

### Mode 1: Trade Mode (Default)
```
┌────────────────────────────────────────────────────┐
│ PENUMBRA │ USDC/ETH │ $3,142.50 │ 24h: +2.3%      │
├──────────────────┬─────────────────────────────────┤
│   ORDER BOOK     │      INSTANT TRADE              │
│                  │                                 │
│ Size │   Price   │  [B]uy          [S]ell          │
│──────┼───────────│                                 │
│ 1.24 │ 3142.50 ← │  Amount: [____] ETH             │
│ 2.51 │ 3142.25   │                                 │
│ 0.84 │ 3142.00   │  Total:  [____] USDC            │
│──────┼───────────│                                 │
│      │ Spread: $0.25                               │
│──────┼───────────│  Price: 3141.75 (market)        │
│ 1.92 │ 3141.75 → │                                 │
│ 3.41 │ 3141.50   │  [Enter] Submit                 │
│ 2.18 │ 3141.25   │  [Esc]   Cancel                 │
├──────────────────┴─────────────────────────────────┤
│ Pending: 2 orders │ Filled: +0.5 ETH @ 3140        │
└────────────────────────────────────────────────────┘
```

**Hotkeys:**
- `b` - Buy instantly at market
- `s` - Sell instantly at market
- `l` - Limit order (opens price input)
- `c` - Cancel all orders
- `Tab` - Focus order entry

### Mode 2: Chart Mode (F2)
```
┌────────────────────────────────────────────────────┐
│ USDC/ETH │ 1D │ $3,142.50 │ High: 3,180 │ Low: 3,100│
├────────────────────────────────────────────────────┤
│                                                    │
│  3,180 │                         ╱╲                │
│        │                    ╱╲  ╱  ╲               │
│  3,160 │               ╱╲  ╱  ╲╱    ╲              │
│        │          ╱╲  ╱  ╲╱            ╲            │
│  3,140 │     ╱╲  ╱  ╲╱                  ╲  ╱       │
│        │╲   ╱  ╲╱                         ╲╱        │
│  3,120 │ ╲╱                                         │
│        │                                            │
│  3,100 └────────────────────────────────────────   │
│        00:00    04:00    08:00    12:00    16:00   │
│                                                    │
│  [1h] [4h] [1D] [1W] [1M]     Vol: 1,234 ETH      │
└────────────────────────────────────────────────────┘
```

**Hotkeys:**
- `1-5` - Change timeframe
- `F1` - Back to trade mode
- `z` - Zoom in
- `x` - Zoom out

### Mode 3: History Mode (F3)
```
┌────────────────────────────────────────────────────┐
│ MY TRADING HISTORY                                 │
├────────────────────────────────────────────────────┤
│                                                    │
│  Time     │ Pair     │ Side │ Amount │ Price      │
│  ─────────┼──────────┼──────┼────────┼──────────  │
│  12:45:23 │ USDC/ETH │ BUY  │ 0.50   │ 3,142.50   │
│  12:12:01 │ USDC/ETH │ SELL │ 0.25   │ 3,140.00   │
│  11:58:32 │ USDC/ETH │ BUY  │ 1.00   │ 3,138.75   │
│  11:23:45 │ USDC/ETH │ BUY  │ 0.75   │ 3,135.00   │
│                                                    │
│  Total P&L: +$234.56 (+0.74%)                     │
│  Total Volume: 2.50 ETH                            │
│                                                    │
│  [↑/↓] Navigate │ [Enter] Details │ [F1] Trade    │
└────────────────────────────────────────────────────┘
```

## The Command Line

**Pro traders use commands:**

```
:buy 100 usdc           # Market buy
:sell 50 eth @ 3150     # Limit sell
:cancel all             # Cancel all orders
:balance                # Show balances
:pairs                  # List trading pairs
:help                   # Show commands
```

## The Data Flow

```
Terminator
    ↓
PenumbraClient::connect("https://penumbra.rotko.net")
    ↓
gRPC queries:
    ├─ DexQueryService::liquidity_positions()  → Order book
    ├─ DexQueryService::candles()              → Price chart
    └─ ViewService::transaction_info_by_hash() → Trade history
    ↓
Real-time updates:
    └─ EventService::subscribe() → New fills
```

## The Implementation Order

### Week 1: Core Trading
1. ✅ Basic TUI structure (done)
2. Connect to penumbra.rotko.net
3. Display real order book
4. Show current price

### Week 2: Order Entry
1. Implement buy/sell forms
2. Connect to Penumbra transaction building
3. Submit swaps via gRPC
4. Show confirmation

### Week 3: History & Charts
1. Fetch user's trade history
2. Display fills
3. Basic ASCII chart
4. P&L tracking

### Week 4: Polish
1. Real-time updates
2. Sound notifications (optional)
3. Hotkey system
4. Command mode

## Key Insights from Penumbra Code

### From `pcli/src/dex_utils.rs`:

```rust
// Penumbra has helpers for:
- route_and_fill simulation
- liquidity position math
- price calculations
```

We should use these directly!

### From Veil:

```typescript
// Good patterns:
- useSummary() hook for 24h stats
- useCandles() for chart data
- useLiquidityPositions() for order book
```

We need Rust equivalents.

## The Zen of Terminal Trading

```
Fast beats pretty.
Keyboard beats mouse.
Focus beats features.
Real-time beats fancy.
Simple beats complex.
```

---

*The best interface is invisible.* 🎯
