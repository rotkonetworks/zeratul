//! lobby - main entry point for creating/joining tables
//!
//! flow:
//! 1. login (email + PIN in test mode)
//! 2. show balance + faucet
//! 3. create table or join by code

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use std::sync::mpsc;

use crate::auth::{AuthEvent, AuthState, AuthStatus};
use crate::multitable::{HintAction, HintModeState, HintTarget};
use crate::p2p::P2PNotification;
use crate::vault_client::{VaultCheckResult, VaultClient};
use crate::DebugMode;

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LobbyState>()
            .init_resource::<VaultConnection>()
            .add_systems(Update, (render_lobby, check_vault_status, handle_p2p_notifications));
    }
}

/// handle P2P notifications
fn handle_p2p_notifications(
    mut lobby: ResMut<LobbyState>,
    mut notifications: EventReader<P2PNotification>,
) {
    for notification in notifications.read() {
        match notification {
            P2PNotification::TableCreated { code } => {
                lobby.created_code = Some(code.clone());
                lobby.view = LobbyView::WaitingForPlayers;
                info!("lobby: table created with code {}", code);
            }
            P2PNotification::JoinedTable { seat } => {
                info!("lobby: joined table at seat {}", seat);
                lobby.connected = true;
            }
            P2PNotification::PlayerJoined { seat } => {
                info!("lobby: player joined at seat {}", seat);
            }
            P2PNotification::ReadyToStart => {
                info!("lobby: ready to start game");
                lobby.connected = true;
            }
            P2PNotification::Error { message } => {
                lobby.error = Some(message.clone());
                warn!("lobby: p2p error - {}", message);
            }
        }
    }
}

/// vault connection state
#[derive(Resource)]
pub struct VaultConnection {
    pub status: VaultStatus,
    pub nodes: Vec<String>,
    pub connected_count: usize,
}

