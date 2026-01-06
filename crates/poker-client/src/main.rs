//! poker client - professional multi-table poker client
//!
//! uses ghettobox for extension-free web3 identity (email + PIN)
//!
//! supports 240hz+ displays with webgpu/webgl and optional vsync

mod auth;
mod blockout;
mod lobby;
mod multitable;
mod p2p;
mod settings;
mod storage;
mod table_2d;
mod tiling;
mod vault_client;

// chat and voice
mod chat;
mod voice;
mod mental_poker;
// mod wallet_ui;

use bevy::prelude::*;
use bevy::window::PresentMode;
use bevy_egui::EguiPlugin;
use tiling::{TilingState, WindowKind};

/// frame rate settings
#[derive(Resource, Clone, Copy)]
pub struct FrameRateSettings {
    /// target fps (0 = unlimited)
    pub target_fps: u32,
    /// vsync mode
    pub vsync: bool,
    /// competitive mode (minimum input lag)
    pub competitive: bool,
}

impl Default for FrameRateSettings {
    fn default() -> Self {
        Self {
            target_fps: 240,
            vsync: true,
            competitive: false,
        }
    }
}

/// debug mode - enables demo shortcuts, hidden by default
/// toggle with F12 or enable with POKER_DEBUG=1 env var
#[derive(Resource)]
pub struct DebugMode {
    pub enabled: bool,
}

impl Default for DebugMode {
    fn default() -> Self {
        // check env var
        let enabled = std::env::var("POKER_DEBUG").map(|v| v == "1").unwrap_or(false);
        Self { enabled }
    }
}

fn main() {
    let frame_settings = FrameRateSettings::default();

    // select present mode based on settings
    let present_mode = if frame_settings.competitive {
        // minimum latency - no vsync, immediate present
        PresentMode::Immediate
    } else if frame_settings.vsync {
        // smooth with vsync - mailbox for low latency vsync
        PresentMode::Mailbox
    } else {
        // no vsync, fifo fallback
        PresentMode::Fifo
    };

    App::new()
        .insert_resource(frame_settings)
        .init_resource::<DebugMode>()
        .add_plugins(DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ghettobox poker".into(),
                    resolution: (1280., 720.).into(),
                    present_mode,
                    resizable: true,
                    ..default()
                }),
                ..default()
            })
            .set(ImagePlugin::default_nearest()))
        .add_plugins(EguiPlugin)
        .add_plugins(tiling::TilingPlugin)
        .add_plugins(auth::AuthPlugin)
        .add_plugins(lobby::LobbyPlugin)
        .add_plugins(multitable::MultiTablePlugin)
        .add_plugins(p2p::P2PPlugin)
        .add_plugins(settings::SettingsPlugin)
        .add_plugins(blockout::BlockoutPlugin)
        .add_plugins(chat::ChatPlugin)
        .add_plugins(voice::VoicePlugin)
        .add_systems(Startup, setup_initial_state)
        .add_systems(Update, (toggle_competitive_mode, toggle_training_game, toggle_debug_mode, handle_lobby_to_game_transition))
        .run();
}

/// toggle competitive mode with F1
fn toggle_competitive_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<FrameRateSettings>,
    mut windows: Query<&mut Window>,
) {
    if keys.just_pressed(KeyCode::F1) {
        settings.competitive = !settings.competitive;

        if let Ok(mut window) = windows.get_single_mut() {
            window.present_mode = if settings.competitive {
                info!("competitive mode: ON (vsync disabled, minimum latency)");
                PresentMode::Immediate
            } else {
                info!("competitive mode: OFF (vsync enabled)");
                PresentMode::Mailbox
            };
        }
    }
}

/// toggle blockout training game with F2
fn toggle_training_game(
    keys: Res<ButtonInput<KeyCode>>,
    mut blockout_state: ResMut<blockout::BlockoutState>,
    blockout_settings: Res<blockout::BlockoutSettings>,
    mut tiling: ResMut<TilingState>,
) {
    if keys.just_pressed(KeyCode::F2) {
        if tiling.is_open(WindowKind::Blockout) {
            // close blockout window
            tiling.close_window(WindowKind::Blockout);
            blockout_state.active = false;
            info!("blockout training: OFF");
        } else {
            // open blockout in tiling
            tiling.open_window(WindowKind::Blockout);
            blockout::toggle_blockout(&mut blockout_state, &blockout_settings);
            info!("blockout training: ON (learn vim keys + poker grid)");
        }
    }
}

/// toggle debug mode with F12
fn toggle_debug_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut debug: ResMut<DebugMode>,
) {
    if keys.just_pressed(KeyCode::F12) {
        debug.enabled = !debug.enabled;
        if debug.enabled {
            info!("debug mode: ON (demo shortcuts enabled)");
        } else {
            info!("debug mode: OFF");
        }
    }
}

/// setup initial state (empty - lobby will handle connection)
fn setup_initial_state() {
    info!("ghettobox poker starting...");
    info!("use lobby to login and join/create tables");
}

/// handle transition from lobby to game when connected
fn handle_lobby_to_game_transition(
    lobby: Res<lobby::LobbyState>,
    mut state: ResMut<multitable::MultiTableState>,
    mut has_initialized: Local<bool>,
) {
    // only setup tables once when lobby.connected becomes true
    if lobby.connected && !*has_initialized {
        *has_initialized = true;

        info!("connected to table, setting up game...");

        // create a demo table for now
        state.add_table(multitable::TableInstance::new_with_demo(
            1,
            [1u8; 32],
            "table 1".into(),
            "1/2".into(),
        ));

        // set our turn on table 1
        if let Some(table) = state.tables.get_mut(0) {
            table.is_our_turn = true;
            table.time_remaining = 30.0;
            table.pot = 150;
            table.game_state.is_our_turn = true;
            table.game_state.time_remaining = 30.0;
            table.game_state.current_bet = 10;
            table.game_state.min_raise = 20;
        }

        state.active_table = Some(0);
    }
}
