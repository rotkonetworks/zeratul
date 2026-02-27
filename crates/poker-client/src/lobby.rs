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
use crate::chain_client::ChainConnection;
use crate::multitable::{HintAction, HintModeState, HintTarget};
use crate::p2p::P2PNotification;
use crate::vault_client::{OprfVaultClient, OprfVaultNode, OprfHealthResponse};
use crate::DebugMode;
use ghettobox::ServerPublicKey;

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LobbyState>()
            .init_resource::<VaultConnection>()
            .init_resource::<PublicTables>()
            .add_systems(Update, (
                render_lobby,
                check_vault_status,
                sync_vault_to_auth,
                handle_p2p_notifications,
                connect_chain_on_login,
                sync_chain_balance,
                trigger_table_discovery,
                handle_pending_create,
            ));
    }
}

/// trigger P2P table discovery when requested
fn trigger_table_discovery(
    mut public_tables: ResMut<PublicTables>,
    mut p2p_commands: EventWriter<crate::p2p::P2PCommand>,
) {
    if public_tables.needs_refresh {
        public_tables.needs_refresh = false;
        public_tables.refreshing = true;
        p2p_commands.send(crate::p2p::P2PCommand::DiscoverTables);
    }
}

/// handle pending table creation
fn handle_pending_create(
    mut lobby: ResMut<LobbyState>,
    auth: Res<AuthState>,
    mut p2p_commands: EventWriter<crate::p2p::P2PCommand>,
) {
    if let Some(pending) = lobby.pending_create.take() {
        let (small_blind, big_blind) = pending.stakes.blinds();
        let rules = crate::p2p::TableRules {
            seats: pending.seats,
            small_blind: small_blind as u128,
            big_blind: big_blind as u128,
            min_buy_in: pending.stakes.min_buy_in() as u128,
            max_buy_in: (pending.stakes.min_buy_in() * 5) as u128,
            ante: 0,
            allow_spectators: true,
            max_spectators: 10,
        };

        let host_name = auth.account_address
            .as_ref()
            .map(|s| format!("{}...", &s[..8]))
            .unwrap_or_else(|| "Anonymous".to_string());

        let visibility = match pending.visibility {
            TableVisibility::Public => crate::p2p::TableVisibility::Public,
            TableVisibility::FriendsOnly => crate::p2p::TableVisibility::FriendsOnly,
            TableVisibility::Private => crate::p2p::TableVisibility::Private,
        };

        lobby.created_visibility = pending.visibility;

        p2p_commands.send(crate::p2p::P2PCommand::CreateTable {
            rules,
            visibility,
            host_name,
        });

        info!("lobby: requesting table creation with {:?} visibility", pending.visibility);
    }
}

/// connect to chain when user logs in
fn connect_chain_on_login(
    auth: Res<AuthState>,
    mut chain: ResMut<ChainConnection>,
    mut connected: Local<bool>,
) {
    // trigger chain connect once when auth becomes LoggedIn
    if auth.status == AuthStatus::LoggedIn && !*connected {
        *connected = true;

        // derive account from auth pubkey
        if let Some(ref addr) = auth.account_address {
            let mut account = [0u8; 32];
            // use first 32 bytes of address or hash
            let bytes = addr.as_bytes();
            let len = bytes.len().min(32);
            account[..len].copy_from_slice(&bytes[..len]);

            info!("chain: connecting with account from auth...");
            chain.connect("ws://127.0.0.1:9944", account);
        }
    }

    // reset when logged out
    if auth.status == AuthStatus::NotLoggedIn && *connected {
        *connected = false;
        chain.disconnect();
    }
}

/// sync chain balance to lobby
fn sync_chain_balance(
    chain: Res<ChainConnection>,
    mut lobby: ResMut<LobbyState>,
) {
    if chain.is_connected() {
        // use chain balance for lobby
        lobby.balance = chain.transferable_balance();
    }
}

/// sync vault connection to auth config when vault connects
fn sync_vault_to_auth(
    vault: Res<VaultConnection>,
    mut auth_config: ResMut<crate::auth::VaultConfig>,
    mut synced: Local<bool>,
) {
    // only sync once when vault becomes connected and has nodes
    if !*synced && matches!(vault.status, VaultStatus::Connected) && !vault.nodes.is_empty() {
        *synced = true;
        auth_config.nodes = vault.nodes.clone();
        auth_config.threshold = 2; // 2-of-3 threshold
        info!("synced {} OPRF vault nodes to auth config", vault.nodes.len());
    }
}

