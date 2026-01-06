//! poker ui overlay using egui
//!
//! displays player info, cards, betting controls
//! vimium-style keyboard shortcuts for fast play

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use zk_shuffle::poker::{Card, Rank, Suit};

use crate::auth::{AuthState, AuthStatus, AuthEvent};
use crate::game::GameAction;

pub struct PokerUiPlugin;

impl Plugin for PokerUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameState>()
            .init_resource::<KeybindState>()
            .add_systems(Update, (handle_keyboard_input, render_ui, render_login_ui).chain());
    }
}

/// keyboard input state for vimium f-search style bindings
#[derive(Resource, Default)]
pub struct KeybindState {
    /// f-search mode active - shows hint labels on buttons
    pub hint_mode: bool,
    /// current hint input being typed
    pub hint_input: String,
    /// current raise amount being typed (after selecting raise)
    pub raise_input: String,
    /// radial wheel mode active (alt+drag or scrollwheel)
    pub radial_mode: bool,
    /// radial wheel selection angle (0-360)
    pub radial_angle: f32,
    /// radial wheel center (screen coords when activated)
    pub radial_center: (f32, f32),
}

/// hint labels for buttons (vimium style: a, s, d, f, j, k, l)
const HINT_LABELS: [&str; 7] = ["a", "s", "d", "f", "j", "k", "l"];

/// available actions with their hint labels
#[derive(Clone, Debug)]
pub struct HintedAction {
    pub label: &'static str,
    pub name: String,
    pub action: PokerAction,
}

/// poker action triggered by keybind
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PokerAction {
    Fold,
    Check,
    Call,
    Raise,
    AllIn,
}

/// current game phase
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GamePhase {
    #[default]
    Lobby,
    Dealing,
    PreFlop,
    Flop,
    Turn,
    River,
    Showdown,
}

/// player info
#[derive(Clone, Debug)]
pub struct PlayerInfo {
    pub name: String,
    pub chips: u32,
    pub current_bet: u32,
    pub hole_cards: Option<[Card; 2]>,
    pub is_folded: bool,
    pub is_active: bool,
    pub seat_index: usize,
}

impl Default for PlayerInfo {
    fn default() -> Self {
        Self {
            name: "Player".to_string(),
            chips: 1000,
            current_bet: 0,
            hole_cards: None,
            is_folded: false,
            is_active: false,
            seat_index: 0,
        }
    }
}

/// bet sizing presets (fractions of pot or multiples of big blind)
#[derive(Clone, Copy, Debug)]
pub enum BetSize {
    /// fraction of pot (0.5 = half pot)
    PotFraction(f32),
    /// multiple of big blind (2.0 = 2x BB)
    BlindMultiple(f32),
}

impl BetSize {
    pub fn label(&self) -> String {
        match self {
            BetSize::PotFraction(f) if *f == 0.5 => "1/2".to_string(),
            BetSize::PotFraction(f) if *f == 0.67 => "2/3".to_string(),
            BetSize::PotFraction(f) if *f == 1.0 => "Pot".to_string(),
            BetSize::PotFraction(f) => format!("{}x", f),
            BetSize::BlindMultiple(m) => format!("{}BB", m),
        }
    }

    pub fn calculate(&self, pot: u32, big_blind: u32) -> u32 {
        match self {
            BetSize::PotFraction(f) => (pot as f32 * f) as u32,
            BetSize::BlindMultiple(m) => (big_blind as f32 * m) as u32,
        }
    }
}

/// configurable bet size buttons
#[derive(Clone, Debug)]
pub struct BetSizeConfig {
    pub sizes: Vec<BetSize>,
}

impl Default for BetSizeConfig {
    fn default() -> Self {
        Self {
            sizes: vec![
                BetSize::PotFraction(0.5),   // 1/2 pot
                BetSize::PotFraction(0.67),  // 2/3 pot
                BetSize::PotFraction(1.0),   // pot
                BetSize::BlindMultiple(3.0), // 3x BB (standard open)
            ],
        }
    }
}

