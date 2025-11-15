//! ASCII candlestick chart renderer

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style, Modifier},
    widgets::{Block, Borders, Paragraph},
    text::{Line, Span},
};

use crate::network::penumbra::grpc_client::Candle;

/// Render ASCII candlestick chart
pub fn render_candle_chart(
    f: &mut Frame,
    candles: &[Candle],
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

    if candles.is_empty() {
        let empty = Paragraph::new("No data available")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, inner);
        return;
    }

    // Calculate price range
    let mut min_price = f64::MAX;
    let mut max_price = f64::MIN;

    for candle in candles {
        min_price = min_price.min(candle.low);
        max_price = max_price.max(candle.high);
    }

    // Add 5% padding
    let padding = (max_price - min_price) * 0.05;
    min_price -= padding;
    max_price += padding;

    let price_range = max_price - min_price;
    let chart_height = inner.height as usize;
    let chart_width = inner.width as usize;

    // Build chart lines
    let mut lines = Vec::new();

    // Render candles row by row (top to bottom)
    for row in 0..chart_height {
        let price_at_row = max_price - (row as f64 / chart_height as f64) * price_range;
        let mut line_spans = Vec::new();

        // Y-axis label (price)
        if row % 3 == 0 {
            line_spans.push(Span::styled(
                format!("{:>8.2} │ ", price_at_row),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            line_spans.push(Span::raw("         │ "));
        }

        // Render candles
        let candles_to_show = candles.len().min(chart_width - 12);
        let candle_width = (chart_width - 12) / candles_to_show;

        for (i, candle) in candles.iter().rev().take(candles_to_show).rev().enumerate() {
            let candle_char = get_candle_char_at_price(
                price_at_row,
                candle,
                min_price,
                max_price,
                chart_height,
            );

            let color = if candle.close >= candle.open {
                Color::Green
            } else {
                Color::Red
            };

            if i * candle_width < chart_width - 12 {
                line_spans.push(Span::styled(candle_char, Style::default().fg(color)));
            }
        }

        lines.push(Line::from(line_spans));
    }

    // Add time axis at bottom
    if !candles.is_empty() {
        let first = candles.first().unwrap();
        let last = candles.last().unwrap();

        let time_line = Line::from(vec![
            Span::raw("         └"),
            Span::raw("─".repeat(chart_width - 12)),
            Span::raw("→"),
        ]);
        lines.push(time_line);

        let time_labels = Line::from(vec![
            Span::styled(
                format!("  {:>8}", first.timestamp.format("%H:%M")),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(" ".repeat(chart_width - 30)),
            Span::styled(
                format!("{:>8}", last.timestamp.format("%H:%M")),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        lines.push(time_labels);

        // Add stats
        if let Some(latest) = candles.last() {
            let change = latest.close - candles.first().unwrap().open;
            let change_pct = (change / candles.first().unwrap().open) * 100.0;

            let change_color = if change >= 0.0 {
                Color::Green
            } else {
                Color::Red
            };

            let stats_line = Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("Close: ${:.2}", latest.close),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  │  "),
                Span::styled(
                    format!("{:+.2} ({:+.2}%)", change, change_pct),
                    Style::default().fg(change_color),
                ),
                Span::raw("  │  "),
                Span::styled(
                    format!("Vol: {:.0}", candles.iter().map(|c| c.volume).sum::<f64>()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            lines.push(stats_line);
        }
    }

    let chart = Paragraph::new(lines);
    f.render_widget(chart, inner);
}

/// Get character to render at this price level for a candle
fn get_candle_char_at_price(
    price_at_row: f64,
    candle: &Candle,
    min_price: f64,
    max_price: f64,
    chart_height: usize,
) -> &'static str {
    let price_range = max_price - min_price;
    let candle_top = candle.high;
    let candle_bottom = candle.low;
    let body_top = candle.open.max(candle.close);
    let body_bottom = candle.open.min(candle.close);

    // Normalize prices to 0-1 range
    let normalize = |p: f64| (p - min_price) / price_range;

    let row_price_norm = normalize(price_at_row);
    let candle_top_norm = normalize(candle_top);
    let candle_bottom_norm = normalize(candle_bottom);
    let body_top_norm = normalize(body_top);
    let body_bottom_norm = normalize(body_bottom);

    // Convert to row positions (inverted because top of screen is 0)
    let row_pos = (1.0 - row_price_norm) * chart_height as f64;
    let wick_top = (1.0 - candle_top_norm) * chart_height as f64;
    let wick_bottom = (1.0 - candle_bottom_norm) * chart_height as f64;
    let body_top_pos = (1.0 - body_top_norm) * chart_height as f64;
    let body_bottom_pos = (1.0 - body_bottom_norm) * chart_height as f64;

    // Check if current row intersects with candle
    if row_pos < wick_top || row_pos > wick_bottom {
        return " ";
    }

    // Wick above body
    if row_pos < body_top_pos {
        return "│";
    }

    // Body
    if row_pos >= body_top_pos && row_pos <= body_bottom_pos {
        return "█";
    }

    // Wick below body
    if row_pos > body_bottom_pos && row_pos <= wick_bottom {
        return "│";
    }

    " "
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_candle_char_selection() {
        let candle = Candle {
            timestamp: Utc::now(),
            open: 100.0,
            high: 110.0,
            low: 90.0,
            close: 105.0,
            volume: 1000.0,
        };

        // Price above wick should be empty
        let char = get_candle_char_at_price(120.0, &candle, 80.0, 120.0, 40);
        assert_eq!(char, " ");

        // Price in upper wick should be wick
        let char = get_candle_char_at_price(108.0, &candle, 80.0, 120.0, 40);
        assert_eq!(char, "│");

        // Price in body should be body
        let char = get_candle_char_at_price(103.0, &candle, 80.0, 120.0, 40);
        assert_eq!(char, "█");

        // Price below should be empty
        let char = get_candle_char_at_price(85.0, &candle, 80.0, 120.0, 40);
        assert_eq!(char, " ");
    }
}