/// handle P2P notifications
fn handle_p2p_notifications(
    mut lobby: ResMut<LobbyState>,
    mut public_tables: ResMut<PublicTables>,
    mut notifications: EventReader<P2PNotification>,
    friends: Res<crate::friends::FriendsState>,
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
            P2PNotification::TablesDiscovered { tables } => {
                info!("lobby: discovered {} tables", tables.len());
                public_tables.tables = tables.iter().map(|t| {
                    let host_is_friend = friends.is_friend_by_pubkey(&t.host_pubkey);
                    PublicTable {
                        code: t.code.clone(),
                        host: t.host_name.clone(),
                        stakes: t.stakes.clone(),
                        players: t.players,
                        friends_only: matches!(t.visibility, crate::p2p::TableVisibility::FriendsOnly),
                        host_is_friend,
                        announced_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                        ping_ms: Some(rand::random::<u32>() % 100 + 20), // mock ping
                    }
                }).collect();
                public_tables.refreshing = false;
                public_tables.last_refresh = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
            }
            P2PNotification::Error { message } => {
                lobby.error = Some(message.clone());
                warn!("lobby: p2p error - {}", message);
            }
        }
    }
}

/// vault connection state (uses OPRF protocol)
#[derive(Resource)]
pub struct VaultConnection {
    pub status: VaultStatus,
    pub node_urls: Vec<String>,
    pub nodes: Vec<OprfVaultNode>,
    pub connected_count: usize,
}

