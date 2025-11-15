//! Core application logic - pure business logic with no side effects

use super::*;

/// Right panel view mode
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RightPanelView {
    OrderBook,
    Positions,
}

/// Core application state - platform agnostic
#[derive(Clone, Debug)]
pub struct AppCore {
    // Market data
    pub order_book: OrderBook,
    pub recent_trades: Vec<Trade>,

    // User state
    pub positions: Vec<Position>,
    pub balances: Vec<Balance>,

    // UI state
    pub interaction: Interaction,
    pub active_panel: PanelType,
    pub cursor_pos: Option<(u16, u16)>,  // Track mouse cursor position
    pub right_panel_view: RightPanelView,  // Current right panel view
    pub slider_position: f64,  // Position slider (0.0 to 1.0) for logarithmic size

    // Spot panel state
    pub spot_side: Side,  // Buy or Sell
    pub spot_mode: OrderMode,  // Limit or Market
    pub spot_price: Option<f64>,  // Price for limit orders
}

impl Default for AppCore {
    fn default() -> Self {
        Self::new()
    }
}

impl AppCore {
    /// Create new core instance
    pub fn new() -> Self {
        Self {
            order_book: OrderBook::default(),
            recent_trades: Vec::new(),
            positions: Vec::new(),
            balances: Vec::new(),
            interaction: Interaction::None,
            active_panel: PanelType::Chart,
            cursor_pos: None,
            right_panel_view: RightPanelView::OrderBook,
            slider_position: 0.5,  // Start at middle (geometric mean of min/max)
            spot_side: Side::Buy,  // Default to Buy
            spot_mode: OrderMode::Limit,  // Default to Limit
            spot_price: None,  // Will be set on chart click
        }
    }
    
    /// Pure update function - returns effects to execute
    pub fn update(&mut self, event: Event) -> Vec<Effect> {
        use Event::*;
        use Effect as E;
        
        match event {
            // ===== Market Data =====
            OrderBookUpdated(book) => {
                self.order_book = book;
                vec![E::Render(self.view_model())]
            }
            
            TradeExecuted(trade) => {
                self.recent_trades.insert(0, trade);
                if self.recent_trades.len() > 100 {
                    self.recent_trades.truncate(100);
                }
                vec![E::Render(self.view_model())]
            }
            
            BalancesUpdated(balances) => {
                self.balances = balances;
                vec![E::Render(self.view_model())]
            }
            
            // ===== User Interactions =====
            ChartClicked { price, .. } => {
                // Auto-detect buy/sell based on current price
                let mid_price = self.order_book.mid_price().unwrap_or(price);
                let side = if price > mid_price {
                    Side::Sell
                } else {
                    Side::Buy
                };

                // Update spot panel with clicked price and detected side
                self.spot_price = Some(price);
                self.spot_side = side;
                self.spot_mode = OrderMode::Limit;  // Switch to limit mode on chart click

                vec![
                    E::Render(self.view_model()),
                    E::info(format!("{} order at ${:.2}", side, price)),
                ]
            }
            
            SliderMoved { position } => {
                // Update slider position (will be used to calculate size from balance)
                self.slider_position = position;
                vec![E::Render(self.view_model())]
            }
            
            CreatePosition { side, price, size } => {
                self.interaction = Interaction::None;
                
                vec![
                    E::SubmitPosition {
                        side,
                        price,
                        size,
                        fee_bps: 30, // 0.30% default fee
                    },
                    E::success(format!(
                        "Creating {} position: {} @ ${}",
                        side, size, price
                    )),
                    E::Render(self.view_model()),
                ]
            }
            
            CancelPosition { position_id } => {
                vec![
                    E::ClosePosition { position_id: position_id.clone() },
                    E::info(format!("Canceling position {}", position_id)),
                ]
            }
            
            ClosePosition { position_id } => {
                // Find position and mark as closing
                if let Some(pos) = self.positions.iter_mut().find(|p| p.id == position_id) {
                    pos.status = PositionStatus::Closed;
                }
                
                vec![
                    E::ClosePosition { position_id },
                    E::Render(self.view_model()),
                ]
            }
            
            // ===== Navigation =====
            PanelFocused(panel) => {
                self.active_panel = panel;
                vec![E::Render(self.view_model())]
            }
            
            NextPanel => {
                self.active_panel = match self.active_panel {
                    PanelType::OrderBook => PanelType::Chart,
                    PanelType::Chart => PanelType::OrderEntry,
                    PanelType::OrderEntry => PanelType::Positions,
                    PanelType::Positions => PanelType::RecentTrades,
                    PanelType::RecentTrades => PanelType::OrderBook,
                };
                vec![E::Render(self.view_model())]
            }

            ToggleRightPanelView => {
                self.right_panel_view = match self.right_panel_view {
                    RightPanelView::OrderBook => RightPanelView::Positions,
                    RightPanelView::Positions => RightPanelView::OrderBook,
                };
                vec![E::Render(self.view_model())]
            }

            // Vim-style navigation (Ctrl+hjkl)
            FocusDown => {
                // Move focus down (Chart -> Positions)
                self.active_panel = PanelType::Positions;
                vec![E::Render(self.view_model())]
            }

            FocusUp => {
                // Move focus up (Positions -> Chart)
                self.active_panel = PanelType::Chart;
                vec![E::Render(self.view_model())]
            }

            FocusLeft | FocusRight => {
                // For now, left/right do nothing in vertical layout
                // Later can be used for spot panel or other side panels
                vec![]
            }

            CancelAction => {
                self.interaction = Interaction::None;
                vec![E::Render(self.view_model())]
            }
            
            Quit => {
                vec![E::Exit]
            }

            // ===== Mouse Events =====
            MouseMove { x, y } => {
                self.cursor_pos = Some((x, y));
                // Don't render on every mouse move - too expensive
                vec![]
            }

            RightClick { x, y } => {
                // Open context menu at click position
                let options = vec![
                    "Create Buy Order".to_string(),
                    "Create Sell Order".to_string(),
                    "Cancel".to_string(),
                ];

                self.interaction = Interaction::ContextMenu { x, y, options };
                vec![E::Render(self.view_model())]
            }

            // ===== Ignored =====
            Tick | Ignored => vec![],

            // TODO: Implement remaining events
            _ => vec![],
        }
    }
    