impl Default for VaultConnection {
    fn default() -> Self {
        Self {
            status: VaultStatus::Disconnected,
            nodes: vec![
                "http://127.0.0.1:4201".to_string(),
                "http://127.0.0.1:4202".to_string(),
                "http://127.0.0.1:4203".to_string(),
            ],
            connected_count: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum VaultStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// lobby state
#[derive(Resource, Default)]
pub struct LobbyState {
    /// current view
    pub view: LobbyView,
    /// create table form
    pub create_form: CreateTableForm,
    /// join table form
    pub join_form: JoinTableForm,
    /// test balance (play chips)
    pub balance: u64,
    /// created table code (to show after creation)
    pub created_code: Option<String>,
    /// error message
    pub error: Option<String>,
    /// connected to table
    pub connected: bool,
    /// console open
    pub console_open: bool,
    /// console log buffer
    pub console_logs: Vec<ConsoleEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LobbyView {
    #[default]
    Login,
    Main,
    CreateTable,
    JoinTable,
    WaitingForPlayers,
    Connecting,
}

/// console log entry
#[derive(Clone, Debug)]
pub struct ConsoleEntry {
    pub level: LogLevel,
    pub message: String,
    pub timestamp: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogLevel {
    #[default]
    Info,
    Warn,
    Error,
    Debug,
}

impl LobbyState {
    /// add a log entry to the console
    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.console_logs.push(ConsoleEntry {
            level,
            message: message.into(),
            timestamp,
        });
        // keep last 500 entries
        if self.console_logs.len() > 500 {
            self.console_logs.remove(0);
        }
    }

    pub fn log_info(&mut self, message: impl Into<String>) {
        self.log(LogLevel::Info, message);
    }

    pub fn log_warn(&mut self, message: impl Into<String>) {
        self.log(LogLevel::Warn, message);
    }

    pub fn log_error(&mut self, message: impl Into<String>) {
        self.log(LogLevel::Error, message);
    }
}

#[derive(Clone, Debug, Default)]
pub struct CreateTableForm {
    pub stakes: StakesPreset,
    pub seats: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StakesPreset {
    #[default]
    Micro,   // 1/2
    Low,     // 5/10
    Medium,  // 25/50
    High,    // 100/200
}

impl StakesPreset {
    fn label(&self) -> &'static str {
        match self {
            StakesPreset::Micro => "1/2",
            StakesPreset::Low => "5/10",
            StakesPreset::Medium => "25/50",
            StakesPreset::High => "100/200",
        }
    }

    fn blinds(&self) -> (u64, u64) {
        match self {
            StakesPreset::Micro => (1, 2),
            StakesPreset::Low => (5, 10),
            StakesPreset::Medium => (25, 50),
            StakesPreset::High => (100, 200),
        }
    }

    fn min_buy_in(&self) -> u64 {
        let (_, bb) = self.blinds();
        bb * 50
    }
}

#[derive(Clone, Debug, Default)]
pub struct JoinTableForm {
    pub code: String,
}

/// check vault testnet status and poll for results
fn check_vault_status(
    mut vault: ResMut<VaultConnection>,
    mut receiver: Local<Option<mpsc::Receiver<VaultCheckResult>>>,
    mut checked: Local<bool>,
) {
    // start check once at startup
    if !*checked {
        *checked = true;
        vault.status = VaultStatus::Connecting;
        info!("vault: checking testnet nodes...");

        // spawn async vault check
        let (tx, rx) = mpsc::channel();
        *receiver = Some(rx);
        VaultClient::check_async(vault.nodes.clone(), tx);
    }

    // poll for results
    if let Some(ref rx) = *receiver {
        if let Ok(result) = rx.try_recv() {
            vault.connected_count = result.connected;

            if result.is_healthy() {
                vault.status = VaultStatus::Connected;
                info!("vault: connected to {}/{} nodes", result.connected, result.total);
                for (url, info) in &result.node_infos {
                    info!("  {} - node {} (pubkey: {}...)", url, info.index, &info.pubkey[..16]);
                }
            } else if result.connected > 0 {
                vault.status = VaultStatus::Error(format!(
                    "only {}/{} nodes online (need 2)",
                    result.connected, result.total
                ));
                warn!("vault: insufficient nodes for threshold");
            } else {
                vault.status = VaultStatus::Disconnected;
                warn!("vault: no nodes reachable");
            }

            // clear receiver after processing
            *receiver = None;
        }
    }
}

/// poll for vault check results (merged into check_vault_status)
fn poll_vault_result() {
    // no-op, kept for backwards compat
}

/// poker theme colors
mod theme {
    use bevy_egui::egui::Color32;

    pub const BG_DARK: Color32 = Color32::from_rgb(18, 22, 28);
    pub const BG_PANEL: Color32 = Color32::from_rgb(28, 35, 45);
    pub const BG_CARD: Color32 = Color32::from_rgb(38, 48, 62);
    pub const BORDER: Color32 = Color32::from_rgb(55, 70, 90);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 235, 245);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(140, 155, 175);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(90, 105, 125);
    pub const ACCENT_GOLD: Color32 = Color32::from_rgb(255, 200, 60);
    pub const ACCENT_GREEN: Color32 = Color32::from_rgb(80, 200, 120);
    pub const ACCENT_BLUE: Color32 = Color32::from_rgb(70, 140, 220);
    pub const ACCENT_RED: Color32 = Color32::from_rgb(220, 80, 80);
    pub const BTN_PRIMARY: Color32 = Color32::from_rgb(60, 120, 200);
    pub const BTN_PRIMARY_HOVER: Color32 = Color32::from_rgb(80, 140, 220);
    pub const BTN_SECONDARY: Color32 = Color32::from_rgb(50, 65, 85);
    pub const BTN_SECONDARY_HOVER: Color32 = Color32::from_rgb(65, 85, 110);
}

fn apply_poker_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // font sizes
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::proportional(36.0),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::proportional(18.0),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::proportional(20.0),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::monospace(22.0),
    );

    // visual styling
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = theme::BG_DARK;
    style.visuals.window_fill = theme::BG_PANEL;
    style.visuals.extreme_bg_color = theme::BG_CARD;
    style.visuals.faint_bg_color = theme::BG_PANEL;

    // widget styling
    style.visuals.widgets.noninteractive.bg_fill = theme::BG_PANEL;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::BORDER);
    style.visuals.widgets.noninteractive.rounding = egui::Rounding::same(8.0);

