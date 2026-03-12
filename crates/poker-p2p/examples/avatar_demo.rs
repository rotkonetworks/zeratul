//! avatar face demo — interactive blend shape visualization
//!
//! run with: cargo run --example avatar_demo

use eframe::egui;
use poker_p2p::protocol::{AvatarFrame, blend_shape};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_title("zk.poker avatar demo"),
        ..Default::default()
    };

    eframe::run_native(
        "avatar_demo",
        options,
        Box::new(|_cc| Ok(Box::new(AvatarDemo::default()))),
    )
}

struct AvatarDemo {
    /// raw blend shape values (0.0 - 1.0)
    shapes: Vec<f32>,
    /// head rotation (pitch, yaw, roll) in radians
    head_rotation: [f32; 3],
    /// smoothed shapes for rendering
    smoothed: Vec<f32>,
    /// smoothed rotation
    smoothed_rot: [f32; 3],
    /// expression intensity multiplier
    intensity: f32,
    /// avatar size
    avatar_size: f32,
    /// auto-animate (random expressions)
    auto_animate: bool,
    /// animation time
    anim_time: f64,
    /// preset expressions
    presets: Vec<(&'static str, Vec<(usize, f32)>)>,
    /// show all sliders or just common ones
    show_all: bool,
    /// active preset index
    active_preset: Option<usize>,
    /// background color
    bg_dark: bool,
}

impl Default for AvatarDemo {
    fn default() -> Self {
        Self {
            shapes: vec![0.0; blend_shape::COUNT],
            head_rotation: [0.0; 3],
            smoothed: vec![0.0; blend_shape::COUNT],
            smoothed_rot: [0.0; 3],
            intensity: 1.0,
            avatar_size: 120.0,
            auto_animate: false,
            anim_time: 0.0,
            presets: vec![
                ("neutral", vec![]),
                ("smile", vec![
                    (blend_shape::MOUTH_SMILE_LEFT, 0.8),
                    (blend_shape::MOUTH_SMILE_RIGHT, 0.8),
                    (blend_shape::CHEEK_SQUINT_LEFT, 0.3),
                    (blend_shape::CHEEK_SQUINT_RIGHT, 0.3),
                    (blend_shape::EYE_SQUINT_LEFT, 0.2),
                    (blend_shape::EYE_SQUINT_RIGHT, 0.2),
                ]),
                ("surprise", vec![
                    (blend_shape::EYE_WIDE_LEFT, 0.9),
                    (blend_shape::EYE_WIDE_RIGHT, 0.9),
                    (blend_shape::BROW_INNER_UP, 0.8),
                    (blend_shape::BROW_OUTER_UP_LEFT, 0.7),
                    (blend_shape::BROW_OUTER_UP_RIGHT, 0.7),
                    (blend_shape::JAW_OPEN, 0.6),
                ]),
                ("angry", vec![
                    (blend_shape::BROW_DOWN_LEFT, 0.9),
                    (blend_shape::BROW_DOWN_RIGHT, 0.9),
                    (blend_shape::EYE_SQUINT_LEFT, 0.5),
                    (blend_shape::EYE_SQUINT_RIGHT, 0.5),
                    (blend_shape::MOUTH_FROWN_LEFT, 0.6),
                    (blend_shape::MOUTH_FROWN_RIGHT, 0.6),
                    (blend_shape::NOSE_SNEER_LEFT, 0.4),
                    (blend_shape::NOSE_SNEER_RIGHT, 0.4),
                ]),
                ("thinking", vec![
                    (blend_shape::BROW_DOWN_LEFT, 0.3),
                    (blend_shape::BROW_OUTER_UP_RIGHT, 0.6),
                    (blend_shape::EYE_SQUINT_LEFT, 0.3),
                    (blend_shape::MOUTH_LEFT, 0.4),
                    (blend_shape::MOUTH_PUCKER, 0.3),
                    (blend_shape::EYE_LOOK_UP_LEFT, 0.5),
                    (blend_shape::EYE_LOOK_UP_RIGHT, 0.5),
                ]),
                ("wink", vec![
                    (blend_shape::EYE_BLINK_LEFT, 0.95),
                    (blend_shape::MOUTH_SMILE_LEFT, 0.5),
                    (blend_shape::MOUTH_SMILE_RIGHT, 0.7),
                    (blend_shape::CHEEK_SQUINT_LEFT, 0.4),
                ]),
                ("poker face", vec![
                    // almost nothing — deliberately flat
                    (blend_shape::MOUTH_PRESS_LEFT, 0.2),
                    (blend_shape::MOUTH_PRESS_RIGHT, 0.2),
                ]),
                ("bluff detected", vec![
                    (blend_shape::EYE_WIDE_LEFT, 0.3),
                    (blend_shape::EYE_WIDE_RIGHT, 0.3),
                    (blend_shape::BROW_INNER_UP, 0.4),
                    (blend_shape::MOUTH_SMILE_LEFT, 0.2),
                    (blend_shape::MOUTH_SMILE_RIGHT, 0.2),
                    (blend_shape::CHEEK_PUFF, 0.15),
                ]),
                ("all-in face", vec![
                    (blend_shape::BROW_DOWN_LEFT, 0.4),
                    (blend_shape::BROW_DOWN_RIGHT, 0.4),
                    (blend_shape::EYE_SQUINT_LEFT, 0.3),
                    (blend_shape::EYE_SQUINT_RIGHT, 0.3),
                    (blend_shape::JAW_FORWARD, 0.3),
                    (blend_shape::MOUTH_PRESS_LEFT, 0.5),
                    (blend_shape::MOUTH_PRESS_RIGHT, 0.5),
                ]),
            ],
            show_all: false,
            active_preset: None,
            bg_dark: true,
        }
    }
}

impl eframe::App for AvatarDemo {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // smooth blend shapes
        let alpha = 0.3;
        for i in 0..blend_shape::COUNT {
            let target = (self.shapes[i] * self.intensity).clamp(0.0, 1.0);
            self.smoothed[i] = self.smoothed[i] * (1.0 - alpha) + target * alpha;
        }
        for i in 0..3 {
            self.smoothed_rot[i] = self.smoothed_rot[i] * (1.0 - alpha) + self.head_rotation[i] * alpha;
        }

