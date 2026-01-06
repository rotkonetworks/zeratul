//! multi-table layout and management
//!
//! professional poker client supporting 1-24+ simultaneous tables
//!
//! layout modes:
//! - tiled: even grid of tables
//! - cascade: overlapping windows
//! - stack: one active table, others minimized
//! - custom: user-defined positions

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::table_2d::{self, TableState, Player, Card, Suit, Rank, GamePhase};

pub struct MultiTablePlugin;

impl Plugin for MultiTablePlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<MultiTableState>()
            .init_resource::<TableLayoutSettings>()
            .init_resource::<NotificationQueue>()
            .init_resource::<HotkeySettings>()
            .init_resource::<AutoActionSettings>()
            .init_resource::<PendingAction>()
            .init_resource::<HintModeState>()
            .init_resource::<RadialMenuState>()
            .init_resource::<DemoState>()
            .add_systems(Update, (
                update_table_layout,
                render_tables,
                handle_table_navigation,
                handle_action_hotkeys,
                handle_pending_confirm,
                handle_hint_mode,
                handle_radial_menu,
                process_auto_actions,
                process_notifications,
                render_notification_overlay,
                render_hotkey_overlay,
                render_pending_action_overlay,
                render_hint_overlay,
                render_radial_menu,
                render_auto_action_panel,
                demo_turn_cycling,
            ));
    }
}

/// hotkey configuration - sc2 grid style
///
/// layout (left hand home row):
/// ```
///   Q W E R
///   A S D F
/// ```
///
/// Q = cancel/back
/// W = fold
/// E = check/call
/// R = raise (opens sizing)
/// A = all-in
/// S = (reserved)
/// D = (reserved)
/// F = hint mode (vimium-style click anything)
///
/// 1-9,0 = bet sizing presets (configurable %)
/// Enter = confirm action
/// Esc = cancel pending action
/// Alt+1-9 = switch to table N
/// Alt+Left/Right = prev/next table
#[derive(Resource)]
pub struct HotkeySettings {
    // === action keys (grid layout) ===
    /// cancel pending action
    pub cancel: KeyCode,
    /// fold
    pub fold: KeyCode,
    /// check/call
    pub check_call: KeyCode,
    /// raise (opens bet sizing)
    pub raise: KeyCode,
    /// all-in
    pub all_in: KeyCode,
    /// hint mode toggle
    pub hint_mode: KeyCode,
    /// confirm action
    pub confirm: KeyCode,

    // === bet sizing presets (% of stack or pot, configurable) ===
    pub bet_presets: [BetPreset; 10],

    // === settings ===
    /// require enter to confirm actions
    pub require_confirm: bool,
    /// show hotkey overlay
    pub show_hints: bool,
}

/// bet sizing preset
#[derive(Clone, Copy, Debug)]
pub struct BetPreset {
    /// percentage value
    pub percent: u8,
    /// what the percentage is of
    pub of: BetPresetBase,
}

/// what bet preset percentage is based on
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BetPresetBase {
    #[default]
    Pot,
    Stack,
    /// minimum legal raise
    MinRaise,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            // grid layout keys
            cancel: KeyCode::KeyQ,
            fold: KeyCode::KeyW,
            check_call: KeyCode::KeyE,
            raise: KeyCode::KeyR,
            all_in: KeyCode::KeyA,
            hint_mode: KeyCode::KeyF,
            confirm: KeyCode::Enter,

            // bet presets: standard poker sizings
            // 1 = min raise, 2-9 = pot %, 0 = all-in
            bet_presets: [
                BetPreset { percent: 0, of: BetPresetBase::MinRaise }, // 1 = min raise
                BetPreset { percent: 33, of: BetPresetBase::Pot },     // 2 = 1/3 pot
                BetPreset { percent: 50, of: BetPresetBase::Pot },     // 3 = 1/2 pot
                BetPreset { percent: 66, of: BetPresetBase::Pot },     // 4 = 2/3 pot
                BetPreset { percent: 75, of: BetPresetBase::Pot },     // 5 = 3/4 pot
                BetPreset { percent: 100, of: BetPresetBase::Pot },    // 6 = pot
                BetPreset { percent: 125, of: BetPresetBase::Pot },    // 7 = 1.25x pot
                BetPreset { percent: 150, of: BetPresetBase::Pot },    // 8 = 1.5x pot
                BetPreset { percent: 200, of: BetPresetBase::Pot },    // 9 = 2x pot
                BetPreset { percent: 100, of: BetPresetBase::Stack },  // 0 = all-in
            ],

            require_confirm: true,
            show_hints: true,
        }
    }
}

/// pending action waiting for confirmation
#[derive(Resource, Default)]
pub struct PendingAction {
    /// action type
    pub action: Option<QueuedAction>,
    /// timestamp when queued
    pub queued_at: f64,
    /// table id
    pub table_id: u64,
}

/// action waiting for enter confirmation
#[derive(Clone, Debug)]
pub enum QueuedAction {
    Fold,
    Check,
    Call(u64),
    Bet(u64),
    Raise(u64),
    AllIn,
}

/// hint mode state for vimium-style navigation
#[derive(Resource, Default)]
pub struct HintModeState {
    /// hint mode active
    pub active: bool,
    /// current hint input buffer
    pub input: String,
    /// available hints (regenerated each frame when active)
    pub hints: Vec<HintTarget>,
    /// external hints from other systems (lobby, settings, etc)
    pub external_hints: Vec<HintTarget>,
    /// egui id to focus after hint activation
    pub pending_focus: Option<egui::Id>,
}

/// clickable target with hint label
#[derive(Clone, Debug)]
pub struct HintTarget {
    pub label: String,
    pub pos: egui::Pos2,
    pub action: HintAction,
}

/// what happens when hint is activated
#[derive(Clone, Debug)]
pub enum HintAction {
    /// focus a table
    FocusTable(usize),
    /// poker action
    Fold,
    Check,
    Call(u64),
    AllIn,
    /// bet sizing preset (index 0-9)
    BetPreset(usize),
    /// click at position (simulates mouse click)
    Click(egui::Pos2),
    /// focus an input field by id
    FocusInput(String),
}

/// radial menu state (right-click drag for betting)
///
/// similar to CS:GO buy menu or Dota 2 chat wheel
#[derive(Resource, Default)]
pub struct RadialMenuState {
    /// menu is active
    pub active: bool,
    /// center position (where right-click started)
    pub center: egui::Pos2,
    /// current mouse position
    pub current: egui::Pos2,
    /// selected segment (-1 = none)
    pub selected: i8,
    /// center button selected (check/call)
    pub center_selected: bool,
    /// menu items
    pub items: Vec<RadialMenuItem>,
    /// require drag confirmation
    pub require_drag_confirm: bool,
    /// center label (check or call)
    pub center_label: String,
}

/// radial menu item
#[derive(Clone, Debug)]
pub struct RadialMenuItem {
    pub label: String,
    pub action: QueuedAction,
    pub color: egui::Color32,
}

impl RadialMenuState {
    /// create with default bet sizing items
    pub fn with_bet_items(pot: u64, current_bet: u64, min_raise: u64, our_stack: u64) -> Self {
        let mut items = Vec::new();

        // center = check/call
        // segments around: fold, 33%, 50%, 66%, pot, 1.5x, 2x, all-in

        // fold at top
        items.push(RadialMenuItem {
            label: "FOLD".into(),
            action: QueuedAction::Fold,
            color: egui::Color32::from_rgb(160, 100, 80),
        });

        // bet sizings clockwise - gradient from conservative to aggressive
        let sizes = [
            ("33%", pot * 33 / 100, egui::Color32::from_rgb(70, 130, 100)),
            ("50%", pot / 2, egui::Color32::from_rgb(80, 140, 110)),
            ("66%", pot * 66 / 100, egui::Color32::from_rgb(100, 150, 100)),
            ("POT", pot, egui::Color32::from_rgb(130, 150, 80)),
            ("1.5x", pot * 150 / 100, egui::Color32::from_rgb(160, 140, 70)),
            ("2x", pot * 2, egui::Color32::from_rgb(180, 120, 60)),
        ];

        for (label, amount, color) in sizes {
            if amount >= min_raise {
                let action = if current_bet == 0 {
                    QueuedAction::Bet(amount)
                } else {
                    QueuedAction::Raise(amount)
                };
                items.push(RadialMenuItem {
                    label: format!("{} ({})", label, format_chips(amount)),
                    action,
                    color,
                });
            }
        }

        // all-in at bottom
        items.push(RadialMenuItem {
            label: format!("ALL-IN ({})", format_chips(our_stack)),
            action: QueuedAction::AllIn,
            color: egui::Color32::from_rgb(180, 70, 70),
        });

        let center_label = if current_bet > 0 {
            format!("CALL {}", format_chips(current_bet))
        } else {
            "CHECK".into()
        };

        Self {
            items,
            require_drag_confirm: true,
            center_label,
            ..Default::default()
        }
    }

    /// get selected item from mouse position
    pub fn get_selected(&self) -> Option<&RadialMenuItem> {
        if self.selected < 0 || self.selected as usize >= self.items.len() {
            return None;
        }
        self.items.get(self.selected as usize)
    }

    /// update selection based on mouse position
    pub fn update_selection(&mut self) {
        let diff = self.current - self.center;
        let dist = diff.length();

        // center = check/call
        if dist < 30.0 {
            self.selected = -1;
            self.center_selected = true;
            return;
        }

        self.center_selected = false;

        // calculate angle
        let angle = diff.y.atan2(diff.x);
        let normalized = (angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI);

        // map to segment
        let segment_count = self.items.len();
        if segment_count == 0 {
            self.selected = -1;
            return;
        }

        let segment = ((normalized * segment_count as f32) as usize) % segment_count;
        self.selected = segment as i8;
    }
}

/// auto-action settings (global)
#[derive(Resource, Default)]
pub struct AutoActionSettings {
    /// per-table auto-action queues
    pub table_actions: std::collections::HashMap<u64, TableAutoAction>,
}

/// auto-action for a specific table
#[derive(Clone, Debug, Default)]
pub struct TableAutoAction {
    /// auto-fold when facing bet
    pub auto_fold: bool,
    /// auto-check when possible
    pub auto_check: bool,
    /// auto-check/fold (check if possible, else fold)
    pub auto_check_fold: bool,
    /// auto-call any bet
    pub auto_call_any: bool,
    /// auto-call up to specific amount
    pub auto_call_limit: Option<u64>,
}

/// layout mode for multiple tables
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutMode {
    #[default]
    Single,     // single table, fullscreen-ish
    Tiled,      // even grid (bspwm style, hjkl nav)
    Floating,   // windows-style overlapping
    Stack,      // one visible, tabs for others
}

/// table layout settings
#[derive(Resource)]
pub struct TableLayoutSettings {
    pub mode: LayoutMode,
    /// gap between tiled tables (px)
    pub tile_gap: f32,
    /// floating window offset (px)
    pub float_offset: Vec2,
    /// minimum table size
    pub min_table_size: Vec2,
    /// preferred aspect ratio (width/height)
    pub aspect_ratio: f32,
    /// show table borders
    pub show_borders: bool,
    /// highlight active table
    pub highlight_active: bool,
    /// grid dimensions for tiled mode (cols, rows)
    pub grid_size: (usize, usize),
    /// next table id counter
    pub next_table_id: u64,
}

