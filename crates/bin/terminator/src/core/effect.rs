//! Effects - requests for side effects from the core to the shell

use serde::{Serialize, Deserialize};
use rust_decimal::Decimal;
use penumbra_dex::TradingPair;

use super::types::{Side, ViewModel};

/// Side effects that the shell must execute
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Effect {
    // ===== Rendering =====
    /// Request UI render with new view model
    Render(ViewModel),
    
    // ===== Penumbra Operations =====
    /// Submit a new LP position to Penumbra
    SubmitPosition {
        side: Side,
        price: Decimal,
        size: Decimal,
        fee_bps: u32,
    },
    
    /// Close an existing position
    ClosePosition {
        position_id: String,
    },
    
    /// Withdraw a closed position
    WithdrawPosition {
        position_id: String,
    },
    
    // ===== Data Fetching =====
    /// Start streaming order book for trading pair
    StreamOrderBook {
        pair: TradingPair,
    },
    
    /// Fetch recent trades
    FetchTrades {
        start_height: u64,
        end_height: u64,
    },
    
    /// Fetch candle data
    FetchCandles {
        pair: TradingPair,
        duration_secs: u64,
    },
    
    /// Refresh balances from wallet
    RefreshBalances,
    
    // ===== UI Feedback =====
    /// Show a notification to user
    ShowNotification {
        message: String,
        level: NotificationLevel,
    },
    
    /// Show confirmation dialog
    ShowConfirmation {
        message: String,
        on_confirm: Box<Effect>,
    },
    
    /// Set cursor position
    SetCursor {
        x: u16,
        y: u16,
    },
    
    // ===== System =====
    /// Exit the application
    Exit,
    
    /// No operation (for testing/debugging)
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl Effect {
    /// Create a success notification
    pub fn success(message: impl Into<String>) -> Self {
        Effect::ShowNotification {
            message: message.into(),
            level: NotificationLevel::Success,
        }
    }
    
    /// Create an error notification
    pub fn error(message: impl Into<String>) -> Self {
        Effect::ShowNotification {
            message: message.into(),
            level: NotificationLevel::Error,
        }
    }
    
    /// Create an info notification
    pub fn info(message: impl Into<String>) -> Self {
        Effect::ShowNotification {
            message: message.into(),
            level: NotificationLevel::Info,
        }
    }
}