        // auto-animate
        if self.auto_animate {
            self.anim_time += ctx.input(|i| i.stable_dt as f64);
            let t = self.anim_time;

            // gentle idle animation
            self.shapes[blend_shape::EYE_BLINK_LEFT] =
                if (t * 0.3).fract() > 0.95 { 1.0 } else { 0.0 };
            self.shapes[blend_shape::EYE_BLINK_RIGHT] =
                if (t * 0.3).fract() > 0.95 { 1.0 } else { 0.0 };

            self.head_rotation[1] = (t * 0.5).sin() as f32 * 0.15; // slow yaw
            self.head_rotation[0] = (t * 0.3).sin() as f32 * 0.05; // slight pitch

            // breathing mouth
            self.shapes[blend_shape::JAW_OPEN] = ((t * 0.8).sin() as f32 * 0.5 + 0.5) * 0.05;

            // occasional micro-expressions
            let cycle = (t * 0.15).fract();
            if cycle < 0.25 {
                // slight smile
                let blend = (cycle / 0.25) as f32;
                self.shapes[blend_shape::MOUTH_SMILE_LEFT] = blend * 0.3;
                self.shapes[blend_shape::MOUTH_SMILE_RIGHT] = blend * 0.3;
            } else if cycle < 0.5 {
                let blend = ((cycle - 0.25) / 0.25) as f32;
                self.shapes[blend_shape::MOUTH_SMILE_LEFT] = (1.0 - blend) * 0.3;
                self.shapes[blend_shape::MOUTH_SMILE_RIGHT] = (1.0 - blend) * 0.3;
            }

            ctx.request_repaint();
        }

