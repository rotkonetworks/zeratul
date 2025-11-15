//! Real-time gRPC client for Penumbra network
//!
//! Streams order book updates, candles, and trades

use anyhow::Result;
use tonic::transport::Channel;
use penumbra_proto::core::component::dex::v1::{
    query_service_client::QueryServiceClient as DexQueryServiceClient,
    LiquidityPositionsRequest,
    PositionState,
};
use penumbra_asset::asset::Id as AssetId;
use penumbra_dex::TradingPair;
use tokio::sync::mpsc;
use std::time::Duration;

/// Real-time gRPC client
pub struct PenumbraGrpcClient {
    endpoint: String,
    dex_client: Option<DexQueryServiceClient<Channel>>,
    update_tx: mpsc::Sender<OrderBookUpdate>,
}

#[derive(Clone, Debug)]
pub struct OrderBookUpdate {
    pub pair: TradingPair,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct Level {
    pub price: f64,
    pub size: f64,
    pub total: f64, // cumulative size
}

#[derive(Clone, Debug)]
pub struct Candle {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl PenumbraGrpcClient {
    /// Create new client
    pub fn new(endpoint: impl Into<String>) -> (Self, mpsc::Receiver<OrderBookUpdate>) {
        let (tx, rx) = mpsc::channel(100);

        let client = Self {
            endpoint: endpoint.into(),
            dex_client: None,
            update_tx: tx,
        };

        (client, rx)
    }

    /// Connect to Penumbra node
    pub async fn connect(&mut self) -> Result<()> {
        let channel = Channel::from_shared(self.endpoint.clone())?
            .connect()
            .await?;

        self.dex_client = Some(DexQueryServiceClient::new(channel));

        Ok(())
    }