impl Default for TableLayoutSettings {
    fn default() -> Self {
        Self {
            mode: LayoutMode::Single,
            tile_gap: 4.0,
            float_offset: Vec2::new(30.0, 30.0),
            min_table_size: Vec2::new(400.0, 300.0),
            aspect_ratio: 16.0 / 10.0,
            show_borders: true,
            highlight_active: true,
            grid_size: (1, 1),
            next_table_id: 2, // start at 2 since we create table 1
        }
    }
}

/// individual table instance
#[derive(Clone, Debug)]
pub struct TableInstance {
    /// unique table id
    pub id: u64,
    /// channel id for this table
    pub channel_id: [u8; 32],
    /// display name
    pub name: String,
    /// stakes (e.g., "0.5/1")
    pub stakes: String,
    /// our seat (0-9)
    pub our_seat: u8,
    /// current pot
    pub pot: u64,
    /// is it our turn?
    pub is_our_turn: bool,
    /// time remaining for action (seconds)
    pub time_remaining: f32,
    /// last action timestamp
    pub last_action_time: f64,
    /// table position/size (for custom layout)
    pub rect: egui::Rect,
    /// minimized state
    pub minimized: bool,
    /// notification priority (0 = normal, higher = more urgent)
    pub priority: u8,
    /// full game state for rendering
    pub game_state: TableState,
    /// grid position (col, row) for tiled navigation
    pub grid_pos: (usize, usize),
}

impl TableInstance {
    pub fn new(id: u64, channel_id: [u8; 32], name: String, stakes: String) -> Self {
        Self {
            id,
            channel_id,
            name,
            stakes,
            our_seat: 0,
            pot: 0,
            is_our_turn: false,
            time_remaining: 0.0,
            last_action_time: 0.0,
            rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0)),
            minimized: false,
            priority: 0,
            game_state: TableState::default(),
            grid_pos: (0, 0),
        }
    }

    /// create with demo state for testing - demo system will populate
    pub fn new_with_demo(id: u64, channel_id: [u8; 32], name: String, stakes: String) -> Self {
        let mut instance = Self::new(id, channel_id, name, stakes);

        // start with empty table - demo_turn_cycling will populate
        instance.game_state.players = vec![None; 6];
        instance.game_state.community_cards = vec![];
        instance.game_state.pot = 0;
        instance.game_state.phase = GamePhase::Waiting;
        instance.game_state.our_seat = 0;
        instance.pot = 0;
        instance.is_our_turn = false;

        instance
    }
}

/// multi-table state
#[derive(Resource, Default)]
pub struct MultiTableState {
    /// all open tables
    pub tables: Vec<TableInstance>,
    /// currently focused table index
    pub active_table: Option<usize>,
    /// previous focused table (for toggle with `)
    pub last_table: Option<usize>,
    /// tables sorted by priority (for rendering order)
    pub render_order: Vec<usize>,
    /// total session stats
    pub session: SessionStats,
    /// auto-focus most urgent table
    pub auto_focus_urgent: bool,
    /// action tables only cycling index
    pub action_cycle_idx: usize,
}

impl MultiTableState {
    /// add a new table
    pub fn add_table(&mut self, table: TableInstance) {
        let idx = self.tables.len();
        self.tables.push(table);
        self.render_order.push(idx);
        self.update_priorities();
    }

    /// remove a table
    pub fn remove_table(&mut self, id: u64) {
        if let Some(idx) = self.tables.iter().position(|t| t.id == id) {
            self.tables.remove(idx);
            self.render_order.retain(|&i| i != idx);
            // fix indices
            for i in &mut self.render_order {
                if *i > idx {
                    *i -= 1;
                }
            }
            if self.active_table == Some(idx) {
                self.active_table = None;
            }
        }
    }

    /// focus next table needing action
    pub fn focus_next_action(&mut self) {
        // find tables where it's our turn, sorted by urgency
        let mut action_tables: Vec<_> = self.tables.iter()
            .enumerate()
            .filter(|(_, t)| t.is_our_turn && !t.minimized)
            .map(|(i, t)| (i, t.time_remaining))
            .collect();

        action_tables.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        if let Some((idx, _)) = action_tables.first() {
            self.active_table = Some(*idx);
        }
    }

    /// update table priorities
    fn update_priorities(&mut self) {
        for table in &mut self.tables {
            table.priority = if table.is_our_turn {
                if table.time_remaining < 5.0 { 3 } // urgent
                else if table.time_remaining < 15.0 { 2 } // attention
                else { 1 } // action needed
            } else {
                0
            };
        }

        // sort render order by priority (highest last = on top)
        self.render_order.sort_by(|&a, &b| {
            self.tables[a].priority.cmp(&self.tables[b].priority)
        });
    }

    /// get table count
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// toggle between current and last table (like alt-tab)
    pub fn toggle_last_table(&mut self) {
        if let Some(last) = self.last_table {
            if last < self.tables.len() {
                let current = self.active_table;
                self.active_table = Some(last);
                self.last_table = current;
            }
        }
    }

    /// set active with history tracking
    pub fn set_active(&mut self, idx: usize) {
        if idx < self.tables.len() && self.active_table != Some(idx) {
            self.last_table = self.active_table;
            self.active_table = Some(idx);
        }
    }

    /// cycle through action tables only (forward)
    pub fn cycle_action_tables(&mut self, reverse: bool) {
        let action_tables: Vec<usize> = self.tables.iter()
            .enumerate()
            .filter(|(_, t)| t.is_our_turn && !t.minimized)
            .map(|(i, _)| i)
            .collect();

        if action_tables.is_empty() {
            return;
        }

        let current_pos = self.active_table
            .and_then(|idx| action_tables.iter().position(|&i| i == idx))
            .unwrap_or(0);

        let new_pos = if reverse {
            if current_pos == 0 { action_tables.len() - 1 } else { current_pos - 1 }
        } else {
            (current_pos + 1) % action_tables.len()
        };

        self.set_active(action_tables[new_pos]);
    }

    /// get tables needing action count
    pub fn action_count(&self) -> usize {
        self.tables.iter().filter(|t| t.is_our_turn).count()
    }

    /// get most urgent table index (lowest time remaining)
    pub fn most_urgent_table(&self) -> Option<usize> {
        self.tables.iter()
            .enumerate()
            .filter(|(_, t)| t.is_our_turn && !t.minimized)
            .min_by(|a, b| a.1.time_remaining.partial_cmp(&b.1.time_remaining).unwrap())
            .map(|(i, _)| i)
    }
}

/// session statistics
#[derive(Clone, Debug, Default)]
pub struct SessionStats {
    pub hands_played: u64,
    pub vpip: f32, // voluntarily put in pot %
    pub pfr: f32,  // preflop raise %
    pub total_won: i64,
    pub total_lost: u64,
    pub biggest_pot: u64,
    pub session_start: f64,
}

impl SessionStats {
    pub fn net(&self) -> i64 {
        self.total_won - self.total_lost as i64
    }

    pub fn duration_minutes(&self, now: f64) -> f64 {
        (now - self.session_start) / 60.0
    }

    pub fn hands_per_hour(&self, now: f64) -> f64 {
        let hours = (now - self.session_start) / 3600.0;
        if hours > 0.0 {
            self.hands_played as f64 / hours
        } else {
            0.0
        }
    }
}

// === notification system ===

/// notification types
#[derive(Clone, Debug)]
pub enum NotificationType {
    YourTurn { table_id: u64, time_remaining: f32 },
    BigPot { table_id: u64, pot: u64 },
    AllIn { table_id: u64 },
    YouWon { table_id: u64, amount: u64 },
    YouLost { table_id: u64, amount: u64 },
    PlayerJoined { table_id: u64, name: String },
    PlayerLeft { table_id: u64, name: String },
    TableClosing { table_id: u64 },
    LowTimeWarning { table_id: u64, seconds: f32 },
}

/// notification entry
#[derive(Clone, Debug)]
pub struct Notification {
    pub kind: NotificationType,
    pub created_at: f64,
    pub expires_at: f64,
    pub sound: Option<&'static str>,
    pub dismissed: bool,
}

/// notification queue
#[derive(Resource, Default)]
pub struct NotificationQueue {
    pub notifications: Vec<Notification>,
    /// sound enabled
    pub sound_enabled: bool,
    /// visual notifications enabled
    pub visual_enabled: bool,
    /// your turn sound path
    pub your_turn_sound: Option<String>,
}

impl NotificationQueue {
    pub fn push(&mut self, kind: NotificationType, duration: f64, now: f64) {
        let sound = match &kind {
            NotificationType::YourTurn { .. } => Some("your_turn.ogg"),
            NotificationType::BigPot { .. } => Some("big_pot.ogg"),
            NotificationType::LowTimeWarning { .. } => Some("warning.ogg"),
            NotificationType::YouWon { .. } => Some("win.ogg"),
            _ => None,
        };

        self.notifications.push(Notification {
            kind,
            created_at: now,
            expires_at: now + duration,
            sound,
            dismissed: false,
        });
    }

    pub fn clear_expired(&mut self, now: f64) {
        self.notifications.retain(|n| n.expires_at > now && !n.dismissed);
    }

    pub fn dismiss_for_table(&mut self, table_id: u64) {
        for n in &mut self.notifications {
            let matches = match &n.kind {
                NotificationType::YourTurn { table_id: id, .. } => *id == table_id,
                NotificationType::LowTimeWarning { table_id: id, .. } => *id == table_id,
                _ => false,
            };
            if matches {
                n.dismissed = true;
            }
        }
    }
}

// === systems ===

/// calculate table positions based on layout mode
fn update_table_layout(
    mut state: ResMut<MultiTableState>,
    mut settings: ResMut<TableLayoutSettings>,
    windows: Query<&Window>,
) {
    let Ok(window) = windows.get_single() else { return };
    let screen = Vec2::new(window.width(), window.height());

    let table_count = state.tables.len();
    if table_count == 0 {
        return;
    }

    match settings.mode {
        LayoutMode::Single => {
            // single table, nearly fullscreen
            let table_size = fit_aspect_ratio(screen * 0.98, settings.aspect_ratio);
            let center = (screen - table_size) / 2.0;

            if let Some(table) = state.tables.first_mut() {
                table.rect = egui::Rect::from_min_size(
                    egui::pos2(center.x, center.y),
                    egui::vec2(table_size.x, table_size.y),
                );
                table.grid_pos = (0, 0);
            }
            settings.grid_size = (1, 1);
        }

        LayoutMode::Tiled => {
            // bspwm-style tiled grid with hjkl navigation
            let (cols, rows) = optimal_grid(table_count);
            settings.grid_size = (cols, rows);

            let cell_size = Vec2::new(
                (screen.x - settings.tile_gap * (cols as f32 + 1.0)) / cols as f32,
                (screen.y - settings.tile_gap * (rows as f32 + 1.0)) / rows as f32,
            );

            let table_size = fit_aspect_ratio(cell_size, settings.aspect_ratio);

            for (i, table) in state.tables.iter_mut().enumerate() {
                if table.minimized {
                    continue;
                }
                let col = i % cols;
                let row = i / cols;

                table.grid_pos = (col, row);

                let x = settings.tile_gap + col as f32 * (cell_size.x + settings.tile_gap);
                let y = settings.tile_gap + row as f32 * (cell_size.y + settings.tile_gap);

                let offset = (cell_size - table_size) / 2.0;

                table.rect = egui::Rect::from_min_size(
                    egui::pos2(x + offset.x, y + offset.y),
                    egui::vec2(table_size.x, table_size.y),
                );
            }
        }

        LayoutMode::Floating => {
            // windows-style overlapping
            let base_size = settings.min_table_size.max(screen * 0.5);
            let table_size = fit_aspect_ratio(base_size, settings.aspect_ratio);

            for (i, table) in state.tables.iter_mut().enumerate() {
                if table.minimized {
                    continue;
                }
                let offset = settings.float_offset * i as f32;
                table.rect = egui::Rect::from_min_size(
                    egui::pos2(offset.x, offset.y),
                    egui::vec2(table_size.x, table_size.y),
                );
                table.grid_pos = (i, 0);
            }
            settings.grid_size = (table_count, 1);
        }

        LayoutMode::Stack => {
            // one visible, tabs at bottom
            let table_size = fit_aspect_ratio(screen * Vec2::new(0.95, 0.88), settings.aspect_ratio);
            let center_x = (screen.x - table_size.x) / 2.0;
            let active = state.active_table;

            for (i, table) in state.tables.iter_mut().enumerate() {
                if Some(i) == active && !table.minimized {
                    table.rect = egui::Rect::from_min_size(
                        egui::pos2(center_x, 4.0),
                        egui::vec2(table_size.x, table_size.y),
                    );
                } else {
                    // tab at bottom
                    let tab_width = (screen.x / table_count.max(1) as f32).min(150.0);
                    table.rect = egui::Rect::from_min_size(
                        egui::pos2(i as f32 * tab_width, screen.y - 40.0),
                        egui::vec2(tab_width - 2.0, 36.0),
                    );
                }
                table.grid_pos = (i, 0);
            }
            settings.grid_size = (table_count, 1);
        }
    }
}

