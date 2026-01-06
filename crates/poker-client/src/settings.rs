//! settings - professional poker client configuration
//!
//! game-like settings menu with categories:
//! - gameplay (pro mode, confirmations, auto-actions)
//! - controls (keybindings, bet presets)
//! - display (bb display, animations, colors)
//! - audio (sounds, notifications)

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameSettings>()
            .init_resource::<SettingsMenuState>()
            .add_systems(Update, (toggle_settings_menu, render_settings_menu));
    }
}

/// all game settings
#[derive(Resource)]
pub struct GameSettings {
    // gameplay
    pub pro_mode: bool,              // instant execution, no confirms
    pub require_confirm: bool,       // require enter for actions
    pub auto_muck_losing: bool,      // auto-fold losing hands at showdown
    pub auto_post_blinds: bool,      // auto-post blinds
    pub sit_out_next_hand: bool,     // sit out after current hand
    pub auto_rebuy: bool,            // auto rebuy when below threshold
    pub auto_rebuy_threshold: u8,    // % of max buy-in to trigger rebuy

    // display
    pub show_bb_stacks: bool,        // show stacks in BB
    pub show_pot_odds: bool,         // show pot odds
    pub show_equity: bool,           // show hand equity (if allowed)
    pub animations_enabled: bool,    // card/chip animations
    pub animation_speed: f32,        // 0.5 = slow, 1.0 = normal, 2.0 = fast
    pub four_color_deck: bool,       // four color deck
    pub show_bet_slider: bool,       // show bet slider vs buttons only
    pub compact_tables: bool,        // compact multi-table view
    pub highlight_nuts: bool,        // highlight when you have the nuts
    pub time_pressure_threshold: f32, // seconds to start flashing timer

    // controls
    pub hotkey_instant_fold: bool,   // fold key executes instantly
    pub hotkey_instant_check: bool,  // check key executes instantly
    pub radial_instant_mode: bool,   // radial menu executes on release (no confirm)
    pub scroll_bet_adjust: bool,     // mouse scroll adjusts bet size
    pub scroll_bet_step: u8,         // % step for scroll adjustment

    // audio
    pub master_volume: f32,
    pub sfx_volume: f32,
    pub your_turn_sound: bool,
    pub action_sounds: bool,
    pub chat_sounds: bool,
    pub time_warning_sound: bool,

    // keybindings
    pub key_fold: KeyCode,
    pub key_check_call: KeyCode,
    pub key_raise: KeyCode,
    pub key_all_in: KeyCode,
    pub key_cancel: KeyCode,
    pub key_confirm: KeyCode,
    pub key_hint_mode: KeyCode,
    pub key_settings: KeyCode,
    pub key_table_list: KeyCode,

    // bet preset keys (1-0)
    pub bet_preset_keys: [KeyCode; 10],
    pub bet_preset_values: [BetPresetConfig; 10],
}