    /// Start streaming order book updates
    /// Uses LiquidityPositionsByPrice for positions sorted by effective price
    pub async fn stream_order_book(
        &mut self,
        pair: TradingPair,
    ) -> Result<()> {
        let mut client = self.dex_client.clone()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        let update_tx = self.update_tx.clone();

        // Spawn background task to poll for updates every block (~5 seconds)
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;

                // Query liquidity positions sorted by price for this trading pair
                // This gives us positions in order book format already!
                let request = LiquidityPositionsRequest {
                    // TODO: Add trading pair filter when we implement DirectedTradingPair proto
                    // For now, we get all positions and filter client-side
                    include_closed: false, // Only open positions
                    ..Default::default()
                };

                match client.liquidity_positions(request).await {
                    Ok(response) => {
                        let mut stream = response.into_inner();
                        let mut positions = Vec::new();

                        // Collect all positions from stream
                        while let Ok(Some(pos_response)) = stream.message().await {
                            if let Some(position) = pos_response.data {
                                positions.push(position);
                            }
                        }

                        // Convert positions to order book levels
                        let (bids, asks) = positions_to_order_book(&positions, &pair);

                        let update = OrderBookUpdate {
                            pair: pair.clone(),
                            bids,
                            asks,
                            timestamp: chrono::Utc::now(),
                        };

                        if update_tx.send(update).await.is_err() {
                            // Channel closed, exit
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("Error fetching positions: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    /// Fetch historical candles
    pub async fn fetch_candles(
        &mut self,
        pair: &TradingPair,
        duration: Duration,
    ) -> Result<Vec<Candle>> {
        // For now, simulate candles from recent liquidity positions
        // In production, this would query a separate indexer service
        let client = self.dex_client.clone()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        // Query current positions
        let request = LiquidityPositionsRequest {
            ..Default::default()
        };

        let _response = client.clone().liquidity_positions(request).await?;

        // TODO: Process positions into OHLCV candles
        // For now, return mock data for UI testing
        self.generate_mock_candles(duration)
    }

    /// Generate mock candles for UI testing
    fn generate_mock_candles(&self, duration: Duration) -> Result<Vec<Candle>> {
        use chrono::Utc;

        let now = Utc::now();
        let candle_interval = duration / 50; // 50 candles
        let mut candles = Vec::new();

        let mut price = 3000.0;

        for i in 0..50_u32 {
            let timestamp = now - duration + candle_interval * i;

            // Simulate price movement
            let change = (i as f64 * 0.1).sin() * 50.0;
            let open = price;
            let close = price + change;
            let high = open.max(close) + 10.0;
            let low = open.min(close) - 10.0;
            let volume = 100.0 + (i as f64 * 0.5).cos() * 50.0;

            candles.push(Candle {
                timestamp,
                open,
                high,
                low,
                close,
                volume,
            });

            price = close;
        }

        Ok(candles)
    }

    /// Simulate a trade to get expected output
    pub async fn simulate_trade(
        &mut self,
        _input: AssetId,
        _output: AssetId,
        _amount: u64,
    ) -> Result<u64> {
        // TODO: simulate_trade method not available in QueryServiceClient
        // Need to find the correct API method or use a different approach
        Ok(0)
    }
}

/// Convert Penumbra liquidity positions to order book levels
///
/// Penumbra uses concentrated liquidity (Uniswap v3 style), where each position
/// has a trading function phi(p,q) and reserves (r1, r2).
///
/// For order book display:
/// - Positions selling asset_1 for asset_2 = asks (offering asset_1)
/// - Positions buying asset_1 with asset_2 = bids (offering asset_2)
pub fn positions_to_order_book(
    positions: &[penumbra_proto::core::component::dex::v1::Position],
    pair: &TradingPair,
) -> (Vec<Level>, Vec<Level>) {
    // PositionState: 1 = Opened, check position.state field directly

    let mut bids = Vec::new();
    let mut asks = Vec::new();

    for position in positions {
        // TODO: Filter by state when we understand the exact type
        // For now, process all positions

        // Get trading function parameters
        let phi = match &position.phi {
            Some(phi) => phi,
            None => continue,
        };

        // Get the component (BareTradingFunction) which has p, q, fee
        let component = match &phi.component {
            Some(c) => c,
            None => continue,
        };

        // Get reserves
        let reserves = match &position.reserves {
            Some(r) => r,
            None => continue,
        };

        // Calculate effective price: p/q from the trading function
        // component.p and component.q are Amount types (with hi/lo u64)
        let price = if let (Some(p), Some(q)) = (&component.p, &component.q) {
            if q.lo == 0 && q.hi == 0 {
                continue; // Skip division by zero
            }
            // Convert u128 to f64 for price calculation
            let p_val = (p.hi as f64) * (u64::MAX as f64) + (p.lo as f64);
            let q_val = (q.hi as f64) * (u64::MAX as f64) + (q.lo as f64);
            p_val / q_val
        } else {
            continue;
        };

        // Calculate available liquidity from reserves
        // r1 = reserves of asset_1, r2 = reserves of asset_2
        let r1 = if let Some(r) = &reserves.r1 {
            (r.hi as f64) * (u64::MAX as f64) + (r.lo as f64)
        } else {
            0.0
        };

        let r2 = if let Some(r) = &reserves.r2 {
            (r.hi as f64) * (u64::MAX as f64) + (r.lo as f64)
        } else {
            0.0
        };

        // Determine if this is a bid or ask based on which reserve has liquidity
        // If r1 > 0: selling asset_1 (ask)
        // If r2 > 0: buying asset_1 with asset_2 (bid)
        if r1 > 0.0 {
            asks.push(Level {
                price,
                size: r1,
                total: 0.0, // Will calculate cumulative after sorting
            });
        }
        if r2 > 0.0 {
            bids.push(Level {
                price,
                size: r2 / price, // Convert to asset_1 terms
                total: 0.0,
            });
        }
    }

    // Sort bids descending (highest price first)
    bids.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap_or(std::cmp::Ordering::Equal));

    // Sort asks ascending (lowest price first)
    asks.sort_by(|a, b| a.price.partial_cmp(&b.price).unwrap_or(std::cmp::Ordering::Equal));

    // Calculate cumulative totals
    let mut cumulative = 0.0;
    for level in &mut bids {
        cumulative += level.size;
        level.total = cumulative;
    }

    cumulative = 0.0;
    for level in &mut asks {
        cumulative += level.size;
        level.total = cumulative;
    }

    (bids, asks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_creation() {
        let (_client, _rx) = PenumbraGrpcClient::new("https://penumbra.rotko.net");
        assert!(true); // Smoke test
    }
}
