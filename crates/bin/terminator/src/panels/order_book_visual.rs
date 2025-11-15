//! Visual order book with depth bars

use ratatui::{
    Frame,
    layout::{Rect, Constraint, Layout, Direction},
    style::{Color, Style, Modifier},
    widgets::{Block, Borders, Paragraph, Row, Table, Cell},
    text::{Line, Span},
};

use crate::network::penumbra::grpc_client::{OrderBookUpdate, Level};

/// Render visual order book with depth bars
pub fn render_visual_order_book(
    f: &mut Frame,
    order_book: &OrderBookUpdate,
    rect: Rect,
    border_style: Style,
    title: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, border_style));

    let inner = block.inner(rect);
    f.render_widget(block, rect);

    // Split into asks (top) and bids (bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45), // Asks
            Constraint::Length(1),       // Spread
            Constraint::Percentage(45), // Bids
        ])
        .split(inner);

    // Render asks (descending order, red)
    render_side(f, &order_book.asks, chunks[0], true, Color::Red);

    // Render spread
    if let (Some(best_bid), Some(best_ask)) = (
        order_book.bids.first(),
        order_book.asks.first(),
    ) {
        let spread = best_ask.price - best_bid.price;
        let spread_pct = (spread / best_bid.price) * 100.0;

        let spread_line = Line::from(vec![
            Span::styled("─────", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" Spread: ${:.2} ({:.2}%) ", spread, spread_pct),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("─────", Style::default().fg(Color::DarkGray)),
        ]);

        let spread_para = Paragraph::new(spread_line)
            .style(Style::default().fg(Color::DarkGray));

        f.render_widget(spread_para, chunks[1]);
    }

    // Render bids (ascending order, green)
    render_side(f, &order_book.bids, chunks[2], false, Color::Green);
}

/// Render one side of the book (bids or asks)
fn render_side(
    f: &mut Frame,
    levels: &[Level],
    rect: Rect,
    is_asks: bool,
    color: Color,
) {
    if levels.is_empty() {
        let empty = Paragraph::new("No liquidity")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, rect);
        return;
    }

    // Show up to 10 levels
    let display_levels: Vec<_> = if is_asks {
        // Asks: show top 10, reverse order (highest price at top)
        levels.iter().take(10).rev().cloned().collect()
    } else {
        // Bids: show top 10 (highest price at top)
        levels.iter().take(10).cloned().collect()
    };

    // Calculate max size for depth bar scaling
    let max_size = levels.iter()
        .take(10)
        .map(|l| l.size)
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(1.0);

    // Build rows
    let mut rows = vec![];

    for level in display_levels {
        let depth_pct = (level.size / max_size * 100.0) as usize;
        let depth_bar = render_depth_bar(depth_pct, rect.width as usize, color);

        // Format: [Depth Bar] Price | Size | Total
        let row = Row::new(vec![
            Cell::from(depth_bar),
            Cell::from(Span::styled(
                format!("{:>10.2}", level.price),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            Cell::from(format!("{:>8.4}", level.size)),
            Cell::from(Span::styled(
                format!("{:>8.4}", level.total),
                Style::default().fg(Color::DarkGray),
            )),
        ]);

        rows.push(row);
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30), // Depth bar
            Constraint::Percentage(30), // Price
            Constraint::Percentage(20), // Size
            Constraint::Percentage(20), // Total
        ],
    );

    f.render_widget(table, rect);
}

/// Render a horizontal depth bar
fn render_depth_bar(percent: usize, max_width: usize, color: Color) -> Span<'static> {
    let bar_width = (percent * max_width / 100).min(max_width);
    let bar = "█".repeat(bar_width);

    Span::styled(bar, Style::default().fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use penumbra_asset::asset;

    #[test]
    fn test_depth_bar() {
        let bar = render_depth_bar(50, 20, Color::Green);
        assert_eq!(bar.content.len(), 10); // 50% of 20
    }

    #[test]
    fn test_empty_order_book() {
        // Just a smoke test - can't actually render without a Frame
        let update = OrderBookUpdate {
            pair: penumbra_dex::TradingPair::new(
                asset::REGISTRY.parse_unit("upenumbra").id(),
                asset::REGISTRY.parse_unit("upenumbra").id(),
            ),
            bids: vec![],
            asks: vec![],
            timestamp: Utc::now(),
        };

        assert_eq!(update.bids.len(), 0);
    }
}