impl Default for VaultConnection {
    fn default() -> Self {
        Self {
            status: VaultStatus::Disconnected,
            node_urls: vec![
                "http://127.0.0.1:4200".to_string(),
                "http://127.0.0.1:4201".to_string(),
                "http://127.0.0.1:4202".to_string(),
            ],
            nodes: Vec::new(),
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
    /// created table visibility
    pub created_visibility: TableVisibility,
    /// pending table creation request
    pub pending_create: Option<PendingTableCreate>,
    /// error message
    pub error: Option<String>,
    /// connected to table
    pub connected: bool,
    /// console open
    pub console_open: bool,
    /// console log buffer
    pub console_logs: Vec<ConsoleEntry>,
}

/// pending table creation
#[derive(Clone, Debug)]
pub struct PendingTableCreate {
    pub stakes: StakesPreset,
    pub seats: u8,
    pub visibility: TableVisibility,
}

/// a public table listing
#[derive(Clone, Debug)]
pub struct PublicTable {
    /// table code to join
    pub code: String,
    /// host name
    pub host: String,
    /// stakes (e.g. "1/2")
    pub stakes: String,
    /// current players / max players
    pub players: (u8, u8),
    /// is friends-only?
    pub friends_only: bool,
    /// host is a friend?
    pub host_is_friend: bool,
    /// when the table was announced
    pub announced_at: u64,
    /// ping/latency in ms (if known)
    pub ping_ms: Option<u32>,
}

/// public tables resource
#[derive(Resource, Default)]
pub struct PublicTables {
    /// list of discovered tables
    pub tables: Vec<PublicTable>,
    /// last refresh timestamp
    pub last_refresh: u64,
    /// is currently refreshing?
    pub refreshing: bool,
    /// needs refresh (triggers P2P discovery)
    pub needs_refresh: bool,
    /// filter: show friends' tables only
    pub filter_friends: bool,
    /// filter: stakes preset
    pub filter_stakes: Option<StakesPreset>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LobbyView {
    #[default]
    Login,
    Main,
    CreateTable,
    JoinTable,
    BrowseTables,
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
    pub visibility: TableVisibility,
}

/// table visibility setting
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TableVisibility {
    /// anyone can find and join
    #[default]
    Public,
    /// only friends can see in browser
    FriendsOnly,
    /// private - need code to join
    Private,
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
    pub fn label(&self) -> &'static str {
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

/// OPRF vault check result
pub struct OprfVaultCheckResult {
    pub total: usize,
    pub connected: usize,
    pub nodes: Vec<OprfVaultNode>,
}

/// check vault testnet status and poll for results (OPRF protocol)
fn check_vault_status(
    mut vault: ResMut<VaultConnection>,
    mut receiver: Local<Option<mpsc::Receiver<OprfVaultCheckResult>>>,
    mut checked: Local<bool>,
) {
    // start check once at startup
    if !*checked {
        *checked = true;
        vault.status = VaultStatus::Connecting;
        info!("vault: checking OPRF testnet nodes...");

        // spawn async vault check
        let (tx, rx) = mpsc::channel();
        *receiver = Some(rx);

        let urls = vault.node_urls.clone();
        std::thread::spawn(move || {
            let mut connected = 0;
            let mut nodes = Vec::new();

            for (i, url) in urls.iter().enumerate() {
                // try to fetch OPRF health (includes public key)
                if let Ok(resp) = ureq::get(&format!("{}/oprf/health", url))
                    .timeout(std::time::Duration::from_secs(3))
                    .call()
                {
                    if let Ok(health) = resp.into_json::<OprfHealthResponse>() {
                        if health.ok {
                            // decode public key
                            if let Ok(pk_bytes) = hex::decode(&health.oprf_pubkey) {
                                if pk_bytes.len() == 32 {
                                    let mut public_key = [0u8; 32];
                                    public_key.copy_from_slice(&pk_bytes);

                                    nodes.push(OprfVaultNode {
                                        url: url.clone(),
                                        oprf_pubkey: ServerPublicKey {
                                            index: health.index,
                                            public_key,
                                        },
                                        index: health.index,
                                    });
                                    connected += 1;
                                }
                            }
                        }
                    }
                }
            }

            let _ = tx.send(OprfVaultCheckResult {
                total: urls.len(),
                connected,
                nodes,
            });
        });
    }

    // poll for results
    if let Some(ref rx) = *receiver {
        if let Ok(result) = rx.try_recv() {
            vault.connected_count = result.connected;
            vault.nodes = result.nodes.clone();

            if result.connected >= 2 {
                vault.status = VaultStatus::Connected;
                info!("vault: connected to {}/{} OPRF nodes", result.connected, result.total);
                for node in &result.nodes {
                    info!("  {} - node {} (oprf pubkey: {}...)",
                        node.url, node.index, &hex::encode(&node.oprf_pubkey.public_key)[..16]);
                }
            } else if result.connected > 0 {
                vault.status = VaultStatus::Error(format!(
                    "only {}/{} nodes online (need 2)",
                    result.connected, result.total
                ));
                warn!("vault: insufficient OPRF nodes for threshold");
            } else {
                vault.status = VaultStatus::Disconnected;
                warn!("vault: no OPRF nodes reachable");
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
    mut chain: ResMut<ChainConnection>,
    mut friends: ResMut<crate::friends::FriendsState>,
    mut public_tables: ResMut<PublicTables>,
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
        LobbyView::Main => render_main_inner(ctx, &mut lobby, &auth_state, &*vault, &mut chain, &mut friends, &mut hint_state, debug_mode.enabled),
        LobbyView::CreateTable => render_create_table(ctx, &mut lobby),
        LobbyView::JoinTable => render_join_table(ctx, &mut lobby, &mut hint_state),
        LobbyView::BrowseTables => render_browse_tables(ctx, &mut lobby, &mut public_tables, &friends),
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

fn render_main_inner(
    ctx: &egui::Context,
    lobby: &mut LobbyState,
    auth_state: &AuthState,
    vault: &VaultConnection,
    chain: &mut ChainConnection,
    friends: &mut crate::friends::FriendsState,
    hint_state: &mut HintModeState,
    debug_enabled: bool,
) {
    let mut create_rect: Option<egui::Rect> = None;
    let mut join_rect: Option<egui::Rect> = None;
    let mut faucet_rect: Option<egui::Rect> = None;
    let mut demo_rect: Option<egui::Rect> = None;
    let mut friends_rect: Option<egui::Rect> = None;

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
                            // add to chain balance (will sync to lobby)
                            chain.add_mock_balance(1_000_000_000_000); // 1000 chips in base units
                            lobby.balance = chain.transferable_balance();
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
                        visibility: TableVisibility::Public,
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

            ui.add_space(20.0);

            // browse tables button
            let browse_btn = egui::Button::new(
                egui::RichText::new("BROWSE TABLES")
                    .size(17.0)
                    .color(theme::TEXT_PRIMARY)
            ).fill(theme::BTN_SECONDARY);
            if ui.add_sized([200.0, 50.0], browse_btn).clicked() {
                lobby.view = LobbyView::BrowseTables;
            }

            ui.add_space(15.0);

            // friends button
            let friends_btn = egui::Button::new(
                egui::RichText::new(format!("FRIENDS ({})", friends.friends().len()))
                    .size(17.0)
                    .color(theme::TEXT_PRIMARY)
            ).fill(theme::BTN_SECONDARY);
            let resp = ui.add_sized([160.0, 45.0], friends_btn);
            friends_rect = Some(resp.rect);
            if resp.clicked() {
                friends.panel_open = !friends.panel_open;
            }

            ui.add_space(25.0);

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

            // chain status
            let (chain_text, chain_color) = match &chain.state {
                crate::chain::ChainState::Disconnected => ("chain: offline".to_string(), theme::TEXT_MUTED),
                crate::chain::ChainState::Connecting => ("chain: connecting...".to_string(), theme::ACCENT_GOLD),
                crate::chain::ChainState::Syncing { finalized, target } => (format!("chain: syncing {}/{}", finalized, target), theme::ACCENT_GOLD),
                crate::chain::ChainState::Connected { finalized } => (format!("chain: block #{}", finalized), theme::ACCENT_GREEN),
                crate::chain::ChainState::Error(e) => (format!("chain: {}", e), theme::ACCENT_RED),
            };
            ui.label(egui::RichText::new(chain_text).size(13.0).color(chain_color));

            // show any error
            if let Some(ref err) = lobby.error {
                ui.add_space(15.0);
                ui.label(egui::RichText::new(err)
                    .size(15.0)
                    .color(theme::ACCENT_RED));
            }
        });
    });

    // friends panel (side window)
    if friends.panel_open {
        render_friends_panel(ctx, friends, lobby);
    }

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
        if let Some(rect) = friends_rect {
            hint_state.external_hints.push(HintTarget {
                label: String::new(),
                pos: rect.center(),
                action: HintAction::Click(rect.center()),
            });
        }
    }
}

/// render the friends panel
fn render_friends_panel(
    ctx: &egui::Context,
    friends: &mut crate::friends::FriendsState,
    lobby: &LobbyState,
) {
    use crate::friends::FriendsView;

    egui::Window::new("Friends & Playmates")
        .default_width(350.0)
        .default_height(500.0)
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            // tab bar
            ui.horizontal(|ui| {
                if ui.selectable_label(friends.view == FriendsView::Friends, "Friends").clicked() {
                    friends.view = FriendsView::Friends;
                }
                if ui.selectable_label(friends.view == FriendsView::RecentPlaymates, "Recent").clicked() {
                    friends.view = FriendsView::RecentPlaymates;
                }
                if ui.selectable_label(friends.view == FriendsView::Stats, "Stats").clicked() {
                    friends.view = FriendsView::Stats;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("X").clicked() {
                        friends.panel_open = false;
                    }
                });
            });

            ui.separator();

            // search bar
            ui.horizontal(|ui| {
                ui.label("Search:");
                ui.text_edit_singleline(&mut friends.search);
            });

            ui.separator();

            match friends.view {
                FriendsView::Friends => {
                    let friend_list = friends.friends();
                    if friend_list.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new("No friends yet")
                                .size(18.0)
                                .color(theme::TEXT_MUTED));
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("Play with someone and add them as a friend!")
                                .size(14.0)
                                .color(theme::TEXT_MUTED));
                        });
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for playmate in friend_list {
                                render_playmate_card(ui, playmate, lobby, friends);
                            }
                        });
                    }
                }

                FriendsView::RecentPlaymates => {
                    let recent = if friends.search.is_empty() {
                        friends.recent()
                    } else {
                        friends.search_playmates(&friends.search)
                    };

                    if recent.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new("No recent playmates")
                                .size(18.0)
                                .color(theme::TEXT_MUTED));
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("Join a table to meet other players")
                                .size(14.0)
                                .color(theme::TEXT_MUTED));
                        });
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for playmate in recent {
                                render_playmate_card(ui, playmate, lobby, friends);
                            }
                        });
                    }
                }

                FriendsView::Stats => {
                    ui.add_space(20.0);

                    egui::Grid::new("stats_grid")
                        .num_columns(2)
                        .spacing([40.0, 10.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Total playmates:").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new(format!("{}", friends.total_playmates()))
                                .strong()
                                .color(theme::ACCENT_GOLD));
                            ui.end_row();

                            ui.label(egui::RichText::new("Friends:").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new(format!("{}", friends.friends().len()))
                                .strong()
                                .color(theme::ACCENT_GREEN));
                            ui.end_row();

                            ui.label(egui::RichText::new("Total games:").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new(format!("{}", friends.total_games()))
                                .strong()
                                .color(theme::TEXT_PRIMARY));
                            ui.end_row();

                            ui.label(egui::RichText::new("Win rate:").color(theme::TEXT_SECONDARY));
                            let wr = friends.overall_win_rate();
                            let wr_color = if wr > 0.5 { theme::ACCENT_GREEN } else if wr < 0.5 { theme::ACCENT_RED } else { theme::TEXT_PRIMARY };
                            ui.label(egui::RichText::new(format!("{:.1}%", wr * 100.0))
                                .strong()
                                .color(wr_color));
                            ui.end_row();
                        });
                }

                FriendsView::Blocked => {
                    let blocked = friends.blocked();
                    if blocked.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new("No blocked players")
                                .color(theme::TEXT_MUTED));
                        });
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for playmate in blocked {
                                render_playmate_card(ui, playmate, lobby, friends);
                            }
                        });
                    }
                }
            }
        });
}