/// game state resource
#[derive(Resource)]
pub struct GameState {
    pub phase: GamePhase,
    pub players: Vec<PlayerInfo>,
    pub community_cards: Vec<Card>,
    pub pot: u32,
    pub current_bet: u32,
    pub dealer_seat: usize,
    pub active_seat: usize,
    pub local_player_seat: usize,
    pub show_betting_controls: bool,
    /// small blind amount
    pub small_blind: u32,
    /// big blind amount
    pub big_blind: u32,
    /// configurable bet sizes
    pub bet_sizes: BetSizeConfig,
    /// selected bet amount (from sizing buttons or typed)
    pub selected_bet: u32,
}

impl Default for GameState {
    fn default() -> Self {
        // demo state with some players
        let mut players = vec![
            PlayerInfo {
                name: "You".to_string(),
                chips: 1000,
                seat_index: 0,
                hole_cards: Some([
                    Card::new(Rank::Ace, Suit::Spades),
                    Card::new(Rank::King, Suit::Spades),
                ]),
                is_active: true,
                ..default()
            },
            PlayerInfo {
                name: "Alice".to_string(),
                chips: 1200,
                seat_index: 1,
                ..default()
            },
            PlayerInfo {
                name: "Bob".to_string(),
                chips: 800,
                seat_index: 3,
                is_folded: true,
                ..default()
            },
            PlayerInfo {
                name: "Carol".to_string(),
                chips: 1500,
                seat_index: 5,
                ..default()
            },
        ];

        Self {
            phase: GamePhase::Flop,
            players,
            community_cards: vec![
                Card::new(Rank::Queen, Suit::Spades),
                Card::new(Rank::Jack, Suit::Spades),
                Card::new(Rank::Ten, Suit::Hearts),
            ],
            pot: 150,
            current_bet: 50,
            dealer_seat: 1,
            active_seat: 0,
            local_player_seat: 0,
            show_betting_controls: true,
            small_blind: 5,
            big_blind: 10,
            bet_sizes: BetSizeConfig::default(),
            selected_bet: 0,
        }
    }
}

/// get available actions for current game state
fn get_available_actions(game_state: &GameState) -> Vec<HintedAction> {
    let mut actions = Vec::new();
    let mut idx = 0;

    // fold is always available
    actions.push(HintedAction {
        label: HINT_LABELS[idx],
        name: "Fold".to_string(),
        action: PokerAction::Fold,
    });
    idx += 1;

    // check or call depending on current bet
    if game_state.current_bet == 0 {
        actions.push(HintedAction {
            label: HINT_LABELS[idx],
            name: "Check".to_string(),
            action: PokerAction::Check,
        });
    } else {
        actions.push(HintedAction {
            label: HINT_LABELS[idx],
            name: format!("Call ${}", game_state.current_bet),
            action: PokerAction::Call,
        });
    }
    idx += 1;

    // raise
    actions.push(HintedAction {
        label: HINT_LABELS[idx],
        name: "Raise".to_string(),
        action: PokerAction::Raise,
    });
    idx += 1;

    // all-in
    actions.push(HintedAction {
        label: HINT_LABELS[idx],
        name: "All-In".to_string(),
        action: PokerAction::AllIn,
    });

    actions
}

/// radial wheel segment for dota2-style selection
#[derive(Clone, Debug)]
pub struct RadialSegment {
    pub label: String,
    pub action: PokerAction,
    /// angle range (start, end) in degrees
    pub angle_range: (f32, f32),
}

/// get radial segments for current game state
fn get_radial_segments(game_state: &GameState) -> Vec<RadialSegment> {
    let actions = get_available_actions(game_state);
    let count = actions.len();
    let segment_size = 360.0 / count as f32;

    actions.iter().enumerate().map(|(i, a)| {
        let start = i as f32 * segment_size - 90.0; // start from top
        let end = start + segment_size;
        RadialSegment {
            label: a.name.clone(),
            action: a.action,
            angle_range: (start, end),
        }
    }).collect()
}