        // side panel with controls
        egui::SidePanel::left("controls")
            .default_width(350.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.render_controls(ui);
                });
            });

        // central panel with avatar face
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_preview(ui);
        });
    }
}

impl AvatarDemo {
    fn render_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("avatar controls");
        ui.add_space(8.0);

        // settings row
        ui.horizontal(|ui| {
            ui.label("size:");
            ui.add(egui::Slider::new(&mut self.avatar_size, 40.0..=300.0));
        });
        ui.horizontal(|ui| {
            ui.label("intensity:");
            let label = if self.intensity < 0.3 { "poker face" }
                        else if self.intensity > 1.5 { "dramatic" }
                        else { "natural" };
            ui.add(egui::Slider::new(&mut self.intensity, 0.0..=2.0).text(label));
        });
        ui.checkbox(&mut self.auto_animate, "auto-animate (idle)");
        ui.checkbox(&mut self.bg_dark, "dark background");

        ui.separator();

        // presets
        ui.label(egui::RichText::new("presets").strong());
        ui.horizontal_wrapped(|ui| {
            for (i, (name, _)) in self.presets.iter().enumerate() {
                let active = self.active_preset == Some(i);
                if ui.selectable_label(active, *name).clicked() {
                    // reset all shapes
                    self.shapes = vec![0.0; blend_shape::COUNT];
                    self.head_rotation = [0.0; 3];
                    self.auto_animate = false;

                    // apply preset
                    let preset_values: Vec<(usize, f32)> = self.presets[i].1.clone();
                    for (idx, val) in preset_values {
                        self.shapes[idx] = val;
                    }
                    self.active_preset = Some(i);
                }
            }
        });

        if ui.button("reset all").clicked() {
            self.shapes = vec![0.0; blend_shape::COUNT];
            self.head_rotation = [0.0; 3];
            self.active_preset = None;
        }

        ui.separator();

        // head rotation
        ui.label(egui::RichText::new("head").strong());
        ui.horizontal(|ui| {
            ui.label("pitch:");
            ui.add(egui::Slider::new(&mut self.head_rotation[0], -0.5..=0.5));
        });
        ui.horizontal(|ui| {
            ui.label("yaw:");
            ui.add(egui::Slider::new(&mut self.head_rotation[1], -0.5..=0.5));
        });
        ui.horizontal(|ui| {
            ui.label("roll:");
            ui.add(egui::Slider::new(&mut self.head_rotation[2], -0.3..=0.3));
        });

        ui.separator();

        // blend shape sliders
        ui.checkbox(&mut self.show_all, "show all 52 blend shapes");

        let shape_names: &[(usize, &str)] = if self.show_all {
            &ALL_SHAPES
        } else {
            &COMMON_SHAPES
        };