/// render a single playmate card
fn render_playmate_card(
    ui: &mut egui::Ui,
    playmate: &crate::friends::Playmate,
    lobby: &LobbyState,
    _friends: &crate::friends::FriendsState,
) {
    egui::Frame::none()
        .fill(theme::BG_CARD)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // friend indicator
                if playmate.is_friend {
                    ui.label(egui::RichText::new("*")
                        .size(20.0)
                        .color(theme::ACCENT_GOLD));
                }

                ui.vertical(|ui| {
                    // name
                    ui.label(egui::RichText::new(playmate.name())
                        .size(16.0)
                        .strong()
                        .color(theme::TEXT_PRIMARY));

                    // stats
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{} games", playmate.games_together))
                            .size(12.0)
                            .color(theme::TEXT_MUTED));

                        ui.label(egui::RichText::new("|")
                            .size(12.0)
                            .color(theme::TEXT_MUTED));

                        let wr = playmate.win_rate();
                        let wr_color = if wr > 0.5 { theme::ACCENT_GREEN } else if wr < 0.5 { theme::ACCENT_RED } else { theme::TEXT_MUTED };
                        ui.label(egui::RichText::new(format!("{:.0}% WR", wr * 100.0))
                            .size(12.0)
                            .color(wr_color));
                    });

                    // last played
                    let ago = format_time_ago(playmate.last_played);
                    ui.label(egui::RichText::new(format!("last: {}", ago))
                        .size(11.0)
                        .color(theme::TEXT_MUTED));
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // invite button (only if we have a table)
                    if lobby.created_code.is_some() {
                        if ui.small_button("Invite").clicked() {
                            // TODO: send invite event
                        }
                    }
                });
            });
        });

    ui.add_space(6.0);
}

