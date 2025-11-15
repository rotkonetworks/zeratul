# Real-Time Data Implementation for Terminator

## Overview
Terminator gets real-time market data directly from Penumbra nodes (like https://penumbra.rotko.net) with **5-second block updates**.

## Implemented Features

### 1. Order Book Streaming (`grpc_client.rs:73-135`)
**API:** `LiquidityPositions` stream
**Update Frequency:** Every 5 seconds (per block)
**Data Flow:**
```
Penumbra Node → gRPC Stream → positions_to_order_book() → UI
```

**Implementation:**
- Queries all open liquidity positions
- Converts concentrated liquidity to traditional order book format
- Separates into bids (buying asset_1) and asks (selling asset_1)
- Sorts by price and calculates cumulative depth
- Pushes updates via channel to UI

### 2. Position to Order Book Conversion (`grpc_client.rs:209-311`)
**Algorithm:**
```rust
For each position:
1. Extract trading function φ(p, q) → price = p/q
2. Extract reserves (r1, r2) → available liquidity
3. Classify:
   - r1 > 0 → Ask (selling asset_1 at price)
   - r2 > 0 → Bid (buying asset_1 at price)
4. Sort bids descending, asks ascending
5. Calculate cumulative depth for each level
```

**Example:**
```
Position: φ(p=3000, q=1), reserves=(10 USDC, 0)
→ Ask: price=3000, size=10 USDC

Position: φ(p=2900, q=1), reserves=(0, 5.8 USDC)
→ Bid: price=2900, size=0.002 BTC (5.8/2900)
```

## Available Penumbra APIs (Not Yet Used)

### Real-Time Candle Stream
```protobuf
rpc CandlestickDataStream(CandlestickDataStreamRequest) 
  returns (stream CandlestickDataStreamResponse);
```
- **Update:** Push on every block
- **Data:** OHLCV with direct volume + swap volume
- **TODO:** Implement for chart panel

### Current Spread
```protobuf
rpc Spread(SpreadRequest) returns (SpreadResponse);
```
- **Data:** Best bid/ask with approximate prices
- **Use:** Quick market snapshot
- **TODO:** Display in header

### Swap Executions (Trade History)
```protobuf
rpc SwapExecutions(SwapExecutionsRequest) 
  returns (stream SwapExecutionsResponse);
```
- **Data:** Historical swap executions with routing
- **Filter:** By height range and trading pair
- **TODO:** Implement for recent trades panel

### Block-by-Block Updates
```protobuf
rpc CompactBlockRange(CompactBlockRangeRequest) 
  returns (stream CompactBlockRangeResponse);
```
- **Feature:** Set `keep_alive=true` for push notifications
- **Data:** Swap outputs, nullifiers, state changes
- **TODO:** Use for real-time trade notifications

## Block Timing
- **Block Time:** 5 seconds
- **Epoch Duration:** Configurable (default ~27 hours = 19,200 blocks)
- **Historical Data:** Up to 20,000 blocks (~27.7 hours) of candles

## Next Steps

1. **Implement Candle Streaming** - Real-time OHLCV for charts
2. **Add Spread Query** - Show current best bid/ask in header
3. **Swap Execution History** - Recent trades panel
4. **Compact Block Stream** - Push notifications for fills

## Performance Notes
- No external indexer required - all data from node
- Streaming RPCs use tonic gRPC
- Position conversion is O(n log n) due to sorting
- 5-second updates minimize bandwidth while staying real-time
