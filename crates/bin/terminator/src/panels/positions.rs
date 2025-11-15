//! Positions and fills panel

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Table, Row, Cell},
    text::Span,
};

use crate::state::{Order, Fill};

pub fn render(f: &mut Frame, orders: &[Order], fills: &[Fill], rect: Rect, border_style: Style, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));

    // Prepare data
    let mut rows = Vec::new();

    // Header
    rows.push(Row::new(vec![
        Cell::from(Span::styled("Order ID", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("Side", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("Price", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("Size", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("Status", Style::default().fg(Color::DarkGray))),
    ]));

    // Orders (show last 10)
    for order in orders.iter().rev().take(10) {
        let side_color = match order.side {
            crate::state::Side::Buy => Color::Green,
            crate::state::Side::Sell => Color::Red,
        };

        let side_text = match order.side {
            crate::state::Side::Buy => "BUY",
            crate::state::Side::Sell => "SELL",
        };

        let status_text = match order.status {
            crate::state::OrderStatus::Pending => "Pending",
            crate::state::OrderStatus::Submitted => "Submitted",
            crate::state::OrderStatus::PartiallyFilled => "Partial",
            crate::state::OrderStatus::Filled => "Filled",
            crate::state::OrderStatus::Cancelled => "Cancelled",
        };

        rows.push(Row::new(vec![
            Cell::from(&order.id[..8.min(order.id.len())]),
            Cell::from(Span::styled(side_text, Style::default().fg(side_color))),
            Cell::from(format!("{:.2}", order.price)),
            Cell::from(format!("{:.4}", order.size)),
            Cell::from(status_text),
        ]));
    }

    let table = Table::new(
        rows,
        [
            ratatui::layout::Constraint::Percentage(20),
            ratatui::layout::Constraint::Percentage(15),
            ratatui::layout::Constraint::Percentage(25),
            ratatui::layout::Constraint::Percentage(20),
            ratatui::layout::Constraint::Percentage(20),
        ],
    )
    .block(block);

    f.render_widget(table, rect);
}
