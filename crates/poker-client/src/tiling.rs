//! tiling - bspwm-style window manager for poker client
//!
//! binary tree layout with vim-style navigation
//!
//! controls (Alt+):
//! - hjkl = focus window in direction
//! - HJKL = swap window in direction
//! - f = toggle fullscreen
//! - s = split horizontal
//! - v = split vertical
//! - q = close window
//! - 1-9 = focus by index
//! - tab = cycle focus

use bevy::prelude::*;
use bevy_egui::egui;

pub struct TilingPlugin;

impl Plugin for TilingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TilingState>()
            .add_systems(Update, handle_tiling_input);
    }
}

/// window types that can be tiled
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowKind {
    PokerTable(u64),  // table id
    Blockout,
    Settings,
    Lobby,
    Chat,
}

impl WindowKind {
    pub fn title(&self) -> String {
        match self {
            WindowKind::PokerTable(id) => format!("table {}", id),
            WindowKind::Blockout => "blockout".into(),
            WindowKind::Settings => "settings".into(),
            WindowKind::Lobby => "lobby".into(),
            WindowKind::Chat => "chat".into(),
        }
    }
}

/// split direction
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SplitDir {
    #[default]
    Horizontal,
    Vertical,
}

/// a node in the binary tree (either a split or a window)
#[derive(Clone, Debug)]
pub enum Node {
    /// leaf node containing a window
    Window {
        kind: WindowKind,
        /// unique id for this node
        id: u64,
    },
    /// split node containing two children
    Split {
        dir: SplitDir,
        /// split ratio (0.0 - 1.0, position of divider)
        ratio: f32,
        /// left/top child
        first: Box<Node>,
        /// right/bottom child
        second: Box<Node>,
        /// unique id for this node
        id: u64,
    },
}

impl Node {
    pub fn id(&self) -> u64 {
        match self {
            Node::Window { id, .. } => *id,
            Node::Split { id, .. } => *id,
        }
    }

    pub fn window(kind: WindowKind, id: u64) -> Self {
        Node::Window { kind, id }
    }

    pub fn split(dir: SplitDir, first: Node, second: Node, id: u64) -> Self {
        Node::Split {
            dir,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
            id,
        }
    }

    /// find a window by kind
    pub fn find_window(&self, kind: WindowKind) -> Option<u64> {
        match self {
            Node::Window { kind: k, id } if *k == kind => Some(*id),
            Node::Window { .. } => None,
            Node::Split { first, second, .. } => {
                first.find_window(kind).or_else(|| second.find_window(kind))
            }
        }
    }

    /// get all window ids in order (depth-first)
    pub fn window_ids(&self) -> Vec<u64> {
        match self {
            Node::Window { id, .. } => vec![*id],
            Node::Split { first, second, .. } => {
                let mut ids = first.window_ids();
                ids.extend(second.window_ids());
                ids
            }
        }
    }

    /// get all windows in order
    pub fn windows(&self) -> Vec<(u64, WindowKind)> {
        match self {
            Node::Window { id, kind } => vec![(*id, *kind)],
            Node::Split { first, second, .. } => {
                let mut wins = first.windows();
                wins.extend(second.windows());
                wins
            }
        }
    }

    /// count windows
    pub fn window_count(&self) -> usize {
        match self {
            Node::Window { .. } => 1,
            Node::Split { first, second, .. } => {
                first.window_count() + second.window_count()
            }
        }
    }

    /// calculate rect for a specific window id
    pub fn rect_for_window(&self, target_id: u64, bounds: egui::Rect) -> Option<egui::Rect> {
        match self {
            Node::Window { id, .. } => {
                if *id == target_id {
                    Some(bounds)
                } else {
                    None
                }
            }
            Node::Split { dir, ratio, first, second, .. } => {
                let (first_bounds, second_bounds) = split_rect(bounds, *dir, *ratio);
                first.rect_for_window(target_id, first_bounds)
                    .or_else(|| second.rect_for_window(target_id, second_bounds))
            }
        }
    }

    /// get the window kind for a node id
    pub fn kind_for_id(&self, target_id: u64) -> Option<WindowKind> {
        match self {
            Node::Window { id, kind } => {
                if *id == target_id {
                    Some(*kind)
                } else {
                    None
                }
            }
            Node::Split { first, second, .. } => {
                first.kind_for_id(target_id)
                    .or_else(|| second.kind_for_id(target_id))
            }
        }
    }