/// format timestamp as "X ago"
fn format_time_ago(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let diff = now.saturating_sub(timestamp);

    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 604800 {
        format!("{}d ago", diff / 86400)
    } else {
        format!("{}w ago", diff / 604800)
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

                    // visibility selection
                    ui.label(egui::RichText::new("Visibility")
                        .size(20.0)
                        .color(theme::TEXT_SECONDARY));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(50.0);
                        for vis in [TableVisibility::Public, TableVisibility::FriendsOnly, TableVisibility::Private] {
                            let selected = lobby.create_form.visibility == vis;
                            let (label, desc) = match vis {
                                TableVisibility::Public => ("PUBLIC", "anyone can find"),
                                TableVisibility::FriendsOnly => ("FRIENDS", "friends only"),
                                TableVisibility::Private => ("PRIVATE", "code only"),
                            };
                            let fill = if selected { theme::ACCENT_BLUE } else { theme::BG_CARD };
                            let text_color = if selected { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };

                            let btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .size(15.0)
                                    .color(text_color)
                            ).fill(fill);
                            let resp = ui.add_sized([95.0, 42.0], btn);
                            if resp.clicked() {
                                lobby.create_form.visibility = vis;
                            }
                            resp.on_hover_text(desc);
                            ui.add_space(8.0);
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
                                // queue table creation request
                                lobby.pending_create = Some(PendingTableCreate {
                                    stakes: lobby.create_form.stakes,
                                    seats: lobby.create_form.seats,
                                    visibility: lobby.create_form.visibility,
                                });
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

fn render_browse_tables(
    ctx: &egui::Context,
    lobby: &mut LobbyState,
    public_tables: &mut PublicTables,
    friends: &crate::friends::FriendsState,
) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);

            ui.label(egui::RichText::new("BROWSE TABLES")
                .size(38.0)
                .strong()
                .color(theme::TEXT_PRIMARY));

            ui.add_space(25.0);

            // filter bar
            egui::Frame::none()
                .fill(theme::BG_PANEL)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::same(15.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // refresh button
                        let refresh_text = if public_tables.refreshing { "..." } else { "REFRESH" };
                        let refresh_btn = egui::Button::new(
                            egui::RichText::new(refresh_text)
                                .size(14.0)
                                .color(theme::TEXT_PRIMARY)
                        ).fill(theme::BTN_SECONDARY);
                        if ui.add_sized([90.0, 32.0], refresh_btn).clicked() && !public_tables.refreshing {
                            public_tables.needs_refresh = true;
                        }

                        ui.add_space(20.0);

                        // friends filter toggle
                        let friends_label = if public_tables.filter_friends { "FRIENDS [x]" } else { "FRIENDS [ ]" };
                        let friends_btn = egui::Button::new(
                            egui::RichText::new(friends_label)
                                .size(13.0)
                                .color(if public_tables.filter_friends { theme::ACCENT_GOLD } else { theme::TEXT_SECONDARY })
                        ).fill(theme::BG_CARD);
                        if ui.add_sized([100.0, 32.0], friends_btn).clicked() {
                            public_tables.filter_friends = !public_tables.filter_friends;
                        }

                        ui.add_space(10.0);

                        // stakes filter
                        egui::ComboBox::from_id_source("stakes_filter")
                            .selected_text(
                                public_tables.filter_stakes
                                    .map(|s| s.label())
                                    .unwrap_or("All Stakes")
                            )
                            .show_ui(ui, |ui: &mut egui::Ui| {
                                if ui.selectable_label(public_tables.filter_stakes.is_none(), "All Stakes").clicked() {
                                    public_tables.filter_stakes = None;
                                }
                                for preset in [StakesPreset::Micro, StakesPreset::Low, StakesPreset::Medium, StakesPreset::High] {
                                    if ui.selectable_label(public_tables.filter_stakes == Some(preset), preset.label()).clicked() {
                                        public_tables.filter_stakes = Some(preset);
                                    }
                                }
                            });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{} tables", public_tables.tables.len()))
                                .size(14.0)
                                .color(theme::TEXT_MUTED));
                        });
                    });
                });

            ui.add_space(20.0);

            // table list
            let filtered_tables: Vec<&PublicTable> = public_tables.tables.iter()
                .filter(|t| {
                    if public_tables.filter_friends && !t.host_is_friend {
                        return false;
                    }
                    if let Some(stakes) = public_tables.filter_stakes {
                        if t.stakes != stakes.label() {
                            return false;
                        }
                    }
                    true
                })
                .collect();

            if filtered_tables.is_empty() {
                ui.add_space(60.0);
                ui.label(egui::RichText::new("No tables found")
                    .size(22.0)
                    .color(theme::TEXT_MUTED));
                ui.add_space(15.0);
                ui.label(egui::RichText::new("Click REFRESH to search for tables")
                    .size(16.0)
                    .color(theme::TEXT_MUTED));
            } else {
                egui::ScrollArea::vertical()
                    .max_height(350.0)
                    .show(ui, |ui| {
                        for table in filtered_tables {
                            render_table_row(ui, table, lobby, friends);
                        }
                    });
            }

            ui.add_space(30.0);

            // back button
            let back_btn = egui::Button::new(
                egui::RichText::new("BACK")
                    .size(17.0)
                    .color(theme::TEXT_PRIMARY)
            ).fill(theme::BTN_SECONDARY);
            if ui.add_sized([120.0, 45.0], back_btn).clicked() {
                lobby.view = LobbyView::Main;
            }
        });
    });
}