    style.visuals.widgets.inactive.bg_fill = theme::BTN_SECONDARY;
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_SECONDARY);
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(8.0);

    style.visuals.widgets.hovered.bg_fill = theme::BTN_SECONDARY_HOVER;
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, theme::TEXT_PRIMARY);
    style.visuals.widgets.hovered.rounding = egui::Rounding::same(8.0);

    style.visuals.widgets.active.bg_fill = theme::BTN_PRIMARY;
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(2.0, theme::TEXT_PRIMARY);
    style.visuals.widgets.active.rounding = egui::Rounding::same(8.0);

    // selection and text
    style.visuals.selection.bg_fill = theme::ACCENT_BLUE.linear_multiply(0.4);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, theme::ACCENT_BLUE);

    // spacing
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(20.0, 12.0);
    style.spacing.window_margin = egui::Margin::same(20.0);

    ctx.set_style(style);
}

fn render_lobby(
    mut contexts: EguiContexts,
    mut lobby: ResMut<LobbyState>,
    mut auth_state: ResMut<AuthState>,
    mut auth_events: EventWriter<AuthEvent>,
    vault: Res<VaultConnection>,
    mut hint_state: ResMut<HintModeState>,
    debug_mode: Res<DebugMode>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let ctx = contexts.ctx_mut();
    apply_poker_theme(ctx);

    // toggle console with backtick (`) in debug mode
    if debug_mode.enabled && keys.just_pressed(KeyCode::Backquote) {
        lobby.console_open = !lobby.console_open;
    }

    // apply pending focus from hint mode
    if let Some(id) = hint_state.pending_focus.take() {
        ctx.memory_mut(|mem| mem.request_focus(id));
    }

    // check login status and update view
    match auth_state.status {
        AuthStatus::NotLoggedIn | AuthStatus::Error => {
            lobby.view = LobbyView::Login;
        }
        AuthStatus::LoggingIn | AuthStatus::Registering => {
            // show loading
        }
        AuthStatus::LoggedIn => {
            if lobby.view == LobbyView::Login {
                lobby.view = LobbyView::Main;
                // give initial balance
                if lobby.balance == 0 {
                    lobby.balance = 1000;
                }
            }
        }
    }

    // render console (debug overlay) - always available when open
    if debug_mode.enabled {
        render_console(ctx, &mut lobby);
    }

    // skip rendering main views if connected to a table
    if lobby.connected {
        return;
    }

    match lobby.view {
        LobbyView::Login => render_login(ctx, &mut auth_state, &mut auth_events, &*vault, &mut hint_state),
        LobbyView::Main => render_main(ctx, &mut lobby, &auth_state, &*vault, &mut hint_state, debug_mode.enabled),
        LobbyView::CreateTable => render_create_table(ctx, &mut lobby),
        LobbyView::JoinTable => render_join_table(ctx, &mut lobby, &mut hint_state),
        LobbyView::WaitingForPlayers => render_waiting(ctx, &mut lobby, debug_mode.enabled),
        LobbyView::Connecting => render_connecting(ctx, &mut lobby, debug_mode.enabled),
    }
}