    /// remove a window by id, returns new tree (or None if empty)
    pub fn remove_window(&self, target_id: u64) -> Option<Node> {
        match self {
            Node::Window { id, .. } => {
                if *id == target_id {
                    None
                } else {
                    Some(self.clone())
                }
            }
            Node::Split { dir, ratio, first, second, id } => {
                let first_removed = first.remove_window(target_id);
                let second_removed = second.remove_window(target_id);

                match (first_removed, second_removed) {
                    (None, None) => None,
                    (Some(f), None) => Some(f),
                    (None, Some(s)) => Some(s),
                    (Some(f), Some(s)) => Some(Node::Split {
                        dir: *dir,
                        ratio: *ratio,
                        first: Box::new(f),
                        second: Box::new(s),
                        id: *id,
                    }),
                }
            }
        }
    }

    /// insert a window next to target, splitting in given direction
    pub fn insert_next_to(&self, target_id: u64, new_node: Node, dir: SplitDir, new_split_id: u64) -> Node {
        match self {
            Node::Window { id, kind } => {
                if *id == target_id {
                    // split this window
                    Node::Split {
                        dir,
                        ratio: 0.5,
                        first: Box::new(self.clone()),
                        second: Box::new(new_node),
                        id: new_split_id,
                    }
                } else {
                    self.clone()
                }
            }
            Node::Split { dir: d, ratio, first, second, id } => {
                Node::Split {
                    dir: *d,
                    ratio: *ratio,
                    first: Box::new(first.insert_next_to(target_id, new_node.clone(), dir, new_split_id)),
                    second: Box::new(second.insert_next_to(target_id, new_node, dir, new_split_id + 1000)),
                    id: *id,
                }
            }
        }
    }
}

/// split a rect into two parts
fn split_rect(rect: egui::Rect, dir: SplitDir, ratio: f32) -> (egui::Rect, egui::Rect) {
    match dir {
        SplitDir::Horizontal => {
            let mid = rect.left() + rect.width() * ratio;
            (
                egui::Rect::from_min_max(rect.min, egui::pos2(mid, rect.max.y)),
                egui::Rect::from_min_max(egui::pos2(mid, rect.min.y), rect.max),
            )
        }
        SplitDir::Vertical => {
            let mid = rect.top() + rect.height() * ratio;
            (
                egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, mid)),
                egui::Rect::from_min_max(egui::pos2(rect.min.x, mid), rect.max),
            )
        }
    }
}

/// direction for navigation
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// tiling window manager state
#[derive(Resource)]
pub struct TilingState {
    /// root of the binary tree (None = empty)
    pub root: Option<Node>,
    /// currently focused window id
    pub focused: Option<u64>,
    /// fullscreen window (overrides tiling)
    pub fullscreen: Option<u64>,
    /// next node id
    next_id: u64,
    /// gap between windows (pixels)
    pub gap: f32,
    /// border width
    pub border: f32,
    /// focused border color
    pub border_focused: egui::Color32,
    /// unfocused border color
    pub border_unfocused: egui::Color32,
}

impl Default for TilingState {
    fn default() -> Self {
        Self {
            root: None,
            focused: None,
            fullscreen: None,
            next_id: 1,
            gap: 4.0,
            border: 2.0,
            border_focused: egui::Color32::from_rgb(100, 150, 255),
            border_unfocused: egui::Color32::from_rgb(60, 60, 80),
        }
    }
}

impl TilingState {
    /// allocate next node id
    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// open a new window (adds to tree)
    pub fn open_window(&mut self, kind: WindowKind) {
        // check if already open
        if let Some(existing_id) = self.root.as_ref().and_then(|r| r.find_window(kind)) {
            self.focused = Some(existing_id);
            return;
        }

        let id = self.next_id();
        let new_node = Node::window(kind, id);

        if let Some(root) = self.root.take() {
            let focused_id = self.focused;
            let split_id = self.next_id();

            if let Some(fid) = focused_id {
                // insert next to focused
                self.root = Some(root.insert_next_to(fid, new_node, SplitDir::Horizontal, split_id));
            } else {
                // insert at end (split with last window)
                let windows = root.window_ids();
                if let Some(&last_id) = windows.last() {
                    self.root = Some(root.insert_next_to(last_id, new_node, SplitDir::Horizontal, split_id));
                } else {
                    self.root = Some(new_node);
                }
            }
        } else {
            // first window
            self.root = Some(new_node);
        }

        self.focused = Some(id);
    }

