//! blockout - 3d tetris training game
//!
//! teaches vim-style movement and poker hotkey grid
//!
//! controls (matching poker client):
//! - hjkl = move piece (vim style)
//! - q/e = rotate X axis (like fold/raise)
//! - w/r = rotate Y axis (like check/all-in)
//! - a/d = rotate Z axis
//! - s/space = soft/hard drop
//! - esc = pause/menu

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

pub struct BlockoutPlugin;

impl Plugin for BlockoutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BlockoutState>()
            .init_resource::<BlockoutSettings>()
            .init_resource::<Blockout3DState>()
            .add_systems(Startup, setup_blockout_3d)
            .add_systems(Update, (
                blockout_input,
                blockout_tick,
                sync_blockout_3d,
                toggle_blockout_3d_visibility,
                render_blockout_ui,
            ).run_if(|state: Res<BlockoutState>| state.active))
            .add_systems(Update, hide_blockout_3d.run_if(|state: Res<BlockoutState>| !state.active));
    }
}

/// marker for blockout 3d camera
#[derive(Component)]
pub struct Blockout3DCamera;

/// marker for blockout cube entities
#[derive(Component)]
pub struct BlockoutCube {
    pub grid_pos: IVec3,
    pub is_piece: bool,  // true = current piece, false = placed
}

/// marker for pit wireframe
#[derive(Component)]
pub struct BlockoutPit;

/// 3d rendering state
#[derive(Resource, Default)]
pub struct Blockout3DState {
    pub initialized: bool,
    pub cube_mesh: Option<Handle<Mesh>>,
    pub materials: Vec<Handle<StandardMaterial>>,  // layer colors
    pub shadow_material: Option<Handle<StandardMaterial>>,
    pub pit_material: Option<Handle<StandardMaterial>>,
}

/// 3d piece shape
#[derive(Clone, Debug)]
pub struct Piece3D {
    /// piece type for hold tracking
    pub piece_type: PieceType,
    /// voxels relative to center (x, y, z)
    pub voxels: Vec<IVec3>,
    /// current rotation
    pub rotation: IVec3,
    /// position in pit
    pub pos: IVec3,
    /// color
    pub color: egui::Color32,
}

impl Piece3D {
    /// rotate around X axis
    pub fn rotate_x(&mut self) {
        for v in &mut self.voxels {
            let (y, z) = (v.y, v.z);
            v.y = -z;
            v.z = y;
        }
        self.rotation.x = (self.rotation.x + 1) % 4;
    }

    /// rotate around Y axis
    pub fn rotate_y(&mut self) {
        for v in &mut self.voxels {
            let (x, z) = (v.x, v.z);
            v.x = -z;
            v.z = x;
        }
        self.rotation.y = (self.rotation.y + 1) % 4;
    }

    /// rotate around Z axis
    pub fn rotate_z(&mut self) {
        for v in &mut self.voxels {
            let (x, y) = (v.x, v.y);
            v.x = -y;
            v.y = x;
        }
        self.rotation.z = (self.rotation.z + 1) % 4;
    }

    /// get world positions of all voxels
    pub fn world_voxels(&self) -> Vec<IVec3> {
        self.voxels.iter().map(|v| *v + self.pos).collect()
    }
}

/// piece types (classic blockout)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PieceType {
    // flat pieces (basic set)
    #[default]
    Cube,       // 1x1x1
    Bar2,       // 2x1x1
    Bar3,       // 3x1x1
    Bar4,       // 4x1x1
    L2,         // L shape 2x2
    L3,         // L shape 3x2
    T,          // T shape
    S,          // S/Z shape
    // 3d pieces (extended set)
    Tower2,     // 1x1x2 vertical
    Tower3,     // 1x1x3 vertical
    Corner,     // 3d corner
    Step,       // 3d step
}