/// bet preset configuration
#[derive(Clone, Copy, Debug)]
pub struct BetPresetConfig {
    pub percent: u16,
    pub base: BetBase,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BetBase {
    #[default]
    Pot,
    Stack,
    MinRaise,
    BigBlind,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            // gameplay - pro defaults
            pro_mode: false,
            require_confirm: true,
            auto_muck_losing: true,
            auto_post_blinds: true,
            sit_out_next_hand: false,
            auto_rebuy: false,
            auto_rebuy_threshold: 50,

            // display
            show_bb_stacks: true,
            show_pot_odds: true,
            show_equity: false,
            animations_enabled: true,
            animation_speed: 1.5,
            four_color_deck: true,
            show_bet_slider: false,  // pros use presets
            compact_tables: false,
            highlight_nuts: true,
            time_pressure_threshold: 5.0,

            // controls - speed optimized
            hotkey_instant_fold: true,   // fold is always safe to instant
            hotkey_instant_check: true,  // check is safe
            radial_instant_mode: false,  // requires learning
            scroll_bet_adjust: true,
            scroll_bet_step: 10,

            // audio
            master_volume: 0.7,
            sfx_volume: 0.8,
            your_turn_sound: true,
            action_sounds: true,
            chat_sounds: false,
            time_warning_sound: true,

            // keybindings - SC2/CS style grid
            key_fold: KeyCode::KeyQ,
            key_check_call: KeyCode::KeyW,
            key_raise: KeyCode::KeyE,
            key_all_in: KeyCode::KeyR,
            key_cancel: KeyCode::Escape,
            key_confirm: KeyCode::Enter,
            key_hint_mode: KeyCode::KeyF,
            key_settings: KeyCode::F10,
            key_table_list: KeyCode::Tab,

            // bet presets on number row
            bet_preset_keys: [
                KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4,
                KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8,
                KeyCode::Digit9, KeyCode::Digit0,
            ],
            bet_preset_values: [
                BetPresetConfig { percent: 0, base: BetBase::MinRaise },   // 1 = min
                BetPresetConfig { percent: 33, base: BetBase::Pot },       // 2 = 1/3
                BetPresetConfig { percent: 50, base: BetBase::Pot },       // 3 = 1/2
                BetPresetConfig { percent: 66, base: BetBase::Pot },       // 4 = 2/3
                BetPresetConfig { percent: 75, base: BetBase::Pot },       // 5 = 3/4
                BetPresetConfig { percent: 100, base: BetBase::Pot },      // 6 = pot
                BetPresetConfig { percent: 125, base: BetBase::Pot },      // 7 = 1.25x
                BetPresetConfig { percent: 150, base: BetBase::Pot },      // 8 = 1.5x
                BetPresetConfig { percent: 200, base: BetBase::Pot },      // 9 = 2x
                BetPresetConfig { percent: 100, base: BetBase::Stack },    // 0 = all-in
            ],
        }
    }
}

impl GameSettings {
    /// pro mode preset - maximum speed
    pub fn pro_preset(&mut self) {
        self.pro_mode = true;
        self.require_confirm = false;
        self.hotkey_instant_fold = true;
        self.hotkey_instant_check = true;
        self.radial_instant_mode = true;
        self.animations_enabled = false;
        self.animation_speed = 2.0;
        self.show_bet_slider = false;
        self.compact_tables = true;
    }

    /// casual mode preset - more visual feedback
    pub fn casual_preset(&mut self) {
        self.pro_mode = false;
        self.require_confirm = true;
        self.hotkey_instant_fold = false;
        self.hotkey_instant_check = false;
        self.radial_instant_mode = false;
        self.animations_enabled = true;
        self.animation_speed = 1.0;
        self.show_bet_slider = true;
        self.compact_tables = false;
    }

    /// should this action execute instantly?
    pub fn is_instant_action(&self, action: &str) -> bool {
        if self.pro_mode {
            return true;
        }
        match action {
            "fold" => self.hotkey_instant_fold,
            "check" => self.hotkey_instant_check,
            _ => !self.require_confirm,
        }
    }
}

/// settings menu state
#[derive(Resource, Default)]
pub struct SettingsMenuState {
    pub open: bool,
    pub tab: SettingsTab,
    pub rebinding_key: Option<String>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    Gameplay,
    Controls,
    Display,
    Audio,
}

/// toggle settings menu with F10
fn toggle_settings_menu(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<GameSettings>,
    mut menu: ResMut<SettingsMenuState>,
) {
    if keys.just_pressed(settings.key_settings) {
        menu.open = !menu.open;
        if menu.open {
            info!("settings menu opened");
        }
    }
    // also close on escape
    if menu.open && keys.just_pressed(KeyCode::Escape) && menu.rebinding_key.is_none() {
        menu.open = false;
    }
}