    /// close focused window
    pub fn close_focused(&mut self) {
        if let Some(focused_id) = self.focused {
            if let Some(ref root) = self.root {
                // find next window to focus
                let windows = root.window_ids();
                let next_focus = windows.iter()
                    .position(|&id| id == focused_id)
                    .and_then(|idx| {
                        if idx + 1 < windows.len() {
                            Some(windows[idx + 1])
                        } else if idx > 0 {
                            Some(windows[idx - 1])
                        } else {
                            None
                        }
                    });

                self.root = root.remove_window(focused_id);
                self.focused = next_focus;

                if self.fullscreen == Some(focused_id) {
                    self.fullscreen = None;
                }
            }
        }
    }

    /// close window by kind
    pub fn close_window(&mut self, kind: WindowKind) {
        if let Some(ref root) = self.root {
            if let Some(id) = root.find_window(kind) {
                let was_focused = self.focused == Some(id);
                self.root = root.remove_window(id);

                if was_focused {
                    // focus first remaining window
                    self.focused = self.root.as_ref().and_then(|r| r.window_ids().first().copied());
                }

                if self.fullscreen == Some(id) {
                    self.fullscreen = None;
                }
            }
        }
    }

    /// toggle fullscreen for focused window
    pub fn toggle_fullscreen(&mut self) {
        if let Some(focused) = self.focused {
            if self.fullscreen == Some(focused) {
                self.fullscreen = None;
            } else {
                self.fullscreen = Some(focused);
            }
        }
    }

    /// cycle focus to next window
    pub fn focus_next(&mut self) {
        if let Some(ref root) = self.root {
            let windows = root.window_ids();
            if windows.is_empty() {
                return;
            }

            let next = if let Some(focused) = self.focused {
                let idx = windows.iter().position(|&id| id == focused).unwrap_or(0);
                windows[(idx + 1) % windows.len()]
            } else {
                windows[0]
            };

            self.focused = Some(next);
        }
    }

    /// cycle focus to previous window
    pub fn focus_prev(&mut self) {
        if let Some(ref root) = self.root {
            let windows = root.window_ids();
            if windows.is_empty() {
                return;
            }

            let prev = if let Some(focused) = self.focused {
                let idx = windows.iter().position(|&id| id == focused).unwrap_or(0);
                windows[(idx + windows.len() - 1) % windows.len()]
            } else {
                windows[windows.len() - 1]
            };

            self.focused = Some(prev);
        }
    }

    /// focus window by index (1-based)
    pub fn focus_index(&mut self, index: usize) {
        if let Some(ref root) = self.root {
            let windows = root.window_ids();
            if index > 0 && index <= windows.len() {
                self.focused = Some(windows[index - 1]);
            }
        }
    }

    /// focus window by kind
    pub fn focus_kind(&mut self, kind: WindowKind) {
        if let Some(ref root) = self.root {
            if let Some(id) = root.find_window(kind) {
                self.focused = Some(id);
            }
        }
    }

    /// get rect for a window, accounting for fullscreen
    pub fn get_window_rect(&self, id: u64, screen: egui::Rect) -> Option<egui::Rect> {
        if self.fullscreen == Some(id) {
            return Some(screen);
        }

        if self.fullscreen.is_some() {
            // another window is fullscreen, hide this one
            return None;
        }

        self.root.as_ref()?.rect_for_window(id, screen)
    }

    /// get focused window kind
    pub fn focused_kind(&self) -> Option<WindowKind> {
        let id = self.focused?;
        self.root.as_ref()?.kind_for_id(id)
    }

    /// check if a kind is open
    pub fn is_open(&self, kind: WindowKind) -> bool {
        self.root.as_ref()
            .map(|r| r.find_window(kind).is_some())
            .unwrap_or(false)
    }

