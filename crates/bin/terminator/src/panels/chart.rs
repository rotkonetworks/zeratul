//! Price chart panel

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    text::Span,
};
use std::collections::VecDeque;

use crate::state::PricePoint;

pub fn render(f: &mut Frame, _price_history: &VecDeque<PricePoint>, rect: Rect, border_style: Style, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));

    // TODO: Implement sparkline or canvas chart
    let content = Paragraph::new("[ Price Chart - Coming Soon ]")
        .block(block)
        .style(Style::default().fg(Color::DarkGray));

    f.render_widget(content, rect);
}