impl PieceType {
    pub fn voxels(&self) -> Vec<IVec3> {
        match self {
            PieceType::Cube => vec![IVec3::ZERO],
            PieceType::Bar2 => vec![IVec3::new(0, 0, 0), IVec3::new(1, 0, 0)],
            PieceType::Bar3 => vec![
                IVec3::new(-1, 0, 0), IVec3::new(0, 0, 0), IVec3::new(1, 0, 0)
            ],
            PieceType::Bar4 => vec![
                IVec3::new(-1, 0, 0), IVec3::new(0, 0, 0),
                IVec3::new(1, 0, 0), IVec3::new(2, 0, 0)
            ],
            PieceType::L2 => vec![
                IVec3::new(0, 0, 0), IVec3::new(1, 0, 0),
                IVec3::new(0, 1, 0),
            ],
            PieceType::L3 => vec![
                IVec3::new(0, 0, 0), IVec3::new(1, 0, 0), IVec3::new(2, 0, 0),
                IVec3::new(0, 1, 0),
            ],
            PieceType::T => vec![
                IVec3::new(-1, 0, 0), IVec3::new(0, 0, 0), IVec3::new(1, 0, 0),
                IVec3::new(0, 1, 0),
            ],
            PieceType::S => vec![
                IVec3::new(0, 0, 0), IVec3::new(1, 0, 0),
                IVec3::new(-1, 1, 0), IVec3::new(0, 1, 0),
            ],
            PieceType::Tower2 => vec![IVec3::new(0, 0, 0), IVec3::new(0, 0, 1)],
            PieceType::Tower3 => vec![
                IVec3::new(0, 0, 0), IVec3::new(0, 0, 1), IVec3::new(0, 0, 2)
            ],
            PieceType::Corner => vec![
                IVec3::new(0, 0, 0), IVec3::new(1, 0, 0),
                IVec3::new(0, 1, 0), IVec3::new(0, 0, 1),
            ],
            PieceType::Step => vec![
                IVec3::new(0, 0, 0), IVec3::new(1, 0, 0),
                IVec3::new(1, 0, 1),
            ],
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self {
            PieceType::Cube => egui::Color32::from_rgb(255, 255, 100),
            PieceType::Bar2 => egui::Color32::from_rgb(100, 200, 255),
            PieceType::Bar3 => egui::Color32::from_rgb(100, 255, 200),
            PieceType::Bar4 => egui::Color32::from_rgb(255, 100, 100),
            PieceType::L2 => egui::Color32::from_rgb(255, 180, 100),
            PieceType::L3 => egui::Color32::from_rgb(200, 100, 255),
            PieceType::T => egui::Color32::from_rgb(255, 100, 200),
            PieceType::S => egui::Color32::from_rgb(100, 255, 100),
            PieceType::Tower2 => egui::Color32::from_rgb(200, 200, 255),
            PieceType::Tower3 => egui::Color32::from_rgb(255, 200, 200),
            PieceType::Corner => egui::Color32::from_rgb(200, 255, 200),
            PieceType::Step => egui::Color32::from_rgb(255, 255, 200),
        }
    }

    pub fn random() -> Self {
        use rand::Rng;
        let types = [
            PieceType::Cube, PieceType::Bar2, PieceType::Bar3, PieceType::Bar4,
            PieceType::L2, PieceType::L3, PieceType::T, PieceType::S,
            PieceType::Tower2, PieceType::Tower3, PieceType::Corner, PieceType::Step,
        ];
        types[rand::thread_rng().gen_range(0..types.len())]
    }
}

/// game settings
#[derive(Resource)]
pub struct BlockoutSettings {
    /// pit dimensions (width, depth, height)
    pub pit_size: IVec3,
    /// starting drop speed (seconds per row)
    pub base_speed: f32,
    /// speed increase per level
    pub speed_multiplier: f32,
    /// lines per level
    pub lines_per_level: u32,
    /// piece set
    pub piece_set: PieceSet,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PieceSet {
    Flat,       // 2d pieces only
    #[default]
    Basic,      // basic 3d pieces
    Extended,   // all pieces
}

impl Default for BlockoutSettings {
    fn default() -> Self {
        Self {
            pit_size: IVec3::new(5, 5, 12),
            base_speed: 1.0,
            speed_multiplier: 0.9,
            lines_per_level: 5,
            piece_set: PieceSet::Basic,
        }
    }
}

/// game state
#[derive(Resource, Default)]
pub struct BlockoutState {
    /// game active
    pub active: bool,
    /// paused
    pub paused: bool,
    /// game over
    pub game_over: bool,
    /// current piece
    pub current_piece: Option<Piece3D>,
    /// next piece type
    pub next_piece: Option<PieceType>,
    /// hold piece (C to swap)
    pub hold_piece: Option<PieceType>,
    /// already used hold this turn
    pub hold_used: bool,
    /// piece bag for fair randomization
    pub piece_bag: Vec<PieceType>,
    /// filled voxels in pit (x, y, z) -> color
    pub pit: Vec<(IVec3, egui::Color32)>,
    /// score
    pub score: u64,
    /// level
    pub level: u32,
    /// lines cleared
    pub lines: u32,
    /// drop timer
    pub drop_timer: f32,
    /// current drop speed
    pub drop_speed: f32,
    /// key hints visible
    pub show_hints: bool,
    /// stats for training
    pub stats: BlockoutStats,
    /// layer clear animation timer
    pub clear_animation: f32,
    /// layers being cleared (for animation)
    pub clearing_layers: Vec<i32>,
    /// combo counter
    pub combo: u32,
}

/// training stats
#[derive(Clone, Debug, Default)]
pub struct BlockoutStats {
    pub pieces_placed: u32,
    pub rotations: u32,
    pub moves: u32,
    pub layers_cleared: u32,
    /// key usage counts for training feedback
    pub key_usage: KeyUsage,
}

#[derive(Clone, Debug, Default)]
pub struct KeyUsage {
    pub h: u32, pub j: u32, pub k: u32, pub l: u32,
    pub q: u32, pub w: u32, pub e: u32, pub r: u32,
    pub a: u32, pub d: u32, pub s: u32, pub space: u32,
}

impl BlockoutState {
    pub fn new_game(&mut self, settings: &BlockoutSettings) {
        self.active = true;
        self.paused = false;
        self.game_over = false;
        self.pit.clear();
        self.score = 0;
        self.level = 1;
        self.lines = 0;
        self.drop_speed = settings.base_speed;
        self.drop_timer = 0.0;
        self.show_hints = true;
        self.stats = BlockoutStats::default();
        self.hold_piece = None;
        self.hold_used = false;
        self.clear_animation = 0.0;
        self.clearing_layers.clear();
        self.combo = 0;

        // initialize piece bag
        self.piece_bag.clear();
        self.refill_bag(settings);
        self.next_piece = Some(self.draw_from_bag(settings));
        self.spawn_piece(settings);
    }

    /// refill the piece bag with shuffled pieces
    fn refill_bag(&mut self, settings: &BlockoutSettings) {
        use rand::seq::SliceRandom;

        let pieces: Vec<PieceType> = match settings.piece_set {
            PieceSet::Flat => vec![
                PieceType::Cube, PieceType::Bar2, PieceType::Bar3, PieceType::Bar4,
                PieceType::L2, PieceType::L3, PieceType::T, PieceType::S,
            ],
            PieceSet::Basic => vec![
                PieceType::Cube, PieceType::Bar2, PieceType::Bar3,
                PieceType::L2, PieceType::T, PieceType::S,
                PieceType::Tower2, PieceType::Corner, PieceType::Step,
            ],
            PieceSet::Extended => vec![
                PieceType::Cube, PieceType::Bar2, PieceType::Bar3, PieceType::Bar4,
                PieceType::L2, PieceType::L3, PieceType::T, PieceType::S,
                PieceType::Tower2, PieceType::Tower3, PieceType::Corner, PieceType::Step,
            ],
        };

        // add all pieces to bag, then shuffle
        self.piece_bag.extend(pieces.clone());
        self.piece_bag.shuffle(&mut rand::thread_rng());
    }

    /// draw next piece from bag
    fn draw_from_bag(&mut self, settings: &BlockoutSettings) -> PieceType {
        if self.piece_bag.is_empty() {
            self.refill_bag(settings);
        }
        self.piece_bag.pop().unwrap_or(PieceType::Cube)
    }

    /// hold current piece and get held piece (or next from bag)
    pub fn hold(&mut self, settings: &BlockoutSettings) {
        if self.hold_used {
            return; // can only hold once per piece
        }

        if let Some(piece) = self.current_piece.take() {
            let current_type = piece.piece_type;

            if let Some(held) = self.hold_piece.take() {
                // swap with held piece
                self.hold_piece = Some(current_type);
                self.spawn_piece_of_type(held, settings);
            } else {
                // first hold - put current in hold, spawn next
                self.hold_piece = Some(current_type);
                self.spawn_piece(settings);
            }
            self.hold_used = true;
        }
    }

    fn spawn_piece(&mut self, settings: &BlockoutSettings) {
        let piece_type = self.next_piece.unwrap_or_else(|| self.draw_from_bag(settings));
        self.next_piece = Some(self.draw_from_bag(settings));
        self.spawn_piece_of_type(piece_type, settings);
    }

    fn spawn_piece_of_type(&mut self, piece_type: PieceType, settings: &BlockoutSettings) {
        let start_pos = IVec3::new(
            settings.pit_size.x / 2,
            settings.pit_size.y / 2,
            settings.pit_size.z - 2,
        );

        self.current_piece = Some(Piece3D {
            piece_type,
            voxels: piece_type.voxels(),
            rotation: IVec3::ZERO,
            pos: start_pos,
            color: piece_type.color(),
        });

        self.hold_used = false; // reset hold for new piece

        // check game over
        let voxels = self.current_piece.as_ref().unwrap().world_voxels();
        if Self::check_collision_with_size(&voxels, &self.pit, settings.pit_size) {
            self.game_over = true;
        }
    }

    fn collides(&self, piece: &Piece3D) -> bool {
        Self::check_collision_with_size(&piece.world_voxels(), &self.pit, IVec3::new(5, 5, 12))
    }