/// check if angle is within segment range
fn angle_in_segment(angle: f32, segment: &RadialSegment) -> bool {
    let (start, end) = segment.angle_range;
    // normalize angle to -180 to 180
    let mut a = angle % 360.0;
    if a > 180.0 { a -= 360.0; }
    if a < -180.0 { a += 360.0; }

    let mut s = start % 360.0;
    if s > 180.0 { s -= 360.0; }
    if s < -180.0 { s += 360.0; }

    let mut e = end % 360.0;
    if e > 180.0 { e -= 360.0; }
    if e < -180.0 { e += 360.0; }

    if s < e {
        a >= s && a < e
    } else {
        // wraps around
        a >= s || a < e
    }
}

/// vimium f-search style keyboard shortcuts
/// press 'f' to enter hint mode, then type hint label to activate button
/// press 'Escape' to cancel hint mode
/// hold 'Alt' + move mouse for radial wheel selection
fn handle_keyboard_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    game_state: Res<GameState>,
    mut keybind_state: ResMut<KeybindState>,
    mut game_actions: EventWriter<GameAction>,
) {
    // handle raise amount input mode
    if !keybind_state.raise_input.is_empty() {
        // number keys for amount
        for (key, digit) in [
            (KeyCode::Digit0, '0'),
            (KeyCode::Digit1, '1'),
            (KeyCode::Digit2, '2'),
            (KeyCode::Digit3, '3'),
            (KeyCode::Digit4, '4'),
            (KeyCode::Digit5, '5'),
            (KeyCode::Digit6, '6'),
            (KeyCode::Digit7, '7'),
            (KeyCode::Digit8, '8'),
            (KeyCode::Digit9, '9'),
        ] {
            if keyboard.just_pressed(key) {
                keybind_state.raise_input.push(digit);
            }
        }

        // backspace to delete
        if keyboard.just_pressed(KeyCode::Backspace) {
            keybind_state.raise_input.pop();
            if keybind_state.raise_input.is_empty() {
                keybind_state.raise_input.clear();
            }
        }

        // escape to cancel
        if keyboard.just_pressed(KeyCode::Escape) {
            keybind_state.raise_input.clear();
        }

        // enter to confirm raise
        if keyboard.just_pressed(KeyCode::Enter) {
            if let Ok(amount) = keybind_state.raise_input.parse::<u32>() {
                info!("raise: ${}", amount);
                game_actions.send(GameAction::Raise(amount));
            }
            keybind_state.raise_input.clear();
        }
        return;
    }

    // escape cancels hint mode
    if keyboard.just_pressed(KeyCode::Escape) {
        keybind_state.hint_mode = false;
        keybind_state.hint_input.clear();
        return;
    }

    // 'f' enters hint mode (like vimium)
    if keyboard.just_pressed(KeyCode::KeyF) && !keybind_state.hint_mode {
        keybind_state.hint_mode = true;
        keybind_state.hint_input.clear();
        return;
    }

    // in hint mode, collect typed characters and match against hints
    if keybind_state.hint_mode {
        // only handle input when it's our turn
        if game_state.active_seat != game_state.local_player_seat {
            keybind_state.hint_mode = false;
            return;
        }

        let actions = get_available_actions(&game_state);

        // check for hint key presses
        for (key, ch) in [
            (KeyCode::KeyA, 'a'),
            (KeyCode::KeyS, 's'),
            (KeyCode::KeyD, 'd'),
            (KeyCode::KeyF, 'f'),
            (KeyCode::KeyJ, 'j'),
            (KeyCode::KeyK, 'k'),
            (KeyCode::KeyL, 'l'),
        ] {
            if keyboard.just_pressed(key) {
                keybind_state.hint_input.push(ch);

                // check if input matches any hint
                for action in &actions {
                    if action.label == keybind_state.hint_input {
                        // execute action
                        match action.action {
                            PokerAction::Fold => {
                                info!("action: fold");
                                game_actions.send(GameAction::Fold);
                            }
                            PokerAction::Check => {
                                info!("action: check");
                                game_actions.send(GameAction::Check);
                            }
                            PokerAction::Call => {
                                info!("action: call");
                                game_actions.send(GameAction::Call);
                            }
                            PokerAction::Raise => {
                                info!("action: raise (enter amount)");
                                // start raise input mode with marker
                                keybind_state.raise_input = String::from("0");
                            }
                            PokerAction::AllIn => {
                                info!("action: all-in");
                                game_actions.send(GameAction::AllIn);
                            }
                        }

                        // exit hint mode after action
                        keybind_state.hint_mode = false;
                        keybind_state.hint_input.clear();

                        // start raise input mode if raise was selected
                        if action.action == PokerAction::Raise {
                            keybind_state.raise_input = String::from("0");
                        }
                        return;
                    }
                }

                // no match yet, check if any hint starts with input
                let has_prefix = actions.iter().any(|a| a.label.starts_with(&keybind_state.hint_input));
                if !has_prefix {
                    // invalid input, reset
                    keybind_state.hint_input.clear();
                }
            }
        }
    }

    // radial wheel mode: Alt + mouse drag
    let alt_held = keyboard.pressed(KeyCode::AltLeft) || keyboard.pressed(KeyCode::AltRight);
    let alt_just_pressed = keyboard.just_pressed(KeyCode::AltLeft) || keyboard.just_pressed(KeyCode::AltRight);

    // only when it's our turn
    if game_state.active_seat != game_state.local_player_seat {
        keybind_state.radial_mode = false;
        return;
    }

    // get cursor position
    let cursor_pos = windows.single().cursor_position();

    if alt_just_pressed && !keybind_state.radial_mode {
        // start radial mode
        if let Some(pos) = cursor_pos {
            keybind_state.radial_mode = true;
            keybind_state.radial_center = (pos.x, pos.y);
            keybind_state.radial_angle = 0.0;
        }
    }

    if alt_held && keybind_state.radial_mode {
        // update angle based on cursor position
        if let Some(pos) = cursor_pos {
            let dx = pos.x - keybind_state.radial_center.0;
            let dy = pos.y - keybind_state.radial_center.1;
            let distance = (dx * dx + dy * dy).sqrt();

            if distance > 20.0 {
                // atan2 gives angle in radians, convert to degrees
                keybind_state.radial_angle = dy.atan2(dx).to_degrees();
            }
        }
    }

    if !alt_held && keybind_state.radial_mode {
        // release: execute selected action
        let segments = get_radial_segments(&game_state);
        for segment in &segments {
            if angle_in_segment(keybind_state.radial_angle, segment) {
                match segment.action {
                    PokerAction::Fold => {
                        info!("radial action: fold");
                        game_actions.send(GameAction::Fold);
                    }
                    PokerAction::Check => {
                        info!("radial action: check");
                        game_actions.send(GameAction::Check);
                    }
                    PokerAction::Call => {
                        info!("radial action: call");
                        game_actions.send(GameAction::Call);
                    }
                    PokerAction::Raise => {
                        info!("radial action: raise");
                        keybind_state.raise_input = String::from("0");
                    }
                    PokerAction::AllIn => {
                        info!("radial action: all-in");
                        game_actions.send(GameAction::AllIn);
                    }
                }
                break;
            }
        }
        keybind_state.radial_mode = false;
    }

    // escape also cancels radial mode
    if keyboard.just_pressed(KeyCode::Escape) && keybind_state.radial_mode {
        keybind_state.radial_mode = false;
    }
}

