//! Order entry panel - place buy/sell orders

use ratatui::{
    Frame,
    layout::{Rect, Constraint, Layout, Direction},
    style::{Color, Style, Modifier},
    widgets::{Block, Borders, Paragraph},
    text::{Line, Span},
};

pub fn render(f: &mut Frame, rect: Rect, border_style: Style, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));

    let inner = block.inner(rect);
    f.render_widget(block, rect);

    // Split into buy/sell sections
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    // Buy section
    let buy_lines = vec![
        Line::from(vec![
            Span::styled("BUY", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from("Price: _______"),
        Line::from("Amount: _______"),
        Line::from(""),
        Line::from(Span::styled("[Enter] Place Order", Style::default().fg(Color::DarkGray))),
    ];

    let buy_block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray));

    let buy_para = Paragraph::new(buy_lines).block(buy_block);
    f.render_widget(buy_para, chunks[0]);

    // Sell section
    let sell_lines = vec![
        Line::from(vec![
            Span::styled("SELL", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from("Price: _______"),
        Line::from("Amount: _______"),
        Line::from(""),
        Line::from(Span::styled("[Enter] Place Order", Style::default().fg(Color::DarkGray))),
    ];

    let sell_para = Paragraph::new(sell_lines);
    f.render_widget(sell_para, chunks[1]);
}