        for &(idx, name) in shape_names {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(name).small().monospace());
                ui.add(egui::Slider::new(&mut self.shapes[idx], 0.0..=1.0).show_value(false));
                ui.label(egui::RichText::new(format!("{:.2}", self.shapes[idx])).small());
            });
        }

        ui.separator();

        // wire stats
        let frame = AvatarFrame::from_floats(
            1, 0, &self.shapes, self.head_rotation, [0.0; 3],
        );
        let encoded = parity_scale_codec::Encode::encode(
            &poker_p2p::protocol::Message::AvatarFrame(frame),
        );
        ui.label(egui::RichText::new(format!(
            "wire: {} bytes/frame, {:.1} KB/s at 30fps",
            encoded.len(),
            encoded.len() as f32 * 30.0 / 1024.0,
        )).small().weak());
    }

    fn render_preview(&self, ui: &mut egui::Ui) {
        let bg = if self.bg_dark {
            egui::Color32::from_rgb(25, 30, 38)
        } else {
            egui::Color32::from_rgb(200, 200, 210)
        };

        let available = ui.available_size();
        let (response, painter) = ui.allocate_painter(available, egui::Sense::hover());
        painter.rect_filled(response.rect, 0.0, bg);

        // render main avatar (large, center)
        let center = response.rect.center();
        render_avatar_face(
            &painter,
            egui::pos2(center.x, center.y - 30.0),
            self.avatar_size,
            &self.smoothed,
            &self.smoothed_rot,
        );

        // label
        painter.text(
            egui::pos2(center.x, center.y + self.avatar_size * 0.5 + 20.0),
            egui::Align2::CENTER_TOP,
            "Player 1",
            egui::FontId::proportional(14.0),
            egui::Color32::from_rgb(180, 180, 190),
        );

        // render a row of smaller avatars below (simulating a poker table)
        let small_size = self.avatar_size * 0.45;
        let bottom_y = response.rect.max.y - small_size - 30.0;
        let count = 5;
        let spacing = available.x / (count as f32 + 1.0);

        for i in 0..count {
            let x = spacing * (i as f32 + 1.0) + response.rect.min.x;
            // slightly varied expressions for the other players
            let mut varied = self.smoothed.clone();
            let offset = (i as f32 + 1.0) * 0.1;
            // shift some expressions
            if i < varied.len() {
                varied[blend_shape::MOUTH_SMILE_LEFT] =
                    (varied[blend_shape::MOUTH_SMILE_LEFT] + offset * 0.3).clamp(0.0, 1.0);
                varied[blend_shape::BROW_INNER_UP] =
                    (varied[blend_shape::BROW_INNER_UP] + offset * 0.2).clamp(0.0, 1.0);
            }

            let mut rot = self.smoothed_rot;
            rot[1] += (i as f32 - 2.0) * 0.1; // vary yaw

            render_avatar_face(&painter, egui::pos2(x, bottom_y), small_size, &varied, &rot);

            painter.text(
                egui::pos2(x, bottom_y + small_size * 0.5 + 8.0),
                egui::Align2::CENTER_TOP,
                format!("P{}", i + 2),
                egui::FontId::proportional(11.0),
                egui::Color32::from_rgb(140, 140, 150),
            );
        }
    }
}