fn render_ui(
    mut contexts: EguiContexts,
    mut game_state: ResMut<GameState>,
    keybind_state: Res<KeybindState>,
    mut game_actions: EventWriter<GameAction>,
) {
    let ctx = contexts.ctx_mut();

    // top bar - pot, blinds, and community cards
    egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading(format!("Pot: ${}", game_state.pot));
            ui.add_space(10.0);
            ui.label(format!("Blinds: ${}/{}", game_state.small_blind, game_state.big_blind));
            ui.add_space(20.0);

            ui.label("Community: ");
            for card in &game_state.community_cards {
                render_card_label(ui, Some(*card), false);
            }

            // placeholder cards for unrevealed streets
            let shown = game_state.community_cards.len();
            for _ in shown..5 {
                render_card_label(ui, None, false);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{:?}", game_state.phase));
                ui.add_space(10.0);
                // deal button in lobby or showdown
                if game_state.phase == GamePhase::Lobby || game_state.phase == GamePhase::Showdown {
                    if ui.button(egui::RichText::new("Deal New Hand").size(16.0).color(egui::Color32::WHITE))
                        .clicked()
                    {
                        game_actions.send(GameAction::NewHand);
                    }
                }
            });
        });
    });

    // bottom panel - your cards and betting controls
    egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
        ui.vertical(|ui| {
            // first row: hole cards and action buttons
            ui.horizontal(|ui| {
                // your hole cards
                ui.label("Your hand: ");
                if let Some(player) = game_state.players.iter().find(|p| p.seat_index == game_state.local_player_seat) {
                    if let Some(cards) = player.hole_cards {
                        render_card_label(ui, Some(cards[0]), true);
                        render_card_label(ui, Some(cards[1]), true);
                    }
                }

                ui.add_space(30.0);

                // betting controls with vimium f-search hints
                if game_state.show_betting_controls && game_state.active_seat == game_state.local_player_seat {
                    // show raise input if typing
                    if !keybind_state.raise_input.is_empty() {
                        ui.label(format!("Raise: ${}", keybind_state.raise_input));
                        ui.label("[Enter] confirm  [Esc] cancel");
                    } else {
                        // get available actions with hint labels
                        let actions = get_available_actions(&game_state);
                        let hint_mode = keybind_state.hint_mode;

                        for action in &actions {
                            // show hint label in yellow box when hint mode active
                            let button_text = if hint_mode {
                                format!("[{}] {}", action.label.to_uppercase(), action.name)
                            } else {
                                action.name.clone()
                            };

                            let button = if hint_mode {
                                egui::Button::new(
                                    egui::RichText::new(&button_text)
                                        .color(egui::Color32::BLACK)
                                ).fill(egui::Color32::YELLOW)
                            } else {
                                egui::Button::new(&button_text)
                            };

                            if ui.add(button).clicked() {
                                // handle click (same as keyboard)
                                match action.action {
                                    PokerAction::Fold => {
                                        info!("action: fold");
                                        game_actions.send(GameAction::Fold);
                                    }
                                    PokerAction::Check => {
                                        info!("action: check");
                                        game_actions.send(GameAction::Check);
                                    }
                                    PokerAction::Call => {
                                        info!("action: call");
                                        game_actions.send(GameAction::Call);
                                    }
                                    PokerAction::Raise => {
                                        let amount = if game_state.selected_bet > 0 {
                                            game_state.selected_bet
                                        } else {
                                            game_state.big_blind * 2
                                        };
                                        info!("action: raise ${}", amount);
                                        game_actions.send(GameAction::Raise(amount));
                                    }
                                    PokerAction::AllIn => {
                                        info!("action: all-in");
                                        game_actions.send(GameAction::AllIn);
                                    }
                                }
                            }
                        }

                        // show hint mode status
                        if hint_mode {
                            ui.add_space(10.0);
                            ui.colored_label(egui::Color32::YELLOW, "HINT MODE - press label key");
                        } else {
                            ui.add_space(10.0);
                            ui.colored_label(egui::Color32::GRAY, "press [f] for hints");
                        }
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(player) = game_state.players.iter().find(|p| p.seat_index == game_state.local_player_seat) {
                        ui.label(format!("Chips: ${}", player.chips));
                    }
                });
            });

            // second row: bet sizing buttons (only when our turn and not in raise input mode)
            if game_state.show_betting_controls
                && game_state.active_seat == game_state.local_player_seat
                && keybind_state.raise_input.is_empty()
            {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Bet size:");

                    // calculate sizes and store for iteration
                    let pot = game_state.pot;
                    let bb = game_state.big_blind;
                    let sizes: Vec<_> = game_state.bet_sizes.sizes.iter()
                        .map(|s| (s.label(), s.calculate(pot, bb)))
                        .collect();

                    for (label, amount) in sizes {
                        let is_selected = game_state.selected_bet == amount;
                        let button = if is_selected {
                            egui::Button::new(
                                egui::RichText::new(format!("{} (${})", label, amount))
                                    .color(egui::Color32::BLACK)
                            ).fill(egui::Color32::LIGHT_GREEN)
                        } else {
                            egui::Button::new(format!("{} (${})", label, amount))
                        };

                        if ui.add(button).clicked() {
                            game_state.selected_bet = amount;
                            info!("bet size selected: ${}", amount);
                        }
                    }

                    ui.add_space(10.0);

                    // custom amount slider/input
                    ui.label("Custom:");
                    let player_chips = game_state.players.iter()
                        .find(|p| p.seat_index == game_state.local_player_seat)
                        .map(|p| p.chips)
                        .unwrap_or(0);
                    let min_bet = game_state.big_blind;
                    let mut bet = game_state.selected_bet;
                    if ui.add(egui::Slider::new(&mut bet, min_bet..=player_chips).show_value(true)).changed() {
                        game_state.selected_bet = bet;
                    }
                });
            }
        });
    });

    // player info panels (around the table)
    for player in &game_state.players {
        if player.seat_index == game_state.local_player_seat {
            continue; // don't show panel for local player
        }

        let (x, y) = seat_to_screen_position(player.seat_index);

        egui::Window::new(&player.name)
            .fixed_pos(egui::pos2(x, y))
            .resizable(false)
            .title_bar(false)
            .frame(egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180))
                .rounding(5.0)
                .inner_margin(8.0))
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    let name_color = if player.is_folded {
                        egui::Color32::GRAY
                    } else if player.seat_index == game_state.active_seat {
                        egui::Color32::YELLOW
                    } else {
                        egui::Color32::WHITE
                    };

                    ui.colored_label(name_color, &player.name);
                    ui.label(format!("${}", player.chips));

                    if player.current_bet > 0 {
                        ui.label(format!("Bet: ${}", player.current_bet));
                    }

                    if player.is_folded {
                        ui.colored_label(egui::Color32::RED, "FOLDED");
                    }

                    // show cards in showdown
                    if game_state.phase == GamePhase::Showdown {
                        if let Some(cards) = player.hole_cards {
                            ui.horizontal(|ui| {
                                render_card_label(ui, Some(cards[0]), true);
                                render_card_label(ui, Some(cards[1]), true);
                            });
                        }
                    }
                });
            });
    }

    // dealer button indicator
    let (dx, dy) = seat_to_screen_position(game_state.dealer_seat);
    egui::Window::new("dealer")
        .fixed_pos(egui::pos2(dx + 60.0, dy))
        .resizable(false)
        .title_bar(false)
        .frame(egui::Frame::none()
            .fill(egui::Color32::WHITE)
            .rounding(12.0)
            .inner_margin(4.0))
        .show(ctx, |ui| {
            ui.colored_label(egui::Color32::BLACK, "D");
        });

    // show hint mode overlay when active
    if keybind_state.hint_mode {
        egui::Window::new("hint_overlay")
            .fixed_pos(egui::pos2(500.0, 300.0))
            .resizable(false)
            .title_bar(false)
            .frame(egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200))
                .rounding(8.0)
                .inner_margin(16.0))
            .show(ctx, |ui| {
                ui.colored_label(egui::Color32::YELLOW, "HINT MODE");
                ui.add_space(4.0);
                ui.label("Type label key to activate button");
                ui.label("[Esc] to cancel");
                if !keybind_state.hint_input.is_empty() {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::GREEN, format!("Input: {}", keybind_state.hint_input));
                }
            });
    }

    // show radial wheel when active (dota2-style)
    if keybind_state.radial_mode {
        let center = egui::pos2(keybind_state.radial_center.0, keybind_state.radial_center.1);
        let radius = 100.0;
        let segments = get_radial_segments(&game_state);

        egui::Area::new(egui::Id::new("radial_wheel"))
            .fixed_pos(center - egui::vec2(radius + 40.0, radius + 40.0))
            .show(ctx, |ui| {
                let painter = ui.painter();

                // draw wheel background
                painter.circle_filled(center, radius + 10.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180));
                painter.circle_stroke(center, radius + 10.0, egui::Stroke::new(2.0, egui::Color32::WHITE));

                // draw segments
                for segment in &segments {
                    let (start_deg, end_deg) = segment.angle_range;
                    let mid_deg = (start_deg + end_deg) / 2.0;
                    let mid_rad = mid_deg.to_radians();

                    // check if this segment is selected
                    let is_selected = angle_in_segment(keybind_state.radial_angle, segment);

                    // draw segment line
                    let line_start_rad = start_deg.to_radians();
                    let line_end = center + egui::vec2(
                        line_start_rad.cos() as f32 * radius,
                        line_start_rad.sin() as f32 * radius,
                    );
                    painter.line_segment(
                        [center, line_end],
                        egui::Stroke::new(1.0, egui::Color32::GRAY),
                    );

                    // draw label
                    let label_pos = center + egui::vec2(
                        mid_rad.cos() as f32 * (radius * 0.65),
                        mid_rad.sin() as f32 * (radius * 0.65),
                    );

                    let color = if is_selected {
                        egui::Color32::YELLOW
                    } else {
                        egui::Color32::WHITE
                    };

                    painter.text(
                        label_pos,
                        egui::Align2::CENTER_CENTER,
                        &segment.label,
                        egui::FontId::proportional(14.0),
                        color,
                    );

                    // highlight selected segment
                    if is_selected {
                        let highlight_pos = center + egui::vec2(
                            mid_rad.cos() as f32 * (radius + 5.0),
                            mid_rad.sin() as f32 * (radius + 5.0),
                        );
                        painter.circle_filled(highlight_pos, 8.0, egui::Color32::YELLOW);
                    }
                }

                // draw center dot
                painter.circle_filled(center, 5.0, egui::Color32::WHITE);

                // draw direction indicator
                let angle_rad = keybind_state.radial_angle.to_radians();
                let indicator_end = center + egui::vec2(
                    angle_rad.cos() as f32 * (radius - 20.0),
                    angle_rad.sin() as f32 * (radius - 20.0),
                );
                painter.line_segment(
                    [center, indicator_end],
                    egui::Stroke::new(3.0, egui::Color32::YELLOW),
                );
            });

        // instructions
        egui::Window::new("radial_instructions")
            .fixed_pos(center + egui::vec2(0.0, radius + 30.0))
            .resizable(false)
            .title_bar(false)
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                ui.colored_label(egui::Color32::YELLOW, "Drag to select, release Alt to confirm");
            });
    }
}