    /// static collision check with configurable pit size
    fn check_collision_with_size(voxels: &[IVec3], pit: &[(IVec3, egui::Color32)], pit_size: IVec3) -> bool {
        for v in voxels {
            // bounds check
            if v.x < 0 || v.x >= pit_size.x ||
               v.y < 0 || v.y >= pit_size.y ||
               v.z < 0 {
                return true;
            }
            // collision with placed voxels
            if pit.iter().any(|(p, _)| *p == *v) {
                return true;
            }
        }
        false
    }

    /// wall kick offsets to try when rotation is blocked
    const WALL_KICKS: [IVec3; 9] = [
        IVec3::new(0, 0, 0),   // original
        IVec3::new(1, 0, 0),   // right
        IVec3::new(-1, 0, 0),  // left
        IVec3::new(0, 1, 0),   // forward
        IVec3::new(0, -1, 0),  // back
        IVec3::new(0, 0, 1),   // up
        IVec3::new(1, 1, 0),   // diagonal
        IVec3::new(-1, -1, 0), // diagonal
        IVec3::new(0, 0, -1),  // down (last resort)
    ];

    fn try_move(&mut self, delta: IVec3, pit_size: IVec3) -> bool {
        let Some(ref mut piece) = self.current_piece else { return false };
        let old_pos = piece.pos;
        piece.pos += delta;
        let new_voxels = piece.world_voxels();
        if Self::check_collision_with_size(&new_voxels, &self.pit, pit_size) {
            self.current_piece.as_mut().unwrap().pos = old_pos;
            return false;
        }
        true
    }

    /// try rotation with wall kicks
    fn try_rotate_x_with_kick(&mut self, pit_size: IVec3) -> bool {
        let Some(ref mut piece) = self.current_piece else { return false };
        let old_voxels = piece.voxels.clone();
        let old_rotation = piece.rotation;
        let old_pos = piece.pos;

        piece.rotate_x();

        // try wall kicks
        for kick in Self::WALL_KICKS {
            piece.pos = old_pos + kick;
            let voxels = piece.world_voxels();
            if !Self::check_collision_with_size(&voxels, &self.pit, pit_size) {
                self.stats.rotations += 1;
                return true;
            }
        }

        // all kicks failed, revert
        let piece = self.current_piece.as_mut().unwrap();
        piece.voxels = old_voxels;
        piece.rotation = old_rotation;
        piece.pos = old_pos;
        false
    }

    fn try_rotate_y_with_kick(&mut self, pit_size: IVec3) -> bool {
        let Some(ref mut piece) = self.current_piece else { return false };
        let old_voxels = piece.voxels.clone();
        let old_rotation = piece.rotation;
        let old_pos = piece.pos;

        piece.rotate_y();

        for kick in Self::WALL_KICKS {
            piece.pos = old_pos + kick;
            let voxels = piece.world_voxels();
            if !Self::check_collision_with_size(&voxels, &self.pit, pit_size) {
                self.stats.rotations += 1;
                return true;
            }
        }

        let piece = self.current_piece.as_mut().unwrap();
        piece.voxels = old_voxels;
        piece.rotation = old_rotation;
        piece.pos = old_pos;
        false
    }

    fn try_rotate_z_with_kick(&mut self, pit_size: IVec3) -> bool {
        let Some(ref mut piece) = self.current_piece else { return false };
        let old_voxels = piece.voxels.clone();
        let old_rotation = piece.rotation;
        let old_pos = piece.pos;

        piece.rotate_z();

        for kick in Self::WALL_KICKS {
            piece.pos = old_pos + kick;
            let voxels = piece.world_voxels();
            if !Self::check_collision_with_size(&voxels, &self.pit, pit_size) {
                self.stats.rotations += 1;
                return true;
            }
        }

        let piece = self.current_piece.as_mut().unwrap();
        piece.voxels = old_voxels;
        piece.rotation = old_rotation;
        piece.pos = old_pos;
        false
    }

    fn lock_piece(&mut self, settings: &BlockoutSettings) {
        if let Some(piece) = self.current_piece.take() {
            for v in piece.world_voxels() {
                self.pit.push((v, piece.color));
            }
            self.stats.pieces_placed += 1;

            let layers_before = self.lines;
            self.clear_layers(settings);
            let layers_cleared = self.lines - layers_before;

            // combo scoring
            if layers_cleared > 0 {
                self.combo += 1;
                // bonus for multiple layers
                if layers_cleared > 1 {
                    self.score += 50 * layers_cleared as u64 * self.combo as u64;
                }
                // combo bonus
                if self.combo > 1 {
                    self.score += 25 * self.combo as u64;
                }
            } else {
                self.combo = 0;
            }

            self.spawn_piece(settings);
        }
    }

    fn clear_layers(&mut self, settings: &BlockoutSettings) {
        let mut z = 0;
        let layer_size = (settings.pit_size.x * settings.pit_size.y) as usize;

        while z < settings.pit_size.z {
            let count = self.pit.iter().filter(|(v, _)| v.z == z).count();
            if count == layer_size {
                // clear this layer
                self.clearing_layers.push(z);
                self.pit.retain(|(v, _)| v.z != z);
                // move everything above down
                for (v, _) in &mut self.pit {
                    if v.z > z {
                        v.z -= 1;
                    }
                }
                self.lines += 1;
                self.stats.layers_cleared += 1;
                self.score += 100 * self.level as u64;

                // level up
                if self.lines >= settings.lines_per_level * self.level {
                    self.level += 1;
                    self.drop_speed *= settings.speed_multiplier;
                }
            } else {
                z += 1;
            }
        }
    }

