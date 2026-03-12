//! avatar system - privacy-preserving video avatars for social poker
//!
//! instead of sending raw camera video, we:
//! 1. extract face blend shapes locally (52 floats from face tracking)
//! 2. send only blend shape data over WebRTC data channel (~2 KB/s)
//! 3. render an avatar locally from those blend shapes
//!
//! this means:
//! - no face pixels ever leave the device (privacy by design)
//! - avatars are assets that can be bought/sold/traded
//! - poker tells are preserved through expression mapping
//! - works on low bandwidth connections
//!
//! avatar economy:
//! - free basic avatars (geometric, emoji-style)
//! - premium avatars with more detail, animations, accessories
//! - seasonal/limited edition avatars
//! - avatars can be on-chain NFTs for provable ownership

use bevy::prelude::*;
use bevy_egui::egui;
use std::collections::HashMap;

use poker_p2p::protocol::{AvatarFrame, blend_shape};

pub struct AvatarPlugin;

impl Plugin for AvatarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AvatarManager>()
            .init_resource::<AvatarCatalog>()
            .add_event::<AvatarEvent>()
            .add_systems(Update, (
                process_avatar_frames,
                update_avatar_animations,
            ));
    }
}

/// events for the avatar system
#[derive(Event, Clone, Debug)]
pub enum AvatarEvent {
    /// received blend shape frame from a peer
    FrameReceived(AvatarFrame),
    /// local face tracking produced a frame
    LocalFrame { blend_shapes: Vec<f32>, rotation: [f32; 3], position: [f32; 3] },
    /// player equipped a new avatar
    AvatarEquipped { seat: u8, avatar_id: String },
    /// toggle face tracking on/off
    ToggleTracking,
}

/// manages avatar state for all players at the table
#[derive(Resource)]
pub struct AvatarManager {
    /// per-seat avatar state
    pub seats: HashMap<u8, SeatAvatar>,
    /// our local avatar id
    pub my_avatar: String,
    /// is face tracking active
    pub tracking_active: bool,
    /// face tracking frame rate target
    pub tracking_fps: u32,
    /// outgoing frames to send via WebRTC
    pub outgoing_frames: Vec<AvatarFrame>,
    /// local seat
    pub local_seat: u8,
    /// sequence counter
    seq: u32,
    /// show avatar settings
    pub show_settings: bool,
    /// expression intensity multiplier (poker face mode = 0.0, expressive = 2.0)
    pub expression_intensity: f32,
}

impl Default for AvatarManager {
    fn default() -> Self {
        Self {
            seats: HashMap::new(),
            my_avatar: "default".into(),
            tracking_active: false,
            tracking_fps: 30,
            outgoing_frames: Vec::new(),
            local_seat: 0,
            seq: 0,
            show_settings: false,
            expression_intensity: 1.0,
        }
    }
}

impl AvatarManager {
    /// process a local face tracking frame
    pub fn on_local_frame(&mut self, blend_shapes: &[f32], rotation: [f32; 3], position: [f32; 3]) {
        // apply expression intensity
        let scaled: Vec<f32> = blend_shapes.iter()
            .map(|v| (v * self.expression_intensity).clamp(0.0, 1.0))
            .collect();

        let frame = AvatarFrame::from_floats(self.local_seat, self.seq, &scaled, rotation, position);
        self.seq = self.seq.wrapping_add(1);

        // update our own avatar for local preview
        self.update_seat(self.local_seat, &frame);

        // queue for sending
        self.outgoing_frames.push(frame);
    }

    /// process a received frame from a peer
    pub fn on_remote_frame(&mut self, frame: &AvatarFrame) {
        self.update_seat(frame.seat, frame);
    }