/// convert seat index to screen position (10 player layout)
fn seat_to_screen_position(seat: usize) -> (f32, f32) {
    // positions around an oval, matching SEAT_POSITIONS in poker_table.rs
    match seat {
        0 => (640.0 - 60.0, 580.0),  // front center (dealer)
        1 => (320.0, 520.0),          // front left
        2 => (120.0, 380.0),          // left front
        3 => (80.0, 220.0),           // left back
        4 => (280.0, 100.0),          // back left
        5 => (640.0 - 60.0, 60.0),   // back center
        6 => (940.0, 100.0),          // back right
        7 => (1140.0, 220.0),         // right back
        8 => (1100.0, 380.0),         // right front
        9 => (900.0, 520.0),          // front right
        _ => (640.0, 360.0),          // fallback center
    }
}

/// render a card as a label
fn render_card_label(ui: &mut egui::Ui, card: Option<Card>, large: bool) {
    let (text, color) = match card {
        Some(c) => {
            let suit_color = match c.suit {
                Suit::Hearts | Suit::Diamonds => egui::Color32::RED,
                Suit::Clubs | Suit::Spades => egui::Color32::BLACK,
            };
            (format!("{}", c), suit_color)
        }
        None => ("🂠".to_string(), egui::Color32::DARK_BLUE),
    };

    let font_size = if large { 24.0 } else { 18.0 };

    ui.add(
        egui::Label::new(
            egui::RichText::new(&text)
                .size(font_size)
                .color(color)
                .background_color(egui::Color32::WHITE)
        )
        .wrap_mode(egui::TextWrapMode::Extend)
    );
    ui.add_space(4.0);
}