    fn hard_drop(&mut self, settings: &BlockoutSettings) {
        while self.try_move(IVec3::new(0, 0, -1), settings.pit_size) {}
        self.lock_piece(settings);
    }
}

/// handle input
fn blockout_input(
    mut state: ResMut<BlockoutState>,
    settings: Res<BlockoutSettings>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if !state.active || state.game_over {
        // start new game on space
        if keys.just_pressed(KeyCode::Space) {
            state.new_game(&settings);
        }
        return;
    }

    // pause
    if keys.just_pressed(KeyCode::Escape) {
        state.paused = !state.paused;
        return;
    }

    if state.paused {
        return;
    }

    let pit_size = settings.pit_size;

    // === VIM MOVEMENT (hjkl) ===
    if keys.just_pressed(KeyCode::KeyH) {
        if state.try_move(IVec3::new(-1, 0, 0), pit_size) {
            state.stats.moves += 1;
            state.stats.key_usage.h += 1;
        }
    }
    if keys.just_pressed(KeyCode::KeyL) {
        if state.try_move(IVec3::new(1, 0, 0), pit_size) {
            state.stats.moves += 1;
            state.stats.key_usage.l += 1;
        }
    }
    if keys.just_pressed(KeyCode::KeyK) {
        if state.try_move(IVec3::new(0, -1, 0), pit_size) {
            state.stats.moves += 1;
            state.stats.key_usage.k += 1;
        }
    }
    if keys.just_pressed(KeyCode::KeyJ) {
        if state.try_move(IVec3::new(0, 1, 0), pit_size) {
            state.stats.moves += 1;
            state.stats.key_usage.j += 1;
        }
    }

    // === POKER GRID ROTATIONS (qwer / ad) with wall kicks ===
    // Q/E = rotate X (left row in poker = fold/raise equivalent)
    if keys.just_pressed(KeyCode::KeyQ) {
        state.try_rotate_x_with_kick(pit_size);
        state.stats.key_usage.q += 1;
    }
    if keys.just_pressed(KeyCode::KeyE) {
        // rotate X reverse (3 forward = 1 back)
        state.try_rotate_x_with_kick(pit_size);
        state.try_rotate_x_with_kick(pit_size);
        state.try_rotate_x_with_kick(pit_size);
        state.stats.key_usage.e += 1;
    }

    // W/R = rotate Y
    if keys.just_pressed(KeyCode::KeyW) {
        state.try_rotate_y_with_kick(pit_size);
        state.stats.key_usage.w += 1;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        state.try_rotate_y_with_kick(pit_size);
        state.try_rotate_y_with_kick(pit_size);
        state.try_rotate_y_with_kick(pit_size);
        state.stats.key_usage.r += 1;
    }

    // A/D = rotate Z
    if keys.just_pressed(KeyCode::KeyA) {
        state.try_rotate_z_with_kick(pit_size);
        state.stats.key_usage.a += 1;
    }
    if keys.just_pressed(KeyCode::KeyD) {
        state.try_rotate_z_with_kick(pit_size);
        state.try_rotate_z_with_kick(pit_size);
        state.try_rotate_z_with_kick(pit_size);
        state.stats.key_usage.d += 1;
    }

    // === DROP ===
    // S = soft drop (one step)
    if keys.just_pressed(KeyCode::KeyS) {
        if !state.try_move(IVec3::new(0, 0, -1), pit_size) {
            state.lock_piece(&settings);
        }
        state.stats.key_usage.s += 1;
    }

    // Space = hard drop
    if keys.just_pressed(KeyCode::Space) {
        state.hard_drop(&settings);
        state.stats.key_usage.space += 1;
    }

    // C = hold piece
    if keys.just_pressed(KeyCode::KeyC) {
        state.hold(&settings);
    }

    // F1 = toggle hints
    if keys.just_pressed(KeyCode::F1) {
        state.show_hints = !state.show_hints;
    }
}

/// gravity tick
fn blockout_tick(
    mut state: ResMut<BlockoutState>,
    settings: Res<BlockoutSettings>,
    time: Res<Time>,
) {
    if !state.active || state.paused || state.game_over {
        return;
    }

    state.drop_timer += time.delta_seconds();

    if state.drop_timer >= state.drop_speed {
        state.drop_timer = 0.0;
        if !state.try_move(IVec3::new(0, 0, -1), settings.pit_size) {
            state.lock_piece(&settings);
        }
    }
}

/// setup 3d camera and lighting for blockout
fn setup_blockout_3d(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut state_3d: ResMut<Blockout3DState>,
    settings: Res<BlockoutSettings>,
) {
    // cube mesh for blocks
    let cube = meshes.add(Cuboid::new(0.9, 0.9, 0.9));
    state_3d.cube_mesh = Some(cube);

    // layer-based materials (rainbow gradient)
    let pit_h = settings.pit_size.z as f32;
    for z in 0..=settings.pit_size.z {
        let ratio = z as f32 / pit_h;
        let hue = ratio * 300.0;
        let color = hue_to_color(hue);
        let mat = materials.add(StandardMaterial {
            base_color: color,
            emissive: LinearRgba::from(color) * 0.2,
            ..default()
        });
        state_3d.materials.push(mat);
    }

    // shadow material (transparent)
    state_3d.shadow_material = Some(materials.add(StandardMaterial {
        base_color: Color::srgba(0.5, 0.5, 0.5, 0.3),
        alpha_mode: AlphaMode::Blend,
        ..default()
    }));

    // pit material (green wireframe-like)
    state_3d.pit_material = Some(materials.add(StandardMaterial {
        base_color: Color::srgb(0.0, 0.3, 0.0),
        emissive: LinearRgba::rgb(0.0, 0.5, 0.0),
        ..default()
    }));

    // camera looking down into the pit
    let pit_center = Vec3::new(
        settings.pit_size.x as f32 / 2.0,
        settings.pit_size.y as f32 / 2.0,
        settings.pit_size.z as f32 / 2.0,
    );

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(pit_center.x, pit_center.y - 15.0, pit_center.z + 20.0)
            .looking_at(pit_center, Vec3::Z),
        Blockout3DCamera,
        Visibility::Hidden,
    ));

    // directional light
    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(5.0, -10.0, 15.0).looking_at(pit_center, Vec3::Z),
        Blockout3DCamera,  // reuse marker for cleanup
        Visibility::Hidden,
    ));

    // ambient light
    commands.insert_resource(AmbientLight {
        color: Color::srgb(0.0, 0.3, 0.0),  // green tint
        brightness: 200.0,
    });

    // spawn pit walls (floor)
    let floor_mesh = meshes.add(Cuboid::new(
        settings.pit_size.x as f32,
        settings.pit_size.y as f32,
        0.1,
    ));
    commands.spawn((
        PbrBundle {
            mesh: floor_mesh,
            material: state_3d.pit_material.clone().unwrap(),
            transform: Transform::from_xyz(
                settings.pit_size.x as f32 / 2.0,
                settings.pit_size.y as f32 / 2.0,
                -0.05,
            ),
            visibility: Visibility::Hidden,
            ..default()
        },
        BlockoutPit,
    ));

    state_3d.initialized = true;
}