fn render_login(
    ctx: &egui::Context,
    auth_state: &mut AuthState,
    auth_events: &mut EventWriter<AuthEvent>,
    vault: &VaultConnection,
    hint_state: &mut HintModeState,
) {
    // track widget positions for hints
    let mut email_rect: Option<egui::Rect> = None;
    let mut pin_rect: Option<egui::Rect> = None;
    let mut login_rect: Option<egui::Rect> = None;

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);

            // title with gold accent
            ui.label(egui::RichText::new("GHETTOBOX")
                .size(56.0)
                .strong()
                .color(theme::ACCENT_GOLD));
            ui.label(egui::RichText::new("POKER")
                .size(42.0)
                .color(theme::TEXT_PRIMARY));

            ui.add_space(50.0);

            // login card
            egui::Frame::none()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(2.0, theme::BORDER))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(egui::Margin::same(30.0))
                .show(ui, |ui| {
                    ui.set_min_width(420.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Sign In")
                            .size(28.0)
                            .color(theme::TEXT_PRIMARY));
                        ui.add_space(25.0);

                        // email field
                        ui.allocate_ui_with_layout(
                            egui::vec2(340.0, 70.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.label(egui::RichText::new("Email")
                                    .size(18.0)
                                    .color(theme::TEXT_SECONDARY));
                                ui.add_space(6.0);
                                let resp = ui.add_sized(
                                    [340.0, 38.0],
                                    egui::TextEdit::singleline(&mut auth_state.login_form.email)
                                        .id(egui::Id::new("login_email"))
                                        .font(egui::FontId::proportional(18.0))
                                );
                                email_rect = Some(resp.rect);
                            }
                        );

                        ui.add_space(12.0);

                        // pin field
                        ui.allocate_ui_with_layout(
                            egui::vec2(340.0, 70.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.label(egui::RichText::new("PIN")
                                    .size(18.0)
                                    .color(theme::TEXT_SECONDARY));
                                ui.add_space(6.0);
                                let resp = ui.add_sized(
                                    [340.0, 38.0],
                                    egui::TextEdit::singleline(&mut auth_state.login_form.pin)
                                        .id(egui::Id::new("login_pin"))
                                        .password(true)
                                        .font(egui::FontId::proportional(18.0))
                                );
                                pin_rect = Some(resp.rect);
                            }
                        );

                        ui.add_space(25.0);

                        if let Some(ref err) = auth_state.login_form.error {
                            ui.label(egui::RichText::new(err)
                                .size(15.0)
                                .color(theme::ACCENT_RED));
                            ui.add_space(10.0);
                        }

                        let can_login = !auth_state.login_form.email.is_empty()
                            && auth_state.login_form.pin.len() >= 4;

                        ui.add_enabled_ui(can_login, |ui| {
                            let btn = egui::Button::new(
                                egui::RichText::new("LOGIN")
                                    .size(20.0)
                                    .color(theme::TEXT_PRIMARY)
                            ).fill(theme::BTN_PRIMARY);
                            let resp = ui.add_sized([200.0, 48.0], btn);
                            login_rect = Some(resp.rect);
                            if resp.clicked() {
                                auth_events.send(AuthEvent::Login {
                                    email: auth_state.login_form.email.clone(),
                                    pin: auth_state.login_form.pin.clone(),
                                });
                            }
                        });
                    });
                });

            ui.add_space(35.0);

            // vault status indicator
            let (status_text, status_color) = match &vault.status {
                VaultStatus::Disconnected => ("vault: offline", theme::TEXT_MUTED),
                VaultStatus::Connecting => ("vault: connecting...", theme::ACCENT_GOLD),
                VaultStatus::Connected => ("vault: connected", theme::ACCENT_GREEN),
                VaultStatus::Error(e) => (e.as_str(), theme::ACCENT_RED),
            };
            ui.label(egui::RichText::new(status_text).size(14.0).color(status_color));

            ui.add_space(8.0);
            ui.label(egui::RichText::new("test mode - keys derived locally")
                .size(13.0)
                .color(theme::TEXT_MUTED));
        });
    });

    // register hints for login form when hint mode is active
    if hint_state.active {
        if let Some(rect) = email_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(), // label assigned by generate_hints
                pos: rect.center(),
                action: HintAction::FocusInput("login_email".to_string()),
            });
        }
        if let Some(rect) = pin_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::FocusInput("login_pin".to_string()),
            });
        }
        if let Some(rect) = login_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
    }
}