/// render all tables
fn render_tables(
    state: Res<MultiTableState>,
    settings: Res<TableLayoutSettings>,
    mut contexts: EguiContexts,
) {
    // skip if no tables
    if state.tables.is_empty() {
        return;
    }

    let ctx = contexts.ctx_mut();

    // render in priority order (lowest first)
    for &idx in &state.render_order {
        let table = &state.tables[idx];
        let is_active = state.active_table == Some(idx);

        render_table_window(ctx, table, is_active, &settings);
    }
}

// theme colors for multitable UI
mod table_theme {
    use bevy_egui::egui::Color32;
    pub const FELT_GREEN: Color32 = Color32::from_rgb(25, 65, 35);
    pub const BORDER_ACTIVE: Color32 = Color32::from_rgb(100, 200, 120);
    pub const BORDER_YOUR_TURN: Color32 = Color32::from_rgb(220, 160, 60);
    pub const BORDER_IDLE: Color32 = Color32::from_rgb(50, 60, 70);
    pub const PANEL_BG: Color32 = Color32::from_rgb(20, 25, 35);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(240, 240, 245);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(150, 160, 175);
    pub const GOLD: Color32 = Color32::from_rgb(255, 200, 60);
}

/// render a single table
fn render_table_window(
    ctx: &egui::Context,
    table: &TableInstance,
    is_active: bool,
    settings: &TableLayoutSettings,
) {
    let border_color = if is_active && settings.highlight_active {
        table_theme::BORDER_ACTIVE
    } else if table.is_our_turn {
        // pulsing effect based on time
        let pulse = (table.time_remaining * 2.0).sin().abs();
        let r = (220.0 + 35.0 * pulse) as u8;
        let g = (160.0 + 30.0 * pulse) as u8;
        egui::Color32::from_rgb(r, g, 60)
    } else {
        table_theme::BORDER_IDLE
    };

    let frame = egui::Frame::none()
        .fill(table_theme::FELT_GREEN)
        .stroke(egui::Stroke::new(
            if settings.show_borders { 2.5 } else { 0.0 },
            border_color,
        ))
        .rounding(egui::Rounding::same(6.0));

    egui::Area::new(egui::Id::new(format!("table_{}", table.id)))
        .fixed_pos(table.rect.min)
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_min_size(table.rect.size());

                if table.minimized {
                    // minimized view - just name and pot
                    ui.horizontal(|ui| {
                        ui.label(&table.name);
                        if table.is_our_turn {
                            ui.label(egui::RichText::new("ACTION").color(egui::Color32::RED));
                        }
                    });
                } else {
                    // full table view
                    render_full_table(ui, table);
                }
            });
        });
}

/// render full table content
fn render_full_table(ui: &mut egui::Ui, table: &TableInstance) {
    // create adjusted game state with current table info
    let mut state = table.game_state.clone();
    state.is_our_turn = table.is_our_turn;
    state.time_remaining = table.time_remaining;
    state.pot = table.pot;

    // render using 2d table renderer
    if let Some(action) = table_2d::render_table(ui, &state, table.rect) {
        // handle action - will be wired to p2p/chain later
        match action {
            table_2d::PlayerAction::Fold => {
                info!("table {}: fold", table.id);
            }
            table_2d::PlayerAction::Check => {
                info!("table {}: check", table.id);
            }
            table_2d::PlayerAction::Call(amount) => {
                info!("table {}: call {}", table.id, amount);
            }
            table_2d::PlayerAction::Bet(amount) => {
                info!("table {}: bet {}", table.id, amount);
            }
            table_2d::PlayerAction::Raise(amount) => {
                info!("table {}: raise to {}", table.id, amount);
            }
            table_2d::PlayerAction::AllIn => {
                info!("table {}: all-in", table.id);
            }
        }
    }
}

/// handle table navigation
///
/// hjkl = vim-style navigation in tiled mode
/// Alt+1-9 = switch to table N
/// Alt+Left/Right = prev/next table
/// Ctrl+n = open new table
/// Ctrl+w = close current table
/// Ctrl+1-4 = switch layout mode
/// Tab = focus next table needing action
fn handle_table_navigation(
    mut state: ResMut<MultiTableState>,
    mut settings: ResMut<TableLayoutSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    mut notifications: ResMut<NotificationQueue>,
    pending: Res<PendingAction>,
) {
    let ctrl_held = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt_held = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);

    // don't handle navigation if we have a pending action to confirm
    if pending.action.is_some() {
        return;
    }

    // Ctrl+n = open new table
    if ctrl_held && keys.just_pressed(KeyCode::KeyN) {
        let id = settings.next_table_id;
        settings.next_table_id += 1;

        let name = format!("table {}", id);
        let stakes = ["0.25/0.5", "0.5/1", "1/2", "2/5", "5/10"]
            [(id as usize) % 5].to_string();

        state.add_table(TableInstance::new_with_demo(
            id,
            [id as u8; 32],
            name.clone(),
            stakes,
        ));

        // auto-switch to tiled mode when opening 2nd table
        if state.tables.len() == 2 && settings.mode == LayoutMode::Single {
            settings.mode = LayoutMode::Tiled;
            info!("layout: tiled (use hjkl to navigate)");
        }

        state.active_table = Some(state.tables.len() - 1);
        info!("opened {}", name);
        return;
    }

    // Ctrl+w = close current table
    if ctrl_held && keys.just_pressed(KeyCode::KeyW) {
        if let Some(idx) = state.active_table {
            if state.tables.len() > 1 {
                let table_id = state.tables[idx].id;
                state.tables.remove(idx);
                state.render_order.retain(|&i| i != idx);
                for i in &mut state.render_order {
                    if *i > idx { *i -= 1; }
                }
                state.active_table = Some(idx.saturating_sub(1).min(state.tables.len() - 1));

                // back to single mode if only 1 table
                if state.tables.len() == 1 {
                    settings.mode = LayoutMode::Single;
                }
                info!("closed table {}", table_id);
            }
        }
        return;
    }

    // Ctrl+1-4 = switch layout mode
    if ctrl_held {
        if keys.just_pressed(KeyCode::Digit1) {
            settings.mode = LayoutMode::Single;
            info!("layout: single");
            return;
        }
        if keys.just_pressed(KeyCode::Digit2) {
            settings.mode = LayoutMode::Tiled;
            info!("layout: tiled (hjkl navigation)");
            return;
        }
        if keys.just_pressed(KeyCode::Digit3) {
            settings.mode = LayoutMode::Floating;
            info!("layout: floating (windows-style)");
            return;
        }
        if keys.just_pressed(KeyCode::Digit4) {
            settings.mode = LayoutMode::Stack;
            info!("layout: stack (tabs)");
            return;
        }
    }

    // vim-style hjkl navigation in tiled mode
    if settings.mode == LayoutMode::Tiled && !alt_held && !ctrl_held {
        let (cols, rows) = settings.grid_size;
        if cols > 0 && rows > 0 {
            if let Some(idx) = state.active_table {
                if let Some(table) = state.tables.get(idx) {
                    let (col, row) = table.grid_pos;

                    let new_pos = if keys.just_pressed(KeyCode::KeyH) {
                        // left
                        Some((col.saturating_sub(1), row))
                    } else if keys.just_pressed(KeyCode::KeyL) {
                        // right
                        Some(((col + 1).min(cols - 1), row))
                    } else if keys.just_pressed(KeyCode::KeyK) {
                        // up
                        Some((col, row.saturating_sub(1)))
                    } else if keys.just_pressed(KeyCode::KeyJ) {
                        // down
                        Some((col, (row + 1).min(rows - 1)))
                    } else {
                        None
                    };

                    if let Some((new_col, new_row)) = new_pos {
                        // find table at new position
                        if let Some(new_idx) = state.tables.iter().position(|t| t.grid_pos == (new_col, new_row)) {
                            if new_idx != idx {
                                state.active_table = Some(new_idx);
                                if let Some(t) = state.tables.get(new_idx) {
                                    notifications.dismiss_for_table(t.id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Alt+number = switch to table N
    if alt_held {
        for (key, idx) in [
            (KeyCode::Digit1, 0),
            (KeyCode::Digit2, 1),
            (KeyCode::Digit3, 2),
            (KeyCode::Digit4, 3),
            (KeyCode::Digit5, 4),
            (KeyCode::Digit6, 5),
            (KeyCode::Digit7, 6),
            (KeyCode::Digit8, 7),
            (KeyCode::Digit9, 8),
        ] {
            if keys.just_pressed(key) && idx < state.tables.len() {
                state.active_table = Some(idx);
                if let Some(table) = state.tables.get(idx) {
                    notifications.dismiss_for_table(table.id);
                }
                return;
            }
        }

        // Alt+Left = previous table
        if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyH) {
            if let Some(idx) = state.active_table {
                let new_idx = if idx == 0 { state.tables.len() - 1 } else { idx - 1 };
                state.active_table = Some(new_idx);
            } else if !state.tables.is_empty() {
                state.active_table = Some(0);
            }
            return;
        }

        // Alt+Right = next table
        if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyL) {
            if let Some(idx) = state.active_table {
                let new_idx = (idx + 1) % state.tables.len();
                state.active_table = Some(new_idx);
            } else if !state.tables.is_empty() {
                state.active_table = Some(0);
            }
            return;
        }
    }

    // Tab = focus next table needing action (native app only)
    #[cfg(not(target_arch = "wasm32"))]
    if keys.just_pressed(KeyCode::Tab) {
        state.focus_next_action();
        if let Some(idx) = state.active_table {
            let table_id = state.tables[idx].id;
            notifications.dismiss_for_table(table_id);
        }
    }

    // Backtick = toggle last table (like alt-tab)
    if keys.just_pressed(KeyCode::Backquote) && !ctrl_held && !alt_held {
        state.toggle_last_table();
        if let Some(idx) = state.active_table {
            let table_id = state.tables[idx].id;
            notifications.dismiss_for_table(table_id);
        }
    }

    // === PRO BROWSER-FRIENDLY BINDINGS (Alt-based) ===
    // Note: Super/Win key doesn't work in browsers, so we use Alt for pro bindings
    let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    if alt_held {
        // Alt+u = jump to most urgent table (lowest time remaining)
        if keys.just_pressed(KeyCode::KeyU) {
            if let Some(idx) = state.most_urgent_table() {
                state.set_active(idx);
                info!("jumped to urgent table {}", idx + 1);
            }
            return;
        }

        // Alt+Space = cycle layout mode
        if keys.just_pressed(KeyCode::Space) {
            settings.mode = match settings.mode {
                LayoutMode::Single => LayoutMode::Tiled,
                LayoutMode::Tiled => LayoutMode::Stack,
                LayoutMode::Stack => LayoutMode::Floating,
                LayoutMode::Floating => LayoutMode::Single,
            };
            info!("layout: {:?}", settings.mode);
            return;
        }

        // Alt+j/k = cycle through action tables (tables needing your decision)
        if keys.just_pressed(KeyCode::KeyJ) {
            state.cycle_action_tables(false);
            return;
        }
        if keys.just_pressed(KeyCode::KeyK) {
            state.cycle_action_tables(true);
            return;
        }

        // Alt+m = minimize/restore current table
        if keys.just_pressed(KeyCode::KeyM) {
            if let Some(idx) = state.active_table {
                state.tables[idx].minimized = !state.tables[idx].minimized;
            }
            return;
        }

        // Alt+0 = table 10
        if keys.just_pressed(KeyCode::Digit0) && state.tables.len() > 9 {
            state.set_active(9);
            return;
        }
    }

    // Space (no modifiers) = jump to next action table (fastest way to grind)
    if keys.just_pressed(KeyCode::Space) && !ctrl_held && !alt_held {
        if state.action_count() > 0 {
            state.cycle_action_tables(false);
        }
    }

    // Shift+Space = cycle action tables in reverse
    if keys.just_pressed(KeyCode::Space) && shift_held && !ctrl_held && !alt_held {
        if state.action_count() > 0 {
            state.cycle_action_tables(true);
        }
    }
}

/// helper: navigate tables in direction
fn navigate_direction(
    state: &mut MultiTableState,
    settings: &TableLayoutSettings,
    dx: i32,
    dy: i32,
) {
    let Some(idx) = state.active_table else { return };
    let Some(table) = state.tables.get(idx) else { return };
    let (cols, rows) = settings.grid_size;

    if cols == 0 || rows == 0 {
        // fallback: just cycle
        let new_idx = if dx > 0 || dy > 0 {
            (idx + 1) % state.tables.len()
        } else if idx == 0 {
            state.tables.len() - 1
        } else {
            idx - 1
        };
        state.set_active(new_idx);
        return;
    }

    let (col, row) = table.grid_pos;
    let new_col = (col as i32 + dx).max(0).min(cols as i32 - 1) as usize;
    let new_row = (row as i32 + dy).max(0).min(rows as i32 - 1) as usize;

    if let Some(new_idx) = state.tables.iter().position(|t| t.grid_pos == (new_col, new_row)) {
        state.set_active(new_idx);
    }
}

/// process notifications
fn process_notifications(
    mut notifications: ResMut<NotificationQueue>,
    time: Res<Time>,
) {
    let now = time.elapsed_seconds_f64();
    notifications.clear_expired(now);
}

/// render notification overlay
fn render_notification_overlay(
    notifications: Res<NotificationQueue>,
    mut contexts: EguiContexts,
) {
    if notifications.notifications.is_empty() {
        return;
    }

    let ctx = contexts.ctx_mut();

    egui::Area::new(egui::Id::new("notifications"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-15.0, 15.0))
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                for notif in &notifications.notifications {
                    if notif.dismissed {
                        continue;
                    }

                    let (text, accent, icon) = match &notif.kind {
                        NotificationType::YourTurn { time_remaining, .. } => {
                            (format!("Your turn ({:.0}s)", time_remaining),
                             table_theme::GOLD, "⚡")
                        }
                        NotificationType::LowTimeWarning { seconds, .. } => {
                            (format!("LOW TIME: {:.0}s", seconds),
                             egui::Color32::from_rgb(220, 80, 80), "⚠")
                        }
                        NotificationType::YouWon { amount, .. } => {
                            (format!("Won {}", format_chips(*amount)),
                             egui::Color32::from_rgb(80, 200, 120), "✓")
                        }
                        NotificationType::BigPot { pot, .. } => {
                            (format!("Big pot: {}", format_chips(*pot)),
                             table_theme::GOLD, "💰")
                        }
                        _ => continue,
                    };

                    egui::Frame::none()
                        .fill(table_theme::PANEL_BG)
                        .stroke(egui::Stroke::new(1.5, accent.linear_multiply(0.6)))
                        .rounding(egui::Rounding::same(6.0))
                        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(icon)
                                    .size(14.0)
                                    .color(accent));
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(text)
                                    .size(13.0)
                                    .color(accent)
                                    .strong());
                            });
                        });
                    ui.add_space(4.0);
                }
            });
        });
}