/// convert hue to bevy color
fn hue_to_color(hue: f32) -> Color {
    let h = hue / 60.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let q = 1.0 - f;

    let (r, g, b) = match i % 6 {
        0 => (1.0, f, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, q, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };

    Color::srgb(r * 0.9 + 0.1, g * 0.9 + 0.1, b * 0.9 + 0.1)
}

/// sync 3d entities with game state
fn sync_blockout_3d(
    mut commands: Commands,
    state: Res<BlockoutState>,
    settings: Res<BlockoutSettings>,
    state_3d: Res<Blockout3DState>,
    cubes: Query<Entity, With<BlockoutCube>>,
) {
    if !state_3d.initialized || !state.active {
        return;
    }

    // clear existing cubes
    for entity in cubes.iter() {
        commands.entity(entity).despawn();
    }

    let cube_mesh = match &state_3d.cube_mesh {
        Some(m) => m.clone(),
        None => return,
    };

    // spawn placed blocks
    for (v, _) in &state.pit {
        let z_idx = (v.z as usize).min(state_3d.materials.len() - 1);
        if let Some(mat) = state_3d.materials.get(z_idx) {
            commands.spawn((
                PbrBundle {
                    mesh: cube_mesh.clone(),
                    material: mat.clone(),
                    transform: Transform::from_xyz(v.x as f32 + 0.5, v.y as f32 + 0.5, v.z as f32 + 0.5),
                    ..default()
                },
                BlockoutCube { grid_pos: *v, is_piece: false },
            ));
        }
    }

    // spawn current piece
    if let Some(ref piece) = state.current_piece {
        for v in piece.world_voxels() {
            let z_idx = (v.z as usize).min(state_3d.materials.len() - 1);
            if let Some(mat) = state_3d.materials.get(z_idx) {
                commands.spawn((
                    PbrBundle {
                        mesh: cube_mesh.clone(),
                        material: mat.clone(),
                        transform: Transform::from_xyz(v.x as f32 + 0.5, v.y as f32 + 0.5, v.z as f32 + 0.5),
                        ..default()
                    },
                    BlockoutCube { grid_pos: v, is_piece: true },
                ));
            }
        }

        // spawn shadow at landing position
        if let Some(shadow_mat) = &state_3d.shadow_material {
            let mut shadow_z = piece.pos.z;
            loop {
                let test_z = shadow_z - 1;
                let collides = piece.voxels.iter().any(|pv| {
                    let world = *pv + IVec3::new(piece.pos.x, piece.pos.y, test_z);
                    world.z < 0 || state.pit.iter().any(|(p, _)| *p == world)
                });
                if collides { break; }
                shadow_z = test_z;
            }

            if shadow_z != piece.pos.z {
                for v in &piece.voxels {
                    let world = *v + IVec3::new(piece.pos.x, piece.pos.y, shadow_z);
                    commands.spawn((
                        PbrBundle {
                            mesh: cube_mesh.clone(),
                            material: shadow_mat.clone(),
                            transform: Transform::from_xyz(world.x as f32 + 0.5, world.y as f32 + 0.5, world.z as f32 + 0.5),
                            ..default()
                        },
                        BlockoutCube { grid_pos: world, is_piece: true },
                    ));
                }
            }
        }
    }
}

/// show 3d scene when blockout active
fn toggle_blockout_3d_visibility(
    mut cameras: Query<&mut Visibility, With<Blockout3DCamera>>,
    mut pit: Query<&mut Visibility, (With<BlockoutPit>, Without<Blockout3DCamera>)>,
) {
    for mut vis in cameras.iter_mut() {
        *vis = Visibility::Visible;
    }
    for mut vis in pit.iter_mut() {
        *vis = Visibility::Visible;
    }
}

/// hide 3d scene when blockout inactive
fn hide_blockout_3d(
    mut commands: Commands,
    mut cameras: Query<&mut Visibility, With<Blockout3DCamera>>,
    mut pit: Query<&mut Visibility, (With<BlockoutPit>, Without<Blockout3DCamera>)>,
    cubes: Query<Entity, With<BlockoutCube>>,
) {
    for mut vis in cameras.iter_mut() {
        *vis = Visibility::Hidden;
    }
    for mut vis in pit.iter_mut() {
        *vis = Visibility::Hidden;
    }
    // despawn cubes
    for entity in cubes.iter() {
        commands.entity(entity).despawn();
    }
}

/// render UI overlay (stats, tutorial)
fn render_blockout_ui(
    state: Res<BlockoutState>,
    settings: Res<BlockoutSettings>,
    mut contexts: EguiContexts,
) {
    let ctx = contexts.ctx_mut();
    let screen = ctx.screen_rect();

    // stats bar at top
    let stats_height = 50.0;
    let stats_rect = egui::Rect::from_min_size(screen.min, egui::vec2(screen.width(), stats_height));

    egui::Area::new(egui::Id::new("blockout_stats_3d"))
        .fixed_pos(stats_rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(stats_rect);

            let painter = ui.painter();
            painter.rect_filled(stats_rect, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180));

            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(egui::RichText::new("BLOCKOUT 3D")
                    .color(egui::Color32::from_rgb(0, 255, 0))
                    .size(16.0)
                    .strong());
                ui.add_space(20.0);

                let green = egui::Color32::from_rgb(0, 200, 0);
                ui.colored_label(green, format!("SCORE: {}", state.score));
                ui.add_space(15.0);
                ui.colored_label(green, format!("LEVEL: {}", state.level));
                ui.add_space(15.0);
                ui.colored_label(green, format!("LINES: {}", state.lines));

                if state.combo > 1 {
                    ui.add_space(15.0);
                    ui.colored_label(egui::Color32::from_rgb(255, 200, 50), format!("COMBO x{}", state.combo));
                }

                if state.game_over {
                    ui.add_space(20.0);
                    ui.colored_label(egui::Color32::RED, "GAME OVER - SPACE");
                }
                if state.paused {
                    ui.add_space(20.0);
                    ui.colored_label(egui::Color32::YELLOW, "PAUSED - ESC");
                }
            });
        });

    // key tutorial bar at bottom
    let tutorial_height = 60.0;
    let tutorial_rect = egui::Rect::from_min_max(
        egui::pos2(screen.min.x, screen.max.y - tutorial_height),
        screen.max,
    );

    egui::Area::new(egui::Id::new("blockout_tutorial_3d"))
        .fixed_pos(tutorial_rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(tutorial_rect);

            let painter = ui.painter();
            painter.rect_filled(tutorial_rect, 0.0, egui::Color32::from_rgba_unmultiplied(0, 20, 0, 200));

            let green = egui::Color32::from_rgb(0, 180, 0);
            let dim = egui::Color32::from_rgb(0, 100, 0);

            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.colored_label(green, "POKER HOTKEYS:");
                ui.add_space(10.0);
                ui.colored_label(dim, "H J K L");
                ui.colored_label(green, "move");
                ui.add_space(10.0);
                ui.colored_label(dim, "Q W E R");
                ui.colored_label(green, "rotate");
                ui.add_space(10.0);
                ui.colored_label(dim, "SPACE");
                ui.colored_label(green, "drop");
                ui.add_space(10.0);
                ui.colored_label(dim, "C");
                ui.colored_label(green, "hold");
                ui.add_space(10.0);
                ui.colored_label(dim, "F2");
                ui.colored_label(green, "exit");
            });
        });
}

