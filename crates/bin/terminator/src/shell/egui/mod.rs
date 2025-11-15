//! egui Shell - Native GUI implementation

use anyhow::Result;
use eframe::egui;
use std::sync::{Arc, Mutex};

use crate::core::{AppCore, Event, Effect, ViewModel};
use crate::network::penumbra::grpc_client::PenumbraGrpcClient;
use crate::wallet::Wallet;

mod executor;
mod widgets;

use executor::EffectExecutor;
use widgets::*;

/// egui Shell - manages native GUI and core interaction
pub struct EguiShell {
    /// Core business logic (shared with executor thread)
    core: Arc<Mutex<AppCore>>,

    /// Effect executor
    executor: Arc<Mutex<EffectExecutor>>,

    /// Pending effects to execute
    pending_effects: Arc<Mutex<Vec<Effect>>>,
}

impl EguiShell {
    /// Create new egui shell
    pub async fn new() -> Result<Self> {
        let core = Arc::new(Mutex::new(AppCore::new()));

        // Try to load wallet
        let wallet = Wallet::load().await.ok();

        // Create executor
        let executor = Arc::new(Mutex::new(EffectExecutor::new(wallet).await?));

        Ok(Self {
            core,
            executor,
            pending_effects: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Run the GUI
    pub fn run(self) -> Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1920.0, 1080.0])
                .with_title("Terminator - Penumbra Trading Terminal"),
            ..Default::default()
        };

        eframe::run_native(
            "Terminator",
            options,
            Box::new(|_cc| Ok(Box::new(TerminatorApp::new(self)))),
        ).map_err(|e| anyhow::anyhow!("eframe error: {}", e))?;

        Ok(())
    }
}

/// Main egui application
struct TerminatorApp {
    shell: EguiShell,
}

impl TerminatorApp {
    fn new(shell: EguiShell) -> Self {
        Self { shell }
    }

    /// Handle an event and execute effects
    fn handle_event(&mut self, event: Event) {
        let mut core = self.shell.core.lock().unwrap();
        let effects = core.update(event);
        drop(core);

        // Queue effects for execution
        let mut pending = self.shell.pending_effects.lock().unwrap();
        pending.extend(effects);
    }
}

impl eframe::App for TerminatorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Get current view model
        let view_model = {
            let core = self.shell.core.lock().unwrap();
            core.view_model()
        };

        // Process any pending effects
        {
            let mut pending = self.shell.pending_effects.lock().unwrap();
            if !pending.is_empty() {
                let effects = pending.drain(..).collect::<Vec<_>>();
                drop(pending);

                let executor = self.shell.executor.clone();
                let core = self.shell.core.clone();

                // Simple synchronous effect handling for now
                // TODO: Properly handle async effects with a channel
                for effect in effects {
                    match effect {
                        Effect::Exit => std::process::exit(0),
                        Effect::Render(_) => {}, // egui handles rendering
                        _ => {} // Ignore other effects for now
                    }
                }
            }
        }

        // Main layout
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            render_header(ui, &view_model);
        });

        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            render_footer(ui, &view_model);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // 3-column layout: Chart | Spot Panel | Positions
            ui.horizontal(|ui| {
                // Chart (50%)
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() * 0.5);
                    if let Some(event) = render_chart(ui, &view_model) {
                        self.handle_event(event);
                    }
                });

                // Spot Panel (25%)
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() * 0.5);
                    if let Some(event) = render_spot_panel(ui, &view_model) {
                        self.handle_event(event);
                    }
                });

                // Positions (25%)
                ui.vertical(|ui| {
                    if let Some(event) = render_positions(ui, &view_model) {
                        self.handle_event(event);
                    }
                });
            });
        });

        // Request continuous repaints for live data
        ctx.request_repaint();
    }
}