fn render_main(ctx: &egui::Context, lobby: &mut LobbyState, auth_state: &AuthState, vault: &VaultConnection, hint_state: &mut HintModeState, debug_enabled: bool) {
    let mut create_rect: Option<egui::Rect> = None;
    let mut join_rect: Option<egui::Rect> = None;
    let mut faucet_rect: Option<egui::Rect> = None;
    let mut demo_rect: Option<egui::Rect> = None;

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);

            // title
            ui.label(egui::RichText::new("GHETTOBOX")
                .size(48.0)
                .strong()
                .color(theme::ACCENT_GOLD));
            ui.label(egui::RichText::new("POKER")
                .size(36.0)
                .color(theme::TEXT_PRIMARY));

            ui.add_space(15.0);

            // user info
            if let Some(ref addr) = auth_state.account_address {
                ui.label(egui::RichText::new(format!("{}...", &addr[..20]))
                    .size(14.0)
                    .color(theme::TEXT_MUTED));
            }

            ui.add_space(25.0);

            // balance card
            egui::Frame::none()
                .fill(theme::BG_CARD)
                .stroke(egui::Stroke::new(2.0, theme::ACCENT_GOLD.linear_multiply(0.5)))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(40.0, 20.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{}", lobby.balance))
                            .size(36.0)
                            .strong()
                            .color(theme::ACCENT_GOLD));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("chips")
                            .size(22.0)
                            .color(theme::TEXT_SECONDARY));
                        ui.add_space(30.0);
                        let faucet_btn = egui::Button::new(
                            egui::RichText::new("+ FREE CHIPS")
                                .size(16.0)
                                .color(theme::TEXT_PRIMARY)
                        ).fill(theme::ACCENT_GREEN.linear_multiply(0.7));
                        let resp = ui.add_sized([140.0, 40.0], faucet_btn);
                        faucet_rect = Some(resp.rect);
                        if resp.clicked() {
                            lobby.balance += 1000;
                            info!("faucet: +1000 chips (total: {})", lobby.balance);
                        }
                    });
                });

            ui.add_space(45.0);

            // main action buttons
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 2.0 - 210.0);

                let create_btn = egui::Button::new(
                    egui::RichText::new("CREATE TABLE")
                        .size(20.0)
                        .color(theme::TEXT_PRIMARY)
                ).fill(theme::BTN_PRIMARY);
                let resp = ui.add_sized([190.0, 75.0], create_btn);
                create_rect = Some(resp.rect);
                if resp.clicked() {
                    lobby.view = LobbyView::CreateTable;
                    lobby.create_form = CreateTableForm {
                        stakes: StakesPreset::Micro,
                        seats: 6,
                    };
                }

                ui.add_space(25.0);

                let join_btn = egui::Button::new(
                    egui::RichText::new("JOIN TABLE")
                        .size(20.0)
                        .color(theme::TEXT_PRIMARY)
                ).fill(theme::ACCENT_BLUE);
                let resp = ui.add_sized([190.0, 75.0], join_btn);
                join_rect = Some(resp.rect);
                if resp.clicked() {
                    lobby.view = LobbyView::JoinTable;
                    lobby.join_form = JoinTableForm::default();
                }
            });

            ui.add_space(40.0);

            // demo button (only in debug mode)
            if debug_enabled {
                ui.label(egui::RichText::new("— or —")
                    .size(15.0)
                    .color(theme::TEXT_MUTED));
                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 180.0);

                    let demo_btn = egui::Button::new(
                        egui::RichText::new("QUICK PLAY")
                            .size(17.0)
                            .color(theme::TEXT_PRIMARY)
                    ).fill(theme::BTN_SECONDARY);
                    let resp = ui.add_sized([150.0, 45.0], demo_btn);
                    demo_rect = Some(resp.rect);
                    if resp.clicked() {
                        lobby.connected = true;
                    }

                    ui.add_space(15.0);

                    let console_btn = egui::Button::new(
                        egui::RichText::new("CONSOLE (`)")
                            .size(17.0)
                            .color(theme::TEXT_PRIMARY)
                    ).fill(theme::BG_CARD);
                    if ui.add_sized([150.0, 45.0], console_btn).clicked() {
                        lobby.console_open = !lobby.console_open;
                    }
                });
            }

            ui.add_space(35.0);

            // vault status
            let (status_text, status_color) = match &vault.status {
                VaultStatus::Disconnected => ("vault: offline".to_string(), theme::TEXT_MUTED),
                VaultStatus::Connecting => ("vault: connecting...".to_string(), theme::ACCENT_GOLD),
                VaultStatus::Connected => (format!("vault: online ({}/3 nodes)", vault.connected_count), theme::ACCENT_GREEN),
                VaultStatus::Error(e) => (e.clone(), theme::ACCENT_RED),
            };
            ui.label(egui::RichText::new(status_text).size(13.0).color(status_color));

            // show any error
            if let Some(ref err) = lobby.error {
                ui.add_space(15.0);
                ui.label(egui::RichText::new(err)
                    .size(15.0)
                    .color(theme::ACCENT_RED));
            }
        });
    });

    // register hints for main view when hint mode is active
    if hint_state.active {
        if let Some(rect) = create_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
        if let Some(rect) = join_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
        if let Some(rect) = faucet_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
        if let Some(rect) = demo_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
    }
}

