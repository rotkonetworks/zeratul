use anyhow::Result;
use eframe::egui;
use tracing::info;

mod app;
mod client;
mod verifier;
mod scanner;
mod storage;
mod sync;
mod core;
mod ui;

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
            // configure minimalist japanese styling
            configure_style(&cc.egui_ctx);
            Box::new(Zafu::new(cc))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {}", e))?;

    Ok(())
}

/// configure minimalist japanese aesthetic
fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // wabi-sabi: simplicity, asymmetry, natural imperfection
    // color palette: shiro (white), sumi (charcoal), beige, light grey
    use egui::{Color32, Rounding, Stroke, Vec2};

    // spacing: ma (negative space / emptiness)
    style.spacing.item_spacing = Vec2::new(16.0, 12.0);
    style.spacing.window_margin = egui::Margin::same(24.0);
    style.spacing.button_padding = Vec2::new(16.0, 8.0);
    style.spacing.indent = 20.0;

    // minimal rounding
    style.visuals.window_rounding = Rounding::same(2.0);
    style.visuals.widgets.noninteractive.rounding = Rounding::same(1.0);
    style.visuals.widgets.inactive.rounding = Rounding::same(1.0);
    style.visuals.widgets.hovered.rounding = Rounding::same(1.0);
    style.visuals.widgets.active.rounding = Rounding::same(1.0);

    // subtle strokes
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(0.5, Color32::from_gray(220));
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(0.5, Color32::from_gray(200));

    // natural color palette
    let bg = Color32::from_rgb(250, 248, 245);  // warm white (shiro)
    let text = Color32::from_rgb(50, 50, 50);   // charcoal (sumi)
    let accent = Color32::from_rgb(180, 170, 160); // beige

    style.visuals.window_fill = bg;
    style.visuals.panel_fill = bg;
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(245, 243, 240);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(240, 238, 235);
    style.visuals.widgets.hovered.bg_fill = accent;

    style.visuals.override_text_color = Some(text);

    ctx.set_style(style);
}

// generated proto module
pub mod zidecar {
    tonic::include_proto!("zidecar.v1");
}
