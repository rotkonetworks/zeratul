//! UI Widgets for trading terminal

use eframe::egui::{self, Color32, RichText};
use crate::core::{ViewModel, Event, Side, OrderMode};

/// Render header with market info
pub fn render_header(ui: &mut egui::Ui, view_model: &ViewModel) {
    ui.horizontal(|ui| {
        ui.heading("Terminator");
        ui.separator();

        if let Some(mid) = view_model.mid_price {
            ui.label(RichText::new(format!("Mid: ${:.2}", mid)).size(18.0));
        }

        if let Some(spread) = view_model.spread {
            ui.label(RichText::new(format!("Spread: ${:.2}", spread)).size(14.0).color(Color32::GRAY));
        }
    });
}

/// Render footer with status info
pub fn render_footer(ui: &mut egui::Ui, _view_model: &ViewModel) {
    ui.horizontal(|ui| {
        ui.label("Ctrl+Q: Quit | Vim nav: Ctrl+hjkl");
    });
}

/// Render price chart
pub fn render_chart(ui: &mut egui::Ui, view_model: &ViewModel) -> Option<Event> {
    let mut event = None;

    egui::Frame::none()
        .fill(Color32::from_rgb(20, 20, 25))
        .show(ui, |ui| {
            ui.heading("Chart");

            // Price range
            let (min_price, max_price) = if let (Some(bid), Some(ask)) = (view_model.best_bid, view_model.best_ask) {
                let spread = (ask - bid).max(0.01);
                let padding = spread * 0.5;
                (bid - padding, ask + padding)
            } else {
                (2500.0, 3500.0)
            };

            // Draw simplified chart area
            let response = ui.allocate_response(
                egui::vec2(ui.available_width(), ui.available_height()),
                egui::Sense::click()
            );

            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    // Calculate price from Y position
                    let rect = response.rect;
                    let relative_y = (pos.y - rect.min.y) / rect.height();
                    let price = max_price - relative_y as f64 * (max_price - min_price);

                    event = Some(Event::ChartClicked {
                        price,
                        x: pos.x as u16,
                        y: pos.y as u16,
                    });
                }
            }

            // Draw price levels
            let painter = ui.painter();
            let rect = response.rect;

            // Draw mid price line
            if let Some(mid) = view_model.mid_price {
                let y = rect.min.y + (1.0 - (mid - min_price) / (max_price - min_price)) as f32 * rect.height();
                painter.hline(
                    rect.min.x..=rect.max.x,
                    y,
                    (1.0, Color32::YELLOW)
                );
                painter.text(
                    egui::pos2(rect.min.x + 5.0, y),
                    egui::Align2::LEFT_CENTER,
                    format!("${:.2}", mid),
                    egui::FontId::proportional(12.0),
                    Color32::YELLOW
                );
            }
        });

    event
}

/// Render spot trading panel with REAL working buttons
pub fn render_spot_panel(ui: &mut egui::Ui, view_model: &ViewModel) -> Option<Event> {
    let mut event = None;

    egui::Frame::none()
        .fill(Color32::from_rgb(25, 25, 30))
        .show(ui, |ui| {
            ui.heading("Spot");

            ui.add_space(10.0);

            // Buy/Sell toggle
            ui.horizontal(|ui| {
                let buy_color = if view_model.spot_side == Side::Buy {
                    Color32::GREEN
                } else {
                    Color32::DARK_GRAY
                };

                let sell_color = if view_model.spot_side == Side::Sell {
                    Color32::RED
                } else {
                    Color32::DARK_GRAY
                };

                if ui.button(RichText::new("BUY").color(buy_color).size(16.0)).clicked() {
                    event = Some(Event::ToggleSide);
                }

                if ui.button(RichText::new("SELL").color(sell_color).size(16.0)).clicked() {
                    event = Some(Event::ToggleSide);
                }
            });

            ui.add_space(5.0);

            // Limit/Market toggle
            ui.horizontal(|ui| {
                let limit_color = if view_model.spot_mode == OrderMode::Limit {
                    Color32::LIGHT_BLUE
                } else {
                    Color32::DARK_GRAY
                };

                let market_color = if view_model.spot_mode == OrderMode::Market {
                    Color32::LIGHT_BLUE
                } else {
                    Color32::DARK_GRAY
                };

                if ui.button(RichText::new("Limit").color(limit_color)).clicked() {
                    event = Some(Event::ToggleOrderMode);
                }

                if ui.button(RichText::new("Market").color(market_color)).clicked() {
                    event = Some(Event::ToggleOrderMode);
                }
            });

            ui.add_space(10.0);

            // Price field (only for Limit orders)
            if view_model.spot_mode == OrderMode::Limit {
                ui.label("Price:");
                if let Some(price) = view_model.spot_price {
                    ui.label(RichText::new(format!("${:.2}", price)).size(16.0));
                } else {
                    ui.label(RichText::new("Click chart").color(Color32::GRAY));
                }
            }

            ui.add_space(10.0);

            // Size slider
            ui.label("Size:");
            ui.add(egui::Slider::new(&mut 0.5f32, 0.0..=1.0)
                .show_value(false));
            ui.label(format!("{}%", (view_model.slider_position * 100.0) as i32));

            ui.add_space(20.0);

            // Submit button (GREEN and BIG)
            let submit_color = match view_model.spot_side {
                Side::Buy => Color32::GREEN,
                Side::Sell => Color32::RED,
            };

            if ui.button(RichText::new("SUBMIT").color(submit_color).size(18.0).strong())
                .clicked() {
                event = Some(Event::SubmitOrder);
            }

            ui.add_space(10.0);

            // Withdraw/Deposit buttons
            if ui.button(RichText::new("Withdraw").color(Color32::YELLOW)).clicked() {
                event = Some(Event::WithdrawFunds);
            }

            if ui.button(RichText::new("Deposit").color(Color32::YELLOW)).clicked() {
                event = Some(Event::DepositFunds);
            }
        });

    event
}

/// Render positions list
pub fn render_positions(ui: &mut egui::Ui, view_model: &ViewModel) -> Option<Event> {
    egui::Frame::none()
        .fill(Color32::from_rgb(20, 25, 25))
        .show(ui, |ui| {
            ui.heading("Positions");

            if view_model.positions.is_empty() {
                ui.label(RichText::new("No positions").color(Color32::GRAY));
            } else {
                for pos in &view_model.positions {
                    ui.horizontal(|ui| {
                        let side_color = match pos.side {
                            Side::Buy => Color32::GREEN,
                            Side::Sell => Color32::RED,
                        };

                        ui.label(RichText::new(format!("{:?}", pos.side)).color(side_color));
                        ui.label(format!("{} @ ${}", pos.size, pos.price));
                        ui.label(format!("({:?})", pos.status));
                    });
                }
            }
        });

    None
}