fn render_create_table(ctx: &egui::Context, lobby: &mut LobbyState) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);

            ui.label(egui::RichText::new("CREATE TABLE")
                .size(38.0)
                .strong()
                .color(theme::TEXT_PRIMARY));

            ui.add_space(35.0);

            egui::Frame::none()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(2.0, theme::BORDER))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(egui::Margin::same(30.0))
                .show(ui, |ui| {
                    ui.set_min_width(480.0);

                    // stakes selection
                    ui.label(egui::RichText::new("Stakes")
                        .size(20.0)
                        .color(theme::TEXT_SECONDARY));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(30.0);
                        for preset in [StakesPreset::Micro, StakesPreset::Low, StakesPreset::Medium, StakesPreset::High] {
                            let selected = lobby.create_form.stakes == preset;
                            let fill = if selected { theme::ACCENT_BLUE } else { theme::BG_CARD };
                            let text_color = if selected { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
                            let btn = egui::Button::new(
                                egui::RichText::new(preset.label())
                                    .size(17.0)
                                    .color(text_color)
                            ).fill(fill);
                            if ui.add_sized([85.0, 42.0], btn).clicked() {
                                lobby.create_form.stakes = preset;
                            }
                            ui.add_space(8.0);
                        }
                    });

                    ui.add_space(25.0);

                    // seats selection
                    ui.label(egui::RichText::new("Seats")
                        .size(20.0)
                        .color(theme::TEXT_SECONDARY));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(100.0);
                        for seats in [2u8, 6, 9] {
                            let selected = lobby.create_form.seats == seats;
                            let fill = if selected { theme::ACCENT_BLUE } else { theme::BG_CARD };
                            let text_color = if selected { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
                            let btn = egui::Button::new(
                                egui::RichText::new(format!("{}", seats))
                                    .size(18.0)
                                    .color(text_color)
                            ).fill(fill);
                            if ui.add_sized([65.0, 42.0], btn).clicked() {
                                lobby.create_form.seats = seats;
                            }
                            ui.add_space(12.0);
                        }
                    });

                    ui.add_space(25.0);

                    // min buy-in info
                    let min_buy = lobby.create_form.stakes.min_buy_in();
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.label(egui::RichText::new("Min buy-in:")
                            .size(17.0)
                            .color(theme::TEXT_SECONDARY));
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(format!("{} chips", min_buy))
                            .size(17.0)
                            .color(theme::ACCENT_GOLD));
                    });

                    // check if enough balance
                    let can_create = lobby.balance >= min_buy;
                    if !can_create {
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("not enough chips - get more from faucet")
                            .size(15.0)
                            .color(theme::ACCENT_RED));
                    }

                    ui.add_space(30.0);

                    ui.horizontal(|ui| {
                        ui.add_space(90.0);
                        let back_btn = egui::Button::new(
                            egui::RichText::new("BACK")
                                .size(17.0)
                                .color(theme::TEXT_PRIMARY)
                        ).fill(theme::BTN_SECONDARY);
                        if ui.add_sized([110.0, 45.0], back_btn).clicked() {
                            lobby.view = LobbyView::Main;
                        }

                        ui.add_space(25.0);

                        ui.add_enabled_ui(can_create, |ui| {
                            let create_btn = egui::Button::new(
                                egui::RichText::new("CREATE")
                                    .size(17.0)
                                    .color(theme::TEXT_PRIMARY)
                            ).fill(theme::ACCENT_GREEN.linear_multiply(0.8));
                            if ui.add_sized([110.0, 45.0], create_btn).clicked() {
                                let code = generate_table_code();
                                lobby.created_code = Some(code);
                                lobby.view = LobbyView::WaitingForPlayers;
                                info!("created table with code: {:?}", lobby.created_code);
                            }
                        });
                    });
                });
        });
    });
}

