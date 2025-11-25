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
    zidecar::SyncStatus,
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
    blockchain_sync_status: Option<SyncStatus>,

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
            blockchain_sync_status: None,
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
            ui.label(egui::RichText::new("○").size(48.0).color(egui::Color32::from_gray(160)));
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
            ui.add_space(40.0);
            ui.label(egui::RichText::new("⊚").size(42.0).color(egui::Color32::from_gray(170)));
            ui.add_space(16.0);
            ui.label(egui::RichText::new("syncing").size(18.0));
            ui.add_space(30.0);
        });

        if let Some(ref progress) = self.sync_progress {
            // status message
            ui.label(egui::RichText::new(&progress.message).size(13.0));
            ui.add_space(12.0);

            // progress bar
            ui.add(egui::ProgressBar::new(progress.progress).show_percentage());
            ui.add_space(16.0);

            // detailed info grid
            egui::Grid::new("sync_info")
                .num_columns(2)
                .spacing([40.0, 8.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("phase").size(11.0).color(egui::Color32::from_gray(120)));
                    ui.label(format!("{:?}", progress.phase));
                    ui.end_row();

                    if progress.current_height > 0 {
                        ui.label(egui::RichText::new("height").size(11.0).color(egui::Color32::from_gray(120)));
                        ui.label(format!("{}", progress.current_height));
                        ui.end_row();
                    }

                    ui.label(egui::RichText::new("progress").size(11.0).color(egui::Color32::from_gray(120)));
                    ui.label(format!("{:.1}%", progress.progress * 100.0));
                    ui.end_row();

                    ui.label(egui::RichText::new("server").size(11.0).color(egui::Color32::from_gray(120)));
                    ui.label(&self.server_url);
                    ui.end_row();
                });

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
            ui.label(egui::RichText::new("●").size(48.0).color(egui::Color32::from_gray(160)));
            ui.add_space(20.0);

            // balance in large text
            ui.label(egui::RichText::new(format_balance(self.balance))
                .size(32.0)
                .color(egui::Color32::from_gray(190)));
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
        // fetch blockchain sync status if we have a client
        if self.blockchain_sync_status.is_none() && self.client.is_some() {
            if let Some(client) = &self.client {
                let client = Arc::clone(client);
                let status = self.runtime.block_on(async move {
                    let mut c = client.write().await;
                    c.get_sync_status().await.ok()
                });
                self.blockchain_sync_status = status;
            }
        }

        ui.vertical_centered(|ui| {
            ui.add_space(40.0);

            // error symbol
            ui.label(egui::RichText::new("⊚").size(42.0).color(egui::Color32::from_rgb(220, 180, 100)));
            ui.add_space(16.0);
            ui.label(egui::RichText::new("waiting for blockchain sync").size(16.0).color(egui::Color32::from_rgb(220, 180, 100)));
            ui.add_space(30.0);
        });

        // show blockchain sync status with progress bar
        if let Some(ref status) = self.blockchain_sync_status {
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(38, 38, 40))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 58, 60)))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    // epoch progress bar (japanese aesthetic)
                    let epoch_progress = status.blocks_in_epoch as f32 / 1024.0;
                    let bar_width = 300.0;
                    let bar_height = 8.0;

                    ui.label(egui::RichText::new("epoch progress").size(11.0).color(egui::Color32::from_gray(180)));
                    ui.add_space(8.0);

                    // draw progress bar
                    let (rect, _response) = ui.allocate_exact_size(
                        egui::vec2(bar_width, bar_height),
                        egui::Sense::hover(),
                    );

                    let progress_color = if status.gigaproof_status() == crate::zidecar::sync_status::GigaproofStatus::Ready {
                        egui::Color32::from_rgb(140, 180, 120) // green when ready
                    } else {
                        egui::Color32::from_rgb(220, 180, 100) // warm yellow/gold
                    };

                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        egui::Color32::from_rgb(58, 58, 60), // dark background
                    );

                    let progress_rect = egui::Rect::from_min_size(
                        rect.min,
                        egui::vec2(bar_width * epoch_progress, bar_height),
                    );

                    ui.painter().rect_filled(
                        progress_rect,
                        0.0,
                        progress_color,
                    );

                    ui.add_space(12.0);

                    // detailed sync info grid
                    egui::Grid::new("blockchain_sync_live")
                        .num_columns(2)
                        .spacing([40.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("blockchain height").size(11.0).color(egui::Color32::from_gray(152)));
                            ui.label(format!("{}", status.current_height));
                            ui.end_row();

                            ui.label(egui::RichText::new("current epoch").size(11.0).color(egui::Color32::from_gray(152)));
                            ui.label(format!("epoch {}", status.current_epoch));
                            ui.end_row();

                            ui.label(egui::RichText::new("blocks in epoch").size(11.0).color(egui::Color32::from_gray(152)));
                            ui.label(format!("{} / 1024", status.blocks_in_epoch));
                            ui.end_row();

                            ui.label(egui::RichText::new("complete epochs").size(11.0).color(egui::Color32::from_gray(152)));
                            ui.label(format!("{}", status.complete_epochs));
                            ui.end_row();

                            ui.label(egui::RichText::new("gigaproof status").size(11.0).color(egui::Color32::from_gray(152)));
                            let status_text = match status.gigaproof_status() {
                                crate::zidecar::sync_status::GigaproofStatus::WaitingForEpoch => {
                                    format!("waiting ({} blocks remaining)", status.blocks_until_ready)
                                }
                                crate::zidecar::sync_status::GigaproofStatus::Generating => "generating...".to_string(),
                                crate::zidecar::sync_status::GigaproofStatus::Ready => "ready".to_string(),
                            };
                            ui.label(status_text);
                            ui.end_row();
                        });

                    ui.add_space(12.0);
                    ui.label(egui::RichText::new("The server needs at least one complete epoch (1024 blocks) to generate the first gigaproof.").size(10.0).color(egui::Color32::from_gray(170)));
                });
        } else {
            // fallback error display
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(42, 35, 35))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 60, 60)))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("details").size(11.0).color(egui::Color32::from_gray(150)));
                    ui.add_space(8.0);
                    ui.label(&self.status_message);
                });
        }

        ui.add_space(24.0);

        ui.vertical_centered(|ui| {
            if ui.button("retry").clicked() {
                self.blockchain_sync_status = None; // clear cached status
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
