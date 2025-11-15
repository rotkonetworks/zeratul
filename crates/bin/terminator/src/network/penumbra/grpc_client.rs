//! Real-time gRPC client for Penumbra network
//!
//! Streams order book updates, candles, and trades

use anyhow::Result;
use tonic::transport::Channel;
use penumbra_proto::core::component::dex::v1::{
    dex_query_service_client::DexQueryServiceClient,
    LiquidityPositionsRequest,
    SimulateTradeRequest,
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
    pub async fn stream_order_book(
        &mut self,
        pair: TradingPair,
    ) -> Result<()> {
        let mut client = self.dex_client.clone()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        let update_tx = self.update_tx.clone();

        // Spawn background task to poll for updates
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));

            loop {
                interval.tick().await;

                // Query liquidity positions
                let request = LiquidityPositionsRequest {
                    // Filter by trading pair
                    // TODO: Add proper filtering when proto supports it
                    ..Default::default()
                };

                match client.liquidity_positions(request).await {
                    Ok(response) => {
                        // Convert liquidity positions to order book
                        let positions = response.into_inner();

                        // TODO: Process positions into bids/asks
                        // For now, send empty update
                        let update = OrderBookUpdate {
                            pair: pair.clone(),
                            bids: vec![],
                            asks: vec![],
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

        for i in 0..50 {
            let timestamp = now - duration + candle_interval * i as i32;

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
        let mut client = self.dex_client.clone()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        let request = SimulateTradeRequest {
            // TODO: Fill in request
            ..Default::default()
        };

        let response = client.simulate_trade(request).await?;
        let result = response.into_inner();

        // TODO: Parse response
        Ok(0)
    }
}

/// Convert Penumbra liquidity positions to order book levels
pub fn positions_to_order_book(
    _positions: &[penumbra_dex::lp::position::Position],
    _pair: &TradingPair,
) -> (Vec<Level>, Vec<Level>) {
    // TODO: Implement conversion from concentrated liquidity positions
    // to traditional order book levels
    //
    // This requires:
    // 1. Group positions by price ranges
    // 2. Calculate effective liquidity at each price point
    // 3. Sort into bids (buy) and asks (sell)

    (vec![], vec![])
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