fn render_table_row(
    ui: &mut egui::Ui,
    table: &PublicTable,
    lobby: &mut LobbyState,
    _friends: &crate::friends::FriendsState,
) {
    egui::Frame::none()
        .fill(theme::BG_CARD)
        .stroke(egui::Stroke::new(1.0, if table.host_is_friend { theme::ACCENT_GOLD.linear_multiply(0.5) } else { theme::BORDER }))
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(20.0, 12.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // friend indicator
                if table.host_is_friend {
                    ui.label(egui::RichText::new("*")
                        .size(20.0)
                        .color(theme::ACCENT_GOLD));
                }

                // host name
                ui.label(egui::RichText::new(&table.host)
                    .size(16.0)
                    .strong()
                    .color(theme::TEXT_PRIMARY));

                ui.add_space(15.0);

                // stakes
                ui.label(egui::RichText::new(&table.stakes)
                    .size(15.0)
                    .color(theme::ACCENT_GOLD));

                ui.add_space(15.0);

                // players
                let (current, max) = table.players;
                let players_color = if current >= max { theme::ACCENT_RED } else { theme::TEXT_SECONDARY };
                ui.label(egui::RichText::new(format!("{}/{} players", current, max))
                    .size(14.0)
                    .color(players_color));

                // friends only badge
                if table.friends_only {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("[friends]")
                        .size(12.0)
                        .color(theme::ACCENT_BLUE));
                }

                // ping
                if let Some(ping) = table.ping_ms {
                    ui.add_space(10.0);
                    let ping_color = if ping < 50 { theme::ACCENT_GREEN }
                        else if ping < 150 { theme::ACCENT_GOLD }
                        else { theme::ACCENT_RED };
                    ui.label(egui::RichText::new(format!("{}ms", ping))
                        .size(12.0)
                        .color(ping_color));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (current, max) = table.players;
                    let can_join = current < max;

                    ui.add_enabled_ui(can_join, |ui| {
                        let join_btn = egui::Button::new(
                            egui::RichText::new("JOIN")
                                .size(14.0)
                                .color(theme::TEXT_PRIMARY)
                        ).fill(theme::BTN_PRIMARY);
                        if ui.add_sized([70.0, 32.0], join_btn).clicked() {
                            lobby.join_form.code = table.code.clone();
                            lobby.view = LobbyView::Connecting;
                        }
                    });
                });
            });
        });

    ui.add_space(6.0);
}