    fn update_seat(&mut self, seat: u8, frame: &AvatarFrame) {
        let avatar = self.seats.entry(seat).or_insert_with(|| SeatAvatar::new("default"));
        avatar.current_frame = Some(frame.clone());
        avatar.last_update = std::time::Instant::now();

        // smooth blend shapes with exponential moving average
        if avatar.smoothed_shapes.len() != frame.blend_shapes.len() {
            avatar.smoothed_shapes = vec![0.0; frame.blend_shapes.len()];
        }
        let alpha = 0.4; // smoothing factor (higher = more responsive, lower = smoother)
        for (i, &raw) in frame.blend_shapes.iter().enumerate() {
            let target = raw as f32 / 255.0;
            avatar.smoothed_shapes[i] = avatar.smoothed_shapes[i] * (1.0 - alpha) + target * alpha;
        }

        // smooth head rotation
        let rot = frame.rotation_rad();
        for i in 0..3 {
            avatar.smoothed_rotation[i] = avatar.smoothed_rotation[i] * (1.0 - alpha) + rot[i] * alpha;
        }
    }

    /// get the smoothed blend shape value for a seat
    pub fn shape(&self, seat: u8, index: usize) -> f32 {
        self.seats.get(&seat)
            .and_then(|a| a.smoothed_shapes.get(index).copied())
            .unwrap_or(0.0)
    }

    /// check if a seat has active avatar data (received recent frame)
    pub fn is_active(&self, seat: u8) -> bool {
        self.seats.get(&seat)
            .map(|a| a.last_update.elapsed().as_secs_f32() < 2.0)
            .unwrap_or(false)
    }
}

/// avatar state for a single seat
pub struct SeatAvatar {
    /// which avatar asset to render
    pub avatar_id: String,
    /// most recent raw frame
    pub current_frame: Option<AvatarFrame>,
    /// smoothed blend shape values (0.0 - 1.0)
    pub smoothed_shapes: Vec<f32>,
    /// smoothed head rotation (radians)
    pub smoothed_rotation: [f32; 3],
    /// when we last received a frame
    pub last_update: std::time::Instant,
}

impl SeatAvatar {
    pub fn new(avatar_id: &str) -> Self {
        Self {
            avatar_id: avatar_id.into(),
            current_frame: None,
            smoothed_shapes: vec![0.0; blend_shape::COUNT],
            smoothed_rotation: [0.0; 3],
            last_update: std::time::Instant::now(),
        }
    }
}

/// avatar catalog — available avatars for purchase/use
#[derive(Resource, Default)]
pub struct AvatarCatalog {
    pub avatars: Vec<AvatarAsset>,
    pub owned: Vec<String>, // avatar_ids the player owns
}

impl AvatarCatalog {
    pub fn with_defaults() -> Self {
        Self {
            avatars: vec![
                AvatarAsset::free("default", "Classic", AvatarStyle::Geometric,
                    "simple geometric face — eyes, mouth, eyebrows"),
                AvatarAsset::free("emoji", "Emoji", AvatarStyle::Emoji,
                    "round emoji-style face with big expressions"),
                AvatarAsset::free("minimal", "Minimal", AvatarStyle::Minimal,
                    "minimalist dots and lines"),
                AvatarAsset::premium("neon", "Neon Glow", AvatarStyle::Geometric,
                    "glowing neon wireframe face", 500),
                AvatarAsset::premium("skull", "Poker Skull", AvatarStyle::Illustrated,
                    "stylized skull with expressive jaw", 1000),
                AvatarAsset::premium("cat", "Lucky Cat", AvatarStyle::Illustrated,
                    "maneki-neko inspired, ear wiggles on brow raise", 750),
                AvatarAsset::premium("robot", "Chrome Bot", AvatarStyle::Geometric,
                    "robotic face with LED eye expressions", 1200),
                AvatarAsset::seasonal("diamond", "Diamond Face", AvatarStyle::Geometric,
                    "crystalline diamond facets, seasonal edition", 2000),
            ],
            owned: vec!["default".into(), "emoji".into(), "minimal".into()],
        }
    }
}

/// an avatar asset definition
#[derive(Clone, Debug)]
pub struct AvatarAsset {
    pub id: String,
    pub name: String,
    pub style: AvatarStyle,
    pub description: String,
    pub price: u64, // 0 = free
    pub rarity: AvatarRarity,
}

impl AvatarAsset {
    fn free(id: &str, name: &str, style: AvatarStyle, desc: &str) -> Self {
        Self {
            id: id.into(), name: name.into(), style, description: desc.into(),
            price: 0, rarity: AvatarRarity::Free,
        }
    }

