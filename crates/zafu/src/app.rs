//! main egui application state

use eframe::egui;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{
    client::ZidecarClient,
    storage::WalletStorage,
    verifier::ProofVerifier,
    scanner::WalletScanner,
    sync::{SyncOrchestrator, SyncProgress, SyncPhase},
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum AppState {
    Setup,
    Syncing,
    Ready,
    Error,
}

pub struct Zafu {
    state: AppState,
    status_message: String,

    // wallet components
    client: Option<Arc<RwLock<ZidecarClient>>>,
    storage: Option<Arc<WalletStorage>>,
    verifier: Option<Arc<ProofVerifier>>,
    scanner: Option<Arc<RwLock<WalletScanner>>>,
    orchestrator: Option<Arc<SyncOrchestrator>>,

    // sync state
    sync_progress: Option<SyncProgress>,

    // UI state
    server_url: String,
    wallet_path: String,

    // balance
    balance: u64, // zatoshis

    // runtime
    runtime: tokio::runtime::Runtime,
}

impl Zafu {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

        Self {
            state: AppState::Setup,
            status_message: "".into(),
            client: None,
            storage: None,
            verifier: None,
            scanner: None,
            orchestrator: None,
            sync_progress: None,
            server_url: "http://127.0.0.1:50051".into(),
            wallet_path: "./zafu.db".into(),
            balance: 0,
            runtime,
        }
    }

    fn render_setup(&mut self, ui: &mut egui::Ui) {
        // minimalist japanese layout
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);

            // zen circle (ensō) symbol
            ui.label(egui::RichText::new("○").size(48.0).color(egui::Color32::from_gray(100)));
            ui.add_space(20.0);
            ui.label(egui::RichText::new("zafu").size(24.0));
            ui.add_space(40.0);
        });

        ui.add_space(20.0);
        ui.label("server");
        ui.text_edit_singleline(&mut self.server_url);
        ui.add_space(16.0);

        ui.label("path");
        ui.text_edit_singleline(&mut self.wallet_path);
        ui.add_space(32.0);

        ui.vertical_centered(|ui| {
            if ui.button("connect").clicked() {
                self.start_sync();
            }
        });
    }

    fn render_syncing(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.label(egui::RichText::new("⊚").size(48.0).color(egui::Color32::from_gray(100)));
            ui.add_space(20.0);
        });

        if let Some(ref progress) = self.sync_progress {
            ui.label(&progress.message);
            ui.add_space(10.0);

            ui.add(egui::ProgressBar::new(progress.progress).show_percentage());

            if progress.current_height > 0 {
                ui.label(format!("height: {}", progress.current_height));
            }

            // check if sync is complete
            if matches!(progress.phase, SyncPhase::Complete) {
                self.state = AppState::Ready;
                self.balance = self.get_balance();
            } else if matches!(progress.phase, SyncPhase::Error) {
                self.state = AppState::Error;
                self.status_message = progress.message.clone();
            }
        } else {
            ui.label(&self.status_message);
        }
    }

    fn render_ready(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);

            // zen circle (ensō) filled
            ui.label(egui::RichText::new("●").size(48.0).color(egui::Color32::from_gray(100)));
            ui.add_space(20.0);

            // balance in large text
            ui.label(egui::RichText::new(format_balance(self.balance))
                .size(32.0)
                .color(egui::Color32::from_gray(60)));
            ui.add_space(40.0);
        });

        let synced_height = if let Some(ref progress) = self.sync_progress {
            progress.current_height
        } else {
            0
        };

        ui.separator();
        ui.add_space(16.0);

        // minimal info display
        ui.horizontal(|ui| {
            ui.label("height");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{}", synced_height));
            });
        });

        ui.add_space(32.0);
        ui.vertical_centered(|ui| {
            if ui.button("sync").clicked() {
                self.start_sync();
            }
        });
    }

    fn render_error(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);

            // broken circle
            ui.label(egui::RichText::new("⊘").size(48.0).color(egui::Color32::from_rgb(180, 100, 100)));
            ui.add_space(20.0);

            ui.label(&self.status_message);
            ui.add_space(40.0);

            if ui.button("retry").clicked() {
                self.state = AppState::Setup;
                self.status_message = "".into();
            }
        });
    }

    fn start_sync(&mut self) {
        self.state = AppState::Syncing;
        self.status_message = "connecting to zidecar...".into();
        self.sync_progress = None;

        // initialize components if needed
        if self.orchestrator.is_none() {
            if let Err(e) = self.init_components() {
                self.state = AppState::Error;
                self.status_message = format!("failed to initialize: {}", e);
                return;
            }
        }

        // spawn sync task
        if let Some(orchestrator) = self.orchestrator.clone() {
            self.runtime.spawn(async move {
                match orchestrator.sync().await {
                    Ok(progress) => {
                        tracing::info!("sync completed: {:?}", progress);
                    }
                    Err(e) => {
                        tracing::error!("sync failed: {}", e);
                    }
                }
            });
        }
    }

    fn init_components(&mut self) -> anyhow::Result<()> {
        use tracing::info;

        info!("initializing wallet components");

        // connect to zidecar
        let client = self.runtime.block_on(async {
            ZidecarClient::connect(&self.server_url).await
        })?;
        let client = Arc::new(RwLock::new(client));

        // open storage
        let storage = Arc::new(WalletStorage::open(&self.wallet_path)?);

        // create verifier
        let verifier = Arc::new(ProofVerifier::new());

        // create scanner (TODO: actual ivk)
        let scanner = Arc::new(RwLock::new(WalletScanner::new(None)));

        // create orchestrator
        let orchestrator = Arc::new(SyncOrchestrator::new(
            client.clone(),
            verifier.clone(),
            scanner.clone(),
            storage.clone(),
        ));

        self.client = Some(client);
        self.storage = Some(storage);
        self.verifier = Some(verifier);
        self.scanner = Some(scanner);
        self.orchestrator = Some(orchestrator);

        info!("components initialized");
        Ok(())
    }

    fn get_balance(&self) -> u64 {
        if let Some(ref scanner) = self.scanner {
            if let Ok(scanner) = scanner.try_read() {
                return scanner.balance();
            }
        }
        0
    }
}

impl eframe::App for Zafu {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // poll for sync progress updates
        if matches!(self.state, AppState::Syncing) {
            if let Some(ref _orchestrator) = self.orchestrator {
                // check if sync task updated progress
                // (in real impl, would use channels or shared state)
                ctx.request_repaint();
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.state {
                AppState::Setup => self.render_setup(ui),
                AppState::Syncing => self.render_syncing(ui),
                AppState::Ready => self.render_ready(ui),
                AppState::Error => self.render_error(ui),
            }
        });
    }
}

fn format_balance(zatoshis: u64) -> String {
    let zec = zatoshis as f64 / 100_000_000.0;
    format!("{:.8} ZEC", zec)
}
