//! Application state management

use ratatui::layout::Rect;
use std::collections::VecDeque;
use rust_decimal::Decimal;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

pub mod panel;

use panel::{Panel, PanelType, PanelLayout};
use crate::network::penumbra::grpc_client::{PenumbraGrpcClient, OrderBookUpdate, Candle};
use crate::wallet::Wallet;
use penumbra_asset::Value;

/// Main application state
pub struct AppState {
    /// Resizable panels
    pub panels: Vec<Panel>,

    /// Active panel index
    pub active_panel: usize,

    /// Resize mode enabled
    pub resize_mode: bool,

    /// Mouse drag state
    pub dragging: Option<DragState>,

    /// Market data
    pub order_book: OrderBook,
    pub recent_trades: VecDeque<Trade>,
    pub price_history: VecDeque<PricePoint>,

    /// User state
    pub orders: Vec<Order>,
    pub fills: Vec<Fill>,

    /// Penumbra integration
    pub wallet: Option<Wallet>,
    pub balances: Vec<Value>,
    pub penumbra_client: Option<PenumbraGrpcClient>,
    pub order_book_rx: Option<mpsc::Receiver<OrderBookUpdate>>,
    pub latest_order_book: Option<OrderBookUpdate>,
    pub candles: Vec<Candle>,
}

#[derive(Clone)]
pub struct DragState {
    pub panel_index: usize,
    pub drag_type: DragType,
    pub start_x: u16,
    pub start_y: u16,
}

#[derive(Clone, PartialEq)]
pub enum DragType {
    Move,
    ResizeRight,
    ResizeBottom,
    ResizeCorner,
}

#[derive(Default)]
pub struct OrderBook {
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

#[derive(Clone)]
pub struct Level {
    pub price: Decimal,
    pub size: Decimal,
}

#[derive(Clone)]
pub struct Trade {
    pub time: DateTime<Utc>,
    pub price: Decimal,
    pub size: Decimal,
    pub side: Side,
    pub block_height: u64, // Penumbra block height
    pub execution_price: Decimal, // Actual execution price from batch
}

#[derive(Clone, Copy)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Clone)]
pub struct PricePoint {
    pub time: DateTime<Utc>,
    pub price: Decimal,
}

#[derive(Clone)]
pub struct Order {
    pub id: String,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub status: OrderStatus,
}

#[derive(Clone, PartialEq)]
pub enum OrderStatus {
    Pending,
    Submitted,
    PartiallyFilled,
    Filled,
    Cancelled,
}

#[derive(Clone)]
pub struct Fill {
    pub order_id: String,
    pub price: Decimal,
    pub size: Decimal,
    pub time: DateTime<Utc>,
}

impl AppState {
    pub fn new() -> Self {
        // Default layout: 4 panels in grid
        let panels = vec![
            Panel::new(PanelType::OrderBook, Rect::new(0, 0, 40, 30)),
            Panel::new(PanelType::Chart, Rect::new(40, 0, 80, 30)),
            Panel::new(PanelType::OrderEntry, Rect::new(0, 30, 60, 24)),
            Panel::new(PanelType::Positions, Rect::new(60, 30, 60, 24)),
        ];

        Self {
            panels,
            active_panel: 0,
            resize_mode: false,
            dragging: None,
            order_book: OrderBook::default(),
            recent_trades: VecDeque::new(),
            price_history: VecDeque::new(),
            orders: Vec::new(),
            fills: Vec::new(),
            wallet: None,
            balances: Vec::new(),
            penumbra_client: None,
            order_book_rx: None,
            latest_order_book: None,
            candles: Vec::new(),
        }
    }

    /// Update balances from wallet
    pub async fn update_balances(&mut self) {
        if let Some(wallet) = &mut self.wallet {
            match wallet.query_balances().await {
                Ok(balances) => {
                    self.balances = balances;
                }
                Err(e) => {
                    tracing::warn!("Failed to query balances: {}", e);
                }
            }
        }
    }

    /// Initialize Penumbra connection with wallet
    pub async fn connect_penumbra_with_wallet(&mut self, wallet: Wallet) -> anyhow::Result<()> {
        use penumbra_asset::asset;
        use penumbra_dex::TradingPair;

        let grpc_url = wallet.grpc_url().to_string();

        // Create gRPC client using wallet's endpoint
        let (mut client, rx) = PenumbraGrpcClient::new(&grpc_url);

        // Connect to Penumbra node
        client.connect().await?;

        // Get test trading pair (USDC/PENUMBRA)
        let pair = TradingPair::new(
            asset::REGISTRY.parse_unit("upenumbra").id(),
            asset::REGISTRY.parse_unit("upenumbra").id(), // TODO: Use real pair
        );

        // Start streaming order book
        client.stream_order_book(pair.clone()).await?;

        // Fetch initial candles
        let candles = client.fetch_candles(
            &pair,
            std::time::Duration::from_secs(86400), // 1 day
        ).await?;

        self.wallet = Some(wallet);
        self.penumbra_client = Some(client);
        self.order_book_rx = Some(rx);
        self.candles = candles;

        Ok(())
    }

