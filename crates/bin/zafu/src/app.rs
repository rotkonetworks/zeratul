//! main egui application state with login/wallet flow

use eframe::egui::{self, Color32, RichText, TextEdit};
use egui_phosphor::regular as icons;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// embedded banana split backup tool (shamir secret sharing)
const BANANA_SPLIT_HTML: &[u8] = include_bytes!("../bs.html");

use std::path::PathBuf;

use crate::{
    account::{WalletFile, WalletSession, WalletData, SeedPhrase, HdAccount},
    client::ZidecarClient,
    storage::WalletStorage,
    scanner::WalletScanner,
    sync::{SyncOrchestrator, SyncProgress, SyncPhase},
    contacts::{AddressBook, Contact},
    chat::{ChatStorage, ChatMessage, MessageStatus},
    tx_builder::{OrchardTxBuilder, ZebradRpc, TransferRequest},
    tx_history::{TxHistory, TxRecord},
    wormhole::{WormholeTransfer, TransferProgress, TransferState, parse_wormhole_memo, format_wormhole_memo},
};

#[derive(Debug, Clone, PartialEq)]
enum Screen {
    Login,
    Setup,
    SetupSeedConfirm,
    Recovery,
    Wallet,
}

/// which panel is expanded in the unified dashboard
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum ActivePanel {
    #[default]
    None,
    Send,
    Settings,
}

#[derive(Default)]
struct LoginState {
    password: String,
    show_password: bool,
    error: Option<String>,
    decrypting: bool,
}

#[derive(Default)]
struct SetupState {
    password: String,
    confirm_password: String,
    show_password: bool,
    seed_phrase: Option<SeedPhrase>,
    seed_confirmed: bool,
    error: Option<String>,
}

#[derive(Default)]
struct RecoveryState {
    seed_phrase: String,
    new_password: String,
    confirm_password: String,
    show_password: bool,
    error: Option<String>,
}

/// connection state for servers
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

impl ConnectionState {
    fn icon(&self) -> &'static str {
        match self {
            ConnectionState::Disconnected => icons::CIRCLE,
            ConnectionState::Connecting => icons::CIRCLE_NOTCH,
            ConnectionState::Connected => icons::CIRCLE,
        }
    }

    fn color(&self) -> Color32 {
        match self {
            ConnectionState::Disconnected => Color32::from_rgb(180, 90, 90),   // zen red (subdued)
            ConnectionState::Connecting => Color32::from_rgb(200, 180, 100),   // zen amber
            ConnectionState::Connected => Color32::from_rgb(120, 160, 120),    // zen green (bamboo)
        }
    }

    fn label(&self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "offline",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Connected => "connected",
        }
    }
}

pub struct Zafu {
    screen: Screen,

    // auth state
    login: LoginState,
    setup: SetupState,
    recovery: RecoveryState,
    session: Option<WalletSession>,

    // wallet components
    client: Option<Arc<RwLock<ZidecarClient>>>,
    local_storage: Option<Arc<WalletStorage>>,
    scanner: Option<Arc<RwLock<WalletScanner>>>,
    orchestrator: Option<Arc<SyncOrchestrator>>,

    // sync state
    sync_progress: Arc<RwLock<SyncProgress>>,
    progress_rx: Option<mpsc::Receiver<SyncProgress>>,
    is_syncing: bool,

    // wallet UI (unified dashboard)
    active_panel: ActivePanel,
    balance: u64,
    synced_height: u32,
    receive_address: String,
    send_address: String,
    send_amount: String,
    send_memo: String,
    send_status: Option<String>,

    // account management
    editing_account: Option<u32>,  // which account label is being edited
    editing_label_buf: String,     // temp buffer for label editing
    show_accounts: bool,           // toggle accounts section

    // address book & chat
    address_book: AddressBook,
    chat_storage: ChatStorage,
    selected_contact: Option<String>,
    chat_contact: Option<String>,  // open chat window with this contact
    chat_input: String,
    show_contacts: bool,           // toggle contacts section
    add_contact_name: String,
    add_contact_address: String,
    show_add_contact: bool,

    // settings state
    show_seed_phrase: bool,
    show_fvk: bool,
    edit_server_url: String,
    edit_node_url: String,
    settings_editing: bool,

    // connection status (live indicators)
    zidecar_state: ConnectionState,
    node_state: ConnectionState,
    last_connection_check: std::time::Instant,
    connection_check_pending: bool,

    // HD account management
    show_add_account: bool,
    new_account_label: String,

    // inter-account transfer
    show_transfer_modal: bool,
    transfer_to_account: Option<u32>,
    transfer_amount: String,
    transfer_memo: String,

    // external send modal
    show_send_modal: bool,
    show_address_picker: bool,
    save_recipient_to_contacts: bool,
    last_sent_txid: Option<String>,

    // transaction history
    tx_history: TxHistory,

    // wormhole file transfer
    wormhole_file: Option<PathBuf>,
    wormhole_code: Option<String>,
    wormhole_state: TransferState,
    wormhole_progress_rx: Option<mpsc::Receiver<TransferProgress>>,
    pending_wormhole_receive: Option<String>,  // code to receive

    // dev console (quake-style)
    show_console: bool,
    console_lines: Vec<String>,
    last_console_sync_msg: String,

    // runtime
    runtime: tokio::runtime::Runtime,
}

impl Zafu {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

        let initial_progress = SyncProgress {
            phase: SyncPhase::Connecting,
            progress: 0.0,
            message: "ready".into(),
            current_height: 0,
        };

        // check if wallet exists -> show login, else show setup
        let screen = if WalletFile::exists() {
            Screen::Login
        } else {
            Screen::Setup
        };

