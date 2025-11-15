//! Order book panel - displays bids and asks

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style, Modifier},
    widgets::{Block, Borders, Table, Row, Cell},
    text::Span,
};

use crate::state::OrderBook;

pub fn render(f: &mut Frame, order_book: &OrderBook, rect: Rect, border_style: Style, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));

    // Prepare data
    let mut rows = Vec::new();

    // Header
    rows.push(Row::new(vec![
        Cell::from(Span::styled("Bid Size", Style::default().fg(Color::DarkGray))),
        Cell::from(Span::styled("Bid", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))),
        Cell::from(Span::styled("Ask", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
        Cell::from(Span::styled("Ask Size", Style::default().fg(Color::DarkGray))),
    ]));

    // Book levels (show top 10)
    let levels = order_book.bids.len().min(order_book.asks.len()).min(10);
    for i in 0..levels {
        let bid = &order_book.bids[i];
        let ask = &order_book.asks[i];

        rows.push(Row::new(vec![
            Cell::from(format!("{:.4}", bid.size)),
            Cell::from(Span::styled(
                format!("{:.2}", bid.price),
                Style::default().fg(Color::Green),
            )),
            Cell::from(Span::styled(
                format!("{:.2}", ask.price),
                Style::default().fg(Color::Red),
            )),
            Cell::from(format!("{:.4}", ask.size)),
        ]));
    }

    let table = Table::new(
        rows,
        [
            ratatui::layout::Constraint::Percentage(25),
            ratatui::layout::Constraint::Percentage(25),
            ratatui::layout::Constraint::Percentage(25),
            ratatui::layout::Constraint::Percentage(25),
        ],
    )
    .block(block);

    f.render_widget(table, rect);
}
