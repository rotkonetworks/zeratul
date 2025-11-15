//! Renderer - converts ViewModel to terminal UI

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    text::{Line, Span},
};

use crate::core::{ViewModel, Interaction, Side};

/// Clickable button regions
#[derive(Debug, Clone, Copy)]
pub enum ButtonRegion {
    ToggleSide,
    ToggleMode,
    Submit,
    Withdraw,
    Deposit,
}

/// Renders the ViewModel to the terminal
pub struct Renderer {
    /// Last rendered chart area for mouse click detection
    pub last_chart_area: Option<Rect>,

    /// Clickable button regions
    pub button_regions: Vec<(Rect, ButtonRegion)>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            last_chart_area: None,
            button_regions: Vec::new(),
        }
    }

    /// Render the entire UI from a ViewModel
    pub fn render(&mut self, f: &mut Frame, view_model: &ViewModel) {
        let size = f.area();

        // Main layout: [Header | Body | Footer]
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Min(0),      // Body
                Constraint::Length(3),   // Footer
            ])
            .split(size);

        // Render header
        self.render_header(f, view_model, main_chunks[0]);

        // Body layout: [Chart | Spot Panel | Positions] - 3 columns
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),  // Chart
                Constraint::Percentage(25),  // Spot panel
                Constraint::Percentage(25),  // Positions
            ])
            .split(main_chunks[1]);

        // Render chart area and store bounds
        self.last_chart_area = Some(columns[0]);
        self.render_chart(f, view_model, columns[0]);

        // Render spot panel in middle
        self.render_spot_panel(f, view_model, columns[1]);

        // Render positions panel on right
        self.render_positions(f, view_model, columns[2]);

        // Render footer
        self.render_footer(f, view_model, main_chunks[2]);
    }

    fn render_spot_panel(&self, f: &mut Frame, view_model: &ViewModel, area: Rect) {
        use crate::core::{PanelType, OrderMode};

        let mut content = vec![];

        // Mode toggles: [Buy/Sell]
        let side_line = match view_model.spot_side {
            crate::core::Side::Buy => Line::from(vec![
                Span::styled("[BUY]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("SELL", Style::default().fg(Color::DarkGray)),
            ]),
            crate::core::Side::Sell => Line::from(vec![
                Span::styled("BUY", Style::default().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled("[SELL]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            ]),
        };
        content.push(side_line);
        content.push(Line::from(""));

        // Order type toggles: [Limit/Market]
        let mode_line = match view_model.spot_mode {
            OrderMode::Limit => Line::from(vec![
                Span::styled("[Limit]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("Market", Style::default().fg(Color::DarkGray)),
            ]),
            OrderMode::Market => Line::from(vec![
                Span::styled("Limit", Style::default().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled("[Market]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            ]),
        };
        content.push(mode_line);
        content.push(Line::from(""));

        // Price field (only for limit orders)
        if view_model.spot_mode == OrderMode::Limit {
            if let Some(price) = view_model.spot_price {
                content.push(Line::from(vec![
                    Span::raw("Price: "),
                    Span::styled(format!("${:.2}", price), Style::default().fg(Color::Yellow)),
                ]));
            } else {
                content.push(Line::from(vec![
                    Span::styled("Price: Click chart", Style::default().fg(Color::DarkGray)),
                ]));
            }
            content.push(Line::from(""));
        }

        // Slider (logarithmic 0-100%)
        let slider_pct = (view_model.slider_position * 100.0) as u32;
        content.push(Line::from(vec![
            Span::raw("Size: "),
            Span::styled(format!("{}%", slider_pct), Style::default().fg(Color::Cyan)),
        ]));

        // Slider bar visualization
        let bar_width = 15;
        let filled = ((view_model.slider_position * bar_width as f64) as usize).min(bar_width);
        let slider_bar = format!("[{}{}]",
            "=".repeat(filled),
            " ".repeat(bar_width - filled)
        );
        content.push(Line::from(vec![
            Span::styled(slider_bar, Style::default().fg(Color::Cyan)),
        ]));
        content.push(Line::from(""));

        // Buttons
        content.push(Line::from(""));
        content.push(Line::from(vec![
            Span::styled("[ SUBMIT ]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]));
        content.push(Line::from(""));
        content.push(Line::from(vec![
            Span::styled("[ Withdraw ]", Style::default().fg(Color::Yellow)),
        ]));
        content.push(Line::from(vec![
            Span::styled("[ Deposit ]", Style::default().fg(Color::Yellow)),
        ]));

        // Highlight border if spot panel is active
        let border_color = if view_model.active_panel == PanelType::OrderEntry {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let spot = Paragraph::new(content)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title("Spot"));

        f.render_widget(spot, area);
    }

    fn render_right_panel(&self, f: &mut Frame, view_model: &ViewModel, area: Rect) {
        use crate::core::app::RightPanelView;

        // Split into tab bar and content
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),  // Tab bar
                Constraint::Min(0),      // Content
            ])
            .split(area);

        // Render tab bar
        let tab_bar = match view_model.right_panel_view {
            RightPanelView::OrderBook => Line::from(vec![
                Span::styled("[Order Book]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("Positions", Style::default().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled("(v to toggle)", Style::default().fg(Color::DarkGray)),
            ]),
            RightPanelView::Positions => Line::from(vec![
                Span::styled("Order Book", Style::default().fg(Color::DarkGray)),
                Span::raw("  "),
                Span::styled("[Positions]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("(v to toggle)", Style::default().fg(Color::DarkGray)),
            ]),
        };

        let tab_widget = Paragraph::new(tab_bar)
            .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
        f.render_widget(tab_widget, chunks[0]);

        // Render content based on view
        match view_model.right_panel_view {
            RightPanelView::OrderBook => self.render_order_book(f, view_model, chunks[1]),
            RightPanelView::Positions => self.render_positions(f, view_model, chunks[1]),
        }
    }

    fn render_header(&self, f: &mut Frame, view_model: &ViewModel, area: Rect) {
        let title = vec![
            Span::styled("Terminator", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" | "),
            Span::styled("Penumbra DEX", Style::default().fg(Color::White)),
        ];

        let price_info = if let Some(mid_price) = view_model.mid_price {
            vec![
                Span::raw("Mid: "),
                Span::styled(
                    format!("${:.2}", mid_price),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                ),
                Span::raw(" | Spread: "),
                Span::styled(
                    format!("{:.2}%", view_model.spread.unwrap_or(0.0)),
                    Style::default().fg(Color::Gray)
                ),
            ]
        } else {
            vec![Span::styled("No market data", Style::default().fg(Color::DarkGray))]
        };

        let header = Paragraph::new(vec![
            Line::from(title),
            Line::from(price_info),
        ])
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));

        f.render_widget(header, area);
    }

    fn render_chart(&self, f: &mut Frame, view_model: &ViewModel, area: Rect) {
        use crate::core::PanelType;

        let mut content = vec![];

        // Show current interaction state
        match &view_model.interaction {
            Interaction::CreatingPosition { price, side, size } => {
                let side_color = match side {
                    Side::Buy => Color::Green,
                    Side::Sell => Color::Red,
                };

                content.push(Line::from(vec![
                    Span::styled(
                        format!("Creating {} Position", side),
                        Style::default().fg(side_color).add_modifier(Modifier::BOLD)
                    ),
                ]));
                content.push(Line::from(vec![
                    Span::raw("Price: "),
                    Span::styled(format!("${:.2}", price), Style::default().fg(Color::Yellow)),
                ]));
                content.push(Line::from(vec![
                    Span::raw("Size: "),
                    Span::styled(format!("{:.4}", size), Style::default().fg(Color::Cyan)),
                ]));
                content.push(Line::from(""));
                content.push(Line::from(vec![
                    Span::styled("Use slider to adjust size (logarithmic)", Style::default().fg(Color::DarkGray)),
                ]));
                content.push(Line::from(vec![
                    Span::styled("Press Enter to confirm, Esc to cancel", Style::default().fg(Color::DarkGray)),
                ]));
            }

            Interaction::DraggingPosition { position_id, original_price, new_price } => {
                content.push(Line::from(vec![
                    Span::styled("Dragging Position", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                ]));
                content.push(Line::from(vec![
                    Span::raw("ID: "),
                    Span::styled(position_id.clone(), Style::default().fg(Color::Cyan)),
                ]));
                content.push(Line::from(vec![
                    Span::raw("From: "),
                    Span::styled(format!("${:.2}", original_price), Style::default().fg(Color::Gray)),
                    Span::raw(" → To: "),
                    Span::styled(format!("${:.2}", new_price), Style::default().fg(Color::Yellow)),
                ]));
            }

            Interaction::ContextMenu { x, y, options } => {
                // Context menu will be rendered as an overlay
                content.push(Line::from(vec![
                    Span::styled("Context Menu", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                ]));
                content.push(Line::from(vec![
                    Span::raw(format!("Position: ({}, {})", x, y)),
                ]));
                content.push(Line::from(""));
                for option in options {
                    content.push(Line::from(vec![
                        Span::styled(format!("  {}", option), Style::default().fg(Color::White)),
                    ]));
                }
            }

            Interaction::None => {
                // Show chart placeholder
                content.push(Line::from(vec![
                    Span::styled("Chart Area", Style::default().fg(Color::DarkGray)),
                ]));
                content.push(Line::from(""));
                content.push(Line::from(vec![
                    Span::raw("Click to create position at price level"),
                ]));
                content.push(Line::from(vec![
                    Span::styled("Green", Style::default().fg(Color::Green)),
                    Span::raw(" = Buy below mid price"),
                ]));
                content.push(Line::from(vec![
                    Span::styled("Red", Style::default().fg(Color::Red)),
                    Span::raw(" = Sell above mid price"),
                ]));
                content.push(Line::from(""));

                // Show best bid/ask
                if let (Some(bid), Some(ask)) = (view_model.best_bid, view_model.best_ask) {
                    content.push(Line::from(vec![
                        Span::styled("Best Bid: ", Style::default().fg(Color::Green)),
                        Span::styled(format!("${:.2}", bid), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                        Span::raw("  |  "),
                        Span::styled("Best Ask: ", Style::default().fg(Color::Red)),
                        Span::styled(format!("${:.2}", ask), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    ]));
                }
            }
        }

        // Highlight border if this panel is active
        let border_color = if view_model.active_panel == PanelType::Chart {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let chart = Paragraph::new(content)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title("Chart"));

        f.render_widget(chart, area);
    }

    fn render_order_book(&self, f: &mut Frame, view_model: &ViewModel, area: Rect) {
        let mut content = vec![
            Line::from(vec![
                Span::styled("Order Book", Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(""),
        ];

        // Show asks (highest to lowest)
        content.push(Line::from(vec![
            Span::styled("ASKS", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ]));

        for level in view_model.order_book.asks.iter().take(5).rev() {
            content.push(Line::from(vec![
                Span::styled(
                    format!("{:.2}", level.price),
                    Style::default().fg(Color::Red)
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:.4}", level.size),
                    Style::default().fg(Color::Gray)
                ),
            ]));
        }

        // Mid price
        if let Some(mid) = view_model.mid_price {
            content.push(Line::from(""));
            content.push(Line::from(vec![
                Span::styled(
                    format!("─── ${:.2} ───", mid),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                ),
            ]));
            content.push(Line::from(""));
        }

        // Show bids
        content.push(Line::from(vec![
            Span::styled("BIDS", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]));

        for level in view_model.order_book.bids.iter().take(5) {
            content.push(Line::from(vec![
                Span::styled(
                    format!("{:.2}", level.price),
                    Style::default().fg(Color::Green)
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:.4}", level.size),
                    Style::default().fg(Color::Gray)
                ),
            ]));
        }

        let order_book = Paragraph::new(content)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White)));

        f.render_widget(order_book, area);
    }

    fn render_positions(&self, f: &mut Frame, view_model: &ViewModel, area: Rect) {
        use crate::core::PanelType;

        let mut content = vec![
            Line::from(vec![
                Span::styled("Positions", Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(""),
        ];

        if view_model.positions.is_empty() {
            content.push(Line::from(vec![
                Span::styled("No open positions", Style::default().fg(Color::DarkGray)),
            ]));
        } else {
            for position in view_model.positions.iter().take(10) {
                let side_color = match position.side {
                    Side::Buy => Color::Green,
                    Side::Sell => Color::Red,
                };

                content.push(Line::from(vec![
                    Span::styled(
                        format!("{}", position.side),
                        Style::default().fg(side_color).add_modifier(Modifier::BOLD)
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("{:.4}", position.size),
                        Style::default().fg(Color::White)
                    ),
                    Span::raw(" @ "),
                    Span::styled(
                        format!("${:.2}", position.price),
                        Style::default().fg(Color::Yellow)
                    ),
                ]));
            }
        }

        // Highlight border if this panel is active
        let border_color = if view_model.active_panel == PanelType::Positions {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let positions = Paragraph::new(content)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)));

        f.render_widget(positions, area);
    }

    fn render_footer(&self, f: &mut Frame, _view_model: &ViewModel, area: Rect) {
        let help = vec![
            Line::from(vec![
                Span::styled("q", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(": quit  "),
                Span::styled("Ctrl+jk", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(": navigate  "),
                Span::styled("v", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(": toggle  "),
                Span::styled("Esc", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(": cancel"),
            ]),
        ];

        let footer = Paragraph::new(help)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));

        f.render_widget(footer, area);
    }
}