/// render blockout content into a rect
fn render_blockout_content(
    ctx: &egui::Context,
    state: &BlockoutState,
    settings: &BlockoutSettings,
    rect: egui::Rect,
) {
    let tutorial_height = 80.0;
    let stats_height = 60.0;

    // main pit area (between stats and tutorial)
    let pit_rect = egui::Rect::from_min_max(
        egui::pos2(rect.min.x, rect.min.y + stats_height),
        egui::pos2(rect.max.x, rect.max.y - tutorial_height),
    );

    egui::Area::new(egui::Id::new("blockout_content"))
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(rect);
            ui.set_min_size(rect.size());

            // black background
            let painter = ui.painter();
            painter.rect_filled(rect, 0.0, egui::Color32::BLACK);

            // render the 3d pit
            render_pit_in_rect(ui, state, settings, pit_rect);
        });

    // stats bar at top (overlay)
    let stats_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), stats_height));
    egui::Area::new(egui::Id::new("blockout_stats"))
        .fixed_pos(stats_rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(stats_rect);

            let painter = ui.painter();
            painter.rect_filled(stats_rect, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200));

            ui.horizontal(|ui| {
                ui.add_space(10.0);

                // title
                ui.label(egui::RichText::new("BLOCKOUT")
                    .color(egui::Color32::from_rgb(0, 255, 0))
                    .size(18.0)
                    .strong());

                ui.add_space(20.0);

                // stats in green
                let green = egui::Color32::from_rgb(0, 200, 0);
                ui.colored_label(green, format!("SCORE: {}", state.score));
                ui.add_space(15.0);
                ui.colored_label(green, format!("LEVEL: {}", state.level));
                ui.add_space(15.0);
                ui.colored_label(green, format!("LINES: {}", state.lines));

                if state.combo > 1 {
                    ui.add_space(15.0);
                    ui.colored_label(egui::Color32::from_rgb(255, 200, 50), format!("COMBO x{}", state.combo));
                }

                // previews on right side
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(10.0);
                    if let Some(hold) = state.hold_piece {
                        render_preview_mini(ui, hold);
                        ui.small(if state.hold_used { "HOLD*" } else { "HOLD" });
                    }
                    ui.add_space(10.0);
                    if let Some(next) = state.next_piece {
                        render_preview_mini(ui, next);
                        ui.small("NEXT");
                    }
                });
            });

            if state.game_over {
                ui.colored_label(egui::Color32::RED, "GAME OVER - SPACE to restart");
            }
            if state.paused {
                ui.colored_label(egui::Color32::YELLOW, "PAUSED - ESC to resume");
            }
        });

    // key tutorial bar at bottom
    let tutorial_rect = egui::Rect::from_min_max(
        egui::pos2(rect.min.x, rect.max.y - tutorial_height),
        rect.max,
    );
    render_key_tutorial(ctx, tutorial_rect);
}

/// render key tutorial bar - teaches poker hotkeys through blockout
fn render_key_tutorial(ctx: &egui::Context, rect: egui::Rect) {
    egui::Area::new(egui::Id::new("blockout_tutorial"))
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(rect);

            let painter = ui.painter();
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(10, 15, 10));
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 80, 0)));

            let green = egui::Color32::from_rgb(0, 180, 0);
            let dim = egui::Color32::from_rgb(0, 100, 0);

            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.colored_label(green, "POKER HOTKEY TRAINER");
            });

            ui.horizontal(|ui| {
                ui.add_space(10.0);

                // movement keys (vim style)
                ui.vertical(|ui| {
                    ui.colored_label(dim, "MOVEMENT");
                    ui.horizontal(|ui| {
                        key_box(ui, "H", "left");
                        key_box(ui, "J", "fwd");
                        key_box(ui, "K", "back");
                        key_box(ui, "L", "right");
                    });
                });

                ui.add_space(20.0);

                // poker action keys (QWER like SC2/poker)
                ui.vertical(|ui| {
                    ui.colored_label(dim, "POKER ACTIONS");
                    ui.horizontal(|ui| {
                        key_box(ui, "Q", "fold/rotX");
                        key_box(ui, "W", "check/rotY");
                        key_box(ui, "E", "raise/rotX'");
                        key_box(ui, "R", "allin/rotY'");
                    });
                });

                ui.add_space(20.0);

                // other keys
                ui.vertical(|ui| {
                    ui.colored_label(dim, "SPECIAL");
                    ui.horizontal(|ui| {
                        key_box(ui, "A", "rotZ");
                        key_box(ui, "S", "drop");
                        key_box(ui, "D", "rotZ'");
                        key_box(ui, "C", "hold");
                        key_box(ui, "SPC", "hard drop");
                    });
                });
            });
        });
}

/// render a key box for tutorial
fn key_box(ui: &mut egui::Ui, key: &str, action: &str) {
    let green = egui::Color32::from_rgb(0, 200, 0);

    ui.group(|ui| {
        ui.set_min_width(50.0);
        ui.vertical(|ui| {
            ui.colored_label(green, egui::RichText::new(key).strong().size(14.0));
            ui.colored_label(egui::Color32::from_rgb(0, 120, 0), egui::RichText::new(action).size(9.0));
        });
    });
}

/// mini preview for stats bar
fn render_preview_mini(ui: &mut egui::Ui, piece_type: PieceType) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(40.0, 40.0),
        egui::Sense::hover(),
    );

    let center = response.rect.center();
    let scale = 8.0;
    let green = egui::Color32::from_rgb(0, 200, 0);

    for v in piece_type.voxels() {
        let pos = egui::pos2(
            center.x + v.x as f32 * scale,
            center.y + v.y as f32 * scale - v.z as f32 * scale * 0.3,
        );
        let rect = egui::Rect::from_center_size(pos, egui::vec2(scale * 0.8, scale * 0.6));
        painter.rect_filled(rect, 1.0, green);
    }
}