    fn premium(id: &str, name: &str, style: AvatarStyle, desc: &str, price: u64) -> Self {
        Self {
            id: id.into(), name: name.into(), style, description: desc.into(),
            price, rarity: AvatarRarity::Premium,
        }
    }

    fn seasonal(id: &str, name: &str, style: AvatarStyle, desc: &str, price: u64) -> Self {
        Self {
            id: id.into(), name: name.into(), style, description: desc.into(),
            price, rarity: AvatarRarity::Seasonal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarStyle {
    /// simple geometric shapes (circles, lines)
    Geometric,
    /// emoji-style round faces
    Emoji,
    /// minimal dots and lines
    Minimal,
    /// hand-drawn illustrated style
    Illustrated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarRarity {
    Free,
    Premium,
    Seasonal,
    Limited,
}

// === Bevy Systems ===

/// process incoming avatar frames
fn process_avatar_frames(
    mut manager: ResMut<AvatarManager>,
    mut events: EventReader<AvatarEvent>,
) {
    for event in events.read() {
        match event {
            AvatarEvent::FrameReceived(frame) => {
                manager.on_remote_frame(frame);
            }
            AvatarEvent::LocalFrame { blend_shapes, rotation, position } => {
                manager.on_local_frame(blend_shapes, *rotation, *position);
            }
            AvatarEvent::AvatarEquipped { seat, avatar_id } => {
                if let Some(avatar) = manager.seats.get_mut(seat) {
                    avatar.avatar_id = avatar_id.clone();
                }
            }
            AvatarEvent::ToggleTracking => {
                manager.tracking_active = !manager.tracking_active;
                if manager.tracking_active {
                    info!("avatar: face tracking enabled");
                } else {
                    info!("avatar: face tracking disabled");
                }
            }
        }
    }
}

/// animate avatars (decay to neutral when no frames received)
fn update_avatar_animations(
    mut manager: ResMut<AvatarManager>,
) {
    let decay_rate = 0.05;
    for (_, avatar) in manager.seats.iter_mut() {
        if avatar.last_update.elapsed().as_secs_f32() > 0.5 {
            // no recent data — decay toward neutral expression
            for shape in avatar.smoothed_shapes.iter_mut() {
                *shape *= 1.0 - decay_rate;
            }
            for rot in avatar.smoothed_rotation.iter_mut() {
                *rot *= 1.0 - decay_rate;
            }
        }
    }
}

// === Avatar Renderer (egui 2D) ===

/// render an avatar face in an egui rect
/// this is the core rendering function called from table_2d.rs
pub fn render_avatar_face(
    ui: &mut egui::Ui,
    center: egui::Pos2,
    size: f32,
    avatar: &SeatAvatar,
    _style: AvatarStyle,
) {
    let painter = ui.painter();
    let s = &avatar.smoothed_shapes;
    let rot = avatar.smoothed_rotation;

    // head yaw offset for position
    let yaw_offset = rot[1] * size * 0.3;
    let pitch_offset = rot[0] * size * 0.2;
    let center = egui::pos2(center.x + yaw_offset, center.y + pitch_offset);

    let half = size * 0.5;

    // face outline (oval, slight head tilt via roll)
    let face_color = egui::Color32::from_rgb(60, 55, 70);
    let face_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 95, 110));
    painter.circle(center, half, face_color, face_stroke);

    // === eyes ===
    let eye_y = center.y - half * 0.15;
    let eye_spacing = half * 0.35;
    let eye_size = half * 0.18;

    let blink_l = s.get(blend_shape::EYE_BLINK_LEFT).copied().unwrap_or(0.0);
    let blink_r = s.get(blend_shape::EYE_BLINK_RIGHT).copied().unwrap_or(0.0);
    let wide_l = s.get(blend_shape::EYE_WIDE_LEFT).copied().unwrap_or(0.0);
    let wide_r = s.get(blend_shape::EYE_WIDE_RIGHT).copied().unwrap_or(0.0);
    let squint_l = s.get(blend_shape::EYE_SQUINT_LEFT).copied().unwrap_or(0.0);
    let squint_r = s.get(blend_shape::EYE_SQUINT_RIGHT).copied().unwrap_or(0.0);

    // left eye
    let left_eye_center = egui::pos2(center.x - eye_spacing, eye_y);
    let l_openness = (1.0 - blink_l + wide_l * 0.5 - squint_l * 0.3).clamp(0.05, 1.5);
    draw_eye(painter, left_eye_center, eye_size, l_openness,
        s.get(blend_shape::EYE_LOOK_IN_LEFT).copied().unwrap_or(0.0),
        s.get(blend_shape::EYE_LOOK_OUT_LEFT).copied().unwrap_or(0.0),
        s.get(blend_shape::EYE_LOOK_UP_LEFT).copied().unwrap_or(0.0),
        s.get(blend_shape::EYE_LOOK_DOWN_LEFT).copied().unwrap_or(0.0),
    );

    // right eye
    let right_eye_center = egui::pos2(center.x + eye_spacing, eye_y);
    let r_openness = (1.0 - blink_r + wide_r * 0.5 - squint_r * 0.3).clamp(0.05, 1.5);
    draw_eye(painter, right_eye_center, eye_size, r_openness,
        s.get(blend_shape::EYE_LOOK_OUT_RIGHT).copied().unwrap_or(0.0),
        s.get(blend_shape::EYE_LOOK_IN_RIGHT).copied().unwrap_or(0.0),
        s.get(blend_shape::EYE_LOOK_UP_RIGHT).copied().unwrap_or(0.0),
        s.get(blend_shape::EYE_LOOK_DOWN_RIGHT).copied().unwrap_or(0.0),
    );

    // === eyebrows ===
    let brow_y = eye_y - half * 0.22;
    let brow_down_l = s.get(blend_shape::BROW_DOWN_LEFT).copied().unwrap_or(0.0);
    let brow_down_r = s.get(blend_shape::BROW_DOWN_RIGHT).copied().unwrap_or(0.0);
    let brow_inner_up = s.get(blend_shape::BROW_INNER_UP).copied().unwrap_or(0.0);
    let brow_outer_up_l = s.get(blend_shape::BROW_OUTER_UP_LEFT).copied().unwrap_or(0.0);
    let brow_outer_up_r = s.get(blend_shape::BROW_OUTER_UP_RIGHT).copied().unwrap_or(0.0);

    let brow_stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(180, 170, 190));

    // left eyebrow
    let l_inner_y = brow_y - brow_inner_up * half * 0.15 + brow_down_l * half * 0.1;
    let l_outer_y = brow_y - brow_outer_up_l * half * 0.15 + brow_down_l * half * 0.08;
    painter.line_segment(
        [egui::pos2(center.x - eye_spacing * 0.5, l_inner_y),
         egui::pos2(center.x - eye_spacing * 1.4, l_outer_y)],
        brow_stroke,
    );

    // right eyebrow
    let r_inner_y = brow_y - brow_inner_up * half * 0.15 + brow_down_r * half * 0.1;
    let r_outer_y = brow_y - brow_outer_up_r * half * 0.15 + brow_down_r * half * 0.08;
    painter.line_segment(
        [egui::pos2(center.x + eye_spacing * 0.5, r_inner_y),
         egui::pos2(center.x + eye_spacing * 1.4, r_outer_y)],
        brow_stroke,
    );

    // === mouth ===
    let mouth_y = center.y + half * 0.35;
    let jaw_open = s.get(blend_shape::JAW_OPEN).copied().unwrap_or(0.0);
    let smile_l = s.get(blend_shape::MOUTH_SMILE_LEFT).copied().unwrap_or(0.0);
    let smile_r = s.get(blend_shape::MOUTH_SMILE_RIGHT).copied().unwrap_or(0.0);
    let frown_l = s.get(blend_shape::MOUTH_FROWN_LEFT).copied().unwrap_or(0.0);
    let frown_r = s.get(blend_shape::MOUTH_FROWN_RIGHT).copied().unwrap_or(0.0);
    let pucker = s.get(blend_shape::MOUTH_PUCKER).copied().unwrap_or(0.0);

    let mouth_width = half * 0.5 * (1.0 - pucker * 0.5);
    let smile_avg = (smile_l + smile_r) * 0.5;
    let frown_avg = (frown_l + frown_r) * 0.5;
    let mouth_curve = (smile_avg - frown_avg) * half * 0.15;

    let mouth_color = egui::Color32::from_rgb(150, 80, 80);

    if jaw_open > 0.1 {
        // open mouth — ellipse
        let open_height = half * 0.08 + jaw_open * half * 0.2;
        let mouth_rect = egui::Rect::from_center_size(
            egui::pos2(center.x, mouth_y - mouth_curve * 0.5),
            egui::vec2(mouth_width * 2.0, open_height * 2.0),
        );
        painter.rect(mouth_rect, egui::Rounding::same(open_height), mouth_color,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 60, 60)));
    } else {
        // closed mouth — curved line
        let mouth_stroke = egui::Stroke::new(2.0, mouth_color);
        let left = egui::pos2(center.x - mouth_width, mouth_y + mouth_curve);
        let mid = egui::pos2(center.x, mouth_y - mouth_curve);
        let right = egui::pos2(center.x + mouth_width, mouth_y + mouth_curve);
        // approximate curve with 3 segments
        let q1 = egui::pos2(
            (left.x + mid.x) * 0.5,
            (left.y + mid.y) * 0.5,
        );
        let q2 = egui::pos2(
            (mid.x + right.x) * 0.5,
            (mid.y + right.y) * 0.5,
        );
        painter.line_segment([left, q1], mouth_stroke);
        painter.line_segment([q1, mid], mouth_stroke);
        painter.line_segment([mid, q2], mouth_stroke);
        painter.line_segment([q2, right], mouth_stroke);
    }

    // === cheeks (blush on smile) ===
    if smile_avg > 0.3 {
        let blush_alpha = ((smile_avg - 0.3) * 100.0).min(40.0) as u8;
        let cheek_size = half * 0.15;
        painter.circle_filled(
            egui::pos2(center.x - eye_spacing * 1.2, mouth_y - half * 0.1),
            cheek_size,
            egui::Color32::from_rgba_premultiplied(200, 80, 80, blush_alpha),
        );
        painter.circle_filled(
            egui::pos2(center.x + eye_spacing * 1.2, mouth_y - half * 0.1),
            cheek_size,
            egui::Color32::from_rgba_premultiplied(200, 80, 80, blush_alpha),
        );
    }
}

