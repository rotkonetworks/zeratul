//! Market trades panel - shows recent swap executions
//! 
//! Trades are ordered intelligently:
//! 1. Most recent block first
//! 2. Within same block: ordered by price movement (asc/desc based on trend)

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style, Modifier},
    widgets::{Block, Borders, Table, Row, Cell},
    text::Span,
};
use std::collections::VecDeque;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::state::{Trade, Side};

pub fn render(f: &mut Frame, trades: &VecDeque<Trade>, rect: Rect, border_style: Style, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));

    // Header
    let header = Row::new(vec![
        Cell::from("Time"),
        Cell::from("Price"),
        Cell::from("Size"),
        Cell::from("Block"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .bottom_margin(1);

    // Trade rows
    let mut rows = Vec::new();
    let mut prev_block: Option<u64> = None;
    
    for trade in trades.iter().take(20) {
        // Detect block boundary for visual separation
        let block_changed = prev_block.map_or(false, |pb| pb != trade.block_height);
        prev_block = Some(trade.block_height);

        // Color based on side
        let price_style = match trade.side {
            Side::Buy => Style::default().fg(Color::Green),
            Side::Sell => Style::default().fg(Color::Red),
        };

        // Format time
        let time_str = trade.time.format("%H:%M:%S").to_string();
        
        // Format price with execution price
        let price_str = if (trade.price - trade.execution_price).abs() > Decimal::new(1, 2) {
            // Show both if significantly different
            format!("{:.2} ({})", trade.price, trade.execution_price)
        } else {
            format!("{:.2}", trade.execution_price)
        };

        let mut row = Row::new(vec![
            Cell::from(time_str),
            Cell::from(price_str).style(price_style),
            Cell::from(format!("{:.4}", trade.size)),
            Cell::from(trade.block_height.to_string()).style(Style::default().fg(Color::DarkGray)),
        ]);

        // Add visual separator between blocks
        if block_changed {
            row = row.top_margin(1);
        }

        rows.push(row);
    }

    // Column widths
    let widths = [
        ratatui::layout::Constraint::Length(10), // Time
        ratatui::layout::Constraint::Length(15), // Price
        ratatui::layout::Constraint::Length(12), // Size
        ratatui::layout::Constraint::Length(8),  // Block
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1);

    f.render_widget(table, rect);
}