// === helpers ===

/// calculate optimal grid dimensions
fn optimal_grid(count: usize) -> (usize, usize) {
    match count {
        0 => (1, 1),
        1 => (1, 1),
        2 => (2, 1),
        3 => (3, 1),
        4 => (2, 2),
        5 | 6 => (3, 2),
        7 | 8 | 9 => (3, 3),
        10..=12 => (4, 3),
        13..=16 => (4, 4),
        17..=20 => (5, 4),
        _ => (6, 4), // max 24 tables
    }
}

/// fit size to aspect ratio
fn fit_aspect_ratio(available: Vec2, ratio: f32) -> Vec2 {
    let by_width = Vec2::new(available.x, available.x / ratio);
    let by_height = Vec2::new(available.y * ratio, available.y);

    if by_width.y <= available.y {
        by_width
    } else {
        by_height
    }
}

/// format chip count
fn format_chips(chips: u64) -> String {
    if chips >= 1_000_000 {
        format!("{:.1}M", chips as f64 / 1_000_000.0)
    } else if chips >= 1_000 {
        format!("{:.1}K", chips as f64 / 1_000.0)
    } else {
        chips.to_string()
    }
}

// === action hotkeys ===

/// handle keyboard shortcuts for poker actions (sc2 grid style)
///
/// Q = cancel, W = fold, E = check/call, R = raise, A = all-in
/// 1-9,0 = bet sizing presets
fn handle_action_hotkeys(
    state: Res<MultiTableState>,
    hotkeys: Res<HotkeySettings>,
    keys: Res<ButtonInput<KeyCode>>,
    mut pending: ResMut<PendingAction>,
    hint_state: Res<HintModeState>,
    time: Res<Time>,
) {
    // block action keys when in hint mode (F+A shouldn't all-in)
    if hint_state.active {
        return;
    }

    // only process if we have an active table where it's our turn
    let Some(idx) = state.active_table else { return };
    let Some(table) = state.tables.get(idx) else { return };

    if !table.is_our_turn {
        return;
    }

    let game = &table.game_state;
    let now = time.elapsed_seconds_f64();

    // Q = cancel pending action
    if keys.just_pressed(hotkeys.cancel) || keys.just_pressed(KeyCode::Escape) {
        if pending.action.is_some() {
            info!("cancelled pending action");
            pending.action = None;
        }
        return;
    }

    // W = fold
    if keys.just_pressed(hotkeys.fold) {
        let action = QueuedAction::Fold;
        if hotkeys.require_confirm {
            pending.action = Some(action);
            pending.queued_at = now;
            pending.table_id = table.id;
            info!("queued: fold (press Enter to confirm)");
        } else {
            info!("action: fold on table {}", table.id);
            // TODO: send fold action via p2p/chain
        }
        return;
    }

    // E = check/call
    if keys.just_pressed(hotkeys.check_call) {
        let action = if game.current_bet == 0 {
            QueuedAction::Check
        } else {
            QueuedAction::Call(game.current_bet)
        };

        if hotkeys.require_confirm {
            pending.action = Some(action.clone());
            pending.queued_at = now;
            pending.table_id = table.id;
            match &action {
                QueuedAction::Check => info!("queued: check (press Enter to confirm)"),
                QueuedAction::Call(amt) => info!("queued: call {} (press Enter to confirm)", amt),
                _ => {}
            }
        } else {
            match &action {
                QueuedAction::Check => info!("action: check on table {}", table.id),
                QueuedAction::Call(amt) => info!("action: call {} on table {}", amt, table.id),
                _ => {}
            }
            // TODO: send action via p2p/chain
        }
        return;
    }

    // A = all-in
    if keys.just_pressed(hotkeys.all_in) {
        let action = QueuedAction::AllIn;
        if hotkeys.require_confirm {
            pending.action = Some(action);
            pending.queued_at = now;
            pending.table_id = table.id;
            info!("queued: ALL-IN (press Enter to confirm)");
        } else {
            info!("action: all-in on table {}", table.id);
            // TODO: send all-in action via p2p/chain
        }
        return;
    }

    // number keys 1-9,0 = bet sizing presets
    let number_keys = [
        (KeyCode::Digit1, 0),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
        (KeyCode::Digit4, 3),
        (KeyCode::Digit5, 4),
        (KeyCode::Digit6, 5),
        (KeyCode::Digit7, 6),
        (KeyCode::Digit8, 7),
        (KeyCode::Digit9, 8),
        (KeyCode::Digit0, 9),
    ];

    for (key, preset_idx) in number_keys {
        if keys.just_pressed(key) {
            let preset = &hotkeys.bet_presets[preset_idx];

            // calculate amount based on preset type
            let (amount, label) = match preset.of {
                BetPresetBase::MinRaise => {
                    // min raise = current bet + min raise increment
                    (game.min_raise, "min".to_string())
                }
                BetPresetBase::Pot => {
                    let amt = (game.pot as u64 * preset.percent as u64) / 100;
                    (amt, format!("{}% pot", preset.percent))
                }
                BetPresetBase::Stack => {
                    // get our stack from player list
                    let stack = game.players.get(game.our_seat)
                        .and_then(|p| p.as_ref())
                        .map(|p| p.stack)
                        .unwrap_or(0);
                    (stack, "all-in".to_string())
                }
            };

            if amount >= game.min_raise || preset.of == BetPresetBase::Stack || preset.of == BetPresetBase::MinRaise {
                let action = if game.current_bet == 0 {
                    QueuedAction::Bet(amount)
                } else {
                    QueuedAction::Raise(amount)
                };

                if hotkeys.require_confirm {
                    pending.action = Some(action.clone());
                    pending.queued_at = now;
                    pending.table_id = table.id;
                    info!("queued: {} = {} (press Enter to confirm)", label, amount);
                } else {
                    info!("action: bet/raise {} on table {}", amount, table.id);
                    // TODO: send action via p2p/chain
                }
            }
            return;
        }
    }
}