/// generate mock tables for testing
fn generate_mock_tables() -> Vec<PublicTable> {
    vec![
        PublicTable {
            code: "42-alpha-bravo".to_string(),
            host: "CryptoKing".to_string(),
            stakes: "1/2".to_string(),
            players: (3, 6),
            friends_only: false,
            host_is_friend: false,
            announced_at: 0,
            ping_ms: Some(35),
        },
        PublicTable {
            code: "17-delta-echo".to_string(),
            host: "AceHunter".to_string(),
            stakes: "5/10".to_string(),
            players: (5, 9),
            friends_only: false,
            host_is_friend: true,
            announced_at: 0,
            ping_ms: Some(48),
        },
        PublicTable {
            code: "88-golf-hotel".to_string(),
            host: "PokerPro99".to_string(),
            stakes: "25/50".to_string(),
            players: (2, 6),
            friends_only: true,
            host_is_friend: true,
            announced_at: 0,
            ping_ms: Some(72),
        },
        PublicTable {
            code: "33-mike-november".to_string(),
            host: "NightOwl".to_string(),
            stakes: "1/2".to_string(),
            players: (6, 6),
            friends_only: false,
            host_is_friend: false,
            announced_at: 0,
            ping_ms: Some(120),
        },
    ]
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
                            // visibility badge
                            let (vis_text, vis_color, vis_desc) = match lobby.created_visibility {
                                TableVisibility::Public => ("PUBLIC", theme::ACCENT_GREEN, "Anyone can find and join"),
                                TableVisibility::FriendsOnly => ("FRIENDS ONLY", theme::ACCENT_BLUE, "Only friends can see this table"),
                                TableVisibility::Private => ("PRIVATE", theme::TEXT_MUTED, "Share the code to invite players"),
                            };
                            ui.label(egui::RichText::new(vis_text)
                                .size(14.0)
                                .color(vis_color));
                            ui.label(egui::RichText::new(vis_desc)
                                .size(16.0)
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
