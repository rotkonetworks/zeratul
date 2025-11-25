use anyhow::Result;
use eframe::egui;
use tracing::info;

mod app;
mod client;
mod scanner;
mod storage;
mod sync;
mod core;
mod ui;
mod crypto;
mod account;
mod contacts;
mod chat;
mod tx_builder;
mod tx_history;
mod wormhole;

use app::Zafu;

fn main() -> Result<()> {
    // initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zafu=info".into()),
        )
        .init();

    info!("starting zafu - zen meditation cushion wallet");

    // configure egui with minimalist japanese aesthetic
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_title("zafu - 座布"),
        ..Default::default()
    };

    // run app
    eframe::run_native(
        "zafu",
        options,
        Box::new(|cc| {
            // configure minimalist styling
            configure_style(&cc.egui_ctx);
            Ok(Box::new(Zafu::new(cc)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {}", e))?;

    Ok(())
}

/// configure minimalist japanese aesthetic
fn configure_style(ctx: &egui::Context) {
    use egui::{Color32, FontFamily, FontId, Rounding, Stroke, Vec2, TextStyle};

    let mut style = (*ctx.style()).clone();

    // dark calming palette: evening zen garden
    let bg = Color32::from_rgb(28, 28, 30);          // deep charcoal
    let surface = Color32::from_rgb(38, 38, 40);     // slightly lighter charcoal
    let border = Color32::from_rgb(58, 58, 60);      // subtle border
    let text_primary = Color32::from_rgb(235, 235, 240);   // soft white
    let text_secondary = Color32::from_rgb(152, 152, 157); // muted gray
    let accent = Color32::from_rgb(142, 142, 147);   // warm gray
    let accent_hover = Color32::from_rgb(174, 174, 178); // lighter gray

    // generous spacing (ma - negative space)
    style.spacing.item_spacing = Vec2::new(12.0, 10.0);
    style.spacing.window_margin = egui::Margin::same(40.0);
    style.spacing.button_padding = Vec2::new(24.0, 10.0);
    style.spacing.indent = 16.0;
    style.spacing.interact_size = Vec2::new(48.0, 32.0);

    // clean geometry - no rounding for pure minimalism
    style.visuals.window_rounding = Rounding::ZERO;
    style.visuals.widgets.noninteractive.rounding = Rounding::ZERO;
    style.visuals.widgets.inactive.rounding = Rounding::ZERO;
    style.visuals.widgets.hovered.rounding = Rounding::ZERO;
    style.visuals.widgets.active.rounding = Rounding::ZERO;

    // refined strokes
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.5, accent);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.5, accent_hover);

    // backgrounds
    style.visuals.window_fill = bg;
    style.visuals.panel_fill = bg;
    style.visuals.widgets.noninteractive.bg_fill = surface;
    style.visuals.widgets.inactive.bg_fill = surface;
    style.visuals.widgets.hovered.bg_fill = accent;
    style.visuals.widgets.active.bg_fill = accent_hover;

    // text colors
    style.visuals.override_text_color = Some(text_primary);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text_secondary);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text_primary);
    style.visuals.selection.bg_fill = accent.linear_multiply(0.3);

    // typography - clean sans-serif
    let mut fonts = egui::FontDefinitions::default();

    // add phosphor icons as fallback font
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Bold);

    style.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
    ]
    .into();

    ctx.set_fonts(fonts);
    ctx.set_style(style);
}

// re-export proto types from zync-core
pub use zync_core::client::zidecar_proto as zidecar;
pub use zync_core::client::lightwalletd_proto as lightwalletd;