/// handle Enter key to confirm pending action
/// arrow keys to adjust bet amount
fn handle_pending_confirm(
    mut pending: ResMut<PendingAction>,
    mut state: ResMut<MultiTableState>,
    mut demo: ResMut<DemoState>,
    hotkeys: Res<HotkeySettings>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    if pending.action.is_none() {
        return;
    }

    // get table info for bet adjustments
    let table_info = state.active_table
        .and_then(|idx| state.tables.get(idx))
        .map(|t| (t.game_state.min_raise, t.game_state.pot));

    // up/down arrows to adjust bet amount
    if let Some(ref mut action) = pending.action {
        let (min_raise, pot) = table_info.unwrap_or((10, 100));

        // step size: shift = big blind, ctrl = pot %, normal = min raise
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

        let step = if ctrl {
            pot / 10  // 10% pot steps
        } else if shift {
            min_raise / 2  // half min raise (usually BB)
        } else {
            min_raise  // min raise steps
        }.max(1);

        let adjust = |amount: &mut u64, delta: i64| {
            if delta > 0 {
                *amount = amount.saturating_add(delta as u64);
            } else {
                *amount = (*amount).saturating_sub((-delta) as u64).max(min_raise);
            }
        };

        if keys.just_pressed(KeyCode::ArrowUp) {
            match action {
                QueuedAction::Bet(ref mut amt) |
                QueuedAction::Raise(ref mut amt) |
                QueuedAction::Call(ref mut amt) => {
                    adjust(amt, step as i64);
                    info!("adjusted: {}", amt);
                }
                _ => {}
            }
        }

        if keys.just_pressed(KeyCode::ArrowDown) {
            match action {
                QueuedAction::Bet(ref mut amt) |
                QueuedAction::Raise(ref mut amt) |
                QueuedAction::Call(ref mut amt) => {
                    adjust(amt, -(step as i64));
                    info!("adjusted: {}", amt);
                }
                _ => {}
            }
        }
    }

    // Enter = confirm
    if keys.just_pressed(hotkeys.confirm) {
        if let Some(action) = pending.action.take() {
            let table_id = pending.table_id;
            let now = time.elapsed_seconds_f64();

            match action {
                QueuedAction::Fold => {
                    info!("table {}: fold", table_id);
                }
                QueuedAction::Check => {
                    info!("table {}: check", table_id);
                }
                QueuedAction::Call(amt) => {
                    info!("table {}: call {}", table_id, amt);
                }
                QueuedAction::Bet(amt) => {
                    info!("table {}: bet {}", table_id, amt);
                }
                QueuedAction::Raise(amt) => {
                    info!("table {}: raise to {}", table_id, amt);
                }
                QueuedAction::AllIn => {
                    info!("table {}: all-in", table_id);
                }
            }

            // mark action taken - end our turn on this table
            if let Some(table) = state.tables.iter_mut().find(|t| t.id == table_id) {
                table.is_our_turn = false;
                table.game_state.is_our_turn = false;

                // add chips to pot for bet/call/raise actions
                match &action {
                    QueuedAction::Call(amt) | QueuedAction::Bet(amt) | QueuedAction::Raise(amt) => {
                        table.pot += *amt;
                        table.game_state.pot = table.pot;
                    }
                    _ => {}
                }

                // mark hero as not active, next player active
                for (i, p) in table.game_state.players.iter_mut().enumerate() {
                    if let Some(player) = p {
                        player.is_active = i == 1; // next player
                    }
                }
            }

            // record action for demo cycling
            demo_record_action(&mut demo, table_id, now);
        }
    }
}

/// handle hint mode (vimium-style F key navigation)
///
/// press F to show hints on all clickable elements:
/// - action buttons (fold, check, call, all-in)
/// - bet sizing presets (1-9, 0)
/// - tables (when multiple)
fn handle_hint_mode(
    mut hint_state: ResMut<HintModeState>,
    mut state: ResMut<MultiTableState>,
    mut pending: ResMut<PendingAction>,
    hotkeys: Res<HotkeySettings>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    time: Res<Time>,
) {
    // F = toggle hint mode
    if keys.just_pressed(hotkeys.hint_mode) {
        hint_state.active = !hint_state.active;
        hint_state.input.clear();
        if hint_state.active {
            generate_hints(&mut hint_state, &state, &hotkeys, &windows);
            info!("hint mode: ON - type letter to click");
        } else {
            info!("hint mode: OFF");
        }
    }

    // Escape exits hint mode
    if hint_state.active && keys.just_pressed(KeyCode::Escape) {
        hint_state.active = false;
        hint_state.input.clear();
        hint_state.hints.clear();
    }

    // handle letter input for hint selection
    if hint_state.active {
        let letter_keys = [
            (KeyCode::KeyA, 'A'), (KeyCode::KeyB, 'B'), (KeyCode::KeyC, 'C'),
            (KeyCode::KeyD, 'D'), (KeyCode::KeyE, 'E'), (KeyCode::KeyG, 'G'),
            (KeyCode::KeyH, 'H'), (KeyCode::KeyI, 'I'), (KeyCode::KeyJ, 'J'),
            (KeyCode::KeyK, 'K'), (KeyCode::KeyL, 'L'), (KeyCode::KeyM, 'M'),
            (KeyCode::KeyN, 'N'), (KeyCode::KeyO, 'O'), (KeyCode::KeyP, 'P'),
            (KeyCode::KeyR, 'R'), (KeyCode::KeyS, 'S'), (KeyCode::KeyT, 'T'),
            (KeyCode::KeyU, 'U'), (KeyCode::KeyV, 'V'), (KeyCode::KeyX, 'X'),
            (KeyCode::KeyY, 'Y'), (KeyCode::KeyZ, 'Z'),
        ];

        for (key, ch) in letter_keys {
            if keys.just_pressed(key) {
                // find matching hint
                let label = ch.to_string();
                let action = hint_state.hints.iter()
                    .find(|h| h.label == label)
                    .map(|h| h.action.clone());

                if let Some(action) = action {
                    // handle FocusInput specially - set pending_focus
                    if let HintAction::FocusInput(ref id) = action {
                        hint_state.pending_focus = Some(egui::Id::new(id.as_str()));
                        info!("hint: focus input {}", id);
                    } else {
                        execute_hint_action(
                            &action,
                            &mut state,
                            &mut pending,
                            &hotkeys,
                            time.elapsed_seconds_f64(),
                        );
                    }
                }
                hint_state.active = false;
                hint_state.input.clear();
                hint_state.hints.clear();
                break;
            }
        }
    }
}

/// generate hints for all clickable elements
fn generate_hints(
    hint_state: &mut HintModeState,
    state: &MultiTableState,
    _hotkeys: &HotkeySettings,
    windows: &Query<&Window>,
) {
    hint_state.hints.clear();

    let Ok(window) = windows.get_single() else { return };
    let screen_w = window.width();
    let screen_h = window.height();

    // labels: A-Z excluding F (hint key), Q (cancel), W (fold), etc
    // use letters that don't conflict with action keys
    let labels = ['A', 'B', 'C', 'D', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'S', 'T', 'U', 'V', 'X', 'Y', 'Z'];
    let mut label_idx = 0;

    let mut next_label = || {
        if label_idx < labels.len() {
            let l = labels[label_idx].to_string();
            label_idx += 1;
            Some(l)
        } else {
            None
        }
    };

    // get active table for action hints
    let active_table = state.active_table.and_then(|idx| state.tables.get(idx));

    // === action buttons (bottom center area) ===
    if let Some(table) = active_table {
        if table.is_our_turn {
            let game = &table.game_state;
            let base_y = screen_h - 140.0;
            let center_x = screen_w / 2.0;

            // fold - left
            if let Some(label) = next_label() {
                hint_state.hints.push(HintTarget {
                    label,
                    pos: egui::pos2(center_x - 150.0, base_y),
                    action: HintAction::Fold,
                });
            }

            // check/call - center
            if let Some(label) = next_label() {
                let action = if game.current_bet == 0 {
                    HintAction::Check
                } else {
                    HintAction::Call(game.current_bet)
                };
                hint_state.hints.push(HintTarget {
                    label,
                    pos: egui::pos2(center_x, base_y),
                    action,
                });
            }

            // all-in - right
            if let Some(label) = next_label() {
                hint_state.hints.push(HintTarget {
                    label,
                    pos: egui::pos2(center_x + 150.0, base_y),
                    action: HintAction::AllIn,
                });
            }

            // === bet sizing presets (row above actions) ===
            let sizing_y = base_y - 50.0;
            let sizing_start_x = center_x - 200.0;
            let sizing_spacing = 50.0;

            for i in 0..7 {
                if let Some(label) = next_label() {
                    hint_state.hints.push(HintTarget {
                        label,
                        pos: egui::pos2(sizing_start_x + i as f32 * sizing_spacing, sizing_y),
                        action: HintAction::BetPreset(i),
                    });
                }
            }
        }
    }

    // === tables (when multiple) ===
    if state.tables.len() > 1 {
        for (i, table) in state.tables.iter().enumerate() {
            if let Some(label) = next_label() {
                hint_state.hints.push(HintTarget {
                    label,
                    pos: table.rect.center(),
                    action: HintAction::FocusTable(i),
                });
            }
        }
    }

    // === external hints from lobby/settings ===
    for ext_hint in hint_state.external_hints.drain(..) {
        if let Some(label) = next_label() {
            hint_state.hints.push(HintTarget {
                label,
                pos: ext_hint.pos,
                action: ext_hint.action,
            });
        }
    }
}

/// execute the action from a hint
fn execute_hint_action(
    action: &HintAction,
    state: &mut MultiTableState,
    pending: &mut PendingAction,
    hotkeys: &HotkeySettings,
    now: f64,
) {
    let Some(idx) = state.active_table else { return };
    let Some(table) = state.tables.get(idx) else { return };

    match action {
        HintAction::FocusTable(table_idx) => {
            state.active_table = Some(*table_idx);
            info!("hint: focused table {}", table_idx + 1);
        }
        HintAction::Fold => {
            let queued = QueuedAction::Fold;
            if hotkeys.require_confirm {
                pending.action = Some(queued);
                pending.queued_at = now;
                pending.table_id = table.id;
                info!("queued: fold (Enter to confirm)");
            } else {
                info!("action: fold");
            }
        }
        HintAction::Check => {
            let queued = QueuedAction::Check;
            if hotkeys.require_confirm {
                pending.action = Some(queued);
                pending.queued_at = now;
                pending.table_id = table.id;
                info!("queued: check (Enter to confirm)");
            } else {
                info!("action: check");
            }
        }
        HintAction::Call(amount) => {
            let queued = QueuedAction::Call(*amount);
            if hotkeys.require_confirm {
                pending.action = Some(queued);
                pending.queued_at = now;
                pending.table_id = table.id;
                info!("queued: call {} (Enter to confirm)", amount);
            } else {
                info!("action: call {}", amount);
            }
        }
        HintAction::AllIn => {
            let queued = QueuedAction::AllIn;
            if hotkeys.require_confirm {
                pending.action = Some(queued);
                pending.queued_at = now;
                pending.table_id = table.id;
                info!("queued: ALL-IN (Enter to confirm)");
            } else {
                info!("action: all-in");
            }
        }
        HintAction::BetPreset(preset_idx) => {
            let game = &table.game_state;
            let preset = &hotkeys.bet_presets[*preset_idx];

            let amount = match preset.of {
                BetPresetBase::MinRaise => game.min_raise,
                BetPresetBase::Pot => (game.pot as u64 * preset.percent as u64) / 100,
                BetPresetBase::Stack => {
                    game.players.get(game.our_seat)
                        .and_then(|p| p.as_ref())
                        .map(|p| p.stack)
                        .unwrap_or(0)
                }
            };

            let queued = if game.current_bet == 0 {
                QueuedAction::Bet(amount)
            } else {
                QueuedAction::Raise(amount)
            };

            if hotkeys.require_confirm {
                pending.action = Some(queued);
                pending.queued_at = now;
                pending.table_id = table.id;
                info!("queued: bet {} (Enter to confirm)", amount);
            } else {
                info!("action: bet {}", amount);
            }
        }
        HintAction::Click(_pos) => {
            // click handled separately via simulated mouse events
            info!("hint: click");
        }
        HintAction::FocusInput(id) => {
            info!("hint: focus input {}", id);
        }
    }
}

