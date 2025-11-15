//! Recent trades panel

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    text::Span,
};
use std::collections::VecDeque;

use crate::state::Trade;

pub fn render(f: &mut Frame, _trades: &VecDeque<Trade>, rect: Rect, border_style: Style, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));

    let content = Paragraph::new("[ Recent Trades ]")
        .block(block)
        .style(Style::default().fg(Color::DarkGray));

    f.render_widget(content, rect);
}