/// render an avatar face (standalone version — same logic as poker-client/src/avatar.rs)
fn render_avatar_face(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    shapes: &[f32],
    rotation: &[f32; 3],
) {
    let s = shapes;
    let half = size * 0.5;

    // head offset from rotation
    let yaw_offset = rotation[1] * size * 0.3;
    let pitch_offset = rotation[0] * size * 0.2;
    let center = egui::pos2(center.x + yaw_offset, center.y + pitch_offset);

    // face circle
    let face_color = egui::Color32::from_rgb(60, 55, 70);
    let face_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(100, 95, 110));
    painter.circle(center, half, face_color, face_stroke);

    // === eyes ===
    let eye_y = center.y - half * 0.15;
    let eye_spacing = half * 0.35;
    let eye_size = half * 0.18;

    let blink_l = get(s, blend_shape::EYE_BLINK_LEFT);
    let blink_r = get(s, blend_shape::EYE_BLINK_RIGHT);
    let wide_l = get(s, blend_shape::EYE_WIDE_LEFT);
    let wide_r = get(s, blend_shape::EYE_WIDE_RIGHT);
    let squint_l = get(s, blend_shape::EYE_SQUINT_LEFT);
    let squint_r = get(s, blend_shape::EYE_SQUINT_RIGHT);

    // left eye
    let l_open = (1.0 - blink_l + wide_l * 0.5 - squint_l * 0.3).clamp(0.05, 1.5);
    draw_eye(painter,
        egui::pos2(center.x - eye_spacing, eye_y), eye_size, l_open,
        get(s, blend_shape::EYE_LOOK_IN_LEFT),
        get(s, blend_shape::EYE_LOOK_OUT_LEFT),
        get(s, blend_shape::EYE_LOOK_UP_LEFT),
        get(s, blend_shape::EYE_LOOK_DOWN_LEFT),
    );

    // right eye
    let r_open = (1.0 - blink_r + wide_r * 0.5 - squint_r * 0.3).clamp(0.05, 1.5);
    draw_eye(painter,
        egui::pos2(center.x + eye_spacing, eye_y), eye_size, r_open,
        get(s, blend_shape::EYE_LOOK_OUT_RIGHT),
        get(s, blend_shape::EYE_LOOK_IN_RIGHT),
        get(s, blend_shape::EYE_LOOK_UP_RIGHT),
        get(s, blend_shape::EYE_LOOK_DOWN_RIGHT),
    );

    // === eyebrows ===
    let brow_y = eye_y - half * 0.22;
    let brow_down_l = get(s, blend_shape::BROW_DOWN_LEFT);
    let brow_down_r = get(s, blend_shape::BROW_DOWN_RIGHT);
    let brow_inner_up = get(s, blend_shape::BROW_INNER_UP);
    let brow_outer_up_l = get(s, blend_shape::BROW_OUTER_UP_LEFT);
    let brow_outer_up_r = get(s, blend_shape::BROW_OUTER_UP_RIGHT);

    let brow_stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(180, 170, 190));

    // left brow
    let li_y = brow_y - brow_inner_up * half * 0.15 + brow_down_l * half * 0.1;
    let lo_y = brow_y - brow_outer_up_l * half * 0.15 + brow_down_l * half * 0.08;
    painter.line_segment(
        [egui::pos2(center.x - eye_spacing * 0.5, li_y),
         egui::pos2(center.x - eye_spacing * 1.4, lo_y)],
        brow_stroke,
    );

    // right brow
    let ri_y = brow_y - brow_inner_up * half * 0.15 + brow_down_r * half * 0.1;
    let ro_y = brow_y - brow_outer_up_r * half * 0.15 + brow_down_r * half * 0.08;
    painter.line_segment(
        [egui::pos2(center.x + eye_spacing * 0.5, ri_y),
         egui::pos2(center.x + eye_spacing * 1.4, ro_y)],
        brow_stroke,
    );

    // === mouth ===
    let mouth_y = center.y + half * 0.35;
    let jaw_open = get(s, blend_shape::JAW_OPEN);
    let smile_l = get(s, blend_shape::MOUTH_SMILE_LEFT);
    let smile_r = get(s, blend_shape::MOUTH_SMILE_RIGHT);
    let frown_l = get(s, blend_shape::MOUTH_FROWN_LEFT);
    let frown_r = get(s, blend_shape::MOUTH_FROWN_RIGHT);
    let pucker = get(s, blend_shape::MOUTH_PUCKER);

    let mouth_width = half * 0.5 * (1.0 - pucker * 0.5);
    let smile_avg = (smile_l + smile_r) * 0.5;
    let frown_avg = (frown_l + frown_r) * 0.5;
    let mouth_curve = (smile_avg - frown_avg) * half * 0.15;

    let mouth_color = egui::Color32::from_rgb(150, 80, 80);

    if jaw_open > 0.1 {
        let open_height = half * 0.08 + jaw_open * half * 0.2;
        let mouth_rect = egui::Rect::from_center_size(
            egui::pos2(center.x, mouth_y - mouth_curve * 0.5),
            egui::vec2(mouth_width * 2.0, open_height * 2.0),
        );
        painter.rect(mouth_rect, egui::Rounding::same(open_height), mouth_color,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 60, 60)));
    } else {
        let mouth_stroke = egui::Stroke::new(2.0, mouth_color);
        let left = egui::pos2(center.x - mouth_width, mouth_y + mouth_curve);
        let mid = egui::pos2(center.x, mouth_y - mouth_curve);
        let right = egui::pos2(center.x + mouth_width, mouth_y + mouth_curve);
        let q1 = egui::pos2((left.x + mid.x) * 0.5, (left.y + mid.y) * 0.5);
        let q2 = egui::pos2((mid.x + right.x) * 0.5, (mid.y + right.y) * 0.5);
        painter.line_segment([left, q1], mouth_stroke);
        painter.line_segment([q1, mid], mouth_stroke);
        painter.line_segment([mid, q2], mouth_stroke);
        painter.line_segment([q2, right], mouth_stroke);
    }

    // === cheek blush on smile ===
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

    // === nose hint ===
    let nose_sneer_l = get(s, blend_shape::NOSE_SNEER_LEFT);
    let nose_sneer_r = get(s, blend_shape::NOSE_SNEER_RIGHT);
    if nose_sneer_l > 0.1 || nose_sneer_r > 0.1 {
        let sneer_offset = (nose_sneer_l - nose_sneer_r) * half * 0.05;
        let nose_y = center.y + half * 0.12;
        let nose_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 110, 130));
        // left nostril flare
        painter.line_segment([
            egui::pos2(center.x - half * 0.08 + sneer_offset, nose_y),
            egui::pos2(center.x - half * 0.12 - nose_sneer_l * half * 0.05, nose_y + half * 0.03),
        ], nose_stroke);
        // right nostril flare
        painter.line_segment([
            egui::pos2(center.x + half * 0.08 + sneer_offset, nose_y),
            egui::pos2(center.x + half * 0.12 + nose_sneer_r * half * 0.05, nose_y + half * 0.03),
        ], nose_stroke);
    }

    // === tongue ===
    let tongue = get(s, blend_shape::TONGUE_OUT);
    if tongue > 0.2 && jaw_open > 0.15 {
        let tongue_y = mouth_y + jaw_open * half * 0.15;
        let tongue_width = half * 0.15;
        let tongue_height = tongue * half * 0.12;
        painter.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(center.x, tongue_y),
                egui::vec2(tongue_width * 2.0, tongue_height * 2.0),
            ),
            egui::Rounding::same(tongue_width),
            egui::Color32::from_rgb(200, 100, 100),
        );
    }

    // === cheek puff ===
    let puff = get(s, blend_shape::CHEEK_PUFF);
    if puff > 0.1 {
        let puff_size = half * 0.1 * puff;
        let puff_alpha = (puff * 60.0) as u8;
        painter.circle_filled(
            egui::pos2(center.x - half * 0.55, center.y + half * 0.15),
            puff_size + half * 0.08,
            egui::Color32::from_rgba_premultiplied(80, 70, 90, puff_alpha),
        );
        painter.circle_filled(
            egui::pos2(center.x + half * 0.55, center.y + half * 0.15),
            puff_size + half * 0.08,
            egui::Color32::from_rgba_premultiplied(80, 70, 90, puff_alpha),
        );
    }
}

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

    let eye_rect = egui::Rect::from_center_size(
        center,
        egui::vec2(size * 1.8, eye_height * 2.0),
    );
    painter.rect(eye_rect, egui::Rounding::same(eye_height), eye_white, eye_stroke);

    if openness > 0.1 {
        let gaze_x = (look_right - look_left) * size * 0.4;
        let gaze_y = (look_down - look_up) * size * 0.3;
        let iris_center = egui::pos2(center.x + gaze_x, center.y + gaze_y);
        let iris_size = size * 0.55;
        let pupil_size = iris_size * 0.55;

        painter.circle_filled(iris_center, iris_size, egui::Color32::from_rgb(80, 100, 140));
        painter.circle_filled(iris_center, pupil_size, egui::Color32::from_rgb(20, 20, 30));
        painter.circle_filled(
            egui::pos2(iris_center.x - pupil_size * 0.3, iris_center.y - pupil_size * 0.3),
            pupil_size * 0.25,
            egui::Color32::from_rgba_premultiplied(255, 255, 255, 180),
        );
    }
}