/// get layer-based color (rainbow gradient based on z level)
fn layer_color(z: i32, max_z: i32) -> egui::Color32 {
    let ratio = z as f32 / max_z as f32;
    // rainbow: red -> orange -> yellow -> green -> cyan -> blue -> purple
    let hue = ratio * 300.0;  // 0-300 degrees (red to purple)

    // hsv to rgb
    let h = hue / 60.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let q = 1.0 - f;

    let (r, g, b) = match i % 6 {
        0 => (1.0, f, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, q, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };

    egui::Color32::from_rgb(
        (r * 220.0) as u8 + 35,
        (g * 220.0) as u8 + 35,
        (b * 220.0) as u8 + 35,
    )
}

/// render pit - classic blockout trapezoid perspective
/// looking down into pit: big entrance at top, small floor at bottom
fn render_pit_in_rect(ui: &mut egui::Ui, state: &BlockoutState, settings: &BlockoutSettings, bounds: egui::Rect) {
    let (response, painter) = ui.allocate_painter(bounds.size(), egui::Sense::hover());
    let rect = response.rect;

    // black background
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);

    let center_x = rect.center().x;
    let pit_w = settings.pit_size.x as f32;
    let pit_d = settings.pit_size.y as f32;  // depth (y in game = into screen)
    let pit_h = settings.pit_size.z as f32;  // height (z in game = layers)

    // classic blockout perspective: trapezoid
    // entrance (z=pit_h) is at TOP of screen and WIDE
    // floor (z=0) is at BOTTOM of screen and NARROW
    let margin = 20.0;
    let top_y = rect.top() + margin;           // entrance at screen top
    let bottom_y = rect.bottom() - margin;     // floor at screen bottom

    // entrance is wide, floor is narrow (dramatic perspective)
    let entrance_half_w = (rect.width() - margin * 2.0) / 2.0;
    let floor_half_w = entrance_half_w * 0.25;  // floor is 25% of entrance width

    // cell sizes scale with depth
    let entrance_cell = entrance_half_w * 2.0 / pit_w;
    let floor_cell = floor_half_w * 2.0 / pit_w;

    // project: z=pit_h (entrance, top layer) -> top of screen, large
    //          z=0 (floor) -> bottom of screen, small
    let project = |x: f32, y: f32, z: f32| -> egui::Pos2 {
        // z_ratio: 0 at floor, 1 at entrance
        let z_ratio = z / pit_h;

        // interpolate between floor and entrance
        let half_w = floor_half_w + (entrance_half_w - floor_half_w) * z_ratio;
        let screen_y = bottom_y + (top_y - bottom_y) * z_ratio;

        // x position: centered, scaled by current width
        let cell = floor_cell + (entrance_cell - floor_cell) * z_ratio;
        let px = (x - pit_w / 2.0 + 0.5) * cell;

        // y (depth) shifts position slightly (pseudo-3d depth offset)
        let depth_offset = (y - pit_d / 2.0) * cell * 0.4 * (1.0 - z_ratio * 0.3);

        egui::pos2(center_x + px, screen_y + depth_offset)
    };

    let project_v = |v: IVec3| -> egui::Pos2 {
        project(v.x as f32, v.y as f32, v.z as f32)
    };

    // calculate cell size at given z level
    let cell_at_z = |z: f32| -> f32 {
        let z_ratio = z / pit_h;
        floor_cell + (entrance_cell - floor_cell) * z_ratio
    };

    // colors
    let grid_green = egui::Color32::from_rgb(0, 100, 0);
    let edge_green = egui::Color32::from_rgb(0, 220, 0);
    let dim_green = egui::Color32::from_rgb(0, 50, 0);

    // draw floor (small rectangle at bottom)
    let f00 = project(0.0, 0.0, 0.0);
    let f10 = project(pit_w, 0.0, 0.0);
    let f11 = project(pit_w, pit_d, 0.0);
    let f01 = project(0.0, pit_d, 0.0);
    painter.add(egui::Shape::convex_polygon(
        vec![f00, f10, f11, f01],
        egui::Color32::from_rgb(0, 20, 0),
        egui::Stroke::new(2.0, grid_green),
    ));

    // draw floor grid
    for x in 0..=settings.pit_size.x {
        let p1 = project(x as f32, 0.0, 0.0);
        let p2 = project(x as f32, pit_d, 0.0);
        painter.line_segment([p1, p2], egui::Stroke::new(1.0, dim_green));
    }
    for y in 0..=settings.pit_size.y {
        let p1 = project(0.0, y as f32, 0.0);
        let p2 = project(pit_w, y as f32, 0.0);
        painter.line_segment([p1, p2], egui::Stroke::new(1.0, dim_green));
    }

    // draw entrance (large rectangle at top)
    let e00 = project(0.0, 0.0, pit_h);
    let e10 = project(pit_w, 0.0, pit_h);
    let e11 = project(pit_w, pit_d, pit_h);
    let e01 = project(0.0, pit_d, pit_h);

    // draw pit walls (connecting entrance to floor)
    // back wall
    painter.add(egui::Shape::convex_polygon(
        vec![f00, f10, e10, e00],
        dim_green,
        egui::Stroke::new(1.0, grid_green.linear_multiply(0.5)),
    ));
    // front wall
    painter.add(egui::Shape::convex_polygon(
        vec![f01, f11, e11, e01],
        dim_green.linear_multiply(0.8),
        egui::Stroke::new(1.0, grid_green.linear_multiply(0.5)),
    ));
    // left wall
    painter.add(egui::Shape::convex_polygon(
        vec![f00, f01, e01, e00],
        dim_green.linear_multiply(0.6),
        egui::Stroke::new(1.0, grid_green.linear_multiply(0.5)),
    ));
    // right wall
    painter.add(egui::Shape::convex_polygon(
        vec![f10, f11, e11, e10],
        dim_green.linear_multiply(0.7),
        egui::Stroke::new(1.0, grid_green.linear_multiply(0.5)),
    ));

    // draw placed voxels (sorted by z, draw bottom first)
    let mut sorted_voxels: Vec<_> = state.pit.iter().collect();
    sorted_voxels.sort_by_key(|(v, _)| v.z);

    for (v, _) in sorted_voxels {
        let color = layer_color(v.z, settings.pit_size.z);
        let pos = project_v(*v);
        let cell = cell_at_z(v.z as f32);
        draw_block(&painter, pos, cell * 0.9, color);
    }

    // draw shadow at landing position
    if let Some(ref piece) = state.current_piece {
        let mut shadow_z = piece.pos.z;
        loop {
            let test_z = shadow_z - 1;
            let collides = piece.voxels.iter().any(|v| {
                let world = *v + IVec3::new(piece.pos.x, piece.pos.y, test_z);
                world.z < 0 || state.pit.iter().any(|(p, _)| *p == world)
            });
            if collides { break; }
            shadow_z = test_z;
        }

        if shadow_z != piece.pos.z {
            for v in &piece.voxels {
                let world = *v + IVec3::new(piece.pos.x, piece.pos.y, shadow_z);
                let color = layer_color(world.z, settings.pit_size.z);
                let shadow = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 80);
                let pos = project_v(world);
                let cell = cell_at_z(world.z as f32);
                draw_block(&painter, pos, cell * 0.85, shadow);
            }
        }

        // draw current piece
        let mut piece_voxels: Vec<_> = piece.world_voxels();
        piece_voxels.sort_by_key(|v| v.z);

        for v in piece_voxels {
            let color = layer_color(v.z, settings.pit_size.z);
            let pos = project_v(v);
            let cell = cell_at_z(v.z as f32);
            draw_block(&painter, pos, cell * 0.9, color);
        }
    }

    // bright entrance frame
    painter.line_segment([e00, e10], egui::Stroke::new(3.0, edge_green));
    painter.line_segment([e10, e11], egui::Stroke::new(3.0, edge_green));
    painter.line_segment([e11, e01], egui::Stroke::new(3.0, edge_green));
    painter.line_segment([e01, e00], egui::Stroke::new(3.0, edge_green));
}

/// draw a block (simple rectangle, size scales with depth)
fn draw_block(painter: &egui::Painter, center: egui::Pos2, size: f32, color: egui::Color32) {
    let rect = egui::Rect::from_center_size(center, egui::vec2(size, size * 0.6));
    painter.rect_filled(rect, 2.0, color);
    let bright = egui::Color32::from_rgb(
        color.r().saturating_add(40),
        color.g().saturating_add(40),
        color.b().saturating_add(40),
    );
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, bright));
}

/// draw voxel with perspective
fn draw_voxel_perspective(painter: &egui::Painter, pos: egui::Pos2, size: f32, color: egui::Color32) {
    let rect = egui::Rect::from_center_size(pos, egui::vec2(size, size * 0.7));
    painter.rect_filled(rect, 2.0, color);
    let highlight = egui::Color32::from_rgb(
        color.r().saturating_add(40),
        color.g().saturating_add(40),
        color.b().saturating_add(40),
    );
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, highlight));
}