/// render settings menu
fn render_settings_menu(
    mut menu: ResMut<SettingsMenuState>,
    mut settings: ResMut<GameSettings>,
    mut contexts: EguiContexts,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if !menu.open {
        return;
    }

    let ctx = contexts.ctx_mut();

    // handle key rebinding
    if let Some(ref key_name) = menu.rebinding_key.clone() {
        for key in [
            KeyCode::KeyQ, KeyCode::KeyW, KeyCode::KeyE, KeyCode::KeyR,
            KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD, KeyCode::KeyF,
            KeyCode::KeyZ, KeyCode::KeyX, KeyCode::KeyC, KeyCode::KeyV,
            KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4,
            KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8,
            KeyCode::Digit9, KeyCode::Digit0, KeyCode::Space, KeyCode::Tab,
            KeyCode::Enter, KeyCode::Backspace,
        ] {
            if keys.just_pressed(key) {
                match key_name.as_str() {
                    "fold" => settings.key_fold = key,
                    "check_call" => settings.key_check_call = key,
                    "raise" => settings.key_raise = key,
                    "all_in" => settings.key_all_in = key,
                    "hint_mode" => settings.key_hint_mode = key,
                    _ => {}
                }
                menu.rebinding_key = None;
                break;
            }
        }
        if keys.just_pressed(KeyCode::Escape) {
            menu.rebinding_key = None;
        }
    }

    egui::Window::new("Settings")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .collapsible(false)
        .default_width(500.0)
        .show(ctx, |ui| {
            // tabs
            ui.horizontal(|ui| {
                for (tab, name) in [
                    (SettingsTab::Gameplay, "Gameplay"),
                    (SettingsTab::Controls, "Controls"),
                    (SettingsTab::Display, "Display"),
                    (SettingsTab::Audio, "Audio"),
                ] {
                    if ui.selectable_label(menu.tab == tab, name).clicked() {
                        menu.tab = tab;
                    }
                }
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                match menu.tab {
                    SettingsTab::Gameplay => render_gameplay_tab(ui, &mut settings),
                    SettingsTab::Controls => render_controls_tab(ui, &mut settings, &mut menu),
                    SettingsTab::Display => render_display_tab(ui, &mut settings),
                    SettingsTab::Audio => render_audio_tab(ui, &mut settings),
                }
            });

            ui.separator();

            // preset buttons
            ui.horizontal(|ui| {
                if ui.button("Pro Mode").clicked() {
                    settings.pro_preset();
                }
                if ui.button("Casual Mode").clicked() {
                    settings.casual_preset();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        menu.open = false;
                    }
                });
            });
        });
}

fn render_gameplay_tab(ui: &mut egui::Ui, settings: &mut GameSettings) {
    ui.heading("Speed");
    ui.checkbox(&mut settings.pro_mode, "Pro Mode (instant actions, no confirmations)");
    ui.checkbox(&mut settings.require_confirm, "Require Enter to confirm actions");
    ui.add_enabled(!settings.pro_mode, egui::Checkbox::new(&mut settings.hotkey_instant_fold, "Instant fold (no confirm)"));
    ui.add_enabled(!settings.pro_mode, egui::Checkbox::new(&mut settings.hotkey_instant_check, "Instant check (no confirm)"));
    ui.checkbox(&mut settings.radial_instant_mode, "Radial menu instant mode");

    ui.add_space(10.0);
    ui.heading("Auto Actions");
    ui.checkbox(&mut settings.auto_muck_losing, "Auto-muck losing hands");
    ui.checkbox(&mut settings.auto_post_blinds, "Auto-post blinds");
    ui.checkbox(&mut settings.auto_rebuy, "Auto rebuy");
    if settings.auto_rebuy {
        ui.horizontal(|ui| {
            ui.label("Rebuy when below:");
            ui.add(egui::Slider::new(&mut settings.auto_rebuy_threshold, 10..=100).suffix("% max"));
        });
    }
}

