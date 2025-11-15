//! UI rendering

use ratatui::{
    Frame,
    layout::{Rect, Constraint, Direction, Layout},
    style::{Color, Style, Modifier},
    widgets::{Block, Borders, Paragraph},
    text::{Line, Span},
};

use crate::state::AppState;
use crate::panels;

pub fn render_ui(f: &mut Frame, app: &AppState) {
    // Render each panel
    for (idx, panel) in app.panels.iter().enumerate() {
        let is_active = idx == app.active_panel;

        // Panel styling
        let border_color = if is_active {
            Color::Cyan
        } else if app.resize_mode {
            Color::Yellow
        } else {
            Color::White
        };

        let border_style = if is_active {
            Style::default().fg(border_color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(border_color)
        };

        // Render panel content
        match panel.panel_type {
            crate::state::panel::PanelType::OrderBook => {
                // Use Penumbra data if available, otherwise fall back to mock
                if let Some(ref order_book_update) = app.latest_order_book {
                    panels::order_book_visual::render_visual_order_book(
                        f,
                        order_book_update,
                        panel.rect,
                        border_style,
                        panel.title()
                    );
                } else {
                    panels::order_book::render(f, &app.order_book, panel.rect, border_style, panel.title());
                }
            }
            crate::state::panel::PanelType::Chart => {
                // Use Penumbra candles if available, otherwise fall back to mock
                if !app.candles.is_empty() {
                    panels::chart_candles::render_candle_chart(
                        f,
                        &app.candles,
                        panel.rect,
                        border_style,
                        panel.title()
                    );
                } else {
                    panels::chart::render(f, &app.price_history, panel.rect, border_style, panel.title());
                }
            }
            crate::state::panel::PanelType::OrderEntry => {
                panels::order_entry::render(f, panel.rect, border_style, panel.title());
            }
            crate::state::panel::PanelType::Positions => {
                panels::positions::render(f, &app.orders, &app.fills, panel.rect, border_style, panel.title());
            }
            crate::state::panel::PanelType::RecentTrades => {
                panels::recent_trades::render(f, &app.recent_trades, panel.rect, border_style, panel.title());
            }
        }
    }

    // Render help text
    render_help(f, app);
}

fn render_help(f: &mut Frame, app: &AppState) {
    let help_area = Rect {
        x: 0,
        y: f.area().height.saturating_sub(1),
        width: f.area().width,
        height: 1,
    };

    let mut help_text = if app.resize_mode {
        vec![
            Span::styled("[R] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Exit Resize | "),
            Span::styled("[Drag] ", Style::default().fg(Color::Cyan)),
            Span::raw("Move Panel | "),
            Span::styled("[Borders] ", Style::default().fg(Color::Cyan)),
            Span::raw("Resize | "),
            Span::styled("[Q] ", Style::default().fg(Color::Red)),
            Span::raw("Quit"),
        ]
    } else {
        vec![
            Span::styled("[R] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw("Resize Mode | "),
            Span::styled("[Tab] ", Style::default().fg(Color::Cyan)),
            Span::raw("Next Panel | "),
            Span::styled("[Q] ", Style::default().fg(Color::Red)),
            Span::raw("Quit"),
        ]
    };

    // Add wallet status
    if app.wallet.is_some() {
        help_text.push(Span::raw(" | "));
        help_text.push(Span::styled(
            "✓ Wallet",
            Style::default().fg(Color::Green),
        ));
        if !app.balances.is_empty() {
            help_text.push(Span::raw(format!(" ({} assets)", app.balances.len())));
        }
    }

    let help = Paragraph::new(Line::from(help_text))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    f.render_widget(help, help_area);
}