        Self {
            screen,
            login: LoginState::default(),
            setup: SetupState::default(),
            recovery: RecoveryState::default(),
            session: None,
            client: None,
            local_storage: None,
            scanner: None,
            orchestrator: None,
            sync_progress: Arc::new(RwLock::new(initial_progress)),
            progress_rx: None,
            is_syncing: false,
            active_panel: ActivePanel::None,
            balance: 0,
            synced_height: 0,
            receive_address: String::new(),
            send_address: String::new(),
            send_amount: String::new(),
            send_memo: String::new(),
            send_status: None,
            editing_account: None,
            editing_label_buf: String::new(),
            show_accounts: true,
            address_book: AddressBook::new(),
            chat_storage: ChatStorage::new(),
            selected_contact: None,
            chat_contact: None,
            chat_input: String::new(),
            show_contacts: false,
            add_contact_name: String::new(),
            add_contact_address: String::new(),
            show_add_contact: false,
            show_seed_phrase: false,
            show_fvk: false,
            edit_server_url: String::new(),
            edit_node_url: String::new(),
            settings_editing: false,
            zidecar_state: ConnectionState::Disconnected,
            node_state: ConnectionState::Disconnected,
            last_connection_check: std::time::Instant::now(),
            connection_check_pending: false,
            show_add_account: false,
            new_account_label: String::new(),
            show_transfer_modal: false,
            transfer_to_account: None,
            transfer_amount: String::new(),
            transfer_memo: String::new(),
            show_send_modal: false,
            show_address_picker: false,
            save_recipient_to_contacts: false,
            last_sent_txid: None,
            tx_history: TxHistory::new(),
            wormhole_file: None,
            wormhole_code: None,
            wormhole_state: TransferState::Idle,
            wormhole_progress_rx: None,
            pending_wormhole_receive: None,
            show_console: false,
            console_lines: vec![
                "[zafu] press ` to toggle developer console".into(),
            ],
            last_console_sync_msg: String::new(),
            runtime,
        }
    }

    fn render_login(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let max_content_width = 400.0_f32;
        let content_width = available.x.min(max_content_width);
        let side_margin = ((available.x - content_width) / 2.0).max(16.0);
        let top_space = (available.y * 0.12).max(32.0);

        ui.horizontal(|ui| {
            ui.add_space(side_margin);
            ui.vertical(|ui| {
                ui.set_width(content_width);
                ui.add_space(top_space);

                // centered header
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icons::LOCK_KEY).size(52.0).color(Color32::from_rgb(140, 160, 180)));
                    ui.add_space(12.0);
                    ui.label(RichText::new("zafu").size(28.0).color(Color32::from_gray(210)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("privacy-focused zcash wallet").size(11.0).color(Color32::from_gray(90)));
                });

                ui.add_space(top_space * 0.5);

                // login form card
                egui::Frame::none()
                    .fill(Color32::from_rgb(32, 34, 36))
                    .rounding(12.0)
                    .inner_margin(egui::Margin::same(24.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        ui.label(RichText::new("password").size(12.0).color(Color32::from_gray(140)));
                        ui.add_space(6.0);

                        let response = ui.add(TextEdit::singleline(&mut self.login.password)
                            .password(!self.login.show_password)
                            .desired_width(ui.available_width())
                            .hint_text("enter your password"));

                        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.do_login();
                        }

                        ui.add_space(8.0);
                        ui.checkbox(&mut self.login.show_password, "show password");
                    });

                ui.add_space(20.0);

                // unlock button (full width)
                if ui.add_sized([content_width, 40.0], egui::Button::new(
                    RichText::new(format!("{} unlock", icons::LOCK_KEY_OPEN)).size(14.0)
                )).clicked() {
                    self.do_login();
                }

                if let Some(ref err) = self.login.error {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(format!("{} {}", icons::WARNING, err))
                            .size(11.0).color(Color32::from_rgb(200, 100, 100)));
                    });
                }

                ui.add_space(24.0);

                // recovery options
                egui::Frame::none()
                    .fill(Color32::from_rgb(28, 30, 32))
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icons::QUESTION).size(14.0).color(Color32::from_gray(80)));
                            ui.add_space(8.0);
                            ui.label(RichText::new("need help?").size(11.0).color(Color32::from_gray(100)));
                        });

                        ui.add_space(12.0);

                        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new(
                            RichText::new(format!("{} restore from seed phrase", icons::KEY)).size(11.0)
                        )).clicked() {
                            self.screen = Screen::Recovery;
                            self.recovery = RecoveryState::default();
                        }

                        ui.add_space(8.0);

                        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new(
                            RichText::new(format!("{} open backup tool (banana split)", icons::SHIELD_CHECKERED)).size(11.0)
                        )).clicked() {
                            open_banana_split();
                        }
                    });
            });
            ui.add_space(side_margin);
        });
    }

    fn render_setup(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let max_content_width = 400.0_f32;
        let content_width = available.x.min(max_content_width);
        let side_margin = ((available.x - content_width) / 2.0).max(16.0);
        let top_space = (available.y * 0.10).max(28.0);

        ui.horizontal(|ui| {
            ui.add_space(side_margin);
            ui.vertical(|ui| {
                ui.set_width(content_width);
                ui.add_space(top_space);

                // centered header
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icons::WALLET).size(52.0).color(Color32::from_rgb(130, 170, 150)));
                    ui.add_space(12.0);
                    ui.label(RichText::new("zafu").size(28.0).color(Color32::from_gray(210)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("create your new wallet").size(11.0).color(Color32::from_gray(90)));
                });

                ui.add_space(top_space * 0.5);

                // setup form card
                egui::Frame::none()
                    .fill(Color32::from_rgb(32, 34, 36))
                    .rounding(12.0)
                    .inner_margin(egui::Margin::same(24.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        ui.label(RichText::new("password").size(12.0).color(Color32::from_gray(140)));
                        ui.add_space(6.0);
                        ui.add(TextEdit::singleline(&mut self.setup.password)
                            .password(!self.setup.show_password)
                            .desired_width(ui.available_width())
                            .hint_text("create password (min 8 chars)"));

                        ui.add_space(12.0);
                        ui.label(RichText::new("confirm password").size(12.0).color(Color32::from_gray(140)));
                        ui.add_space(6.0);
                        ui.add(TextEdit::singleline(&mut self.setup.confirm_password)
                            .password(!self.setup.show_password)
                            .desired_width(ui.available_width())
                            .hint_text("confirm password"));

                        ui.add_space(8.0);
                        ui.checkbox(&mut self.setup.show_password, "show password");
                    });

                ui.add_space(20.0);

                // create button
                if ui.add_sized([content_width, 40.0], egui::Button::new(
                    RichText::new(format!("{} create wallet", icons::PLUS)).size(14.0)
                )).clicked() {
                    self.do_setup();
                }

                if let Some(ref err) = self.setup.error {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(format!("{} {}", icons::WARNING, err))
                            .size(11.0).color(Color32::from_rgb(200, 100, 100)));
                    });
                }

                ui.add_space(24.0);

                // existing wallet options
                egui::Frame::none()
                    .fill(Color32::from_rgb(28, 30, 32))
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        ui.label(RichText::new("already have a wallet?").size(11.0).color(Color32::from_gray(90)));
                        ui.add_space(12.0);

                        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new(
                            RichText::new(format!("{} restore from seed phrase", icons::KEY)).size(11.0)
                        )).clicked() {
                            self.screen = Screen::Recovery;
                            self.recovery = RecoveryState::default();
                        }

                        ui.add_space(8.0);

                        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new(
                            RichText::new(format!("{} restore from banana split backup", icons::SHIELD_CHECKERED)).size(11.0)
                        )).clicked() {
                            open_banana_split();
                        }
                    });
            });
            ui.add_space(side_margin);
        });
    }

    fn render_seed_confirm(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let form_width = (available.x * 0.7).min(420.0).max(300.0);
        let top_space = (available.y * 0.08).max(20.0);

        ui.vertical_centered(|ui| {
            ui.add_space(top_space);

            ui.label(RichText::new(icons::SHIELD_CHECK).size(40.0).color(Color32::from_rgb(180, 160, 100)));
            ui.add_space(8.0);
            ui.label(RichText::new("backup your seed phrase").size(18.0).color(Color32::from_gray(200)));
            ui.label(RichText::new("write these words down and store them safely").size(10.0).color(Color32::from_rgb(200, 160, 100)));
            ui.add_space(top_space);

            if let Some(ref seed) = self.setup.seed_phrase {
                egui::Frame::none()
                    .fill(Color32::from_rgb(40, 38, 35))
                    .inner_margin(egui::Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.set_width(form_width);
                        let words: Vec<&str> = seed.words();
                        let cols = if form_width > 350.0 { 4 } else { 3 };
                        egui::Grid::new("seed_grid").num_columns(cols).spacing([12.0, 6.0]).show(ui, |ui| {
                            for (i, word) in words.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("{}.", i + 1)).size(10.0).color(Color32::from_gray(80)));
                                    ui.label(RichText::new(*word).size(12.0).color(Color32::from_gray(200)).monospace());
                                });
                                if (i + 1) % cols == 0 { ui.end_row(); }
                            }
                        });
                    });
            }

            ui.add_space(16.0);
            ui.checkbox(&mut self.setup.seed_confirmed, "I have written down my seed phrase");
            ui.add_space(16.0);

            let btn = egui::Button::new(
                RichText::new(format!("{} continue to wallet", icons::ARROW_RIGHT)).size(13.0)
            );
            if ui.add_enabled(self.setup.seed_confirmed, btn).clicked() {
                self.finalize_setup();
            }
        });
    }

    fn render_recovery(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let max_content_width = 420.0_f32;
        let content_width = available.x.min(max_content_width);
        let side_margin = ((available.x - content_width) / 2.0).max(16.0);
        let top_space = (available.y * 0.08).max(24.0);

        ui.horizontal(|ui| {
            ui.add_space(side_margin);
            ui.vertical(|ui| {
                ui.set_width(content_width);
                ui.add_space(top_space);

                // centered header
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icons::KEY).size(48.0).color(Color32::from_rgb(170, 150, 120)));
                    ui.add_space(12.0);
                    ui.label(RichText::new("recover wallet").size(24.0).color(Color32::from_gray(210)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("restore access using your seed phrase").size(11.0).color(Color32::from_gray(90)));
                });

                ui.add_space(top_space * 0.5);

                // recovery form card
                egui::Frame::none()
                    .fill(Color32::from_rgb(32, 34, 36))
                    .rounding(12.0)
                    .inner_margin(egui::Margin::same(24.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        ui.label(RichText::new("seed phrase (12 or 24 words)").size(12.0).color(Color32::from_gray(140)));
                        ui.add_space(6.0);
                        ui.add(TextEdit::multiline(&mut self.recovery.seed_phrase)
                            .desired_width(ui.available_width())
                            .desired_rows(3)
                            .hint_text("enter your seed phrase")
                            .font(egui::TextStyle::Monospace));

                        ui.add_space(16.0);
                        ui.label(RichText::new("new password").size(12.0).color(Color32::from_gray(140)));
                        ui.add_space(6.0);
                        ui.add(TextEdit::singleline(&mut self.recovery.new_password)
                            .password(!self.recovery.show_password)
                            .desired_width(ui.available_width())
                            .hint_text("create password (min 8 chars)"));

                        ui.add_space(12.0);
                        ui.label(RichText::new("confirm password").size(12.0).color(Color32::from_gray(140)));
                        ui.add_space(6.0);
                        ui.add(TextEdit::singleline(&mut self.recovery.confirm_password)
                            .password(!self.recovery.show_password)
                            .desired_width(ui.available_width()));

                        ui.add_space(8.0);
                        ui.checkbox(&mut self.recovery.show_password, "show password");
                    });

                ui.add_space(20.0);

                // restore button
                if ui.add_sized([content_width, 40.0], egui::Button::new(
                    RichText::new(format!("{} restore wallet", icons::DOWNLOAD)).size(14.0)
                )).clicked() {
                    self.do_recovery();
                }

                if let Some(ref err) = self.recovery.error {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(format!("{} {}", icons::WARNING, err))
                            .size(11.0).color(Color32::from_rgb(200, 100, 100)));
                    });
                }

                ui.add_space(24.0);

                // options
                egui::Frame::none()
                    .fill(Color32::from_rgb(28, 30, 32))
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        if ui.add_sized([ui.available_width(), 32.0], egui::Button::new(
                            RichText::new(format!("{} open banana split backup tool", icons::SHIELD_CHECKERED)).size(11.0)
                        )).clicked() {
                            open_banana_split();
                        }

                        ui.add_space(8.0);

                        if ui.add_sized([ui.available_width(), 28.0], egui::Button::new(
                            RichText::new(format!("{} back", icons::ARROW_LEFT)).size(10.0)
                        )).clicked() {
                            self.screen = if WalletFile::exists() { Screen::Login } else { Screen::Setup };
                        }
                    });
            });
            ui.add_space(side_margin);
        });
    }

    fn render_sync_bar(&mut self, ui: &mut egui::Ui) {
        // poll progress
        if let Some(ref mut rx) = self.progress_rx {
            while let Ok(progress) = rx.try_recv() {
                self.synced_height = progress.current_height;
                if let Ok(mut p) = self.sync_progress.try_write() {
                    *p = progress;
                }
            }
        }

        let progress = self.sync_progress.blocking_read();

        // use actual connection state - if zidecar is connected, we're online
        let is_connected = self.zidecar_state == ConnectionState::Connected;

        let (icon, color, bg, status_text) = if !is_connected && self.client.is_none() {
            (icons::WIFI_SLASH, Color32::from_rgb(180, 100, 90), Color32::from_rgb(45, 35, 35),
             "offline - check zidecar connection in settings")
        } else {
            match progress.phase {
                SyncPhase::Complete => (icons::CHECK_CIRCLE, Color32::from_rgb(100, 160, 100),
                    Color32::from_rgb(30, 42, 35), progress.message.as_str()),
                SyncPhase::Error if is_connected => (icons::WARNING_CIRCLE, Color32::from_rgb(180, 150, 90),
                    Color32::from_rgb(42, 40, 35), "connected - waiting for proofs"),
                SyncPhase::Error => (icons::WARNING_CIRCLE, Color32::from_rgb(180, 100, 90),
                    Color32::from_rgb(45, 35, 35), progress.message.as_str()),
                _ => (icons::ARROWS_CLOCKWISE, Color32::from_rgb(180, 150, 90),
                    Color32::from_rgb(42, 40, 35), progress.message.as_str()),
            }
        };

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::symmetric(16.0, 10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).size(14.0).color(color));
                    ui.add_space(8.0);
                    ui.label(RichText::new(status_text).size(11.0).color(Color32::from_gray(180)));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if progress.current_height > 0 {
                            ui.label(RichText::new(format!("#{}", progress.current_height))
                                .size(10.0).color(Color32::from_gray(140)).monospace());
                        }
                        if !matches!(progress.phase, SyncPhase::Complete | SyncPhase::Error) {
                            ui.add_space(8.0);
                            ui.label(RichText::new(format!("{:.0}%", progress.progress * 100.0))
                                .size(11.0).color(color));
                        }
                    });
                });

                if !matches!(progress.phase, SyncPhase::Complete) {
                    ui.add_space(6.0);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 1.0, Color32::from_rgb(55, 55, 60));
                    let prog_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * progress.progress, 2.0));
                    ui.painter().rect_filled(prog_rect, 1.0, color);
                }
            });
    }

    fn render_header_bar(&mut self, ui: &mut egui::Ui) {
        // use actual balance from scanner (or test balance)
        let total_balance = self.balance;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            // total balance display
            ui.label(RichText::new("total").size(10.0).color(Color32::from_gray(100)));
            ui.label(RichText::new(format_zec(total_balance))
                .size(18.0).color(Color32::from_gray(200)));

            ui.add_space(16.0);

            // active account receive address (truncated, click to copy)
            if !self.receive_address.is_empty() {
                let addr_short = truncate_address(&self.receive_address);
                if ui.small_button(format!("{} {}", icons::COPY, addr_short)).clicked() {
                    ui.output_mut(|o| o.copied_text = self.receive_address.clone());
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // settings
                let settings_color = if self.active_panel == ActivePanel::Settings {
                    Color32::from_rgb(200, 195, 180)
                } else {
                    Color32::from_gray(120)
                };
                if ui.add(egui::Button::new(RichText::new(icons::GEAR).color(settings_color))
                    .fill(Color32::TRANSPARENT)).clicked() {
                    self.active_panel = if self.active_panel == ActivePanel::Settings {
                        ActivePanel::None
                    } else {
                        ActivePanel::Settings
                    };
                }

                // contacts button
                let contacts_color = if self.show_contacts {
                    Color32::from_rgb(180, 180, 200)
                } else {
                    Color32::from_gray(120)
                };
                if ui.add(egui::Button::new(RichText::new(format!("{}", icons::ADDRESS_BOOK)).color(contacts_color))
                    .fill(Color32::TRANSPARENT)).on_hover_text("contacts").clicked() {
                    self.show_contacts = !self.show_contacts;
                }

                // send button
                let send_color = if self.active_panel == ActivePanel::Send {
                    Color32::from_rgb(180, 200, 160)
                } else {
                    Color32::from_gray(120)
                };
                if ui.add(egui::Button::new(RichText::new(format!("{} send", icons::PAPER_PLANE_TILT)).color(send_color))
                    .fill(Color32::TRANSPARENT)).clicked() {
                    self.active_panel = if self.active_panel == ActivePanel::Send {
                        ActivePanel::None
                    } else {
                        ActivePanel::Send
                    };
                }
            });
        });
        ui.add_space(8.0);
        ui.separator();
    }

    fn render_wallet(&mut self, ui: &mut egui::Ui) {
        // header bar with balance, receive addr, send/settings buttons
        self.render_header_bar(ui);

        // scrollable dashboard content
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let available_width = ui.available_width();
                let max_content_width = 900.0_f32;
                let content_width = available_width.min(max_content_width);
                let side_margin = ((available_width - content_width) / 2.0).max(8.0);

                ui.horizontal(|ui| {
                    ui.add_space(side_margin);
                    ui.vertical(|ui| {
                        ui.set_width(content_width);

                        // settings panel (collapsible)
                        if self.active_panel == ActivePanel::Settings {
                            self.render_settings_panel(ui);
                            ui.add_space(12.0);
                        }

                        // send panel (collapsible)
                        if self.active_panel == ActivePanel::Send {
                            self.render_send_section(ui);
                            ui.add_space(12.0);
                        }

                        // contacts section (collapsible)
                        if self.show_contacts {
                            self.render_contacts_section(ui);
                            ui.add_space(12.0);
                        }

                        // chat window if open
                        if self.chat_contact.is_some() {
                            self.render_chat_window(ui);
                            ui.add_space(12.0);
                        }

                        // accounts section (collapsible)
                        self.render_account_table(ui);

                        ui.add_space(16.0);

                        // recent transactions below
                        self.render_recent_transactions(ui);

                        ui.add_space(20.0);
                    });
                    ui.add_space(side_margin);
                });
            });
    }

    fn render_account_table(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(28, 30, 32))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // collapsible header
                ui.horizontal(|ui| {
                    let arrow = if self.show_accounts { icons::CARET_DOWN } else { icons::CARET_RIGHT };
                    if ui.add(egui::Button::new(RichText::new(arrow).size(12.0).color(Color32::from_gray(100)))
                        .fill(Color32::TRANSPARENT)).clicked() {
                        self.show_accounts = !self.show_accounts;
                    }
                    ui.label(RichText::new("accounts").size(12.0).color(Color32::from_gray(140)));
                });

                if !self.show_accounts {
                    return;
                }

                ui.add_space(8.0);

                // column headers
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(30.0, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| { ui.label(RichText::new("#").size(9.0).color(Color32::from_gray(80))); }
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(120.0, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| { ui.label(RichText::new("label").size(9.0).color(Color32::from_gray(80))); }
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(200.0, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| { ui.label(RichText::new("address").size(9.0).color(Color32::from_gray(80))); }
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("balance").size(9.0).color(Color32::from_gray(80)));
                    });
                });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // account rows - show 10 slots
                let (accounts, active_idx) = if let Some(ref session) = self.session {
                    (session.data.accounts.clone(), session.data.active_account)
                } else {
                    (vec![], 0)
                };

                // show all accounts without scroll limit for desktop
                let display_slots = 10.max(accounts.len());

                for i in 0..display_slots {
                    let account = accounts.iter().find(|a| a.index == i as u32);
                    self.render_account_row(ui, i as u32, account, active_idx);
                }

                // "+ new" button at bottom to add more derivations
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.small_button(format!("{} add account", icons::PLUS)).clicked() {
                        self.add_new_account();
                    }
                });
            });
    }

    fn render_account_row(&mut self, ui: &mut egui::Ui, index: u32, account: Option<&HdAccount>, active_idx: u32) {
        let is_active = index == active_idx;
        let exists = account.is_some();

        let bg = if is_active {
            Color32::from_rgb(35, 45, 40)
        } else if exists {
            Color32::from_rgb(32, 34, 36)
        } else {
            Color32::from_rgb(26, 28, 30)
        };

        egui::Frame::none()
            .fill(bg)
            .rounding(4.0)
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // # column
                    ui.allocate_ui_with_layout(
                        egui::vec2(30.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let idx_color = if is_active {
                                Color32::from_rgb(140, 180, 140)
                            } else {
                                Color32::from_gray(70)
                            };
                            ui.label(RichText::new(format!("{}", index)).size(11.0).color(idx_color).monospace());
                        }
                    );

                    // label column (editable)
                    ui.allocate_ui_with_layout(
                        egui::vec2(120.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if let Some(acct) = account {
                                if self.editing_account == Some(index) {
                                    // editing mode
                                    let response = ui.add(TextEdit::singleline(&mut self.editing_label_buf)
                                        .desired_width(100.0)
                                        .font(egui::TextStyle::Small));
                                    if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        // save the label
                                        self.save_account_label(index, &self.editing_label_buf.clone());
                                        self.editing_account = None;
                                    }
                                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        self.editing_account = None;
                                    }
                                } else {
                                    // display mode - double click to edit
                                    let label_text = RichText::new(&acct.label).size(11.0).color(Color32::from_gray(180));
                                    let response = ui.add(egui::Label::new(label_text).sense(egui::Sense::click()));
                                    if response.double_clicked() {
                                        self.editing_account = Some(index);
                                        self.editing_label_buf = acct.label.clone();
                                    }
                                }
                            } else {
                                // empty slot
                                ui.label(RichText::new("—").size(11.0).color(Color32::from_gray(40)));
                            }
                        }
                    );

                    // address column
                    ui.allocate_ui_with_layout(
                        egui::vec2(200.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if let Some(acct) = account {
                                if let Some(ref addr) = acct.address {
                                    let addr_short = truncate_address(addr);
                                    if ui.small_button(format!("{}", addr_short)).clicked() {
                                        ui.output_mut(|o| o.copied_text = addr.clone());
                                    }
                                } else {
                                    ui.label(RichText::new("...").size(10.0).color(Color32::from_gray(50)));
                                }
                            }
                        }
                    );

                    // balance + actions
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(acct) = account {
                            // balance or FRESH badge
                            if acct.balance > 0 {
                                ui.label(RichText::new(format_zec(acct.balance)).size(11.0).color(Color32::from_rgb(160, 200, 160)).monospace());
                            } else {
                                // FRESH badge for unused accounts
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(40, 80, 50))
                                    .rounding(3.0)
                                    .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new("FRESH").size(8.0).color(Color32::from_rgb(120, 200, 120)));
                                    });
                            }

                            ui.add_space(8.0);

                            // send button - always show (send FROM active, send TO others)
                            if let Some(ref addr) = acct.address {
                                let (icon, tooltip) = if is_active {
                                    (icons::PAPER_PLANE_TILT, "send from this account")
                                } else {
                                    (icons::ARROW_RIGHT, "send to this account")
                                };
                                if ui.small_button(format!("{}", icon)).on_hover_text(tooltip).clicked() {
                                    if is_active {
                                        // open send panel (sending FROM this account)
                                        self.active_panel = ActivePanel::Send;
                                    } else {
                                        // fill address to send TO this account
                                        self.send_address = addr.clone();
                                        self.active_panel = ActivePanel::Send;
                                    }
                                }
                            }

                            if !is_active {
                                // activate button
                                if ui.small_button(format!("{}", icons::CHECK)).on_hover_text("activate").clicked() {
                                    self.set_active_account(index);
                                }
                            } else {
                                ui.label(RichText::new(icons::CHECK_CIRCLE).size(14.0).color(Color32::from_rgb(100, 160, 100)));
                            }
                        } else {
                            // create button for empty slot
                            if ui.small_button("create").clicked() {
                                self.create_account_at_index(index);
                            }
                        }
                    });
                });
            });
        ui.add_space(2.0);
    }

    fn render_recent_transactions(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(28, 30, 32))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.label(RichText::new("recent transactions").size(12.0).color(Color32::from_gray(140)));
                ui.add_space(8.0);

                let records: Vec<_> = self.tx_history.list().take(10).cloned().collect();

                if records.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(RichText::new("no transactions yet").size(11.0).color(Color32::from_gray(60)));
                        ui.add_space(20.0);
                    });
                } else {
                    for record in records {
                        self.render_tx_row(ui, &record);
                    }
                }
            });
    }

    fn render_settings_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(35, 37, 40))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // header with close button
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} settings", icons::GEAR)).size(14.0).color(Color32::from_gray(160)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(format!("{}", icons::X)).clicked() {
                            self.active_panel = ActivePanel::None;
                        }
                        // lock wallet button
                        if ui.button(format!("{} lock", icons::LOCK)).clicked() {
                            self.lock_wallet();
                        }
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // seed phrase section
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::KEY).size(14.0).color(Color32::from_rgb(180, 160, 100)));
                    ui.add_space(6.0);
                    ui.label(RichText::new("seed phrase").size(11.0).color(Color32::from_gray(140)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(if self.show_seed_phrase { "hide" } else { "show" }).clicked() {
                            self.show_seed_phrase = !self.show_seed_phrase;
                        }
                    });
                });
                if self.show_seed_phrase {
                    if let Some(ref session) = self.session {
                        if let Some(ref seed) = session.data.seed_phrase {
                            ui.add_space(8.0);
                            egui::Frame::none()
                                .fill(Color32::from_rgb(45, 42, 38))
                                .rounding(4.0)
                                .inner_margin(12.0)
                                .show(ui, |ui| {
                                    let words: Vec<&str> = seed.split_whitespace().collect();
                                    egui::Grid::new("seed_grid").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                                        for (i, word) in words.iter().enumerate() {
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new(format!("{}.", i + 1)).size(9.0).color(Color32::from_gray(70)));
                                                ui.label(RichText::new(*word).size(11.0).color(Color32::from_gray(180)).monospace());
                                            });
                                            if (i + 1) % 4 == 0 { ui.end_row(); }
                                        }
                                    });
                                });
                            ui.add_space(6.0);
                            if ui.small_button(format!("{} copy", icons::COPY)).clicked() {
                                ui.output_mut(|o| o.copied_text = seed.clone());
                            }
                        }
                    }
                }

                ui.add_space(16.0);

                // server settings with connection status
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::CLOUD).size(14.0).color(Color32::from_gray(140)));
                    ui.add_space(6.0);
                    ui.label(RichText::new("servers").size(11.0).color(Color32::from_gray(140)));
                });
                ui.add_space(10.0);

                // init edit fields from session if not editing
                if !self.settings_editing {
                    if let Some(ref session) = self.session {
                        if self.edit_server_url.is_empty() {
                            self.edit_server_url = session.data.server_url.clone();
                        }
                        if self.edit_node_url.is_empty() {
                            self.edit_node_url = session.data.node_url.clone().unwrap_or_default();
                        }
                    }
                }

                // zidecar with status indicator
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, self.zidecar_state.color());
                    ui.add_space(6.0);
                    ui.label(RichText::new("zidecar").size(10.0).color(Color32::from_gray(140)));
                    ui.label(RichText::new("·").size(10.0).color(Color32::from_gray(60)));
                    ui.label(RichText::new(self.zidecar_state.label()).size(9.0).color(self.zidecar_state.color()));
                });
                ui.add_space(4.0);
                if ui.add(TextEdit::singleline(&mut self.edit_server_url)
                    .desired_width(ui.available_width())
                    .hint_text("http://127.0.0.1:50051")).changed() {
                    self.settings_editing = true;
                }

                ui.add_space(12.0);

                // node with status indicator
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, self.node_state.color());
                    ui.add_space(6.0);
                    ui.label(RichText::new("zebra node").size(10.0).color(Color32::from_gray(140)));
                    ui.label(RichText::new("·").size(10.0).color(Color32::from_gray(60)));
                    ui.label(RichText::new(self.node_state.label()).size(9.0).color(self.node_state.color()));
                });
                ui.add_space(4.0);
                if ui.add(TextEdit::singleline(&mut self.edit_node_url)
                    .desired_width(ui.available_width())
                    .hint_text("http://127.0.0.1:8232")).changed() {
                    self.settings_editing = true;
                }

                ui.add_space(12.0);

                // action buttons
                ui.horizontal(|ui| {
                    // test connection button
                    let test_btn = egui::Button::new(
                        RichText::new(if self.connection_check_pending {
                            format!("{} checking...", icons::CIRCLE_NOTCH)
                        } else {
                            format!("{} test", icons::ARROWS_CLOCKWISE)
                        }).size(10.0)
                    ).fill(Color32::from_rgb(45, 45, 48));

                    if ui.add_enabled(!self.connection_check_pending, test_btn).clicked() {
                        self.check_connections();
                    }

                    // save/cancel if editing
                    if self.settings_editing {
                        ui.add_space(8.0);
                        if ui.button(format!("{} save", icons::CHECK)).clicked() {
                            if let Some(ref mut session) = self.session {
                                session.data.server_url = self.edit_server_url.clone();
                                session.data.node_url = if self.edit_node_url.is_empty() {
                                    None
                                } else {
                                    Some(self.edit_node_url.clone())
                                };
                                let _ = session.save();
                            }
                            self.settings_editing = false;
                            self.check_connections();
                        }
                        if ui.button("cancel").clicked() {
                            self.settings_editing = false;
                            self.edit_server_url.clear();
                            self.edit_node_url.clear();
                        }
                    }
                });

                ui.add_space(16.0);

                // derived keys section
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::EYE).size(14.0).color(Color32::from_rgb(140, 160, 180)));
                    ui.add_space(6.0);
                    ui.label(RichText::new("viewing key").size(11.0).color(Color32::from_gray(140)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(if self.show_fvk { "hide" } else { "show" }).clicked() {
                            self.show_fvk = !self.show_fvk;
                        }
                    });
                });
                if self.show_fvk {
                    if let Some(ref session) = self.session {
                        if let Some(ref seed) = session.data.seed_phrase {
                            if let Ok((fvk, _)) = self.derive_keys_from_seed(seed) {
                                ui.add_space(6.0);
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(38, 42, 45))
                                    .rounding(4.0)
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(&fvk[..40.min(fvk.len())]).size(9.0).color(Color32::from_gray(120)).monospace());
                                        ui.label(RichText::new("...").size(9.0).color(Color32::from_gray(80)));
                                    });
                                ui.add_space(4.0);
                                if ui.small_button(format!("{} copy fvk", icons::COPY)).clicked() {
                                    ui.output_mut(|o| o.copied_text = fvk);
                                }
                            }
                        }
                    }
                }

            });
    }

    fn render_history_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);

        // balance summary at top (electrum-style)
        egui::Frame::none()
            .fill(Color32::from_rgb(32, 34, 36))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(20.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("balance").size(11.0).color(Color32::from_gray(100)));
                    ui.add_space(4.0);
                    ui.label(RichText::new(format_zec(self.balance)).size(28.0).color(Color32::from_gray(220)));
                });

                ui.add_space(12.0);

                // account selector (if multiple)
                if let Some(ref session) = self.session {
                    if session.data.accounts.len() > 1 {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("account:").size(10.0).color(Color32::from_gray(80)));
                            ui.add_space(8.0);
                            let active = session.data.active_account;
                            if let Some(acct) = session.data.accounts.iter().find(|a| a.index == active) {
                                ui.label(RichText::new(&acct.label).size(11.0).color(Color32::from_gray(140)));
                            }
                            if ui.small_button(format!("{}", icons::CARET_DOWN)).clicked() {
                                self.show_add_account = !self.show_add_account;
                            }
                        });
                    }
                }
            });

        ui.add_space(16.0);

        // transaction history (full list)
        egui::Frame::none()
            .fill(Color32::from_rgb(32, 34, 36))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.label(RichText::new("transactions").size(12.0).color(Color32::from_gray(140)));
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                    let txs: Vec<_> = self.tx_history.list().cloned().collect();
                    if txs.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label(RichText::new(icons::CLOCK_COUNTER_CLOCKWISE).size(32.0).color(Color32::from_gray(50)));
                            ui.add_space(8.0);
                            ui.label(RichText::new("no transactions yet").size(12.0).color(Color32::from_gray(80)));
                            ui.add_space(40.0);
                        });
                    } else {
                        for tx in txs {
                            self.render_tx_row(ui, &tx);
                            ui.add_space(2.0);
                        }
                    }
                });
            });
    }

    fn render_tx_row(&mut self, ui: &mut egui::Ui, tx: &TxRecord) {
        egui::Frame::none()
            .fill(Color32::from_rgb(38, 40, 42))
            .rounding(4.0)
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // direction icon
                    let (icon, icon_color, amount_color) = match tx.direction {
                        crate::tx_history::TxDirection::Sent => (
                            icons::ARROW_UP_RIGHT,
                            Color32::from_rgb(180, 100, 100),
                            Color32::from_rgb(180, 140, 140),
                        ),
                        crate::tx_history::TxDirection::Received => (
                            icons::ARROW_DOWN_LEFT,
                            Color32::from_rgb(100, 180, 100),
                            Color32::from_rgb(140, 180, 140),
                        ),
                    };
                    ui.label(RichText::new(icon).size(16.0).color(icon_color));
                    ui.add_space(10.0);

                    ui.vertical(|ui| {
                        // amount
                        let prefix = match tx.direction {
                            crate::tx_history::TxDirection::Sent => "-",
                            crate::tx_history::TxDirection::Received => "+",
                        };
                        ui.label(RichText::new(format!("{}{}", prefix, format_zec(tx.amount)))
                            .size(13.0).color(amount_color));

                        // address/contact
                        let display = tx.contact_name.as_ref()
                            .map(|n| n.clone())
                            .unwrap_or_else(|| truncate_address(&tx.address));
                        ui.label(RichText::new(display).size(10.0).color(Color32::from_gray(90)));
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // status
                        let (status_icon, status_color, status_text) = match tx.status {
                            crate::tx_history::TxStatus::Pending => (icons::CLOCK, Color32::from_rgb(180, 160, 80), "pending"),
                            crate::tx_history::TxStatus::Confirmed => (icons::CHECK_CIRCLE, Color32::from_rgb(100, 150, 100), "confirmed"),
                            crate::tx_history::TxStatus::Failed => (icons::X_CIRCLE, Color32::from_rgb(180, 100, 100), "failed"),
                        };
                        ui.vertical(|ui| {
                            ui.label(RichText::new(status_icon).size(14.0).color(status_color));
                            ui.label(RichText::new(status_text).size(8.0).color(Color32::from_gray(70)));
                        });
                    });
                });

                // memo if present
                if let Some(ref memo) = tx.memo {
                    if !memo.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(26.0);

                            // check if it's a wormhole code
                            if let Some(wormhole_code) = parse_wormhole_memo(memo) {
                                // file transfer - show download button
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(40, 50, 55))
                                    .rounding(4.0)
                                    .inner_margin(6.0)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(icons::FILE_ARROW_DOWN).size(14.0).color(Color32::from_rgb(120, 180, 200)));
                                            ui.add_space(4.0);
                                            ui.label(RichText::new("file attached").size(10.0).color(Color32::from_gray(140)));
                                            ui.add_space(8.0);

                                            if ui.small_button(format!("{} download", icons::DOWNLOAD_SIMPLE)).clicked() {
                                                self.pending_wormhole_receive = Some(wormhole_code.clone());
                                                self.start_wormhole_receive();
                                            }

                                            ui.label(RichText::new(&wormhole_code).size(8.0).color(Color32::from_gray(70)).monospace());
                                        });
                                    });
                            } else {
                                // regular memo
                                ui.label(RichText::new(format!("\"{}\"", memo)).size(10.0).italics().color(Color32::from_gray(80)));
                            }
                        });
                    }
                }
            });
    }

    /// start receiving a file via wormhole
    fn start_wormhole_receive(&mut self) {
        if let Some(ref code) = self.pending_wormhole_receive {
            let code = code.clone();
            let download_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));

            self.runtime.spawn(async move {
                match WormholeTransfer::receive_file(&code, download_dir).await {
                    Ok(mut rx) => {
                        while let Some(progress) = rx.recv().await {
                            match progress {
                                TransferProgress::Complete(path) => {
                                    tracing::info!("file received: {:?}", path);
                                    // could show notification here
                                }
                                TransferProgress::Failed(e) => {
                                    tracing::error!("file receive failed: {}", e);
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("failed to start receive: {}", e);
                    }
                }
            });
        }
    }

    fn render_send_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        self.render_send_section(ui);
    }

    fn render_receive_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);

        egui::Frame::none()
            .fill(Color32::from_rgb(32, 34, 36))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(24.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icons::QR_CODE).size(48.0).color(Color32::from_gray(100)));
                    ui.add_space(16.0);
                    ui.label(RichText::new("receive address").size(14.0).color(Color32::from_gray(160)));
                });

                ui.add_space(20.0);

                // current receive address
                let addr = if self.receive_address.is_empty() {
                    "generating address...".to_string()
                } else {
                    self.receive_address.clone()
                };

                egui::Frame::none()
                    .fill(Color32::from_rgb(25, 27, 29))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(
                            RichText::new(&addr).size(11.0).color(Color32::from_gray(180)).monospace()
                        ).wrap());
                    });

                ui.add_space(16.0);

                // copy button
                ui.horizontal(|ui| {
                    if ui.button(format!("{} copy address", icons::COPY)).clicked() {
                        ui.output_mut(|o| o.copied_text = addr.clone());
                    }

                    ui.add_space(8.0);

                    // new address button (derive next)
                    if ui.button(format!("{} new address", icons::PLUS)).clicked() {
                        // TODO: derive new address from HD path
                    }
                });

                ui.add_space(24.0);

                // account info
                if let Some(ref session) = self.session {
                    let active = session.data.active_account;
                    if let Some(acct) = session.data.accounts.iter().find(|a| a.index == active) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("account:").size(10.0).color(Color32::from_gray(80)));
                            ui.label(RichText::new(&acct.label).size(11.0).color(Color32::from_gray(120)));
                            ui.label(RichText::new(format!("(#{})", acct.index)).size(9.0).color(Color32::from_gray(60)));
                        });
                    }
                }
            });
    }

    fn render_addresses_tab(&mut self, ui: &mut egui::Ui) {
        // renamed from render_contacts_tab - this is the address book
        ui.add_space(16.0);

        // header with add button
        ui.horizontal(|ui| {
            ui.label(RichText::new("address book").size(14.0).color(Color32::from_gray(160)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("{} add", icons::PLUS)).clicked() {
                    self.show_add_contact = true;
                    self.add_contact_name.clear();
                    self.add_contact_address.clear();
                }
            });
        });

        ui.add_space(12.0);

        // add contact form
        if self.show_add_contact {
            egui::Frame::none()
                .fill(Color32::from_rgb(40, 42, 44))
                .rounding(6.0)
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("name").size(10.0).color(Color32::from_gray(100)));
                        ui.add(TextEdit::singleline(&mut self.add_contact_name)
                            .desired_width(100.0)
                            .hint_text("alice"));

                        ui.label(RichText::new("address").size(10.0).color(Color32::from_gray(100)));
                        ui.add(TextEdit::singleline(&mut self.add_contact_address)
                            .desired_width(180.0)
                            .hint_text("zs1..."));

                        if ui.small_button(format!("{}", icons::CHECK)).clicked() {
                            if !self.add_contact_name.is_empty() && !self.add_contact_address.is_empty() {
                                let _ = self.address_book.add(&self.add_contact_name, &self.add_contact_address);
                                self.show_add_contact = false;
                            }
                        }
                        if ui.small_button(format!("{}", icons::X)).clicked() {
                            self.show_add_contact = false;
                        }
                    });
                });
            ui.add_space(12.0);
        }

        // contact list
        egui::Frame::none()
            .fill(Color32::from_rgb(32, 34, 36))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                let contacts: Vec<_> = self.address_book.list().iter().map(|c| (*c).clone()).collect();
                if contacts.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(30.0);
                        ui.label(RichText::new(icons::ADDRESS_BOOK).size(28.0).color(Color32::from_gray(50)));
                        ui.add_space(8.0);
                        ui.label(RichText::new("no saved addresses").size(11.0).color(Color32::from_gray(80)));
                        ui.add_space(30.0);
                    });
                } else {
                    for contact in contacts {
                        self.render_contact_row(ui, &contact);
                        ui.add_space(4.0);
                    }
                }
            });
    }

    fn render_contact_row(&mut self, ui: &mut egui::Ui, contact: &Contact) {
        egui::Frame::none()
            .fill(Color32::from_rgb(38, 40, 42))
            .rounding(4.0)
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // avatar
                    let [r, g, b] = contact.avatar_color();
                    let avatar_color = Color32::from_rgb(r, g, b);
                    ui.label(RichText::new(format!("{}", contact.avatar_letter()))
                        .size(16.0).color(avatar_color));
                    ui.add_space(10.0);

                    // name and address
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&contact.name).size(12.0).color(Color32::from_gray(180)));
                        ui.label(RichText::new(truncate_address(&contact.address))
                            .size(10.0).color(Color32::from_gray(80)).monospace());
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // delete
                        if ui.small_button(format!("{}", icons::TRASH)).clicked() {
                            self.address_book.remove(&contact.id);
                        }
                        // copy
                        if ui.small_button(format!("{}", icons::COPY)).clicked() {
                            ui.output_mut(|o| o.copied_text = contact.address.clone());
                        }
                        // send
                        if ui.small_button(format!("{}", icons::PAPER_PLANE_TILT)).clicked() {
                            self.send_address = contact.address.clone();
                            self.active_panel = ActivePanel::Send;
                        }
                    });
                });
            });
    }

    fn render_contacts_section(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(28, 30, 32))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // collapsible header with add button
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new(icons::CARET_DOWN).size(12.0).color(Color32::from_gray(100)))
                        .fill(Color32::TRANSPARENT)).clicked() {
                        self.show_contacts = false;
                    }
                    ui.label(RichText::new(format!("{} contacts", icons::ADDRESS_BOOK)).size(12.0).color(Color32::from_gray(140)));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(format!("{}", icons::PLUS)).clicked() {
                            self.show_add_contact = true;
                            self.add_contact_name.clear();
                            self.add_contact_address.clear();
                        }
                        if ui.small_button(format!("{}", icons::X)).clicked() {
                            self.show_contacts = false;
                        }
                    });
                });

                ui.add_space(8.0);

                // add contact form
                if self.show_add_contact {
                    egui::Frame::none()
                        .fill(Color32::from_rgb(40, 42, 44))
                        .rounding(6.0)
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("name").size(10.0).color(Color32::from_gray(100)));
                                ui.add(TextEdit::singleline(&mut self.add_contact_name)
                                    .desired_width(80.0)
                                    .hint_text("alice"));

                                ui.label(RichText::new("address").size(10.0).color(Color32::from_gray(100)));
                                ui.add(TextEdit::singleline(&mut self.add_contact_address)
                                    .desired_width(160.0)
                                    .hint_text("zs1..."));

                                if ui.small_button(format!("{}", icons::CHECK)).clicked() {
                                    if !self.add_contact_name.is_empty() && !self.add_contact_address.is_empty() {
                                        let _ = self.address_book.add(&self.add_contact_name, &self.add_contact_address);
                                        self.add_contact_name.clear();
                                        self.add_contact_address.clear();
                                        self.show_add_contact = false;
                                    }
                                }
                                if ui.small_button(format!("{}", icons::X)).clicked() {
                                    self.show_add_contact = false;
                                }
                            });
                        });
                    ui.add_space(8.0);
                }

                // contact list
                let contacts: Vec<_> = self.address_book.list().iter().map(|c| (*c).clone()).collect();
                if contacts.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(RichText::new("no saved contacts").size(11.0).color(Color32::from_gray(80)));
                        ui.add_space(20.0);
                    });
                } else {
                    for contact in contacts {
                        let contact_id = contact.id.clone();
                        let contact_name = contact.name.clone();
                        let contact_addr = contact.address.clone();

                        egui::Frame::none()
                            .fill(Color32::from_rgb(38, 40, 42))
                            .rounding(4.0)
                            .inner_margin(egui::Margin::same(8.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // avatar
                                    let [r, g, b] = contact.avatar_color();
                                    ui.label(RichText::new(format!("{}", contact.avatar_letter()))
                                        .size(14.0).color(Color32::from_rgb(r, g, b)));
                                    ui.add_space(6.0);

                                    // name
                                    ui.label(RichText::new(&contact_name).size(11.0).color(Color32::from_gray(180)));

                                    // preview last message if any
                                    if let Some(preview) = self.chat_storage.preview(&contact_id) {
                                        ui.label(RichText::new(format!("- {}", preview)).size(9.0).color(Color32::from_gray(80)));
                                    }

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        // chat button
                                        if ui.small_button(format!("{}", icons::CHAT_TEXT)).clicked() {
                                            self.chat_contact = Some(contact_id.clone());
                                            self.chat_input.clear();
                                        }
                                        // send button
                                        if ui.small_button(format!("{}", icons::PAPER_PLANE_TILT)).clicked() {
                                            self.send_address = contact_addr.clone();
                                            self.active_panel = ActivePanel::Send;
                                        }
                                        // delete
                                        if ui.small_button(format!("{}", icons::TRASH)).clicked() {
                                            self.address_book.remove(&contact_id);
                                        }
                                    });
                                });
                            });
                        ui.add_space(4.0);
                    }
                }
            });
    }

    fn render_chat_window(&mut self, ui: &mut egui::Ui) {
        let contact_id = match &self.chat_contact {
            Some(id) => id.clone(),
            None => return,
        };

        // find contact info
        let contact = self.address_book.list().iter()
            .find(|c| c.id == contact_id)
            .map(|c| (*c).clone());

        let contact_name = contact.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| "unknown".to_string());
        let contact_addr = contact.as_ref().map(|c| c.address.clone()).unwrap_or_default();

        egui::Frame::none()
            .fill(Color32::from_rgb(28, 30, 32))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // header with contact name and close button
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::CHAT_TEXT).size(14.0).color(Color32::from_rgb(100, 180, 140)));
                    ui.add_space(6.0);
                    ui.label(RichText::new(&contact_name).size(12.0).color(Color32::from_gray(180)));
                    ui.label(RichText::new(truncate_address(&contact_addr)).size(9.0).color(Color32::from_gray(80)).monospace());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(format!("{}", icons::X)).clicked() {
                            self.chat_contact = None;
                        }
                    });
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // chat messages
                let messages: Vec<_> = self.chat_storage.get(&contact_id)
                    .map(|h| h.messages().to_vec())
                    .unwrap_or_default();

                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if messages.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(30.0);
                                ui.label(RichText::new("no messages yet").size(11.0).color(Color32::from_gray(80)));
                                ui.label(RichText::new("messages are sent as zcash shielded memos").size(9.0).color(Color32::from_gray(60)));
                                ui.add_space(30.0);
                            });
                        } else {
                            for msg in &messages {
                                self.render_chat_message(ui, msg);
                                ui.add_space(4.0);
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // message input
                ui.horizontal(|ui| {
                    let response = ui.add(TextEdit::singleline(&mut self.chat_input)
                        .desired_width(ui.available_width() - 80.0)
                        .hint_text("type a message..."));

                    let send_clicked = ui.button(format!("{} send", icons::PAPER_PLANE_TILT)).clicked();
                    let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                    if (send_clicked || enter_pressed) && !self.chat_input.trim().is_empty() {
                        let msg_content = self.chat_input.trim().to_string();
                        self.chat_input.clear();

                        // create outgoing message
                        let msg = ChatMessage::outgoing(&contact_id, &msg_content);
                        self.chat_storage.add_message(&contact_id, msg.clone());

                        // TODO: actually send the transaction with memo
                        // for now just mark as sent (demo)
                        if let Some(history) = self.chat_storage.get_mut(&contact_id) {
                            history.update_status(&msg.id, MessageStatus::Sent, None);
                        }
                    }
                });
            });
    }

    fn render_chat_message(&self, ui: &mut egui::Ui, msg: &ChatMessage) {
        let is_outgoing = msg.outgoing;
        let bg_color = if is_outgoing {
            Color32::from_rgb(45, 55, 65)
        } else {
            Color32::from_rgb(35, 40, 42)
        };

        let layout = if is_outgoing {
            egui::Layout::right_to_left(egui::Align::TOP)
        } else {
            egui::Layout::left_to_right(egui::Align::TOP)
        };

        ui.with_layout(layout, |ui| {
            egui::Frame::none()
                .fill(bg_color)
                .rounding(6.0)
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_max_width(300.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&msg.content).size(11.0).color(Color32::from_gray(200)));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(msg.format_time()).size(8.0).color(Color32::from_gray(80)));
                            // status icon
                            let status_icon = match msg.status {
                                MessageStatus::Pending => icons::CLOCK,
                                MessageStatus::Sent => icons::CHECK,
                                MessageStatus::Confirmed => icons::CHECKS,
                                MessageStatus::Failed => icons::X,
                            };
                            let status_color = match msg.status {
                                MessageStatus::Pending => Color32::from_gray(80),
                                MessageStatus::Sent => Color32::from_rgb(100, 140, 180),
                                MessageStatus::Confirmed => Color32::from_rgb(100, 180, 120),
                                MessageStatus::Failed => Color32::from_rgb(180, 100, 100),
                            };
                            ui.label(RichText::new(status_icon).size(9.0).color(status_color));
                        });
                    });
                });
        });
    }

    fn render_send_section(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(32, 34, 36))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // header with cancel button
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::PAPER_PLANE_TILT).size(18.0).color(Color32::from_rgb(120, 160, 180)));
                    ui.add_space(8.0);
                    ui.label(RichText::new("send").size(14.0).color(Color32::from_gray(180)));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(format!("{}", icons::X)).clicked() {
                            self.active_panel = ActivePanel::None;
                        }
                    });
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // recipient address with contact picker
                ui.horizontal(|ui| {
                    ui.label(RichText::new("to").size(11.0).color(Color32::from_gray(120)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(format!("{} contacts", icons::ADDRESS_BOOK)).clicked() {
                            self.show_address_picker = !self.show_address_picker;
                        }
                    });
                });
                ui.add_space(4.0);

                ui.add(TextEdit::singleline(&mut self.send_address)
                    .desired_width(ui.available_width())
                    .hint_text("zs1... or contact name")
                    .font(egui::TextStyle::Monospace));

                // contact picker dropdown
                if self.show_address_picker {
                    ui.add_space(4.0);
                    egui::Frame::none()
                        .fill(Color32::from_rgb(40, 42, 44))
                        .rounding(4.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            let contacts: Vec<_> = self.address_book.list().iter().map(|c| (c.name.clone(), c.address.clone())).collect();
                            if contacts.is_empty() {
                                ui.label(RichText::new("no contacts yet").size(10.0).color(Color32::from_gray(80)));
                            } else {
                                for (name, addr) in contacts.iter().take(5) {
                                    if ui.selectable_label(false, format!("{} - {}", name, truncate_address(addr))).clicked() {
                                        self.send_address = addr.clone();
                                        self.show_address_picker = false;
                                    }
                                }
                            }
                        });
                }

                ui.add_space(12.0);

                // amount
                ui.label(RichText::new("amount (ZEC)").size(11.0).color(Color32::from_gray(120)));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.send_amount)
                        .desired_width(120.0)
                        .hint_text("0.0"));
                    ui.add_space(8.0);
                    // quick amounts
                    if ui.small_button("max").clicked() {
                        let max_zec = (self.balance.saturating_sub(10000)) as f64 / 100_000_000.0;
                        self.send_amount = format!("{:.8}", max_zec);
                    }
                    ui.label(RichText::new(format!("avail: {}", format_zec(self.balance)))
                        .size(9.0).color(Color32::from_gray(80)));
                });

                ui.add_space(12.0);

                // memo or file attachment section
                ui.horizontal(|ui| {
                    ui.label(RichText::new("message / file").size(11.0).color(Color32::from_gray(120)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(format!("{} attach file", icons::PAPERCLIP)).clicked() {
                            // open file picker
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                self.wormhole_file = Some(path);
                                self.start_wormhole_send();
                            }
                        }
                    });
                });
                ui.add_space(4.0);

                // show attached file if any
                let mut clear_file = false;
                if let Some(file) = self.wormhole_file.clone() {
                    egui::Frame::none()
                        .fill(Color32::from_rgb(40, 45, 50))
                        .rounding(4.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(icons::FILE).size(16.0).color(Color32::from_rgb(120, 160, 200)));
                                ui.add_space(6.0);
                                let file_name = file.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "file".into());
                                ui.label(RichText::new(&file_name).size(11.0).color(Color32::from_gray(180)));

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button(format!("{}", icons::X)).clicked() {
                                        clear_file = true;
                                    }
                                });
                            });

                            // wormhole status
                            match &self.wormhole_state {
                                TransferState::WaitingForCode => {
                                    ui.label(RichText::new("generating wormhole code...").size(9.0).color(Color32::from_gray(100)));
                                }
                                TransferState::CodeReady(code) => {
                                    ui.label(RichText::new(format!("code: {}", code)).size(9.0).color(Color32::from_rgb(140, 180, 140)).monospace());
                                }
                                TransferState::Sending { progress, .. } => {
                                    ui.label(RichText::new(format!("sending: {:.0}%", progress * 100.0)).size(9.0).color(Color32::from_rgb(140, 160, 180)));
                                }
                                TransferState::Failed(e) => {
                                    ui.label(RichText::new(format!("error: {}", e)).size(9.0).color(Color32::from_rgb(200, 100, 100)));
                                }
                                _ => {}
                            }
                        });
                    ui.add_space(4.0);
                }
                if clear_file {
                    self.wormhole_file = None;
                    self.wormhole_code = None;
                    self.wormhole_state = TransferState::Idle;
                }

                // memo text (can coexist with file)
                ui.add(TextEdit::singleline(&mut self.send_memo)
                    .desired_width(ui.available_width())
                    .hint_text(if self.wormhole_file.is_some() { "additional message" } else { "private message" }));

                // drag-drop zone hint
                ui.add_space(4.0);
                let drop_zone = ui.allocate_response(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
                let is_hovering = ui.ctx().input(|i| !i.raw.dropped_files.is_empty() || !i.raw.hovered_files.is_empty());

                egui::Frame::none()
                    .fill(if is_hovering { Color32::from_rgb(45, 55, 50) } else { Color32::from_rgb(30, 32, 34) })
                    .stroke(egui::Stroke::new(1.0, if is_hovering { Color32::from_rgb(100, 140, 100) } else { Color32::from_gray(45) }))
                    .rounding(4.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.vertical_centered(|ui| {
                            ui.add_space(6.0);
                            ui.label(RichText::new(format!("{} drop file here", icons::UPLOAD_SIMPLE))
                                .size(10.0).color(Color32::from_gray(70)));
                            ui.add_space(6.0);
                        });
                    });

                // handle dropped files
                ui.ctx().input(|i| {
                    for file in &i.raw.dropped_files {
                        if let Some(path) = &file.path {
                            self.wormhole_file = Some(path.clone());
                            // will trigger send on next frame
                        }
                    }
                });

                ui.add_space(12.0);

                // save to contacts checkbox (only if address not in contacts)
                let addr_in_contacts = self.address_book.find_by_address(&self.send_address).is_some();
                if !self.send_address.is_empty() && !addr_in_contacts {
                    ui.checkbox(&mut self.save_recipient_to_contacts, "save recipient to contacts");
                    ui.add_space(8.0);
                }

                // send button
                ui.horizontal(|ui| {
                    let can_send = !self.send_address.is_empty() &&
                        (!self.send_amount.is_empty() || self.wormhole_code.is_some());

                    if ui.add_enabled(can_send, egui::Button::new(
                        RichText::new(format!("{} send", icons::PAPER_PLANE_TILT)).size(13.0)
                    )).clicked() {
                        // save to contacts if checked
                        if self.save_recipient_to_contacts && !addr_in_contacts {
                            let name = truncate_address(&self.send_address);
                            let _ = self.address_book.add(&name, &self.send_address);
                            self.save_recipient_to_contacts = false;
                        }

                        // if we have a wormhole code, include it in memo
                        if let Some(ref code) = self.wormhole_code {
                            let wormhole_memo = format_wormhole_memo(code);
                            if self.send_memo.is_empty() {
                                self.send_memo = wormhole_memo;
                            } else {
                                self.send_memo = format!("{}\n{}", self.send_memo, wormhole_memo);
                            }
                        }

                        // set minimum amount for file transfers
                        if self.send_amount.is_empty() && self.wormhole_file.is_some() {
                            self.send_amount = "0.0001".to_string(); // dust amount
                        }

                        self.execute_transfer();
                    }

                    if let Some(ref status) = self.send_status {
                        ui.add_space(12.0);
                        let color = if status.contains("error") || status.contains("invalid") || status.contains("insufficient") {
                            Color32::from_rgb(200, 100, 100)
                        } else if status.contains("broadcast") || status.contains("success") {
                            Color32::from_rgb(100, 180, 100)
                        } else {
                            Color32::from_gray(120)
                        };
                        ui.label(RichText::new(status).size(10.0).color(color));
                    }
                });
            });
    }

    /// start wormhole file send and get code
    fn start_wormhole_send(&mut self) {
        if let Some(ref path) = self.wormhole_file {
            self.wormhole_state = TransferState::WaitingForCode;
            let path = path.clone();

            // spawn async task to get wormhole code
            let (code_tx, mut code_rx) = mpsc::channel::<Result<(String, mpsc::Receiver<TransferProgress>), String>>(1);

            self.runtime.spawn(async move {
                match WormholeTransfer::send_file(path).await {
                    Ok((code, rx)) => {
                        code_tx.send(Ok((code, rx))).await.ok();
                    }
                    Err(e) => {
                        code_tx.send(Err(e.to_string())).await.ok();
                    }
                }
            });

            // store receiver to check for results
            // note: this is a simplified approach - in production would need better async handling
        }
    }

    fn render_accounts_section(&mut self, ui: &mut egui::Ui) {
        // outer card
        egui::Frame::none()
            .fill(Color32::from_rgb(32, 34, 36))
            .rounding(8.0)
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // header
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::WALLET).size(18.0).color(Color32::from_rgb(180, 160, 120)));
                    ui.add_space(8.0);
                    ui.label(RichText::new("accounts").size(14.0).color(Color32::from_gray(180)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(format!("{} new", icons::PLUS)).clicked() {
                            self.show_add_account = true;
                        }
                    });
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // add account dialog
                if self.show_add_account {
                    egui::Frame::none()
                        .fill(Color32::from_rgb(45, 48, 50))
                        .rounding(4.0)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("label").size(11.0).color(Color32::from_gray(120)));
                                ui.add(TextEdit::singleline(&mut self.new_account_label)
                                    .desired_width(150.0)
                                    .hint_text("savings"));
                                ui.add_space(8.0);
                                if ui.button(format!("{} create", icons::CHECK)).clicked() {
                                    self.add_new_account();
                                }
                                if ui.small_button(format!("{}", icons::X)).clicked() {
                                    self.show_add_account = false;
                                    self.new_account_label.clear();
                                }
                            });
                        });
                    ui.add_space(12.0);
                }

                // account list - show first 5 by default
                if let Some(ref session) = self.session {
                    let accounts = session.data.accounts.clone();
                    let active = session.data.active_account;
                    let display_count = 5.min(accounts.len().max(5)); // ensure 5 slots

                    if accounts.is_empty() {
                        ui.label(RichText::new("no accounts yet").size(11.0).color(Color32::from_gray(80)));
                    } else {
                        for i in 0..display_count {
                            if let Some(account) = accounts.get(i) {
                                let is_active = account.index == active;
                                let has_balance = account.balance > 0;

                                // active account gets prominent styling
                                let (bg, border_color) = if is_active {
                                    (Color32::from_rgb(40, 55, 50), Color32::from_rgb(100, 140, 110))
                                } else {
                                    (Color32::from_rgb(38, 40, 42), Color32::TRANSPARENT)
                                };

                                egui::Frame::none()
                                    .fill(bg)
                                    .stroke(egui::Stroke::new(if is_active { 2.0 } else { 0.0 }, border_color))
                                    .rounding(6.0)
                                    .inner_margin(egui::Margin::same(12.0))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            // selection indicator - checkmark for active
                                            if is_active {
                                                ui.label(RichText::new(icons::CHECK_CIRCLE)
                                                    .size(16.0)
                                                    .color(Color32::from_rgb(120, 180, 130)));
                                            } else {
                                                // used/unused indicator
                                                let (icon, color) = if has_balance {
                                                    (icons::CIRCLE, Color32::from_rgb(100, 130, 160))
                                                } else {
                                                    (icons::CIRCLE, Color32::from_gray(50))
                                                };
                                                ui.label(RichText::new(icon).size(16.0).color(color));
                                            }
                                            ui.add_space(10.0);

                                            // account info
                                            ui.vertical(|ui| {
                                                ui.horizontal(|ui| {
                                                    let label_color = if is_active {
                                                        Color32::from_gray(220)
                                                    } else {
                                                        Color32::from_gray(160)
                                                    };
                                                    ui.label(RichText::new(&account.label)
                                                        .size(13.0)
                                                        .color(label_color));
                                                    ui.label(RichText::new(format!("#{}", account.index))
                                                        .size(9.0)
                                                        .color(Color32::from_gray(70)));

                                                    // used/unused badge
                                                    if !has_balance {
                                                        ui.label(RichText::new("unused")
                                                            .size(8.0)
                                                            .color(Color32::from_gray(60)));
                                                    }
                                                });

                                                // balance
                                                let balance_color = if has_balance {
                                                    Color32::from_rgb(160, 200, 160)
                                                } else {
                                                    Color32::from_gray(80)
                                                };
                                                ui.label(RichText::new(format_zec(account.balance))
                                                    .size(12.0)
                                                    .color(balance_color));
                                            });

                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if !is_active {
                                                    // transfer button
                                                    if ui.small_button(format!("{}", icons::ARROW_RIGHT)).on_hover_text("transfer to").clicked() {
                                                        self.transfer_to_account = Some(account.index);
                                                        self.transfer_amount.clear();
                                                        self.transfer_memo.clear();
                                                        self.show_transfer_modal = true;
                                                    }
                                                    // select button
                                                    if ui.button("select").clicked() {
                                                        self.set_active_account(account.index);
                                                    }
                                                } else {
                                                    ui.label(RichText::new("active")
                                                        .size(10.0)
                                                        .color(Color32::from_rgb(100, 140, 110)));
                                                }
                                            });
                                        });
                                    });
                                ui.add_space(6.0);
                            } else {
                                // empty slot placeholder
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(30, 32, 34))
                                    .rounding(6.0)
                                    .inner_margin(egui::Margin::same(12.0))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(icons::CIRCLE_DASHED)
                                                .size(16.0)
                                                .color(Color32::from_gray(40)));
                                            ui.add_space(10.0);
                                            ui.label(RichText::new(format!("account #{}", i))
                                                .size(11.0)
                                                .color(Color32::from_gray(50)));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if ui.small_button("create").clicked() {
                                                    self.new_account_label = format!("account {}", i);
                                                    self.show_add_account = true;
                                                }
                                            });
                                        });
                                    });
                                ui.add_space(6.0);
                            }
                        }

                        // show more accounts link if > 5
                        if accounts.len() > 5 {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("+ {} more accounts", accounts.len() - 5))
                                    .size(10.0)
                                    .color(Color32::from_gray(80)));
                            });
                        }
                    }
                }
            });
    }

    fn render_settings_tab(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let form_width = (available.x * 0.8).min(500.0).max(300.0);

        ui.add_space(16.0);
        ui.label(RichText::new(format!("{} settings", icons::GEAR)).size(16.0).color(Color32::from_gray(180)));
        ui.add_space(16.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            // seed phrase section
            egui::Frame::none()
                .fill(Color32::from_rgb(36, 38, 40))
                .inner_margin(egui::Margin::same(16.0))
                .show(ui, |ui| {
                    ui.set_width(form_width);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::KEY).size(16.0).color(Color32::from_rgb(180, 160, 100)));
                        ui.add_space(8.0);
                        ui.label(RichText::new("seed phrase").size(12.0).color(Color32::from_gray(160)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(if self.show_seed_phrase { "hide" } else { "show" }).clicked() {
                                self.show_seed_phrase = !self.show_seed_phrase;
                            }
                        });
                    });

                    if self.show_seed_phrase {
                        ui.add_space(12.0);
                        if let Some(ref session) = self.session {
                            if let Some(ref seed) = session.data.seed_phrase {
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(45, 42, 38))
                                    .inner_margin(12.0)
                                    .show(ui, |ui| {
                                        let words: Vec<&str> = seed.split_whitespace().collect();
                                        egui::Grid::new("seed_view").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                                            for (i, word) in words.iter().enumerate() {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new(format!("{}.", i + 1)).size(9.0).color(Color32::from_gray(70)));
                                                    ui.label(RichText::new(*word).size(11.0).color(Color32::from_gray(180)).monospace());
                                                });
                                                if (i + 1) % 4 == 0 { ui.end_row(); }
                                            }
                                        });
                                    });
                                ui.add_space(8.0);
                                if ui.small_button(format!("{} copy", icons::COPY)).clicked() {
                                    ui.ctx().copy_text(seed.clone());
                                }
                            } else {
                                ui.label(RichText::new("no seed stored").size(11.0).color(Color32::from_gray(100)));
                            }
                        }
                    }
                });

            ui.add_space(12.0);

            // server settings with connection status
            egui::Frame::none()
                .fill(Color32::from_rgb(36, 38, 40))
                .inner_margin(egui::Margin::same(16.0))
                .show(ui, |ui| {
                    ui.set_width(form_width);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::CLOUD).size(16.0).color(Color32::from_gray(140)));
                        ui.add_space(8.0);
                        ui.label(RichText::new("servers").size(12.0).color(Color32::from_gray(160)));
                    });

                    ui.add_space(12.0);

                    // init edit fields from session if not editing
                    if !self.settings_editing {
                        if let Some(ref session) = self.session {
                            self.edit_server_url = session.data.server_url.clone();
                            self.edit_node_url = session.data.node_url.clone().unwrap_or_default();
                        }
                    }

                    // zidecar with live status indicator
                    ui.horizontal(|ui| {
                        // zen-style status dot
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 4.0, self.zidecar_state.color());
                        ui.add_space(6.0);
                        ui.label(RichText::new("zidecar").size(11.0).color(Color32::from_gray(160)));
                        ui.label(RichText::new("·").size(11.0).color(Color32::from_gray(60)));
                        ui.label(RichText::new(self.zidecar_state.label()).size(10.0).color(self.zidecar_state.color()));
                    });
                    ui.add_space(4.0);
                    if ui.add(TextEdit::singleline(&mut self.edit_server_url)
                        .desired_width(form_width - 32.0)
                        .hint_text("http://127.0.0.1:50051")).changed() {
                        self.settings_editing = true;
                    }

                    ui.add_space(14.0);

                    // node with live status indicator
                    ui.horizontal(|ui| {
                        // zen-style status dot
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 4.0, self.node_state.color());
                        ui.add_space(6.0);
                        ui.label(RichText::new("zcash node").size(11.0).color(Color32::from_gray(160)));
                        ui.label(RichText::new("·").size(11.0).color(Color32::from_gray(60)));
                        ui.label(RichText::new(self.node_state.label()).size(10.0).color(self.node_state.color()));
                    });
                    ui.add_space(4.0);
                    if ui.add(TextEdit::singleline(&mut self.edit_node_url)
                        .desired_width(form_width - 32.0)
                        .hint_text("http://127.0.0.1:8232")).changed() {
                        self.settings_editing = true;
                    }

                    ui.add_space(12.0);

                    // zen-style test button
                    let test_btn = egui::Button::new(
                        RichText::new(if self.connection_check_pending {
                            format!("{} checking...", icons::CIRCLE_NOTCH)
                        } else {
                            format!("{} test", icons::ARROWS_CLOCKWISE)
                        }).size(10.0)
                    ).fill(Color32::from_rgb(45, 45, 48));

                    if ui.add_enabled(!self.connection_check_pending, test_btn).clicked() {
                        self.check_connections();
                    }

                    if self.settings_editing {
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui.button(format!("{} save", icons::CHECK)).clicked() {
                                if let Some(ref mut session) = self.session {
                                    session.data.server_url = self.edit_server_url.clone();
                                    session.data.node_url = if self.edit_node_url.is_empty() {
                                        None
                                    } else {
                                        Some(self.edit_node_url.clone())
                                    };
                                    let _ = session.save();
                                }
                                self.settings_editing = false;
                                // re-check connections after save
                                self.check_connections();
                            }
                            if ui.button("cancel").clicked() {
                                self.settings_editing = false;
                            }
                        });
                    }
                });

            ui.add_space(12.0);

            // derived keys section
            egui::Frame::none()
                .fill(Color32::from_rgb(36, 38, 40))
                .inner_margin(egui::Margin::same(16.0))
                .show(ui, |ui| {
                    ui.set_width(form_width);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::EYE).size(16.0).color(Color32::from_rgb(140, 160, 180)));
                        ui.add_space(8.0);
                        ui.label(RichText::new("derived keys").size(12.0).color(Color32::from_gray(160)));
                    });

                    ui.add_space(12.0);

                    if let Some(ref session) = self.session {
                        if let Some(ref seed) = session.data.seed_phrase {
                            if let Ok(keys) = self.derive_keys_from_seed(seed) {
                                // viewing key - hidden by default
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("full viewing key (fvk)").size(10.0).color(Color32::from_gray(100)));
                                    ui.add_space(8.0);
                                    if ui.small_button(if self.show_fvk { "hide" } else { "show" }).clicked() {
                                        self.show_fvk = !self.show_fvk;
                                    }
                                });
                                ui.add_space(4.0);

                                if self.show_fvk {
                                    egui::Frame::none()
                                        .fill(Color32::from_rgb(30, 32, 35))
                                        .inner_margin(8.0)
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let short_fvk = if keys.0.len() > 40 {
                                                    format!("{}...{}", &keys.0[..20], &keys.0[keys.0.len()-16..])
                                                } else { keys.0.clone() };
                                                ui.label(RichText::new(&short_fvk).size(9.0).color(Color32::from_gray(140)).monospace());
                                                if ui.small_button(format!("{}", icons::COPY)).clicked() {
                                                    ui.ctx().copy_text(keys.0.clone());
                                                }
                                            });
                                        });
                                } else {
                                    ui.label(RichText::new("••••••••••••••••").size(9.0).color(Color32::from_gray(80)).monospace());
                                }

                                ui.add_space(10.0);

                                // address (always visible)
                                ui.label(RichText::new("default address").size(10.0).color(Color32::from_gray(100)));
                                ui.add_space(4.0);
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(30, 32, 35))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(&keys.1).size(9.0).color(Color32::from_gray(140)).monospace());
                                            if ui.small_button(format!("{}", icons::COPY)).clicked() {
                                                ui.ctx().copy_text(keys.1.clone());
                                            }
                                        });
                                    });
                            }
                        } else {
                            ui.label(RichText::new("no seed - recover wallet to derive keys").size(11.0).color(Color32::from_gray(100)));
                        }
                    }
                });

            ui.add_space(12.0);

            // status
            egui::Frame::none()
                .fill(Color32::from_rgb(36, 38, 40))
                .inner_margin(egui::Margin::same(16.0))
                .show(ui, |ui| {
                    ui.set_width(form_width);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::SHIELD_CHECK).size(16.0).color(Color32::from_rgb(100, 150, 100)));
                        ui.add_space(8.0);
                        ui.label(RichText::new("ligerito proofs enabled").size(12.0).color(Color32::from_gray(160)));
                    });

                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::DATABASE).size(14.0).color(Color32::from_gray(100)));
                        ui.add_space(8.0);
                        if let Some(ref session) = self.session {
                            ui.label(RichText::new(format!("synced to block #{}", session.data.last_sync_height))
                                .size(11.0).color(Color32::from_gray(120)));
                        }
                    });
                });

            ui.add_space(12.0);

            // danger zone
            egui::Frame::none()
                .fill(Color32::from_rgb(42, 36, 36))
                .inner_margin(egui::Margin::same(16.0))
                .show(ui, |ui| {
                    ui.set_width(form_width);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::TRASH).size(16.0).color(Color32::from_rgb(180, 100, 100)));
                        ui.add_space(8.0);
                        if ui.button("delete wallet").clicked() {
                            let _ = WalletFile::delete();
                            self.session = None;
                            self.screen = Screen::Setup;
                        }
                    });
                });

            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("zafu v0.1.0").size(10.0).color(Color32::from_gray(60)));
                ui.label(RichText::new("zen meditation cushion").size(9.0).color(Color32::from_gray(50)));
            });
        });
    }

    // --- actions ---

    fn do_login(&mut self) {
        self.login.error = None;

        if self.login.password.is_empty() {
            self.login.error = Some("please enter your password".into());
            return;
        }

        match WalletSession::load(&self.login.password) {
            Ok(session) => {
                self.session = Some(session);
                self.login.password.clear();
                self.screen = Screen::Wallet;
                self.initialize_and_sync();
            }
            Err(e) => {
                self.login.error = Some(format!("wrong password or corrupted wallet: {}", e));
            }
        }
    }

    fn do_setup(&mut self) {
        self.setup.error = None;

        if self.setup.password.len() < 8 {
            self.setup.error = Some("password must be at least 8 characters".into());
            return;
        }

        if self.setup.password != self.setup.confirm_password {
            self.setup.error = Some("passwords do not match".into());
            return;
        }

        // generate seed phrase
        self.setup.seed_phrase = Some(SeedPhrase::generate());
        self.screen = Screen::SetupSeedConfirm;
    }

    fn finalize_setup(&mut self) {
        // store seed phrase in wallet data
        let seed_str = self.setup.seed_phrase.as_ref()
            .map(|s| s.as_str().to_string());

        tracing::info!("finalize_setup: seed_str is_some={}", seed_str.is_some());

        // create default account with derived address
        let mut default_account = HdAccount::default_account();
        if let Some(ref seed) = seed_str {
            if let Ok(addr) = self.derive_address_for_account(seed, 0) {
                default_account.address = Some(addr);
            }
        }

        let data = WalletData {
            seed_phrase: seed_str.clone(),
            server_url: "http://127.0.0.1:50051".into(),
            node_url: Some("http://127.0.0.1:8232".into()),
            birthday_height: 0,
            accounts: vec![default_account],
            active_account: 0,
            ..Default::default()
        };

        tracing::info!("finalize_setup: data.seed_phrase is_some={}", data.seed_phrase.is_some());

        // create and save wallet
        let session = WalletSession::new(&self.setup.password, data);
        if let Err(e) = session.save() {
            self.setup.error = Some(format!("failed to save wallet: {}", e));
            self.screen = Screen::Setup;
            return;
        }

        self.session = Some(session);
        self.setup = SetupState::default();
        self.screen = Screen::Wallet;
        self.initialize_and_sync();
    }

    fn do_recovery(&mut self) {
        self.recovery.error = None;

        let words = self.recovery.seed_phrase.split_whitespace().count();
        if words != 12 && words != 24 {
            self.recovery.error = Some("seed phrase must be 12 or 24 words".into());
            return;
        }

        if self.recovery.new_password.len() < 8 {
            self.recovery.error = Some("password must be at least 8 characters".into());
            return;
        }

        if self.recovery.new_password != self.recovery.confirm_password {
            self.recovery.error = Some("passwords do not match".into());
            return;
        }

        // delete old wallet if exists
        let _ = WalletFile::delete();

        // create new wallet from seed
        let data = WalletData {
            seed_phrase: Some(self.recovery.seed_phrase.clone()),
            server_url: "http://127.0.0.1:50051".into(),
            node_url: Some("http://127.0.0.1:8232".into()),
            birthday_height: 0,
            ..Default::default()
        };

        let session = WalletSession::new(&self.recovery.new_password, data);
        if let Err(e) = session.save() {
            self.recovery.error = Some(format!("failed to save wallet: {}", e));
            return;
        }

        self.session = Some(session);
        self.recovery = RecoveryState::default();
        self.screen = Screen::Wallet;
        self.initialize_and_sync();
    }

    fn lock_wallet(&mut self) {
        self.session = None;
        self.client = None;
        self.orchestrator = None;
        self.login = LoginState::default();
        self.screen = Screen::Login;
    }

    fn initialize_and_sync(&mut self) {
        use tracing::info;

        info!("initializing wallet");
        self.console_log("[zafu] initializing wallet session...");

        let server_url = self.session.as_ref()
            .map(|s| s.data.server_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:50051".into());

        // connect to zidecar
        self.console_log(format!("[sync] connecting to zidecar at {}", server_url));
        self.zidecar_state = ConnectionState::Connecting;
        let client = match self.runtime.block_on(async {
            ZidecarClient::connect(&server_url).await
        }) {
            Ok(c) => {
                self.zidecar_state = ConnectionState::Connected;
                self.console_log("[sync] zidecar connected successfully");
                Arc::new(RwLock::new(c))
            }
            Err(e) => {
                tracing::error!("connection failed: {}", e);
                self.console_log(format!("[error] zidecar connection failed: {}", e));
                self.zidecar_state = ConnectionState::Disconnected;
                return;
            }
        };

        self.console_log("[proof] ligerito verifier ready (via zync-core)");
        let scanner = Arc::new(RwLock::new(WalletScanner::new(None)));

        // use local sled storage for sync state
        let storage = match WalletStorage::open("./zafu_sync.db") {
            Ok(s) => {
                self.console_log("[zafu] opened local sync storage");
                Arc::new(s)
            }
            Err(e) => {
                tracing::error!("storage error: {}", e);
                self.console_log(format!("[error] storage open failed: {}", e));
                return;
            }
        };

        let orchestrator = Arc::new(SyncOrchestrator::new(
            client.clone(),
            scanner.clone(),
            storage.clone(),
        ));

        self.client = Some(client);
        self.scanner = Some(scanner);
        self.local_storage = Some(storage);
        self.orchestrator = Some(orchestrator);

        self.start_sync();
    }

    fn start_sync(&mut self) {
        if self.orchestrator.is_none() { return; }

        self.is_syncing = true;
        let (tx, rx) = mpsc::channel(100);
        self.progress_rx = Some(rx);

        let orchestrator = self.orchestrator.clone().unwrap();

        self.runtime.spawn(async move {
            let _ = tx.try_send(SyncProgress {
                phase: SyncPhase::VerifyingProofs,
                progress: 0.1,
                message: "downloading proofs...".into(),
                current_height: 0,
            });

            match orchestrator.sync().await {
                Ok(progress) => { let _ = tx.try_send(progress); }
                Err(e) => {
                    let _ = tx.try_send(SyncProgress {
                        phase: SyncPhase::Error,
                        progress: 0.0,
                        message: format!("sync error: {}", e),
                        current_height: 0,
                    });
                }
            }
        });
    }

    fn generate_address(&mut self) {
        // generate real orchard address from seed for active account
        if let Some(ref session) = self.session {
            if let Some(ref seed_phrase) = session.data.seed_phrase {
                let account_index = session.data.active_account;
                match self.derive_address_for_account(seed_phrase, account_index) {
                    Ok(addr) => {
                        self.receive_address = addr;
                        return;
                    }
                    Err(e) => {
                        tracing::error!("address derivation failed: {}", e);
                    }
                }
            }
        }
        self.receive_address = "no seed - recover wallet first".into();
    }

    fn derive_address_from_seed(&self, seed_phrase: &str) -> anyhow::Result<String> {
        use orchard::keys::{SpendingKey, FullViewingKey, Scope};
        use zip32::AccountId;
        use bip39::Mnemonic;

        // parse mnemonic and derive seed
        let mnemonic = Mnemonic::parse(seed_phrase)
            .map_err(|e| anyhow::anyhow!("invalid seed: {}", e))?;
        let seed = mnemonic.to_seed("");

        // derive orchard spending key (ZIP-32 path for mainnet account 0)
        // coin_type = 133 for zcash
        let account = AccountId::try_from(0u32)
            .map_err(|_| anyhow::anyhow!("invalid account id"))?;
        let sk = SpendingKey::from_zip32_seed(&seed, 133, account)
            .map_err(|_| anyhow::anyhow!("key derivation failed"))?;

        // get full viewing key and derive address
        let fvk = FullViewingKey::from(&sk);
        let address = fvk.address_at(0u32, Scope::External);

        // encode as bech32 (simplified - real impl would use proper encoding)
        let addr_bytes = address.to_raw_address_bytes();
        Ok(format!("zo1{}", hex::encode(&addr_bytes[..20])))
    }

    /// derive full viewing key and address from seed (returns (fvk_hex, address))
    fn derive_keys_from_seed(&self, seed_phrase: &str) -> anyhow::Result<(String, String)> {
        use orchard::keys::{SpendingKey, FullViewingKey, Scope};
        use zip32::AccountId;
        use bip39::Mnemonic;

        let mnemonic = Mnemonic::parse(seed_phrase)
            .map_err(|e| anyhow::anyhow!("invalid seed: {}", e))?;
        let seed = mnemonic.to_seed("");

        let account = AccountId::try_from(0u32)
            .map_err(|_| anyhow::anyhow!("invalid account id"))?;
        let sk = SpendingKey::from_zip32_seed(&seed, 133, account)
            .map_err(|_| anyhow::anyhow!("key derivation failed"))?;

        let fvk = FullViewingKey::from(&sk);
        let address = fvk.address_at(0u32, Scope::External);

        // encode FVK as hex
        let fvk_bytes = fvk.to_bytes();
        let fvk_hex = hex::encode(fvk_bytes);

        // encode address as simplified zo1 format
        let addr_bytes = address.to_raw_address_bytes();
        let addr_str = format!("zo1{}", hex::encode(&addr_bytes[..20]));

        Ok((fvk_hex, addr_str))
    }

    /// non-blocking async connection check (for periodic updates)
    fn check_connections_async(&mut self) {
        self.connection_check_pending = true;
        self.last_connection_check = std::time::Instant::now();

        // get URLs from session or edit fields
        let server_url = if !self.edit_server_url.is_empty() {
            self.edit_server_url.clone()
        } else {
            self.session.as_ref()
                .map(|s| s.data.server_url.clone())
                .unwrap_or_else(|| "http://127.0.0.1:50051".into())
        };

        self.zidecar_state = ConnectionState::Connecting;

        // simple sync check since egui needs immediate state update
        self.zidecar_state = self.runtime.block_on(async {
            match tokio::time::timeout(
                std::time::Duration::from_secs(3),
                crate::client::ZidecarClient::connect(&server_url)
            ).await {
                Ok(Ok(_)) => ConnectionState::Connected,
                _ => ConnectionState::Disconnected,
            }
        });

        self.connection_check_pending = false;
    }

    /// check connection status to zidecar and node (blocking, for settings test button)
    fn check_connections(&mut self) {
        self.last_connection_check = std::time::Instant::now();
        self.connection_check_pending = true;

        // get URLs from edit fields or session
        let server_url = if !self.edit_server_url.is_empty() {
            self.edit_server_url.clone()
        } else {
            self.session.as_ref()
                .map(|s| s.data.server_url.clone())
                .unwrap_or_default()
        };
        let node_url = if !self.edit_node_url.is_empty() {
            self.edit_node_url.clone()
        } else {
            self.session.as_ref()
                .and_then(|s| s.data.node_url.clone())
                .unwrap_or_default()
        };

        if !server_url.is_empty() {
            self.zidecar_state = ConnectionState::Connecting;
        }
        if !node_url.is_empty() {
            self.node_state = ConnectionState::Connecting;
        }

        // check zidecar connection
        if !server_url.is_empty() {
            self.zidecar_state = self.runtime.block_on(async {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    crate::client::ZidecarClient::connect(&server_url)
                ).await {
                    Ok(Ok(_)) => ConnectionState::Connected,
                    _ => ConnectionState::Disconnected,
                }
            });
        } else {
            self.zidecar_state = ConnectionState::Disconnected;
        }

        // check node connection (simple tcp check for now)
        if !node_url.is_empty() {
            self.node_state = self.runtime.block_on(async {
                // parse URL and try TCP connection
                if let Ok(url) = node_url.parse::<url::Url>() {
                    if let Some(host) = url.host_str() {
                        let port = url.port().unwrap_or(8232);
                        let addr = format!("{}:{}", host, port);
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(3),
                            tokio::net::TcpStream::connect(&addr)
                        ).await {
                            Ok(Ok(_)) => return ConnectionState::Connected,
                            _ => {}
                        }
                    }
                }
                ConnectionState::Disconnected
            });
        } else {
            self.node_state = ConnectionState::Disconnected;
        }

        self.connection_check_pending = false;
    }

    /// add new HD account
    fn add_new_account(&mut self) {
        // extract data needed before mutable borrow
        let (seed, num_accounts, next_index) = {
            if let Some(ref session) = self.session {
                let next = session.data.accounts.iter()
                    .map(|a| a.index)
                    .max()
                    .map(|i| i + 1)
                    .unwrap_or(0);
                (session.data.seed_phrase.clone(), session.data.accounts.len(), next)
            } else {
                return;
            }
        };

        let label = if self.new_account_label.is_empty() {
            format!("account {}", num_accounts)
        } else {
            self.new_account_label.clone()
        };

        let mut new_account = HdAccount::new(next_index, &label);

        // derive address for this account
        if let Some(ref seed_str) = seed {
            if let Ok(addr) = self.derive_address_for_account(seed_str, next_index) {
                new_account.address = Some(addr);
            }
        }

        // now mutably borrow to update
        if let Some(ref mut session) = self.session {
            session.data.accounts.push(new_account);
            let _ = session.save();
        }

        self.show_add_account = false;
        self.new_account_label.clear();
    }

    /// set active account
    fn set_active_account(&mut self, index: u32) {
        if let Some(ref mut session) = self.session {
            session.data.active_account = index;
            let _ = session.save();
            // clear cached receive address so it regenerates
            self.receive_address.clear();
        }
    }

    /// save edited account label
    fn save_account_label(&mut self, index: u32, label: &str) {
        if let Some(ref mut session) = self.session {
            if let Some(acct) = session.data.accounts.iter_mut().find(|a| a.index == index) {
                acct.label = label.to_string();
                let _ = session.save();
            }
        }
    }

    /// create account at specific index (for empty slots)
    fn create_account_at_index(&mut self, index: u32) {
        let seed = if let Some(ref session) = self.session {
            session.data.seed_phrase.clone()
        } else {
            return;
        };

        let label = format!("account {}", index);
        let mut new_account = HdAccount::new(index, &label);

        // derive address for this account
        if let Some(ref seed_str) = seed {
            if let Ok(addr) = self.derive_address_for_account(seed_str, index) {
                new_account.address = Some(addr);
            }
        }

        if let Some(ref mut session) = self.session {
            // insert in order
            session.data.accounts.push(new_account);
            session.data.accounts.sort_by_key(|a| a.index);
            let _ = session.save();
        }
    }

    /// derive address for specific account index
    fn derive_address_for_account(&self, seed_phrase: &str, account_index: u32) -> anyhow::Result<String> {
        use orchard::keys::{SpendingKey, FullViewingKey, Scope};
        use zip32::AccountId;
        use bip39::Mnemonic;

        let mnemonic = Mnemonic::parse(seed_phrase)
            .map_err(|e| anyhow::anyhow!("invalid seed: {}", e))?;
        let seed = mnemonic.to_seed("");

        let account = AccountId::try_from(account_index)
            .map_err(|_| anyhow::anyhow!("invalid account id"))?;
        let sk = SpendingKey::from_zip32_seed(&seed, 133, account)
            .map_err(|_| anyhow::anyhow!("key derivation failed"))?;

        let fvk = FullViewingKey::from(&sk);
        let address = fvk.address_at(0u32, Scope::External);
        let addr_bytes = address.to_raw_address_bytes();

        Ok(format!("zo1{}", hex::encode(&addr_bytes[..20])))
    }

    fn send_transaction(&mut self) {
        if self.send_address.is_empty() {
            self.send_status = Some("enter recipient".into());
            return;
        }
        self.send_status = Some("sending requires spending key".into());
    }

    fn get_balance(&self) -> u64 {
        if let Some(ref scanner) = self.scanner {
            if let Ok(s) = scanner.try_read() {
                return s.balance();
            }
        }
        0
    }

    /// log message to dev console
    fn console_log(&mut self, msg: impl Into<String>) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() % 86400) // seconds into day
            .unwrap_or(0);
        let hours = (timestamp / 3600) % 24;
        let mins = (timestamp / 60) % 60;
        let secs = timestamp % 60;
        let line = format!("[{:02}:{:02}:{:02}] {}", hours, mins, secs, msg.into());
        self.console_lines.push(line);
        // keep max 500 lines
        if self.console_lines.len() > 500 {
            self.console_lines.remove(0);
        }
    }

    /// render quake-style dropdown console
    fn render_console(&mut self, ctx: &egui::Context) {
        use egui::{Frame, Pos2, Stroke, Vec2, ScrollArea, Align};

        let screen_rect = ctx.screen_rect();
        let console_height = screen_rect.height() * 0.4; // 40% of screen

        egui::Area::new(egui::Id::new("dev_console"))
            .fixed_pos(Pos2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let bg_color = Color32::from_rgba_unmultiplied(15, 15, 18, 230);
                let border_color = Color32::from_rgb(60, 180, 60); // terminal green

                Frame::none()
                    .fill(bg_color)
                    .stroke(Stroke::new(2.0, border_color))
                    .show(ui, |ui| {
                        ui.set_min_size(Vec2::new(screen_rect.width(), console_height));
                        ui.set_max_size(Vec2::new(screen_rect.width(), console_height));

                        ui.add_space(4.0);

                        // header
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(format!("{} zafu dev console", icons::TERMINAL))
                                    .size(12.0)
                                    .color(border_color)
                                    .monospace()
                            );

                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(12.0);
                                ui.label(
                                    RichText::new("press ` to close")
                                        .size(10.0)
                                        .color(Color32::from_gray(80))
                                        .monospace()
                                );
                            });
                        });

                        ui.separator();

                        // scrollable log area
                        ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .stick_to_bottom(true)
                            .max_height(console_height - 40.0)
                            .show(ui, |ui| {
                                ui.set_min_width(screen_rect.width() - 16.0);
                                for line in &self.console_lines {
                                    // color code by prefix
                                    let color = if line.contains("[error]") || line.contains("ERROR") {
                                        Color32::from_rgb(220, 80, 80)
                                    } else if line.contains("[warn]") || line.contains("WARN") {
                                        Color32::from_rgb(220, 180, 80)
                                    } else if line.contains("[sync]") {
                                        Color32::from_rgb(80, 180, 220)
                                    } else if line.contains("[proof]") {
                                        Color32::from_rgb(180, 120, 220)
                                    } else if line.contains("[tx]") {
                                        Color32::from_rgb(120, 220, 120)
                                    } else {
                                        Color32::from_gray(160)
                                    };
                                    ui.label(
                                        RichText::new(line)
                                            .size(11.0)
                                            .color(color)
                                            .monospace()
                                    );
                                }
                            });
                    });
            });
    }
}