fn render_controls_tab(ui: &mut egui::Ui, settings: &mut GameSettings, menu: &mut SettingsMenuState) {
    ui.heading("Keybindings");

    let key_btn = |ui: &mut egui::Ui, label: &str, key: KeyCode, key_name: &str, menu: &mut SettingsMenuState| {
        ui.horizontal(|ui| {
            ui.label(format!("{:12}", label));
            let is_rebinding = menu.rebinding_key.as_ref() == Some(&key_name.to_string());
            let btn_text = if is_rebinding {
                "Press key...".to_string()
            } else {
                format!("{:?}", key)
            };
            if ui.button(btn_text).clicked() {
                menu.rebinding_key = Some(key_name.to_string());
            }
        });
    };

    key_btn(ui, "Fold", settings.key_fold, "fold", menu);
    key_btn(ui, "Check/Call", settings.key_check_call, "check_call", menu);
    key_btn(ui, "Raise", settings.key_raise, "raise", menu);
    key_btn(ui, "All-In", settings.key_all_in, "all_in", menu);
    key_btn(ui, "Hint Mode", settings.key_hint_mode, "hint_mode", menu);

    ui.add_space(10.0);
    ui.heading("Bet Sizing");
    ui.checkbox(&mut settings.scroll_bet_adjust, "Mouse scroll adjusts bet");
    if settings.scroll_bet_adjust {
        ui.horizontal(|ui| {
            ui.label("Scroll step:");
            ui.add(egui::Slider::new(&mut settings.scroll_bet_step, 1..=25).suffix("%"));
        });
    }

    ui.add_space(10.0);
    ui.label("Bet presets (number keys 1-0):");
    ui.small("1=Min, 2=33%, 3=50%, 4=66%, 5=75%, 6=Pot, 7=1.25x, 8=1.5x, 9=2x, 0=All-in");
}

fn render_display_tab(ui: &mut egui::Ui, settings: &mut GameSettings) {
    ui.heading("Information");
    ui.checkbox(&mut settings.show_bb_stacks, "Show stacks in BB");
    ui.checkbox(&mut settings.show_pot_odds, "Show pot odds");
    ui.checkbox(&mut settings.highlight_nuts, "Highlight when you have the nuts");

    ui.add_space(10.0);
    ui.heading("Visual");
    ui.checkbox(&mut settings.four_color_deck, "Four-color deck");
    ui.checkbox(&mut settings.animations_enabled, "Enable animations");
    if settings.animations_enabled {
        ui.horizontal(|ui| {
            ui.label("Animation speed:");
            ui.add(egui::Slider::new(&mut settings.animation_speed, 0.5..=3.0).suffix("x"));
        });
    }
    ui.checkbox(&mut settings.compact_tables, "Compact multi-table view");
    ui.checkbox(&mut settings.show_bet_slider, "Show bet slider");

    ui.add_space(10.0);
    ui.heading("Timer");
    ui.horizontal(|ui| {
        ui.label("Time pressure warning at:");
        ui.add(egui::Slider::new(&mut settings.time_pressure_threshold, 1.0..=15.0).suffix("s"));
    });
}

fn render_audio_tab(ui: &mut egui::Ui, settings: &mut GameSettings) {
    ui.heading("Volume");
    ui.horizontal(|ui| {
        ui.label("Master:");
        ui.add(egui::Slider::new(&mut settings.master_volume, 0.0..=1.0).show_value(false));
    });
    ui.horizontal(|ui| {
        ui.label("Effects:");
        ui.add(egui::Slider::new(&mut settings.sfx_volume, 0.0..=1.0).show_value(false));
    });

    ui.add_space(10.0);
    ui.heading("Notifications");
    ui.checkbox(&mut settings.your_turn_sound, "Play sound on your turn");
    ui.checkbox(&mut settings.action_sounds, "Action sounds (fold, bet, etc.)");
    ui.checkbox(&mut settings.time_warning_sound, "Time pressure warning sound");
    ui.checkbox(&mut settings.chat_sounds, "Chat notification sounds");
}