fn get(shapes: &[f32], idx: usize) -> f32 {
    shapes.get(idx).copied().unwrap_or(0.0)
}

// common shapes for the default slider view
const COMMON_SHAPES: [(usize, &str); 20] = [
    (blend_shape::EYE_BLINK_LEFT, "blink L"),
    (blend_shape::EYE_BLINK_RIGHT, "blink R"),
    (blend_shape::EYE_WIDE_LEFT, "wide L"),
    (blend_shape::EYE_WIDE_RIGHT, "wide R"),
    (blend_shape::EYE_SQUINT_LEFT, "squint L"),
    (blend_shape::EYE_SQUINT_RIGHT, "squint R"),
    (blend_shape::EYE_LOOK_UP_LEFT, "look up"),
    (blend_shape::EYE_LOOK_DOWN_LEFT, "look down"),
    (blend_shape::EYE_LOOK_IN_LEFT, "look in L"),
    (blend_shape::EYE_LOOK_OUT_LEFT, "look out L"),
    (blend_shape::BROW_DOWN_LEFT, "brow down L"),
    (blend_shape::BROW_DOWN_RIGHT, "brow down R"),
    (blend_shape::BROW_INNER_UP, "brow inner up"),
    (blend_shape::BROW_OUTER_UP_LEFT, "brow outer L"),
    (blend_shape::JAW_OPEN, "jaw open"),
    (blend_shape::MOUTH_SMILE_LEFT, "smile L"),
    (blend_shape::MOUTH_SMILE_RIGHT, "smile R"),
    (blend_shape::MOUTH_FROWN_LEFT, "frown L"),
    (blend_shape::MOUTH_PUCKER, "pucker"),
    (blend_shape::CHEEK_PUFF, "cheek puff"),
];