/// draw a single eye with gaze direction
fn draw_eye(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    openness: f32,
    look_left: f32,
    look_right: f32,
    look_up: f32,
    look_down: f32,
) {
    let eye_height = size * openness;
    let eye_white = egui::Color32::from_rgb(220, 220, 230);
    let eye_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 75, 90));

    // eye white (ellipse approximated by rounded rect)
    let eye_rect = egui::Rect::from_center_size(
        center,
        egui::vec2(size * 1.8, eye_height * 2.0),
    );
    painter.rect(eye_rect, egui::Rounding::same(eye_height), eye_white, eye_stroke);

    if openness > 0.1 {
        // iris/pupil with gaze offset
        let gaze_x = (look_right - look_left) * size * 0.4;
        let gaze_y = (look_down - look_up) * size * 0.3;
        let iris_center = egui::pos2(center.x + gaze_x, center.y + gaze_y);
        let iris_size = size * 0.55;
        let pupil_size = iris_size * 0.55;

        // iris
        painter.circle_filled(iris_center, iris_size, egui::Color32::from_rgb(80, 100, 140));
        // pupil
        painter.circle_filled(iris_center, pupil_size, egui::Color32::from_rgb(20, 20, 30));
        // highlight
        painter.circle_filled(
            egui::pos2(iris_center.x - pupil_size * 0.3, iris_center.y - pupil_size * 0.3),
            pupil_size * 0.25,
            egui::Color32::from_rgba_premultiplied(255, 255, 255, 180),
        );
    }
}

