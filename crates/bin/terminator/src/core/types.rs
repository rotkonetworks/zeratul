//! Core types - pure data structures with no UI dependencies

use serde::{Serialize, Deserialize};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use chrono::{DateTime, Utc};

// Re-export types we need
pub use penumbra_asset::Value as Balance;

/// Trading side
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Order type mode
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderMode {
    Limit,
    Market,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Order book with bids and asks
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrderBook {
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

impl OrderBook {
    /// Get mid price (average of best bid and best ask)
    pub fn mid_price(&self) -> Option<f64> {
        let best_bid = self.bids.first()?.price.to_f64()?;
        let best_ask = self.asks.first()?.price.to_f64()?;
        Some((best_bid + best_ask) / 2.0)
    }
    
    /// Get spread (difference between best ask and best bid)
    pub fn spread(&self) -> Option<f64> {
        let best_bid = self.bids.first()?.price.to_f64()?;
        let best_ask = self.asks.first()?.price.to_f64()?;
        Some(best_ask - best_bid)
    }
}

/// Price level in order book
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Level {
    pub price: Decimal,
    pub size: Decimal,
}

/// Trade execution
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trade {
    pub time: DateTime<Utc>,
    pub price: Decimal,
    pub size: Decimal,
    pub side: Side,
    pub block_height: u64,
    pub execution_price: Decimal,
}

/// User's position (LP or limit order)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Position {
    pub id: String,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub status: PositionStatus,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PositionStatus {
    Pending,
    Open,
    PartiallyFilled,
    Filled,
    Closed,
    Withdrawn,
}

/// Current user interaction state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Interaction {
    /// Creating new position
    CreatingPosition {
        price: f64,
        side: Side,
        size: f64,
    },

    /// Dragging existing position to new price
    DraggingPosition {
        position_id: String,
        original_price: f64,
        new_price: f64,
    },

    /// Context menu open
    ContextMenu {
        x: u16,
        y: u16,
        options: Vec<String>,
    },

    /// No active interaction
    None,
}

/// Panel type
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PanelType {
    OrderBook,
    Chart,
    OrderEntry,
    Positions,
    RecentTrades,
}

/// View model - optimized for rendering
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewModel {
    pub order_book: OrderBook,
    pub recent_trades: Vec<Trade>,
    pub positions: Vec<Position>,
    pub balances: Vec<Balance>,
    pub interaction: Interaction,

    // Computed fields
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub spread: Option<f64>,
    pub mid_price: Option<f64>,

    // UI state
    pub cursor_pos: Option<(u16, u16)>,
    pub right_panel_view: super::app::RightPanelView,
    pub active_panel: PanelType,

    // Spot panel state
    pub spot_side: Side,
    pub spot_mode: OrderMode,
    pub spot_price: Option<f64>,
    pub slider_position: f64,
}

impl ViewModel {
    /// Create from order book and state
    pub fn new(
        order_book: OrderBook,
        recent_trades: Vec<Trade>,
        positions: Vec<Position>,
        balances: Vec<Balance>,
        interaction: Interaction,
        cursor_pos: Option<(u16, u16)>,
        right_panel_view: super::app::RightPanelView,
        active_panel: PanelType,
        spot_side: Side,
        spot_mode: OrderMode,
        spot_price: Option<f64>,
        slider_position: f64,
    ) -> Self {
        let best_bid = order_book.bids.first().map(|l| l.price.to_f64().unwrap_or(0.0));
        let best_ask = order_book.asks.first().map(|l| l.price.to_f64().unwrap_or(0.0));
        let spread = order_book.spread();
        let mid_price = order_book.mid_price();

        Self {
            order_book,
            recent_trades,
            positions,
            balances,
            interaction,
            best_bid,
            best_ask,
            spread,
            mid_price,
            cursor_pos,
            right_panel_view,
            active_panel,
            spot_side,
            spot_mode,
            spot_price,
            slider_position,
        }
    }
}