    /// get layout info for rendering
    pub fn layout(&self, screen: egui::Rect) -> Vec<WindowLayout> {
        let Some(ref root) = self.root else {
            return vec![];
        };

        // if fullscreen, only show that window
        if let Some(fs_id) = self.fullscreen {
            if let Some(kind) = root.kind_for_id(fs_id) {
                return vec![WindowLayout {
                    id: fs_id,
                    kind,
                    rect: screen,
                    focused: self.focused == Some(fs_id),
                }];
            }
        }

        // normal tiled layout
        self.layout_node(root, screen)
    }

    fn layout_node(&self, node: &Node, bounds: egui::Rect) -> Vec<WindowLayout> {
        match node {
            Node::Window { id, kind } => {
                vec![WindowLayout {
                    id: *id,
                    kind: *kind,
                    rect: bounds.shrink(self.gap / 2.0),
                    focused: self.focused == Some(*id),
                }]
            }
            Node::Split { dir, ratio, first, second, .. } => {
                let (first_bounds, second_bounds) = split_rect(bounds, *dir, *ratio);
                let mut layouts = self.layout_node(first, first_bounds);
                layouts.extend(self.layout_node(second, second_bounds));
                layouts
            }
        }
    }
}

/// layout info for a window
#[derive(Clone, Debug)]
pub struct WindowLayout {
    pub id: u64,
    pub kind: WindowKind,
    pub rect: egui::Rect,
    pub focused: bool,
}

/// handle tiling keyboard input
fn handle_tiling_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut tiling: ResMut<TilingState>,
) {
    let alt_held = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    if !alt_held {
        return;
    }

    // Alt+f = toggle fullscreen
    if keys.just_pressed(KeyCode::KeyF) {
        tiling.toggle_fullscreen();
        return;
    }

    // Alt+q = close focused
    if keys.just_pressed(KeyCode::KeyQ) && !shift_held {
        tiling.close_focused();
        return;
    }

    // Alt+Tab = cycle focus
    if keys.just_pressed(KeyCode::Tab) {
        if shift_held {
            tiling.focus_prev();
        } else {
            tiling.focus_next();
        }
        return;
    }

    // Alt+hjkl = focus direction (simplified: just cycle for now)
    if keys.just_pressed(KeyCode::KeyH) && !shift_held {
        tiling.focus_prev();
        return;
    }
    if keys.just_pressed(KeyCode::KeyL) && !shift_held {
        tiling.focus_next();
        return;
    }
    if keys.just_pressed(KeyCode::KeyJ) && !shift_held {
        tiling.focus_next();
        return;
    }
    if keys.just_pressed(KeyCode::KeyK) && !shift_held {
        tiling.focus_prev();
        return;
    }

    // Alt+1-9 = focus by index
    for (i, key) in [
        KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3,
        KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6,
        KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9,
    ].iter().enumerate() {
        if keys.just_pressed(*key) {
            tiling.focus_index(i + 1);
            return;
        }
    }
}

/// helper to render window frame
pub fn render_window_frame(
    painter: &egui::Painter,
    layout: &WindowLayout,
    tiling: &TilingState,
) {
    let border_color = if layout.focused {
        tiling.border_focused
    } else {
        tiling.border_unfocused
    };

    // draw border
    painter.rect_stroke(
        layout.rect,
        0.0,
        egui::Stroke::new(tiling.border, border_color),
    );

    // draw title bar
    let title_height = 20.0;
    let title_rect = egui::Rect::from_min_size(
        layout.rect.min,
        egui::vec2(layout.rect.width(), title_height),
    );

    painter.rect_filled(
        title_rect,
        0.0,
        if layout.focused {
            egui::Color32::from_rgb(40, 50, 70)
        } else {
            egui::Color32::from_rgb(30, 35, 45)
        },
    );

    painter.text(
        title_rect.center(),
        egui::Align2::CENTER_CENTER,
        layout.kind.title(),
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(180, 180, 190),
    );
}

/// get content rect (inside window frame)
pub fn content_rect(layout: &WindowLayout) -> egui::Rect {
    let title_height = 20.0;
    egui::Rect::from_min_max(
        egui::pos2(layout.rect.min.x, layout.rect.min.y + title_height),
        layout.rect.max,
    )
}