const ALL_SHAPES: [(usize, &str); 52] = [
    (0, "eyeBlinkLeft"), (1, "eyeBlinkRight"),
    (2, "eyeLookDownLeft"), (3, "eyeLookDownRight"),
    (4, "eyeLookInLeft"), (5, "eyeLookInRight"),
    (6, "eyeLookOutLeft"), (7, "eyeLookOutRight"),
    (8, "eyeLookUpLeft"), (9, "eyeLookUpRight"),
    (10, "eyeSquintLeft"), (11, "eyeSquintRight"),
    (12, "eyeWideLeft"), (13, "eyeWideRight"),
    (14, "browDownLeft"), (15, "browDownRight"),
    (16, "browInnerUp"),
    (17, "browOuterUpLeft"), (18, "browOuterUpRight"),
    (19, "jawForward"), (20, "jawLeft"), (21, "jawOpen"), (22, "jawRight"),
    (23, "mouthClose"),
    (24, "mouthDimpleLeft"), (25, "mouthDimpleRight"),
    (26, "mouthFrownLeft"), (27, "mouthFrownRight"),
    (28, "mouthFunnel"), (29, "mouthLeft"),
    (30, "mouthLowerDownLeft"), (31, "mouthLowerDownRight"),
    (32, "mouthPressLeft"), (33, "mouthPressRight"),
    (34, "mouthPucker"), (35, "mouthRight"),
    (36, "mouthRollLower"), (37, "mouthRollUpper"),
    (38, "mouthShrugLower"), (39, "mouthShrugUpper"),
    (40, "mouthSmileLeft"), (41, "mouthSmileRight"),
    (42, "mouthStretchLeft"), (43, "mouthStretchRight"),
    (44, "mouthUpperUpLeft"), (45, "mouthUpperUpRight"),
    (46, "cheekPuff"),
    (47, "cheekSquintLeft"), (48, "cheekSquintRight"),
    (49, "noseSneerLeft"), (50, "noseSneerRight"),
    (51, "tongueOut"),
];
