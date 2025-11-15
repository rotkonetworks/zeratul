//! Network clients for Penumbra and Zeratul

pub mod penumbra;

use anyhow::Result;
use penumbra_asset::asset::Id as AssetId;
use penumbra_num::Amount;

/// Trading pair
#[derive(Clone, Debug)]
pub struct TradingPair {
    pub asset_1: AssetId,
    pub asset_2: AssetId,
}

/// Order book level
#[derive(Clone, Debug)]
pub struct Level {
    pub price: f64,
    pub size: Amount,
}

/// Order book snapshot
#[derive(Clone, Debug)]
pub struct OrderBook {
    pub pair: TradingPair,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

/// Recent trade
#[derive(Clone, Debug)]
pub struct Trade {
    pub pair: TradingPair,
    pub price: f64,
    pub size: Amount,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Network client trait - can connect to Penumbra or Zeratul
#[async_trait::async_trait]
pub trait NetworkClient: Send + Sync {
    /// Get current order book
    async fn get_order_book(&mut self, pair: &TradingPair) -> Result<OrderBook>;

    /// Get recent trades
    async fn get_recent_trades(&mut self, pair: &TradingPair, limit: usize) -> Result<Vec<Trade>>;

    /// Submit order (returns order ID)
    async fn submit_order(&mut self, order: Order) -> Result<String>;

    /// Get account address
    fn address(&self) -> String;
}

#[derive(Clone, Debug)]
pub struct Order {
    pub pair: TradingPair,
    pub side: OrderSide,
    pub amount: Amount,
    // For Penumbra: this is implicit from the liquidity position
    // For limit orders: would be explicit price
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}