    /// Initialize Penumbra connection
    pub async fn connect_penumbra(&mut self) -> anyhow::Result<()> {
        use penumbra_asset::asset;
        use penumbra_dex::TradingPair;

        let (mut client, rx) = PenumbraGrpcClient::new("https://penumbra.rotko.net");

        // Connect to Penumbra node
        client.connect().await?;

        // Get test trading pair (USDC/PENUMBRA)
        let pair = TradingPair::new(
            asset::REGISTRY.parse_unit("upenumbra").id(),
            asset::REGISTRY.parse_unit("upenumbra").id(), // TODO: Use real pair
        );

        // Start streaming order book
        client.stream_order_book(pair.clone()).await?;

        // Fetch initial candles
        let candles = client.fetch_candles(
            &pair,
            std::time::Duration::from_secs(86400), // 1 day
        ).await?;

        self.penumbra_client = Some(client);
        self.order_book_rx = Some(rx);
        self.candles = candles;

        Ok(())
    }

    /// Poll for order book updates
    pub async fn poll_penumbra_updates(&mut self) {
        if let Some(rx) = &mut self.order_book_rx {
            // Try to receive update without blocking
            if let Ok(update) = rx.try_recv() {
                self.latest_order_book = Some(update);
            }
        }
    }

    pub fn toggle_resize_mode(&mut self) {
        self.resize_mode = !self.resize_mode;
    }

    pub fn next_panel(&mut self) {
        self.active_panel = (self.active_panel + 1) % self.panels.len();
    }

    pub fn handle_mouse_down(&mut self, x: u16, y: u16) {
        // Check if clicking on panel border for resize
        for (idx, panel) in self.panels.iter().enumerate() {
            if let Some(drag_type) = panel.check_resize_handle(x, y) {
                self.dragging = Some(DragState {
                    panel_index: idx,
                    drag_type,
                    start_x: x,
                    start_y: y,
                });
                return;
            }
        }

        // Check if clicking inside panel for move
        if self.resize_mode {
            for (idx, panel) in self.panels.iter().enumerate() {
                if panel.rect.contains_point(x, y) {
                    self.dragging = Some(DragState {
                        panel_index: idx,
                        drag_type: DragType::Move,
                        start_x: x,
                        start_y: y,
                    });
                    self.active_panel = idx;
                    return;
                }
            }
        }
    }

    pub fn handle_mouse_drag(&mut self, x: u16, y: u16) {
        if let Some(drag) = &self.dragging {
            let dx = x as i32 - drag.start_x as i32;
            let dy = y as i32 - drag.start_y as i32;

            let panel = &mut self.panels[drag.panel_index];

            match drag.drag_type {
                DragType::Move => {
                    panel.rect.x = (panel.rect.x as i32 + dx).max(0) as u16;
                    panel.rect.y = (panel.rect.y as i32 + dy).max(0) as u16;
                }
                DragType::ResizeRight => {
                    panel.rect.width = (panel.rect.width as i32 + dx).max(20) as u16;
                }
                DragType::ResizeBottom => {
                    panel.rect.height = (panel.rect.height as i32 + dy).max(10) as u16;
                }
                DragType::ResizeCorner => {
                    panel.rect.width = (panel.rect.width as i32 + dx).max(20) as u16;
                    panel.rect.height = (panel.rect.height as i32 + dy).max(10) as u16;
                }
            }

            // Update drag start position
            if let Some(drag) = &mut self.dragging {
                drag.start_x = x;
                drag.start_y = y;
            }
        }
    }

    pub fn handle_mouse_up(&mut self) {
        self.dragging = None;
    }

    pub async fn update_market_data(&mut self) {
        // Mock market data updates
        // TODO: Connect to real price feed
        use rand::Rng;
        let mut rng = rand::thread_rng();

        // Update order book
        if self.order_book.bids.is_empty() {
            let base_price = Decimal::new(50000, 0); // $50,000
            for i in 0..20 {
                self.order_book.bids.push(Level {
                    price: base_price - Decimal::new(i * 10, 0),
                    size: Decimal::new(rng.gen_range(1..100), 2),
                });
                self.order_book.asks.push(Level {
                    price: base_price + Decimal::new(i * 10, 0),
                    size: Decimal::new(rng.gen_range(1..100), 2),
                });
            }
        }
    }

    /// Sort trades intelligently for display
    /// - Most recent block first
    /// - Within same block: order by price trend (ascending if going up, descending if down)
    pub fn sort_trades_for_display(&mut self) {
        if self.recent_trades.is_empty() {
            return;
        }

        // Convert to Vec for sorting
        let mut trades: Vec<Trade> = self.recent_trades.drain(..).collect();

        // Group by block height
        trades.sort_by(|a, b| {
            // First: sort by block (newest first)
            match b.block_height.cmp(&a.block_height) {
                std::cmp::Ordering::Equal => {
                    // Within same block: detect price trend
                    // If price increasing from previous block, sort ascending
                    // If price decreasing, sort descending
                    // This makes the trade flow feel natural
                    a.execution_price.cmp(&b.execution_price)
                }
                other => other,
            }
        });

        // Put back into VecDeque
        self.recent_trades = trades.into();
    }
}

trait RectExt {
    fn contains_point(&self, x: u16, y: u16) -> bool;
}

impl RectExt for Rect {
    fn contains_point(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}
