# Market Trades Panel - Intelligent Ordering

## Problem: Penumbra Batch Swaps

Penumbra executes swaps in batches every block (~5 seconds). Within a single block, multiple swaps execute at the same clearing price. This makes traditional chronological ordering confusing.

## Solution: Smart Ordering

### Ordering Algorithm
```
1. Sort by block height (most recent first)
2. Within same block: order by execution price
   - Makes price movement visually coherent
   - Trades "flow" naturally up/down
```

### Example
```
Block 12345:
  15:30:42  $3,001.50  0.5 BTC  🟢
  15:30:42  $3,002.00  0.3 BTC  🟢  <- Ascending within block
  15:30:42  $3,003.25  0.2 BTC  🟢

Block 12344:
  15:30:37  $2,998.00  0.4 BTC  🔴
  15:30:37  $2,997.50  0.3 BTC  🔴  <- Descending within block
  15:30:37  $2,996.00  0.6 BTC  🔴
```

## Visual Features

### Block Separation
- Extra margin between blocks
- Makes batch boundaries obvious
- Easy to scan recent activity

### Price Display
```rust
// If limit price differs significantly from execution:
"$3,000.00 ($2,998.50)"  // limit (execution)

// Otherwise just show execution:
"$2,998.50"
```

### Color Coding
- 🟢 Green = Buy (taker bought asset_1)
- 🔴 Red = Sell (taker sold asset_1)

### Columns
- **Time**: HH:MM:SS format
- **Price**: Execution price (+ limit if different)
- **Size**: Amount traded
- **Block**: Block height (subtle gray)

## Implementation

### Trade Structure
```rust
pub struct Trade {
    pub time: DateTime<Utc>,
    pub price: Decimal,           // Original limit price
    pub size: Decimal,
    pub side: Side,
    pub block_height: u64,        // Penumbra block
    pub execution_price: Decimal, // Actual batch clearing price
}
```

### Sorting Function
```rust
pub fn sort_trades_for_display(&mut self) {
    let mut trades: Vec<Trade> = self.recent_trades.drain(..).collect();
    
    trades.sort_by(|a, b| {
        match b.block_height.cmp(&a.block_height) {
            std::cmp::Ordering::Equal => {
                // Within same block: order by price
                a.execution_price.cmp(&b.execution_price)
            }
            other => other,
        }
    });
    
    self.recent_trades = trades.into();
}
```

## Data Source

### Penumbra SwapExecutions API
```protobuf
rpc SwapExecutions(SwapExecutionsRequest) 
  returns (stream SwapExecutionsResponse);
```

**Features:**
- Stream historical executions
- Filter by height range
- Filter by trading pair
- Includes routing information

### Request Example
```rust
let request = SwapExecutionsRequest {
    start_height: current_height - 1000, // Last 1000 blocks (~83 minutes)
    end_height: current_height,
    trading_pair: Some(pair.into()),
};

let mut stream = client.swap_executions(request).await?;

while let Some(execution) = stream.message().await? {
    let trade = Trade {
        time: execution.timestamp,
        price: calculate_price(&execution),
        size: execution.input.amount,
        side: detect_side(&execution),
        block_height: execution.height,
        execution_price: execution.clearing_price,
    };
    
    app.recent_trades.push_front(trade);
    app.sort_trades_for_display();
}
```

## Panel Layout

### Default Position
- Bottom-right quadrant
- Next to Positions panel
- Above status bar

### Keyboard Shortcuts
- `t` → Focus trades panel
- `↑/↓` → Scroll trades
- `f` → Toggle filter (pair selection)
- `r` → Refresh from network

## Future Enhancements

1. **Trade aggregation** - Combine similar trades in same block
2. **Volume bars** - Visual size representation
3. **Your trades highlight** - Show your executions in different color
4. **Trade details popup** - Click trade for full routing info
5. **Export trades** - CSV export for analysis

## Performance

- Display limit: 20 most recent trades
- Full history stored: Last 1000 blocks (~83 minutes)
- Memory: ~50KB for 1000 trades
- Update frequency: Every 5 seconds (per block)

## Example Output
```
┌─ Recent Trades ───────────────────────────────────────┐
│ Time      Price           Size        Block          │
├───────────────────────────────────────────────────────┤
│ 15:30:42  $3,001.50      0.5000      12345           │
│ 15:30:42  $3,002.00      0.3000      12345           │
│ 15:30:42  $3,003.25      0.2000      12345           │
│                                                       │
│ 15:30:37  $2,998.00      0.4000      12344           │
│ 15:30:37  $2,997.50      0.3000      12344           │
│ 15:30:37  $2,996.00      0.6000      12344           │
│                                                       │
│ 15:30:32  $2,995.00      1.2000      12343           │
└───────────────────────────────────────────────────────┘
```

This makes Penumbra's batch auction mechanism visually intuitive!
