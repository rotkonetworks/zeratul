//! Events - all possible user interactions and external updates

use serde::{Serialize, Deserialize};
use rust_decimal::Decimal;

use super::types::{Side, PanelType, OrderBook, Trade};

/// All events that can occur in the application
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Event {
    // ===== Market Data Events =====
    /// Order book updated from network
    OrderBookUpdated(OrderBook),
    
    /// New trade executed on chain
    TradeExecuted(Trade),
    
    /// Candle data updated
    CandleUpdated { 
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
    },
    
    // ===== User Interaction Events =====
    /// User clicked on chart at specific price level
    ChartClicked { 
        price: f64, 
        x: u16, 
        y: u16 
    },
    
    /// User is dragging an existing position to new price
    PositionDragged { 
        position_id: String, 
        new_price: f64 
    },
    
    /// User moved the size slider
    SliderMoved { 
        /// 0.0 to 1.0
        position: f64 
    },
    
    /// User confirmed position creation
    CreatePosition { 
        side: Side, 
        price: Decimal, 
        size: Decimal 
    },
    
    /// User wants to cancel a position
    CancelPosition { 
        position_id: String 
    },
    
    /// User wants to close a position
    ClosePosition {
        position_id: String,
    },
    
    // ===== Navigation Events =====
    /// User focused a different panel
    PanelFocused(PanelType),

    /// User toggled resize mode
    ResizeModeToggled,

    /// User pressed Tab to cycle panels
    NextPanel,

    /// Toggle right panel view (orderbook/positions)
    ToggleRightPanelView,

    /// Vim-style navigation (Ctrl+hjkl)
    FocusLeft,
    FocusDown,
    FocusUp,
    FocusRight,
    
    // ===== Keyboard Events =====
    /// User pressed 'L' to create limit order at cursor
    LimitOrderAtCursor,
    
    /// User pressed 'M' for market order
    MarketOrder,

    /// User toggled Buy/Sell side
    ToggleSide,

    /// User toggled Limit/Market mode
    ToggleOrderMode,

    /// User clicked Submit button
    SubmitOrder,

    /// User clicked Withdraw button
    WithdrawFunds,

    /// User clicked Deposit button
    DepositFunds,

    /// User pressed 'Esc' to cancel current action
    CancelAction,
    
    /// User pressed 'Q' to quit
    Quit,
    
    // ===== Mouse Events =====
    /// Mouse moved (for cursor tracking)
    MouseMove { x: u16, y: u16 },

    /// Mouse down (start of drag)
    MouseDown { x: u16, y: u16 },

    /// Mouse dragging
    MouseDrag { x: u16, y: u16 },

    /// Mouse up (end of drag)
    MouseUp { x: u16, y: u16 },

    /// Right-click to open context menu
    RightClick { x: u16, y: u16 },
    
    // ===== Wallet Events =====
    /// Balances updated from wallet
    BalancesUpdated(Vec<penumbra_asset::Value>),
    
    // ===== System Events =====
    /// Tick for periodic updates
    Tick,
    
    /// Ignored event (for mapping failures)
    Ignored,
}

impl Event {
    /// Check if this event should trigger a render
    pub fn should_render(&self) -> bool {
        !matches!(self, Event::Tick | Event::Ignored)
    }
}