/// handle radial menu (right-click drag for betting)
fn handle_radial_menu(
    state: Res<MultiTableState>,
    mut radial: ResMut<RadialMenuState>,
    mut pending: ResMut<PendingAction>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    time: Res<Time>,
) {
    let Some(idx) = state.active_table else { return };
    let Some(table) = state.tables.get(idx) else { return };

    if !table.is_our_turn {
        radial.active = false;
        return;
    }

    let Ok(window) = windows.get_single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };
    let cursor = egui::pos2(cursor_pos.x, cursor_pos.y);

    // right-click to open menu
    if mouse.just_pressed(MouseButton::Right) {
        let game = &table.game_state;
        let our_stack = game.players.get(game.our_seat)
            .and_then(|p| p.as_ref())
            .map(|p| p.stack)
            .unwrap_or(0);

        *radial = RadialMenuState::with_bet_items(
            game.pot,
            game.current_bet,
            game.min_raise,
            our_stack,
        );
        radial.active = true;
        radial.center = cursor;
        radial.current = cursor;
    }

    // update selection while dragging
    if radial.active {
        radial.current = cursor;
        radial.update_selection();
    }

    // release to select
    if mouse.just_released(MouseButton::Right) && radial.active {
        let game = &table.game_state;
        let diff = radial.current - radial.center;
        let dist = diff.length();

        // center = check/call
        if dist < 30.0 {
            let action = if game.current_bet > 0 {
                QueuedAction::Call(game.current_bet)
            } else {
                QueuedAction::Check
            };
            let label = if game.current_bet > 0 { "CALL" } else { "CHECK" };

            if radial.require_drag_confirm {
                pending.action = Some(action);
                pending.table_id = table.id;
                pending.queued_at = time.elapsed_seconds_f64();
                info!("radial: queued {} (press Enter)", label);
            } else {
                info!("radial: {}", label);
            }
        } else if let Some(item) = radial.get_selected() {
            let action = item.action.clone();

            if radial.require_drag_confirm {
                // queue for confirmation
                pending.action = Some(action);
                pending.table_id = table.id;
                pending.queued_at = time.elapsed_seconds_f64();
                info!("radial: queued {} (press Enter)", item.label);
            } else {
                // immediate execution
                info!("radial: executing {}", item.label);
            }
        }
        radial.active = false;
    }

    // escape to cancel
    if radial.active && mouse.just_pressed(MouseButton::Left) {
        radial.active = false;
    }
}

/// render radial menu overlay
fn render_radial_menu(
    radial: Res<RadialMenuState>,
    mut contexts: EguiContexts,
) {
    if !radial.active {
        return;
    }

    let ctx = contexts.ctx_mut();
    let segment_count = radial.items.len();
    if segment_count == 0 {
        return;
    }

    egui::Area::new(egui::Id::new("radial_menu"))
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let painter = ui.painter();

            // blur-like dark overlay
            let screen = ui.ctx().screen_rect();
            painter.rect_filled(
                screen,
                egui::Rounding::ZERO,
                egui::Color32::from_rgba_unmultiplied(10, 12, 18, 180),
            );

            let center = radial.center;
            let outer_radius = 130.0;
            let inner_radius = 45.0;
            let gap = 2.0; // small gap between segments

            // draw segments
            let segment_angle = 2.0 * std::f32::consts::PI / segment_count as f32;
            let gap_angle = gap / outer_radius; // convert gap to angle

            for (i, item) in radial.items.iter().enumerate() {
                let start_angle = -std::f32::consts::PI / 2.0 + i as f32 * segment_angle + gap_angle / 2.0;
                let end_angle = start_angle + segment_angle - gap_angle;
                let mid_angle = start_angle + (segment_angle - gap_angle) / 2.0;

                let is_selected = radial.selected == i as i8;

                // premium glassmorphism: semi-transparent with brightness on selection
                let (base_alpha, color_boost) = if is_selected { (220, 60) } else { (140, 0) };
                let color = egui::Color32::from_rgba_unmultiplied(
                    (item.color.r() as u16 + color_boost).min(255) as u8,
                    (item.color.g() as u16 + color_boost).min(255) as u8,
                    (item.color.b() as u16 + color_boost).min(255) as u8,
                    base_alpha,
                );

                // selected segment gets slightly larger radius for "pop" effect
                let (seg_outer, seg_inner) = if is_selected {
                    (outer_radius + 8.0, inner_radius - 3.0)
                } else {
                    (outer_radius, inner_radius)
                };

                // draw arc segment
                let steps = 16;
                let mut points = Vec::new();

                // outer arc
                for s in 0..=steps {
                    let a = start_angle + (end_angle - start_angle) * s as f32 / steps as f32;
                    points.push(egui::pos2(
                        center.x + seg_outer * a.cos(),
                        center.y + seg_outer * a.sin(),
                    ));
                }

                // inner arc (reversed)
                for s in (0..=steps).rev() {
                    let a = start_angle + (end_angle - start_angle) * s as f32 / steps as f32;
                    points.push(egui::pos2(
                        center.x + seg_inner * a.cos(),
                        center.y + seg_inner * a.sin(),
                    ));
                }

                // no border - clean premium look
                painter.add(egui::Shape::convex_polygon(
                    points,
                    color,
                    egui::Stroke::NONE,
                ));

                // glow effect for selected segment
                if is_selected {
                    let glow_color = egui::Color32::from_rgba_unmultiplied(
                        item.color.r(),
                        item.color.g(),
                        item.color.b(),
                        40,
                    );
                    let mut glow_points = Vec::new();
                    let glow_outer = seg_outer + 12.0;
                    for s in 0..=steps {
                        let a = start_angle + (end_angle - start_angle) * s as f32 / steps as f32;
                        glow_points.push(egui::pos2(
                            center.x + glow_outer * a.cos(),
                            center.y + glow_outer * a.sin(),
                        ));
                    }
                    for s in (0..=steps).rev() {
                        let a = start_angle + (end_angle - start_angle) * s as f32 / steps as f32;
                        glow_points.push(egui::pos2(
                            center.x + seg_outer * a.cos(),
                            center.y + seg_outer * a.sin(),
                        ));
                    }
                    painter.add(egui::Shape::convex_polygon(
                        glow_points,
                        glow_color,
                        egui::Stroke::NONE,
                    ));
                }

                // label with shadow for readability
                let label_radius = (seg_outer + seg_inner) / 2.0;
                let label_pos = egui::pos2(
                    center.x + label_radius * mid_angle.cos(),
                    center.y + label_radius * mid_angle.sin(),
                );

                // shadow
                painter.text(
                    egui::pos2(label_pos.x + 1.0, label_pos.y + 1.0),
                    egui::Align2::CENTER_CENTER,
                    &item.label,
                    egui::FontId::proportional(if is_selected { 13.0 } else { 11.0 }),
                    egui::Color32::from_black_alpha(180),
                );

                // text
                let text_color = if is_selected {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::from_gray(200)
                };
                painter.text(
                    label_pos,
                    egui::Align2::CENTER_CENTER,
                    &item.label,
                    egui::FontId::proportional(if is_selected { 13.0 } else { 11.0 }),
                    text_color,
                );
            }

            // center circle (check/call) - premium glassmorphism
            let center_radius = inner_radius - 5.0;
            let (fill_alpha, text_color, scale) = if radial.center_selected {
                (200, egui::Color32::WHITE, 1.1)
            } else {
                (120, egui::Color32::from_gray(220), 1.0)
            };
            let scaled_radius = center_radius * scale;

            // glow when selected
            if radial.center_selected {
                painter.circle_filled(
                    center,
                    scaled_radius + 10.0,
                    egui::Color32::from_rgba_unmultiplied(80, 180, 120, 30),
                );
            }

            // main circle - no border
            painter.circle_filled(
                center,
                scaled_radius,
                egui::Color32::from_rgba_unmultiplied(50, 120, 90, fill_alpha),
            );

            // text shadow
            painter.text(
                egui::pos2(center.x + 1.0, center.y + 1.0),
                egui::Align2::CENTER_CENTER,
                &radial.center_label,
                egui::FontId::proportional(if radial.center_selected { 14.0 } else { 12.0 }),
                egui::Color32::from_black_alpha(180),
            );

            // text
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                &radial.center_label,
                egui::FontId::proportional(if radial.center_selected { 14.0 } else { 12.0 }),
                text_color,
            );

            // subtle line from center to cursor
            let line_alpha = if radial.center_selected { 60 } else { 100 };
            painter.line_segment(
                [center, radial.current],
                egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, line_alpha)),
            );

            // cursor dot with glow
            painter.circle_filled(
                radial.current,
                8.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 30),
            );
            painter.circle_filled(radial.current, 4.0, egui::Color32::WHITE);
        });
}

/// render hotkey hints overlay (sc2 grid style)
fn render_hotkey_overlay(
    state: Res<MultiTableState>,
    hotkeys: Res<HotkeySettings>,
    pending: Res<PendingAction>,
    mut contexts: EguiContexts,
) {
    if !hotkeys.show_hints {
        return;
    }

    // only show if we have an active table
    let Some(idx) = state.active_table else { return };
    let Some(table) = state.tables.get(idx) else { return };

    if !table.is_our_turn {
        return;
    }

    let ctx = contexts.ctx_mut();
    let game = &table.game_state;

    // key badge helper
    let key_badge = |ui: &mut egui::Ui, key: &str, color: egui::Color32| {
        egui::Frame::none()
            .fill(color.linear_multiply(0.3))
            .stroke(egui::Stroke::new(1.0, color))
            .rounding(egui::Rounding::same(3.0))
            .inner_margin(egui::Margin::symmetric(6.0, 2.0))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(key)
                    .size(12.0)
                    .strong()
                    .color(color));
            });
    };

    egui::Area::new(egui::Id::new("hotkey_hints"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -65.0))
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(table_theme::PANEL_BG)
                .stroke(egui::Stroke::new(1.0, table_theme::BORDER_IDLE))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    // header
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("ACTIONS")
                            .size(11.0)
                            .strong()
                            .color(table_theme::GOLD));
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("+ Enter")
                            .size(10.0)
                            .color(table_theme::TEXT_SECONDARY));
                    });
                    ui.add_space(6.0);

                    // action keys row
                    ui.horizontal(|ui| {
                        key_badge(ui, "W", egui::Color32::from_rgb(180, 140, 80));
                        ui.label(egui::RichText::new("fold")
                            .size(12.0)
                            .color(table_theme::TEXT_SECONDARY));
                        ui.add_space(6.0);

                        key_badge(ui, "E", egui::Color32::from_rgb(80, 160, 120));
                        let check_call = if game.current_bet == 0 {
                            "check".to_string()
                        } else {
                            format!("call {}", format_chips(game.current_bet))
                        };
                        ui.label(egui::RichText::new(check_call)
                            .size(12.0)
                            .color(table_theme::TEXT_SECONDARY));
                        ui.add_space(6.0);

                        key_badge(ui, "A", egui::Color32::from_rgb(180, 70, 70));
                        ui.label(egui::RichText::new("all-in")
                            .size(12.0)
                            .color(table_theme::TEXT_SECONDARY));
                    });

                    ui.add_space(8.0);

                    // sizing row
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("BET SIZE")
                            .size(10.0)
                            .color(egui::Color32::from_rgb(100, 120, 140)));
                        ui.add_space(6.0);
                        for (i, preset) in hotkeys.bet_presets.iter().take(6).enumerate() {
                            let label = match preset.of {
                                BetPresetBase::MinRaise => "min".to_string(),
                                BetPresetBase::Pot => format!("{}%", preset.percent),
                                BetPresetBase::Stack => "all".to_string(),
                            };
                            ui.label(egui::RichText::new(format!("{}:{}", i + 1, label))
                                .size(10.0)
                                .color(egui::Color32::from_rgb(120, 140, 160)));
                        }
                    });

                    // pending action indicator
                    if pending.action.is_some() {
                        ui.add_space(6.0);
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(40, 80, 50))
                            .rounding(egui::Rounding::same(4.0))
                            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("ENTER to confirm")
                                    .size(12.0)
                                    .strong()
                                    .color(egui::Color32::from_rgb(120, 220, 140)));
                            });
                    }
                });
        });
}