/// render the avatar shop/settings panel
pub fn render_avatar_settings(
    ui: &mut egui::Ui,
    manager: &mut AvatarManager,
    catalog: &AvatarCatalog,
) {
    ui.heading("avatar");

    ui.horizontal(|ui| {
        ui.label("face tracking:");
        if ui.selectable_label(manager.tracking_active, if manager.tracking_active { "ON" } else { "OFF" }).clicked() {
            manager.tracking_active = !manager.tracking_active;
        }
    });

    ui.horizontal(|ui| {
        ui.label("expression intensity:");
        ui.add(egui::Slider::new(&mut manager.expression_intensity, 0.0..=2.0)
            .text(if manager.expression_intensity < 0.3 { "poker face" }
                  else if manager.expression_intensity > 1.5 { "dramatic" }
                  else { "natural" }));
    });

    ui.separator();

    ui.label(egui::RichText::new("avatar shop").strong());

    for avatar in &catalog.avatars {
        let owned = catalog.owned.contains(&avatar.id);
        let equipped = manager.my_avatar == avatar.id;

        ui.horizontal(|ui| {
            // rarity color
            let rarity_color = match avatar.rarity {
                AvatarRarity::Free => egui::Color32::GRAY,
                AvatarRarity::Premium => egui::Color32::from_rgb(100, 180, 255),
                AvatarRarity::Seasonal => egui::Color32::from_rgb(255, 200, 60),
                AvatarRarity::Limited => egui::Color32::from_rgb(220, 80, 220),
            };

            ui.label(egui::RichText::new(&avatar.name).color(rarity_color));

            if equipped {
                ui.label(egui::RichText::new("[equipped]").small().color(egui::Color32::GREEN));
            } else if owned {
                if ui.small_button("equip").clicked() {
                    manager.my_avatar = avatar.id.clone();
                }
            } else if avatar.price > 0 {
                ui.label(egui::RichText::new(format!("{} chips", avatar.price)).small());
                if ui.small_button("buy").clicked() {
                    // purchase flow
                    info!("avatar: purchase {} for {} chips", avatar.id, avatar.price);
                }
            }
        });

        ui.label(egui::RichText::new(&avatar.description).small().weak());
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_avatar_manager_local_frame() {
        let mut manager = AvatarManager::default();
        manager.local_seat = 1;

        let shapes = vec![0.0; blend_shape::COUNT];
        manager.on_local_frame(&shapes, [0.0; 3], [0.0; 3]);

        assert!(manager.is_active(1));
        assert_eq!(manager.outgoing_frames.len(), 1);
    }

    #[test]
    fn test_avatar_manager_remote_frame() {
        let mut manager = AvatarManager::default();

        let frame = AvatarFrame::from_floats(2, 0, &vec![0.5; blend_shape::COUNT], [0.0; 3], [0.0; 3]);
        manager.on_remote_frame(&frame);

        assert!(manager.is_active(2));
        // smoothed value should be approaching 0.5
        let eye_blink = manager.shape(2, blend_shape::EYE_BLINK_LEFT);
        assert!(eye_blink > 0.1); // after one smoothing step from 0.0
    }

    #[test]
    fn test_expression_intensity() {
        let mut manager = AvatarManager::default();
        manager.local_seat = 1;
        manager.expression_intensity = 0.0; // poker face mode

        let shapes = vec![1.0; blend_shape::COUNT];
        manager.on_local_frame(&shapes, [0.0; 3], [0.0; 3]);

        // with 0.0 intensity, all shapes should be 0
        let frame = &manager.outgoing_frames[0];
        assert_eq!(frame.blend_shapes[0], 0);
    }

    #[test]
    fn test_catalog_defaults() {
        let catalog = AvatarCatalog::with_defaults();
        assert!(catalog.avatars.len() >= 3); // at least 3 free ones
        assert!(catalog.owned.contains(&"default".to_string()));
    }

    #[test]
    fn test_avatar_frame_quantization() {
        let frame = AvatarFrame::from_floats(1, 0, &[0.5, 1.0, 0.0], [0.1, -0.2, 0.05], [0.0; 3]);

        assert_eq!(frame.shape(0), 128.0 / 255.0); // ~0.502 (quantized)
        assert_eq!(frame.shape(1), 1.0);
        assert_eq!(frame.shape(2), 0.0);

        let rot = frame.rotation_rad();
        assert!((rot[0] - 0.1).abs() < 0.01);
        assert!((rot[1] - (-0.2)).abs() < 0.01);
    }
}