    /// Generate view model for rendering
    pub fn view_model(&self) -> ViewModel {
        ViewModel::new(
            self.order_book.clone(),
            self.recent_trades.clone(),
            self.positions.clone(),
            self.balances.clone(),
            self.interaction.clone(),
            self.cursor_pos,
            self.right_panel_view,
            self.active_panel,
            self.spot_side,
            self.spot_mode,
            self.spot_price,
            self.slider_position,
        )
    }
}

/// Convert slider position (0.0 to 1.0) to size using logarithmic scale
fn slider_to_size(position: f64, min: f64, max: f64) -> f64 {
    let log_min = min.ln();
    let log_max = max.ln();
    (log_min + position * (log_max - log_min)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    
    #[test]
    fn test_chart_click_creates_interaction() {
        let mut core = AppCore::new();
        
        let effects = core.update(Event::ChartClicked {
            price: 3000.0,
            x: 100,
            y: 50,
        });
        
        // Should enter creation mode
        assert!(matches!(
            core.interaction,
            Interaction::CreatingPosition { price, side: Side::Sell, .. }
            if (price - 3000.0).abs() < 0.01
        ));
        
        // Should request render
        assert!(effects.iter().any(|e| matches!(e, Effect::Render(_))));
    }
    
    #[test]
    fn test_slider_logarithmic() {
        // Position 0.0 → 0.1
        assert!((slider_to_size(0.0, 0.1, 100.0) - 0.1).abs() < 0.01);
        
        // Position 1.0 → 100.0
        assert!((slider_to_size(1.0, 0.1, 100.0) - 100.0).abs() < 0.01);
        
        // Position 0.5 → ~3.16 (geometric mean)
        let mid = slider_to_size(0.5, 0.1, 100.0);
        assert!(mid > 1.0 && mid < 10.0);
    }
}