/// render pending action confirmation overlay
fn render_pending_action_overlay(
    pending: Res<PendingAction>,
    mut contexts: EguiContexts,
) {
    let Some(action) = &pending.action else { return };

    let ctx = contexts.ctx_mut();

    let action_text = match action {
        QueuedAction::Fold => "FOLD".to_string(),
        QueuedAction::Check => "CHECK".to_string(),
        QueuedAction::Call(amt) => format!("CALL {}", format_chips(*amt)),
        QueuedAction::Bet(amt) => format!("BET {}", format_chips(*amt)),
        QueuedAction::Raise(amt) => format!("RAISE {}", format_chips(*amt)),
        QueuedAction::AllIn => "ALL-IN".to_string(),
    };

    // check if action has adjustable amount
    let is_adjustable = matches!(action,
        QueuedAction::Bet(_) | QueuedAction::Raise(_) | QueuedAction::Call(_)
    );

    egui::Area::new(egui::Id::new("pending_action"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -100.0))
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(table_theme::PANEL_BG)
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::same(24.0))
                .stroke(egui::Stroke::new(2.5, table_theme::GOLD))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(&action_text)
                            .size(28.0)
                            .color(table_theme::GOLD)
                            .strong());
                        ui.add_space(12.0);

                        if is_adjustable {
                            ui.label(egui::RichText::new("↑/↓ adjust   shift=fine  ctrl=pot%")
                                .size(14.0)
                                .color(table_theme::TEXT_SECONDARY));
                            ui.add_space(6.0);
                        }

                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("ENTER")
                                .size(15.0)
                                .color(egui::Color32::from_rgb(100, 200, 120)));
                            ui.label(egui::RichText::new("confirm")
                                .size(15.0)
                                .color(table_theme::TEXT_SECONDARY));
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("ESC")
                                .size(15.0)
                                .color(egui::Color32::from_rgb(180, 100, 100)));
                            ui.label(egui::RichText::new("cancel")
                                .size(15.0)
                                .color(table_theme::TEXT_SECONDARY));
                        });
                    });
                });
        });
}

/// render hint mode overlay (vimium-style)
fn render_hint_overlay(
    hint_state: Res<HintModeState>,
    hotkeys: Res<HotkeySettings>,
    mut contexts: EguiContexts,
) {
    if !hint_state.active {
        return;
    }

    let ctx = contexts.ctx_mut();

    // dim background
    egui::Area::new(egui::Id::new("hint_mode_bg"))
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let screen = ui.ctx().screen_rect();
            ui.painter().rect_filled(
                screen,
                egui::Rounding::ZERO,
                egui::Color32::from_black_alpha(120),
            );
        });

    // hint mode indicator
    egui::Area::new(egui::Id::new("hint_mode_indicator"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 10.0))
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(50, 50, 150))
                .rounding(egui::Rounding::same(4.0))
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("HINT MODE - type letter to click, ESC to exit")
                        .color(egui::Color32::WHITE));
                });
        });

    // render hint labels at each position
    for hint in &hint_state.hints {
        // skip invalid positions
        if !hint.pos.x.is_finite() || !hint.pos.y.is_finite() {
            continue;
        }

        // get description for the action
        let description = match &hint.action {
            HintAction::FocusTable(idx) => format!("table {}", idx + 1),
            HintAction::Fold => "fold".to_string(),
            HintAction::Check => "check".to_string(),
            HintAction::Call(amt) => format!("call {}", format_chips(*amt)),
            HintAction::AllIn => "ALL-IN".to_string(),
            HintAction::BetPreset(idx) => {
                let preset = &hotkeys.bet_presets[*idx];
                match preset.of {
                    BetPresetBase::MinRaise => "min".to_string(),
                    BetPresetBase::Pot => format!("{}%", preset.percent),
                    BetPresetBase::Stack => "all-in".to_string(),
                }
            }
            HintAction::Click(_) => "click".to_string(),
            HintAction::FocusInput(_) => "input".to_string(),
        };

        egui::Area::new(egui::Id::new(format!("hint_{}", hint.label)))
            .fixed_pos(hint.pos)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(255, 200, 50))
                    .rounding(egui::Rounding::same(4.0))
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .stroke(egui::Stroke::new(2.0, egui::Color32::BLACK))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&hint.label)
                                .size(18.0)
                                .color(egui::Color32::BLACK)
                                .strong());
                            ui.label(egui::RichText::new(&description)
                                .size(12.0)
                                .color(egui::Color32::from_gray(60)));
                        });
                    });
            });
    }
}

// === auto-actions ===

/// process auto-actions for all tables
fn process_auto_actions(
    state: Res<MultiTableState>,
    mut auto_settings: ResMut<AutoActionSettings>,
) {
    for table in &state.tables {
        if !table.is_our_turn {
            continue;
        }

        let Some(auto_action) = auto_settings.table_actions.get(&table.id) else {
            continue;
        };

        let game = &table.game_state;

        // process auto-actions in priority order
        if auto_action.auto_check && game.current_bet == 0 {
            info!("auto-action: check on table {}", table.id);
            // TODO: send check action
            // clear auto-action after execution
        }

        if auto_action.auto_check_fold {
            if game.current_bet == 0 {
                info!("auto-action: check on table {}", table.id);
            } else {
                info!("auto-action: fold on table {}", table.id);
            }
            // TODO: send action
        }

        if auto_action.auto_fold && game.current_bet > 0 {
            info!("auto-action: fold on table {}", table.id);
            // TODO: send fold action
        }

        if auto_action.auto_call_any && game.current_bet > 0 {
            info!("auto-action: call {} on table {}", game.current_bet, table.id);
            // TODO: send call action
        }

        if let Some(limit) = auto_action.auto_call_limit {
            if game.current_bet > 0 && game.current_bet <= limit {
                info!("auto-action: call {} (under limit {}) on table {}", game.current_bet, limit, table.id);
                // TODO: send call action
            }
        }
    }

    // clear executed auto-actions
    for table in &state.tables {
        if table.is_our_turn {
            auto_settings.table_actions.remove(&table.id);
        }
    }
}

// === demo mode ===

/// demo hand phase
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DemoPhase {
    #[default]
    Shuffle,
    Deal,
    Preflop,
    DealFlop,
    Flop,
    DealTurn,
    Turn,
    DealRiver,
    River,
    Showdown,
    NewHand,
}

/// demo state for simulating gameplay
#[derive(Resource)]
pub struct DemoState {
    /// current demo phase per table
    pub phases: std::collections::HashMap<u64, DemoPhase>,
    /// time since phase started
    pub phase_started: std::collections::HashMap<u64, f64>,
    /// time since last action on each table
    pub last_action: std::collections::HashMap<u64, f64>,
    /// actions taken this betting round
    pub round_actions: std::collections::HashMap<u64, u32>,
    /// current bet in round
    pub current_bet: std::collections::HashMap<u64, u64>,
    /// demo deck for each table (for dealing cards)
    pub decks: std::collections::HashMap<u64, Vec<Card>>,
}

impl Default for DemoState {
    fn default() -> Self {
        Self {
            phases: std::collections::HashMap::new(),
            phase_started: std::collections::HashMap::new(),
            last_action: std::collections::HashMap::new(),
            round_actions: std::collections::HashMap::new(),
            current_bet: std::collections::HashMap::new(),
            decks: std::collections::HashMap::new(),
        }
    }
}

impl DemoState {
    /// create shuffled deck
    fn create_deck() -> Vec<Card> {
        let mut deck = Vec::with_capacity(52);
        for suit in [Suit::Hearts, Suit::Diamonds, Suit::Clubs, Suit::Spades] {
            for rank in [
                Rank::Two, Rank::Six, Rank::Seven, Rank::Nine, Rank::Ten,
                Rank::Jack, Rank::Queen, Rank::King, Rank::Ace,
            ] {
                deck.push(Card::new(rank, suit));
            }
        }
        // simple shuffle using time
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as usize;
        for i in 0..deck.len() {
            let j = (seed + i * 7) % deck.len();
            deck.swap(i, j);
        }
        deck
    }
}

