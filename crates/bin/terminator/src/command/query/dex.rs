//! DEX query commands - business logic for fetching order book and market data
//!
//! Following pcli's architecture: pure functions that return data structures

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use tonic::transport::Channel;

use penumbra_dex::TradingPair;
use penumbra_proto::core::component::dex::v1::{
    query_service_client::QueryServiceClient as DexQueryServiceClient,
    LiquidityPositionsRequest,
};

/// Order book data structure (serializable, UI-agnostic)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OrderBookData {
    pub pair: String, // Serializable string instead of TradingPair
    pub bids: Vec<LevelData>,
    pub asks: Vec<LevelData>,
    pub timestamp: DateTime<Utc>,
    pub spread: Option<f64>,
    pub mid_price: Option<f64>,
}

/// Price level data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LevelData {
    pub price: f64,
    pub size: f64,
    pub total: f64, // Cumulative size
}

/// Candle data for charts
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CandleData {
    pub timestamp: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Query order book for a trading pair
///
/// Pure business logic - no UI concerns
pub async fn query_order_book(
    client: &mut DexQueryServiceClient<Channel>,
    pair: &TradingPair,
) -> Result<OrderBookData> {
    // Query liquidity positions
    let request = LiquidityPositionsRequest {
        // TODO: Filter by trading pair when proto supports it
        ..Default::default()
    };

    let response = client.liquidity_positions(request).await?;
    let _positions = response.into_inner();

    // TODO: Convert concentrated liquidity positions to order book levels
    // For now, return empty order book
    let timestamp = Utc::now();

    Ok(OrderBookData {
        pair: format!("{:?}", pair), // Temporary string representation
        bids: vec![],
        asks: vec![],
        timestamp,
        spread: None,
        mid_price: None,
    })
}

/// Query historical candles for a trading pair
///
/// Pure business logic - no UI concerns
pub async fn query_candles(
    _client: &mut DexQueryServiceClient<Channel>,
    _pair: &TradingPair,
    _duration: std::time::Duration,
) -> Result<Vec<CandleData>> {
    // TODO: Query actual historical data from indexer
    // For now, return mock data for UI testing
    generate_mock_candles()
}

/// Generate mock candles for testing
fn generate_mock_candles() -> Result<Vec<CandleData>> {
    let now = Utc::now();
    let mut candles = Vec::new();
    let mut price = 3000.0;

    for i in 0..50 {
        let timestamp = now - chrono::Duration::hours(50 - i);

        // Simulate price movement
        let change = (i as f64 * 0.1).sin() * 50.0;
        let open = price;
        let close = price + change;
        let high = open.max(close) + 10.0;
        let low = open.min(close) - 10.0;
        let volume = 100.0 + (i as f64 * 0.5).cos() * 50.0;

        candles.push(CandleData {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_candles() {
        let candles = generate_mock_candles().unwrap();
        assert_eq!(candles.len(), 50);

        // Verify candles are ordered by time
        for i in 1..candles.len() {
            assert!(candles[i].timestamp > candles[i - 1].timestamp);
        }
    }

    #[test]
    fn test_order_book_data_serialization() {
        let data = OrderBookData {
            pair: "USDC/ETH".to_string(),
            bids: vec![
                LevelData {
                    price: 3000.0,
                    size: 1.5,
                    total: 1.5,
                },
            ],
            asks: vec![
                LevelData {
                    price: 3010.0,
                    size: 2.0,
                    total: 2.0,
                },
            ],
            timestamp: Utc::now(),
            spread: Some(10.0),
            mid_price: Some(3005.0),
        };

        // Should serialize to JSON
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("USDC/ETH"));

        // Should deserialize back
        let parsed: OrderBookData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pair, "USDC/ETH");
    }
}