fn render_join_table(ctx: &egui::Context, lobby: &mut LobbyState, hint_state: &mut HintModeState) {
    let mut code_rect: Option<egui::Rect> = None;
    let mut back_rect: Option<egui::Rect> = None;
    let mut join_rect: Option<egui::Rect> = None;

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(50.0);

            ui.label(egui::RichText::new("JOIN TABLE")
                .size(38.0)
                .strong()
                .color(theme::TEXT_PRIMARY));

            ui.add_space(40.0);

            egui::Frame::none()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(2.0, theme::BORDER))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(egui::Margin::same(30.0))
                .show(ui, |ui| {
                    ui.set_min_width(450.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Enter table code")
                            .size(20.0)
                            .color(theme::TEXT_SECONDARY));
                        ui.add_space(18.0);

                        let resp = ui.add_sized(
                            [320.0, 48.0],
                            egui::TextEdit::singleline(&mut lobby.join_form.code)
                                .id(egui::Id::new("join_code"))
                                .hint_text("42-alpha-bravo")
                                .font(egui::FontId::monospace(24.0))
                        );
                        code_rect = Some(resp.rect);

                        ui.add_space(25.0);

                        // show any error
                        if let Some(ref err) = lobby.error {
                            ui.label(egui::RichText::new(err)
                                .size(15.0)
                                .color(theme::ACCENT_RED));
                            ui.add_space(15.0);
                        }

                        let can_join = !lobby.join_form.code.is_empty();

                        ui.horizontal(|ui| {
                            ui.add_space(65.0);
                            let back_btn = egui::Button::new(
                                egui::RichText::new("BACK")
                                    .size(17.0)
                                    .color(theme::TEXT_PRIMARY)
                            ).fill(theme::BTN_SECONDARY);
                            let resp = ui.add_sized([110.0, 45.0], back_btn);
                            back_rect = Some(resp.rect);
                            if resp.clicked() {
                                lobby.view = LobbyView::Main;
                                lobby.error = None;
                            }

                            ui.add_space(25.0);

                            ui.add_enabled_ui(can_join, |ui| {
                                let join_btn = egui::Button::new(
                                    egui::RichText::new("JOIN")
                                        .size(17.0)
                                        .color(theme::TEXT_PRIMARY)
                                ).fill(theme::BTN_PRIMARY);
                                let resp = ui.add_sized([110.0, 45.0], join_btn);
                                join_rect = Some(resp.rect);
                                if resp.clicked() {
                                    info!("joining table: {}", lobby.join_form.code);
                                    lobby.view = LobbyView::Connecting;
                                    lobby.error = None;
                                }
                            });
                        });
                    });
                });
        });
    });

    // register hints for join form when hint mode is active
    if hint_state.active {
        if let Some(rect) = code_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::FocusInput("join_code".to_string()),
            });
        }
        if let Some(rect) = back_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
        if let Some(rect) = join_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
    }
}

fn render_waiting(ctx: &egui::Context, lobby: &mut LobbyState, debug_enabled: bool) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(45.0);

            ui.label(egui::RichText::new("WAITING FOR PLAYERS")
                .size(34.0)
                .strong()
                .color(theme::TEXT_PRIMARY));

            ui.add_space(35.0);

            if let Some(ref code) = lobby.created_code {
                egui::Frame::none()
                    .fill(theme::BG_PANEL)
                    .stroke(egui::Stroke::new(2.0, theme::ACCENT_GOLD.linear_multiply(0.6)))
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(egui::Margin::same(30.0))
                    .show(ui, |ui| {
                        ui.set_min_width(480.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("Share this code with friends")
                                .size(19.0)
                                .color(theme::TEXT_SECONDARY));
                            ui.add_space(20.0);

                            // code display with highlight
                            egui::Frame::none()
                                .fill(theme::BG_DARK)
                                .rounding(egui::Rounding::same(8.0))
                                .inner_margin(egui::Margin::symmetric(30.0, 15.0))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(code)
                                        .monospace()
                                        .size(38.0)
                                        .strong()
                                        .color(theme::ACCENT_GOLD));
                                });

                            ui.add_space(20.0);

                            let copy_btn = egui::Button::new(
                                egui::RichText::new("COPY CODE")
                                    .size(16.0)
                                    .color(theme::TEXT_PRIMARY)
                            ).fill(theme::BTN_SECONDARY);
                            if ui.add_sized([160.0, 42.0], copy_btn).clicked() {
                                ui.output_mut(|o| o.copied_text = code.clone());
                                info!("copied table code to clipboard");
                            }
                        });
                    });
            }

            ui.add_space(30.0);

            // player count
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 2.0 - 80.0);
                ui.label(egui::RichText::new("Players:")
                    .size(20.0)
                    .color(theme::TEXT_SECONDARY));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("1/2")
                    .size(22.0)
                    .strong()
                    .color(theme::TEXT_PRIMARY));
            });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("(need at least 2 to start)")
                .size(14.0)
                .color(theme::TEXT_MUTED));

            ui.add_space(35.0);

            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 2.0 - 170.0);

                let cancel_btn = egui::Button::new(
                    egui::RichText::new("CANCEL")
                        .size(17.0)
                        .color(theme::TEXT_PRIMARY)
                ).fill(theme::BTN_SECONDARY);
                if ui.add_sized([130.0, 48.0], cancel_btn).clicked() {
                    lobby.view = LobbyView::Main;
                    lobby.created_code = None;
                }

                ui.add_space(25.0);

                // start only enabled with enough players or in debug mode
                if debug_enabled {
                    let start_btn = egui::Button::new(
                        egui::RichText::new("START (debug)")
                            .size(17.0)
                            .color(theme::TEXT_PRIMARY)
                    ).fill(theme::ACCENT_GREEN.linear_multiply(0.8));
                    if ui.add_sized([160.0, 48.0], start_btn).clicked() {
                        lobby.connected = true;
                    }
                } else {
                    let start_btn = egui::Button::new(
                        egui::RichText::new("WAITING...")
                            .size(17.0)
                            .color(theme::TEXT_MUTED)
                    ).fill(theme::BG_PANEL);
                    ui.add_sized([160.0, 48.0], start_btn);
                }
            });
        });
    });
}