impl eframe::App for Zafu {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // handle backtick key for dev console toggle
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Backtick) {
                self.show_console = !self.show_console;
            }
        });

        // periodic connection check every 10s when on wallet screen
        if matches!(self.screen, Screen::Wallet) {
            let elapsed = self.last_connection_check.elapsed();
            if elapsed.as_secs() >= 10 && !self.connection_check_pending {
                self.check_connections_async();
            }
            // request repaint to keep checking
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }

        if self.is_syncing {
            ctx.request_repaint();
            let mut log_msg: Option<String> = None;
            if let Ok(p) = self.sync_progress.try_read() {
                // log sync progress changes to console
                if p.message != self.last_console_sync_msg {
                    let tag = match p.phase {
                        SyncPhase::Connecting => "[sync]",
                        SyncPhase::VerifyingProofs => "[proof]",
                        SyncPhase::DownloadingBlocks => "[sync]",
                        SyncPhase::Scanning => "[sync]",
                        SyncPhase::Complete => "[sync]",
                        SyncPhase::Error => "[error]",
                    };
                    log_msg = Some(format!("{} {} (height: {})", tag, p.message, p.current_height));
                    self.last_console_sync_msg = p.message.clone();
                }
                if matches!(p.phase, SyncPhase::Complete | SyncPhase::Error) {
                    self.is_syncing = false;
                    self.balance = self.get_balance();
                }
            }
            if let Some(msg) = log_msg {
                self.console_log(msg);
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.screen {
                Screen::Login => self.render_login(ui),
                Screen::Setup => self.render_setup(ui),
                Screen::SetupSeedConfirm => self.render_seed_confirm(ui),
                Screen::Recovery => self.render_recovery(ui),
                Screen::Wallet => {
                    self.render_sync_bar(ui);
                    self.render_wallet(ui);
                }
            }
        });

        // transfer modal (egui window overlay)
        if self.show_transfer_modal {
            self.render_transfer_modal(ctx);
        }

        // dev console (quake-style dropdown)
        if self.show_console {
            self.render_console(ctx);
        }
    }
}