/// simulate proper poker hand flow in demo mode
fn demo_turn_cycling(
    mut state: ResMut<MultiTableState>,
    mut demo: ResMut<DemoState>,
    time: Res<Time>,
) {
    let now = time.elapsed_seconds_f64();
    let dt = time.delta_seconds();

    for table in &mut state.tables {
        let table_id = table.id;

        // initialize demo state if needed
        if !demo.phases.contains_key(&table_id) {
            demo.phases.insert(table_id, DemoPhase::Shuffle);
            demo.phase_started.insert(table_id, now);
            demo.decks.insert(table_id, DemoState::create_deck());
            demo.round_actions.insert(table_id, 0);
            demo.current_bet.insert(table_id, 0);
            // start with empty table, will populate during deal
            table.game_state.players = vec![None; 6];
            table.game_state.community_cards.clear();
            table.game_state.phase = GamePhase::Waiting;
            table.is_our_turn = false;
        }

        let phase = *demo.phases.get(&table_id).unwrap_or(&DemoPhase::Shuffle);
        let phase_start = *demo.phase_started.get(&table_id).unwrap_or(&now);
        let phase_time = now - phase_start;

        match phase {
            DemoPhase::Shuffle => {
                // shuffle animation for 1.5 seconds
                table.game_state.animation.kind = table_2d::AnimationKind::Shuffle;
                table.game_state.animation.progress = (phase_time / 1.5).min(1.0) as f32;

                if phase_time > 1.5 {
                    table.game_state.animation.kind = table_2d::AnimationKind::None;
                    demo.phases.insert(table_id, DemoPhase::Deal);
                    demo.phase_started.insert(table_id, now);
                    info!("table {}: dealing cards", table_id);
                }
            }

            DemoPhase::Deal => {
                // deal cards over 1 second
                let deck = demo.decks.get_mut(&table_id).unwrap();

                // setup players on first frame
                if phase_time < dt as f64 * 2.0 {
                    table.game_state.players = vec![
                        Some(Player {
                            name: "hero".into(),
                            stack: 1000,
                            hole_cards: None,
                            ..Default::default()
                        }),
                        Some(Player {
                            name: "villain1".into(),
                            stack: 850,
                            is_dealer: true,
                            hole_cards: None,
                            ..Default::default()
                        }),
                        None,
                        Some(Player {
                            name: "villain2".into(),
                            stack: 1200,
                            is_sb: true,
                            current_bet: 5,
                            hole_cards: None,
                            ..Default::default()
                        }),
                        Some(Player {
                            name: "villain3".into(),
                            stack: 500,
                            is_bb: true,
                            current_bet: 10,
                            hole_cards: None,
                            ..Default::default()
                        }),
                        None,
                    ];
                }

                // deal cards progressively
                let deal_progress = (phase_time / 1.0).min(1.0);
                let cards_to_deal = (deal_progress * 8.0) as usize; // 4 players x 2 cards

                // deal to each player
                let player_indices = [0, 1, 3, 4];
                for (i, &seat) in player_indices.iter().enumerate() {
                    if let Some(Some(ref mut player)) = table.game_state.players.get_mut(seat) {
                        if player.hole_cards.is_none() && cards_to_deal > i * 2 + 1 {
                            // deal two cards
                            let c1 = deck.pop().unwrap_or(Card::new(Rank::Ace, Suit::Spades));
                            let c2 = deck.pop().unwrap_or(Card::new(Rank::King, Suit::Spades));
                            player.hole_cards = Some((c1, c2));
                        }
                    }
                }

                if phase_time > 1.2 {
                    demo.phases.insert(table_id, DemoPhase::Preflop);
                    demo.phase_started.insert(table_id, now);
                    demo.round_actions.insert(table_id, 0);
                    demo.current_bet.insert(table_id, 10); // bb
                    table.game_state.phase = GamePhase::Preflop;
                    table.game_state.pot = 15; // sb + bb
                    table.pot = 15;
                    table.is_our_turn = true;
                    table.time_remaining = 30.0;
                    table.game_state.is_our_turn = true;
                    table.game_state.time_remaining = 30.0;
                    table.game_state.current_bet = 10;
                    table.game_state.min_raise = 20;
                    info!("table {}: preflop - your turn", table_id);
                }
            }

            DemoPhase::Preflop | DemoPhase::Flop | DemoPhase::Turn | DemoPhase::River => {
                // betting round - wait for player action
                if table.is_our_turn {
                    table.time_remaining = (table.time_remaining - dt).max(0.0);
                    table.game_state.time_remaining = table.time_remaining;
                } else {
                    // simulate villain action after 1.5 seconds
                    let last = *demo.last_action.get(&table_id).unwrap_or(&0.0);
                    if now - last > 1.5 {
                        let actions = *demo.round_actions.get(&table_id).unwrap_or(&0);
                        if actions >= 3 {
                            // enough actions, move to next street
                            let next_phase = match phase {
                                DemoPhase::Preflop => DemoPhase::DealFlop,
                                DemoPhase::Flop => DemoPhase::DealTurn,
                                DemoPhase::Turn => DemoPhase::DealRiver,
                                DemoPhase::River => DemoPhase::Showdown,
                                _ => DemoPhase::Showdown,
                            };
                            demo.phases.insert(table_id, next_phase);
                            demo.phase_started.insert(table_id, now);
                            demo.round_actions.insert(table_id, 0);
                        } else {
                            // give hero another turn
                            table.is_our_turn = true;
                            table.time_remaining = 30.0;
                            table.game_state.is_our_turn = true;
                            table.game_state.time_remaining = 30.0;
                            let actions = demo.round_actions.get(&table_id).copied().unwrap_or(0);
                            demo.round_actions.insert(table_id, actions + 1);
                            // add some chips to pot
                            table.pot += 20;
                            table.game_state.pot = table.pot;
                        }
                    }
                }
            }

            DemoPhase::DealFlop => {
                // deal 3 community cards
                let deck = demo.decks.get_mut(&table_id).unwrap();
                if table.game_state.community_cards.is_empty() {
                    for _ in 0..3 {
                        if let Some(c) = deck.pop() {
                            table.game_state.community_cards.push(c);
                        }
                    }
                    table.game_state.phase = GamePhase::Flop;
                    info!("table {}: flop dealt", table_id);
                }

                if phase_time > 0.5 {
                    demo.phases.insert(table_id, DemoPhase::Flop);
                    demo.phase_started.insert(table_id, now);
                    demo.current_bet.insert(table_id, 0);
                    table.is_our_turn = true;
                    table.time_remaining = 30.0;
                    table.game_state.is_our_turn = true;
                    table.game_state.time_remaining = 30.0;
                    table.game_state.current_bet = 0;
                    table.game_state.min_raise = 10;
                }
            }

            DemoPhase::DealTurn => {
                let deck = demo.decks.get_mut(&table_id).unwrap();
                if table.game_state.community_cards.len() == 3 {
                    if let Some(c) = deck.pop() {
                        table.game_state.community_cards.push(c);
                    }
                    table.game_state.phase = GamePhase::Flop; // use Flop visually
                    info!("table {}: turn dealt", table_id);
                }

                if phase_time > 0.5 {
                    demo.phases.insert(table_id, DemoPhase::Turn);
                    demo.phase_started.insert(table_id, now);
                    demo.current_bet.insert(table_id, 0);
                    table.is_our_turn = true;
                    table.time_remaining = 30.0;
                    table.game_state.is_our_turn = true;
                    table.game_state.time_remaining = 30.0;
                    table.game_state.current_bet = 0;
                }
            }

            DemoPhase::DealRiver => {
                let deck = demo.decks.get_mut(&table_id).unwrap();
                if table.game_state.community_cards.len() == 4 {
                    if let Some(c) = deck.pop() {
                        table.game_state.community_cards.push(c);
                    }
                    info!("table {}: river dealt", table_id);
                }

                if phase_time > 0.5 {
                    demo.phases.insert(table_id, DemoPhase::River);
                    demo.phase_started.insert(table_id, now);
                    demo.current_bet.insert(table_id, 0);
                    table.is_our_turn = true;
                    table.time_remaining = 30.0;
                    table.game_state.is_our_turn = true;
                    table.game_state.time_remaining = 30.0;
                    table.game_state.current_bet = 0;
                }
            }

            DemoPhase::Showdown => {
                // show all cards
                for player in table.game_state.players.iter_mut().flatten() {
                    player.show_cards = true;
                }

                if phase_time > 3.0 {
                    demo.phases.insert(table_id, DemoPhase::NewHand);
                    demo.phase_started.insert(table_id, now);
                    info!("table {}: hand complete, starting new hand", table_id);
                }
            }

            DemoPhase::NewHand => {
                // reset for new hand
                if phase_time > 1.0 {
                    demo.phases.insert(table_id, DemoPhase::Shuffle);
                    demo.phase_started.insert(table_id, now);
                    demo.decks.insert(table_id, DemoState::create_deck());
                    table.game_state.community_cards.clear();
                    table.game_state.phase = GamePhase::Waiting;
                    table.is_our_turn = false;
                    table.pot = 0;
                    table.game_state.pot = 0;
                    // rotate dealer
                    info!("table {}: new hand starting", table_id);
                }
            }
        }
    }
}

/// record that player took an action (for demo flow)
pub fn demo_record_action(demo: &mut DemoState, table_id: u64, now: f64) {
    demo.last_action.insert(table_id, now);
    let actions = demo.round_actions.get(&table_id).copied().unwrap_or(0);
    demo.round_actions.insert(table_id, actions + 1);
}

/// render auto-action panel
fn render_auto_action_panel(
    state: Res<MultiTableState>,
    mut auto_settings: ResMut<AutoActionSettings>,
    mut contexts: EguiContexts,
) {
    // only show for active table when not our turn
    let Some(idx) = state.active_table else { return };
    let Some(table) = state.tables.get(idx) else { return };

    // show auto-action panel when it's not our turn (to queue actions)
    if table.is_our_turn {
        return;
    }

    let table_id = table.id;
    let ctx = contexts.ctx_mut();

    // get current auto-action state
    let current = auto_settings.table_actions
        .get(&table_id)
        .cloned()
        .unwrap_or_default();

    // track what button was clicked
    let mut new_action: Option<TableAutoAction> = None;
    let mut should_clear = false;

    // helper for styled toggle button
    let toggle_btn = |ui: &mut egui::Ui, label: &str, selected: bool, accent: egui::Color32| -> bool {
        let fill = if selected { accent.linear_multiply(0.4) } else { table_theme::PANEL_BG };
        let stroke = if selected {
            egui::Stroke::new(1.5, accent)
        } else {
            egui::Stroke::new(1.0, table_theme::BORDER_IDLE)
        };
        let text_color = if selected { accent } else { table_theme::TEXT_SECONDARY };

        let btn = egui::Button::new(
            egui::RichText::new(label).size(12.0).color(text_color)
        ).fill(fill).stroke(stroke);
        ui.add_sized([80.0, 28.0], btn).clicked()
    };

    egui::Area::new(egui::Id::new("auto_action_panel"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -20.0))
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(table_theme::PANEL_BG)
                .stroke(egui::Stroke::new(1.0, table_theme::BORDER_IDLE))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("QUEUE")
                            .size(10.0)
                            .color(table_theme::TEXT_SECONDARY));
                        ui.add_space(8.0);

                        if toggle_btn(ui, "fold", current.auto_fold, egui::Color32::from_rgb(180, 120, 80)) {
                            new_action = Some(TableAutoAction { auto_fold: true, ..Default::default() });
                        }
                        ui.add_space(4.0);

                        if toggle_btn(ui, "check", current.auto_check, egui::Color32::from_rgb(80, 160, 120)) {
                            new_action = Some(TableAutoAction { auto_check: true, ..Default::default() });
                        }
                        ui.add_space(4.0);

                        if toggle_btn(ui, "chk/fold", current.auto_check_fold, egui::Color32::from_rgb(140, 140, 100)) {
                            new_action = Some(TableAutoAction { auto_check_fold: true, ..Default::default() });
                        }
                        ui.add_space(4.0);

                        if toggle_btn(ui, "call any", current.auto_call_any, egui::Color32::from_rgb(100, 150, 200)) {
                            new_action = Some(TableAutoAction { auto_call_any: true, ..Default::default() });
                        }
                        ui.add_space(8.0);

                        let clear_btn = egui::Button::new(
                            egui::RichText::new("×").size(14.0).color(table_theme::TEXT_SECONDARY)
                        ).fill(egui::Color32::TRANSPARENT);
                        if ui.add_sized([24.0, 28.0], clear_btn).clicked() {
                            should_clear = true;
                        }
                    });

                    // show queued action indicator
                    if current.auto_fold || current.auto_check || current.auto_check_fold || current.auto_call_any {
                        ui.add_space(6.0);
                        let (action_text, color) = if current.auto_fold {
                            ("→ will FOLD", egui::Color32::from_rgb(200, 140, 80))
                        } else if current.auto_check {
                            ("→ will CHECK", egui::Color32::from_rgb(100, 180, 140))
                        } else if current.auto_check_fold {
                            ("→ will CHECK/FOLD", egui::Color32::from_rgb(160, 160, 120))
                        } else if current.auto_call_any {
                            ("→ will CALL ANY", egui::Color32::from_rgb(120, 170, 220))
                        } else {
                            ("queued", table_theme::TEXT_SECONDARY)
                        };
                        ui.label(egui::RichText::new(action_text)
                            .size(11.0)
                            .strong()
                            .color(color));
                    }
                });
        });

    // apply changes after UI
    if let Some(action) = new_action {
        auto_settings.table_actions.insert(table_id, action);
    }
    if should_clear {
        auto_settings.table_actions.remove(&table_id);
    }
}