fn render_connecting(ctx: &egui::Context, lobby: &mut LobbyState, debug_enabled: bool) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);

            ui.label(egui::RichText::new("CONNECTING")
                .size(38.0)
                .strong()
                .color(theme::ACCENT_BLUE));

            ui.add_space(35.0);

            ui.spinner();

            ui.add_space(35.0);

            ui.label(egui::RichText::new("Joining table")
                .size(18.0)
                .color(theme::TEXT_SECONDARY));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(&lobby.join_form.code)
                .monospace()
                .size(24.0)
                .color(theme::ACCENT_GOLD));

            ui.add_space(50.0);

            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() / 2.0 - 165.0);

                let cancel_btn = egui::Button::new(
                    egui::RichText::new("CANCEL")
                        .size(17.0)
                        .color(theme::TEXT_PRIMARY)
                ).fill(theme::BTN_SECONDARY);
                if ui.add_sized([130.0, 48.0], cancel_btn).clicked() {
                    lobby.view = LobbyView::JoinTable;
                }

                // skip to demo only in debug mode
                if debug_enabled {
                    ui.add_space(25.0);

                    let skip_btn = egui::Button::new(
                        egui::RichText::new("SKIP TO DEMO")
                            .size(17.0)
                            .color(theme::TEXT_PRIMARY)
                    ).fill(theme::BTN_PRIMARY);
                    if ui.add_sized([160.0, 48.0], skip_btn).clicked() {
                        lobby.connected = true;
                    }
                }
            });
        });
    });
}

/// generate a table code (simplified, real impl uses wordlist)
fn generate_table_code() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let words = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
        "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
        "quebec", "romeo", "sierra", "tango", "uniform", "victor", "whiskey", "xray",
        "yankee", "zulu",
    ];

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let n = (seed % 100) as u8;
    let w1 = words[(seed / 100 % 26) as usize];
    let w2 = words[(seed / 2600 % 26) as usize];

    format!("{}-{}-{}", n, w1, w2)
}

/// render console window (debug overlay)
fn render_console(ctx: &egui::Context, lobby: &mut LobbyState) {
    if !lobby.console_open {
        return;
    }

    egui::Window::new("Console")
        .anchor(egui::Align2::LEFT_BOTTOM, [10.0, -10.0])
        .default_width(600.0)
        .default_height(300.0)
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            // toolbar
            ui.horizontal(|ui| {
                if ui.button("Clear").clicked() {
                    lobby.console_logs.clear();
                }
                ui.label(format!("{} entries", lobby.console_logs.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("X").clicked() {
                        lobby.console_open = false;
                    }
                });
            });
            ui.separator();

            // log area with scroll
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for entry in &lobby.console_logs {
                        let color = match entry.level {
                            LogLevel::Info => theme::TEXT_SECONDARY,
                            LogLevel::Warn => theme::ACCENT_GOLD,
                            LogLevel::Error => theme::ACCENT_RED,
                            LogLevel::Debug => theme::TEXT_MUTED,
                        };
                        let level_str = match entry.level {
                            LogLevel::Info => "INF",
                            LogLevel::Warn => "WRN",
                            LogLevel::Error => "ERR",
                            LogLevel::Debug => "DBG",
                        };
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("[{}]", level_str))
                                .monospace()
                                .size(13.0)
                                .color(color));
                            ui.label(egui::RichText::new(&entry.message)
                                .monospace()
                                .size(13.0)
                                .color(theme::TEXT_PRIMARY));
                        });
                    }
                });
        });
}