impl Zafu {
    fn render_transfer_modal(&mut self, ctx: &egui::Context) {
        let mut open = true;

        egui::Window::new("transfer")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([320.0, 280.0])
            .show(ctx, |ui| {
                ui.add_space(8.0);

                // get target account info
                let target_info = if let (Some(target_idx), Some(ref session)) = (self.transfer_to_account, &self.session) {
                    session.data.accounts.iter()
                        .find(|a| a.index == target_idx)
                        .map(|a| (a.label.clone(), a.address.clone()))
                } else {
                    None
                };

                if let Some((label, addr)) = target_info {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}", icons::ARROW_RIGHT)).size(14.0).color(Color32::from_rgb(120, 160, 120)));
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&label).size(13.0).color(Color32::from_gray(180)));
                            if let Some(ref a) = addr {
                                let short = if a.len() > 24 { format!("{}...{}", &a[..12], &a[a.len()-8..]) } else { a.clone() };
                                ui.label(RichText::new(short).size(9.0).color(Color32::from_gray(100)).monospace());
                            }
                        });
                    });

                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(12.0);

                    // amount
                    ui.label(RichText::new("amount (ZEC)").size(10.0).color(Color32::from_gray(100)));
                    ui.add_space(4.0);
                    ui.add(TextEdit::singleline(&mut self.transfer_amount)
                        .desired_width(200.0)
                        .hint_text("0.001"));

                    ui.add_space(12.0);

                    // memo
                    ui.label(RichText::new("memo (optional)").size(10.0).color(Color32::from_gray(100)));
                    ui.add_space(4.0);
                    ui.add(TextEdit::singleline(&mut self.transfer_memo)
                        .desired_width(200.0)
                        .hint_text("internal transfer"));

                    ui.add_space(20.0);

                    ui.horizontal(|ui| {
                        let send_btn = egui::Button::new(
                            RichText::new(format!("{} send", icons::PAPER_PLANE_TILT)).size(12.0)
                        ).fill(Color32::from_rgb(60, 90, 70));

                        if ui.add(send_btn).clicked() {
                            // execute transfer
                            if let Some(target_addr) = addr {
                                self.send_address = target_addr;
                                self.send_amount = self.transfer_amount.clone();
                                self.send_memo = self.transfer_memo.clone();
                                self.execute_transfer();
                            }
                            self.show_transfer_modal = false;
                        }

                        ui.add_space(8.0);

                        if ui.button("cancel").clicked() {
                            self.show_transfer_modal = false;
                        }
                    });

                    // show balance hint
                    ui.add_space(12.0);
                    ui.label(RichText::new(format!("available: {}", format_zec(self.balance)))
                        .size(9.0).color(Color32::from_gray(80)));
                }
            });

        if !open {
            self.show_transfer_modal = false;
        }
    }

    fn execute_transfer(&mut self) {
        // parse amount
        let amount_zec: f64 = self.send_amount.parse().unwrap_or(0.0);
        if amount_zec <= 0.0 {
            self.send_status = Some("invalid amount".into());
            return;
        }

        let zatoshis = (amount_zec * 100_000_000.0) as u64;
        if zatoshis > self.balance {
            self.send_status = Some("insufficient balance".into());
            return;
        }

        // get seed and node URL from session
        let (seed_phrase, node_url, account_index) = match &self.session {
            Some(session) => {
                let seed = session.data.seed_phrase.clone();
                let node = session.data.node_url.clone();
                let acct = session.data.active_account;
                (seed, node, acct)
            }
            None => {
                self.send_status = Some("no wallet session".into());
                return;
            }
        };

        let seed = match seed_phrase {
            Some(s) => s,
            None => {
                self.send_status = Some("no seed phrase - recover wallet first".into());
                return;
            }
        };

        let node = match node_url {
            Some(n) if !n.is_empty() => n,
            _ => {
                self.send_status = Some("configure node URL in settings".into());
                return;
            }
        };

        // derive seed bytes
        let seed_bytes = match self.derive_seed_bytes(&seed) {
            Ok(s) => s,
            Err(e) => {
                self.send_status = Some(format!("seed error: {}", e));
                return;
            }
        };

        // create tx builder
        let builder = match OrchardTxBuilder::from_seed(&seed_bytes, account_index) {
            Ok(b) => b,
            Err(e) => {
                self.send_status = Some(format!("key derivation failed: {}", e));
                return;
            }
        };

        // get recipient address
        let recipient_addr = self.send_address.clone();

        self.send_status = Some(format!(
            "building tx: {} ZEC to {} via {}",
            amount_zec,
            truncate_address(&recipient_addr),
            truncate_address(&node)
        ));

        tracing::info!(
            "execute_transfer: {} zatoshis to {} via {} (account #{})",
            zatoshis,
            recipient_addr,
            node,
            account_index
        );

        // get spendable notes from scanner
        let spendable_notes = if let Some(ref scanner) = self.scanner {
            if let Ok(mut s) = scanner.try_write() {
                s.take_spendable_notes(zatoshis)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        if spendable_notes.is_empty() {
            self.send_status = Some("no spendable notes - sync wallet first".into());
            tracing::warn!("no spendable notes available for transfer");
            return;
        }

        let total_available: u64 = spendable_notes.iter()
            .map(|n| n.note.value().inner())
            .sum();

        tracing::info!(
            "found {} spendable notes totaling {} zatoshis",
            spendable_notes.len(),
            total_available
        );

        // get current anchor from storage
        let anchor = if let Some(ref storage) = self.local_storage {
            storage.get_anchor()
        } else {
            self.send_status = Some("no sync state - sync wallet first".into());
            return;
        };

        // parse recipient address (for now use builder's internal address for testing)
        // TODO: implement proper address parsing from zo1/unified format
        let recipient = builder.default_address();

        // prepare memo if provided
        let memo = if self.send_memo.is_empty() {
            None
        } else {
            let mut memo_bytes = [0u8; 512];
            let bytes = self.send_memo.as_bytes();
            let len = bytes.len().min(512);
            memo_bytes[..len].copy_from_slice(&bytes[..len]);
            Some(memo_bytes)
        };

        // build transaction
        let request = TransferRequest {
            recipient,
            amount: zatoshis,
            memo,
        };

        self.send_status = Some("building transaction...".into());

        match builder.build_transfer(spendable_notes, request, anchor) {
            Ok(tx_bytes) => {
                tracing::info!("transaction built: {} bytes", tx_bytes.len());

                // submit to zebrad
                let rpc = ZebradRpc::new(&node);
                let tx_hex = hex::encode(&tx_bytes);

                self.send_status = Some("submitting to node...".into());

                match rpc.send_raw_transaction(&tx_hex) {
                    Ok(txid) => {
                        self.send_status = Some(format!(
                            "broadcast! txid: {}",
                            truncate_address(&txid)
                        ));
                        tracing::info!("transaction submitted: {}", txid);

                        // add to tx history
                        let memo_text = if self.send_memo.is_empty() { None } else { Some(self.send_memo.clone()) };
                        let mut record = TxRecord::new_sent(&txid, &recipient_addr, zatoshis, memo_text);

                        // resolve contact name if known
                        if let Some(contact) = self.address_book.find_by_address(&recipient_addr) {
                            record.contact_name = Some(contact.name.clone());
                        }
                        self.tx_history.add(record);
                        self.last_sent_txid = Some(txid);

                        // clear form
                        self.send_address.clear();
                        self.send_amount.clear();
                        self.send_memo.clear();
                    }
                    Err(e) => {
                        self.send_status = Some(format!("node error: {}", e));
                        tracing::error!("rpc submission failed: {}", e);
                    }
                }
            }
            Err(e) => {
                self.send_status = Some(format!("build failed: {}", e));
                tracing::error!("transaction build failed: {}", e);
            }
        }
    }

    /// derive seed bytes from BIP-39 phrase
    fn derive_seed_bytes(&self, seed_phrase: &str) -> anyhow::Result<[u8; 64]> {
        use bip39::Mnemonic;
        let mnemonic = Mnemonic::parse(seed_phrase)
            .map_err(|e| anyhow::anyhow!("invalid seed: {}", e))?;
        Ok(mnemonic.to_seed(""))
    }
}

fn format_zec(zatoshis: u64) -> String {
    let zec = zatoshis as f64 / 100_000_000.0;
    format!("{:.8} ZEC", zec)
}

fn truncate_address(addr: &str) -> String {
    if addr.len() > 20 { format!("{}...{}", &addr[..10], &addr[addr.len()-6..]) }
    else { addr.to_string() }
}

/// extract banana split backup tool to temp file and open in browser
fn open_banana_split() {
    use std::io::Write;

    let temp_dir = std::env::temp_dir();
    let path = temp_dir.join("zafu_banana_split.html");

    if let Ok(mut file) = std::fs::File::create(&path) {
        if file.write_all(BANANA_SPLIT_HTML).is_ok() {
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("xdg-open").arg(&path).spawn(); }
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("open").arg(&path).spawn(); }
            #[cfg(target_os = "windows")]
            { let _ = std::process::Command::new("cmd").args(["/c", "start", "", path.to_str().unwrap_or("")]).spawn(); }
        }
    }
}