/// render the 3d pit (isometric projection)
fn render_pit(ui: &mut egui::Ui, state: &BlockoutState, settings: &BlockoutSettings) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(350.0, 500.0),
        egui::Sense::hover(),
    );

    let rect = response.rect;
    let center = rect.center();

    // isometric projection params
    let scale = 25.0;
    let iso_x = egui::vec2(0.866 * scale, 0.5 * scale);   // cos(30), sin(30)
    let iso_y = egui::vec2(-0.866 * scale, 0.5 * scale);  // -cos(30), sin(30)
    let iso_z = egui::vec2(0.0, -scale);                   // straight up

    let project = |v: IVec3| -> egui::Pos2 {
        let x = v.x as f32 - settings.pit_size.x as f32 / 2.0;
        let y = v.y as f32 - settings.pit_size.y as f32 / 2.0;
        let z = v.z as f32;
        egui::pos2(
            center.x + x * iso_x.x + y * iso_y.x + z * iso_z.x,
            center.y + 150.0 + x * iso_x.y + y * iso_y.y + z * iso_z.y,
        )
    };

    // draw pit outline
    let pit_color = egui::Color32::from_rgb(60, 60, 80);
    let corners = [
        IVec3::new(0, 0, 0),
        IVec3::new(settings.pit_size.x, 0, 0),
        IVec3::new(settings.pit_size.x, settings.pit_size.y, 0),
        IVec3::new(0, settings.pit_size.y, 0),
    ];

    // bottom
    for i in 0..4 {
        painter.line_segment(
            [project(corners[i]), project(corners[(i + 1) % 4])],
            egui::Stroke::new(1.0, pit_color),
        );
    }

    // vertical edges
    for corner in &corners {
        painter.line_segment(
            [project(*corner), project(*corner + IVec3::new(0, 0, settings.pit_size.z))],
            egui::Stroke::new(1.0, pit_color.linear_multiply(0.5)),
        );
    }

    // draw placed voxels
    for (v, color) in &state.pit {
        draw_voxel(&painter, project(*v), scale * 0.8, *color);
    }

    // draw current piece
    if let Some(ref piece) = state.current_piece {
        for v in piece.world_voxels() {
            draw_voxel(&painter, project(v), scale * 0.85, piece.color);
        }

        // draw shadow (where piece will land)
        let mut shadow_pos = piece.pos;
        loop {
            let test_pos = shadow_pos + IVec3::new(0, 0, -1);
            let test_voxels: Vec<IVec3> = piece.voxels.iter().map(|v| *v + test_pos).collect();

            let collides = test_voxels.iter().any(|v| {
                v.z < 0 || state.pit.iter().any(|(p, _)| *p == *v)
            });

            if collides {
                break;
            }
            shadow_pos = test_pos;
        }

        if shadow_pos != piece.pos {
            let shadow_color = egui::Color32::from_rgba_unmultiplied(
                piece.color.r(), piece.color.g(), piece.color.b(), 60
            );
            for v in &piece.voxels {
                let world_v = *v + shadow_pos;
                draw_voxel(&painter, project(world_v), scale * 0.8, shadow_color);
            }
        }
    }
}

/// draw a single voxel (cube face)
fn draw_voxel(painter: &egui::Painter, pos: egui::Pos2, size: f32, color: egui::Color32) {
    let half = size / 2.0;

    // top face (brightest)
    let top = [
        egui::pos2(pos.x, pos.y - half * 0.6),
        egui::pos2(pos.x + half, pos.y - half * 0.3),
        egui::pos2(pos.x, pos.y),
        egui::pos2(pos.x - half, pos.y - half * 0.3),
    ];
    // brighter stroke for top face edge
    let highlight = egui::Color32::from_rgb(
        color.r().saturating_add(40),
        color.g().saturating_add(40),
        color.b().saturating_add(40),
    );
    painter.add(egui::Shape::convex_polygon(
        top.to_vec(),
        color,
        egui::Stroke::new(1.0, highlight),
    ));

    // left face
    let left = [
        egui::pos2(pos.x - half, pos.y - half * 0.3),
        egui::pos2(pos.x, pos.y),
        egui::pos2(pos.x, pos.y + half * 0.6),
        egui::pos2(pos.x - half, pos.y + half * 0.3),
    ];
    painter.add(egui::Shape::convex_polygon(
        left.to_vec(),
        color.linear_multiply(0.7),
        egui::Stroke::NONE,
    ));

    // right face
    let right = [
        egui::pos2(pos.x + half, pos.y - half * 0.3),
        egui::pos2(pos.x + half, pos.y + half * 0.3),
        egui::pos2(pos.x, pos.y + half * 0.6),
        egui::pos2(pos.x, pos.y),
    ];
    painter.add(egui::Shape::convex_polygon(
        right.to_vec(),
        color.linear_multiply(0.5),
        egui::Stroke::NONE,
    ));
}

/// render next piece preview
fn render_preview(ui: &mut egui::Ui, piece_type: PieceType) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(80.0, 80.0),
        egui::Sense::hover(),
    );

    let center = response.rect.center();
    let scale = 15.0;

    for v in piece_type.voxels() {
        let pos = egui::pos2(
            center.x + v.x as f32 * scale,
            center.y + v.y as f32 * scale - v.z as f32 * scale * 0.5,
        );
        draw_voxel(&painter, pos, scale * 0.9, piece_type.color());
    }
}

/// render key hints overlay
fn render_key_hints(ui: &mut egui::Ui) {
    ui.group(|ui| {
        ui.label("Controls (F1 to hide):");
        ui.add_space(5.0);

        ui.label("Movement (vim):");
        ui.horizontal(|ui| {
            ui.label("  ");
            hint_key(ui, "K", "back");
        });
        ui.horizontal(|ui| {
            hint_key(ui, "H", "left");
            hint_key(ui, "J", "fwd");
            hint_key(ui, "L", "right");
        });

        ui.add_space(5.0);
        ui.label("Rotation (poker grid):");
        ui.horizontal(|ui| {
            hint_key(ui, "Q", "rot X");
            hint_key(ui, "W", "rot Y");
            hint_key(ui, "E", "rot X'");
            hint_key(ui, "R", "rot Y'");
        });
        ui.horizontal(|ui| {
            hint_key(ui, "A", "rot Z");
            hint_key(ui, "S", "drop");
            hint_key(ui, "D", "rot Z'");
        });

        ui.add_space(5.0);
        ui.label("Drop & Hold:");
        ui.horizontal(|ui| {
            hint_key(ui, "S", "soft");
            hint_key(ui, "SPC", "hard");
            hint_key(ui, "C", "hold");
        });
    });
}

fn hint_key(ui: &mut egui::Ui, key: &str, action: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(
            egui::RichText::new(key)
                .monospace()
                .background_color(egui::Color32::from_rgb(60, 60, 80))
        ));
        ui.small(action);
    });
}

/// toggle blockout mode
pub fn toggle_blockout(state: &mut BlockoutState, settings: &BlockoutSettings) {
    if state.active {
        state.active = false;
    } else {
        state.new_game(settings);
    }
}