/// render login ui when not authenticated
fn render_login_ui(
    mut contexts: EguiContexts,
    mut auth_state: ResMut<AuthState>,
    mut auth_events: EventWriter<AuthEvent>,
) {
    // only show login UI when not logged in
    if auth_state.status == AuthStatus::LoggedIn {
        return;
    }

    let ctx = contexts.ctx_mut();

    egui::Window::new("Login")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.set_min_width(300.0);

            ui.heading(if auth_state.login_form.is_registering {
                "Register"
            } else {
                "Login"
            });

            ui.add_space(10.0);

            // email field
            ui.horizontal(|ui| {
                ui.label("Email:");
                ui.text_edit_singleline(&mut auth_state.login_form.email);
            });

            // pin field
            ui.horizontal(|ui| {
                ui.label("PIN:");
                ui.add(egui::TextEdit::singleline(&mut auth_state.login_form.pin).password(true));
            });

            // confirm pin for registration
            if auth_state.login_form.is_registering {
                ui.horizontal(|ui| {
                    ui.label("Confirm:");
                    ui.add(egui::TextEdit::singleline(&mut auth_state.login_form.pin_confirm).password(true));
                });
            }

            ui.add_space(10.0);

            // error message
            if let Some(ref error) = auth_state.login_form.error {
                ui.colored_label(egui::Color32::RED, error);
                ui.add_space(5.0);
            }

            // loading indicator
            if auth_state.status == AuthStatus::LoggingIn {
                ui.spinner();
                ui.label("Authenticating...");
            } else {
                // submit button
                ui.horizontal(|ui| {
                    if auth_state.login_form.is_registering {
                        if ui.button("Register").clicked() {
                            if auth_state.login_form.pin != auth_state.login_form.pin_confirm {
                                auth_state.login_form.error = Some("PINs do not match".to_string());
                            } else {
                                auth_events.send(AuthEvent::Register {
                                    email: auth_state.login_form.email.clone(),
                                    pin: auth_state.login_form.pin.clone(),
                                });
                            }
                        }
                    } else {
                        if ui.button("Login").clicked() {
                            auth_events.send(AuthEvent::Login {
                                email: auth_state.login_form.email.clone(),
                                pin: auth_state.login_form.pin.clone(),
                            });
                        }
                    }

                    ui.add_space(10.0);

                    // toggle login/register
                    let toggle_text = if auth_state.login_form.is_registering {
                        "Have account? Login"
                    } else {
                        "New? Register"
                    };
                    if ui.button(toggle_text).clicked() {
                        auth_state.login_form.is_registering = !auth_state.login_form.is_registering;
                        auth_state.login_form.error = None;
                    }
                });
            }
        });
}
