//! voice chat for poker table
//!
//! opus codec over iroh QUIC, push-to-talk, per-player mute/volume

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use std::collections::HashMap;

use poker_p2p::{VoiceData, VoiceState as P2PVoiceState, VoiceStateType};

/// opus frame duration in milliseconds
const FRAME_DURATION_MS: u32 = 20;
/// sample rate
const SAMPLE_RATE: u32 = 48000;
/// mono audio
const CHANNELS: u8 = 1;
/// jitter buffer size (frames)
const JITTER_BUFFER_SIZE: usize = 5;

pub struct VoicePlugin;

impl Plugin for VoicePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VoiceSettings>()
            .init_resource::<VoiceState>()
            .add_event::<VoiceEvent>()
            .add_systems(
                Update,
                (
                    handle_ptt_input,
                    process_voice_events,
                    render_voice_indicators,
                    render_voice_settings,
                )
                    .chain(),
            );
    }
}

/// voice events for network send/receive
#[derive(Event, Clone, Debug)]
pub enum VoiceEvent {
    /// local audio captured (to be sent)
    LocalAudio { frame: Vec<u8>, vad: bool },
    /// received voice data from network
    ReceivedVoice(VoiceData),
    /// voice state changed (received from network)
    StateChanged(P2PVoiceState),
    /// local PTT pressed
    PttPressed,
    /// local PTT released
    PttReleased,
    /// toggle mute
    ToggleMute,
    /// toggle deafen
    ToggleDeafen,
}

/// per-player voice state for rendering indicators
#[derive(Clone, Debug)]
pub struct PlayerVoiceState {
    /// is this player currently speaking
    pub speaking: bool,
    /// voice activity level (0.0 - 1.0) for indicator
    pub activity_level: f32,
    /// is muted (by us, not receiving their audio)
    pub muted_by_us: bool,
    /// volume multiplier (0.0 - 2.0)
    pub volume: f32,
    /// last voice packet time
    pub last_packet: f64,
}

impl Default for PlayerVoiceState {
    fn default() -> Self {
        Self {
            speaking: false,
            activity_level: 0.0,
            muted_by_us: false,
            volume: 1.0,
            last_packet: 0.0,
        }
    }
}

/// voice state resource
#[derive(Resource, Default)]
pub struct VoiceState {
    /// is voice enabled
    pub enabled: bool,
    /// is locally muted (not sending)
    pub muted: bool,
    /// is deafened (not receiving)
    pub deafened: bool,
    /// is PTT currently pressed
    pub ptt_active: bool,
    /// per-player states
    pub players: HashMap<u8, PlayerVoiceState>,
    /// local seat
    pub local_seat: u8,
    /// current sequence number (for outgoing packets)
    pub seq: u32,
    /// show settings panel
    pub show_settings: bool,
}

impl VoiceState {
    /// update player speaking state
    pub fn set_speaking(&mut self, seat: u8, speaking: bool, level: f32) {
        let state = self.players.entry(seat).or_default();
        state.speaking = speaking;
        state.activity_level = level;
        if speaking {
            state.last_packet = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
        }
    }

    /// check if we should send voice (PTT mode or voice activity)
    pub fn should_transmit(&self) -> bool {
        self.enabled && !self.muted && self.ptt_active
    }

    /// get next sequence number
    pub fn next_seq(&mut self) -> u32 {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        seq
    }
}

/// voice settings resource
#[derive(Resource)]
pub struct VoiceSettings {
    /// master input volume (0.0 - 2.0)
    pub input_volume: f32,
    /// master output volume (0.0 - 2.0)
    pub output_volume: f32,
    /// PTT key
    pub ptt_key: KeyCode,
    /// alternative PTT key (mouse button)
    pub ptt_mouse: Option<MouseButton>,
    /// voice activity detection threshold (0.0 - 1.0)
    pub vad_threshold: f32,
    /// use voice activity instead of PTT
    pub use_vad: bool,
    /// noise gate threshold
    pub noise_gate: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            input_volume: 1.0,
            output_volume: 1.0,
            ptt_key: KeyCode::Space,
            ptt_mouse: Some(MouseButton::Middle),
            vad_threshold: 0.02,
            use_vad: false,
            noise_gate: 0.01,
        }
    }
}

/// handle push-to-talk input
fn handle_ptt_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    settings: Res<VoiceSettings>,
    mut voice_state: ResMut<VoiceState>,
    mut events: EventWriter<VoiceEvent>,
) {
    if !voice_state.enabled {
        return;
    }

    let ptt_pressed = keyboard.pressed(settings.ptt_key)
        || settings
            .ptt_mouse
            .map(|b| mouse.pressed(b))
            .unwrap_or(false);

    if ptt_pressed && !voice_state.ptt_active {
        voice_state.ptt_active = true;
        events.send(VoiceEvent::PttPressed);
    } else if !ptt_pressed && voice_state.ptt_active {
        voice_state.ptt_active = false;
        events.send(VoiceEvent::PttReleased);
    }

    // toggle mute with M
    if keyboard.just_pressed(KeyCode::KeyM) {
        events.send(VoiceEvent::ToggleMute);
    }

    // toggle deafen with D (when not typing)
    // note: should check if chat input is focused
}

/// process voice events
fn process_voice_events(mut events: EventReader<VoiceEvent>, mut voice_state: ResMut<VoiceState>) {
    for event in events.read() {
        match event {
            VoiceEvent::ReceivedVoice(data) => {
                if !voice_state.deafened {
                    let player = voice_state.players.entry(data.seat).or_default();
                    if !player.muted_by_us {
                        player.speaking = data.vad;
                        player.activity_level = if data.vad { 0.8 } else { 0.0 };
                        player.last_packet = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs_f64();
                    }
                }
            }
            VoiceEvent::StateChanged(state) => {
                let player = voice_state.players.entry(state.seat).or_default();
                match state.state {
                    VoiceStateType::Speaking => player.speaking = true,
                    VoiceStateType::Silent => player.speaking = false,
                    VoiceStateType::Muted => player.speaking = false,
                    VoiceStateType::Disconnected => {
                        player.speaking = false;
                        player.activity_level = 0.0;
                    }
                    _ => {}
                }
            }
            VoiceEvent::ToggleMute => {
                voice_state.muted = !voice_state.muted;
            }
            VoiceEvent::ToggleDeafen => {
                voice_state.deafened = !voice_state.deafened;
            }
            _ => {}
        }
    }

    // decay activity levels for players who stopped speaking
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    for (_, player) in voice_state.players.iter_mut() {
        if now - player.last_packet > 0.3 {
            player.speaking = false;
            player.activity_level *= 0.9; // decay
        }
    }
}

/// render voice activity indicators on player avatars
fn render_voice_indicators(mut contexts: EguiContexts, voice_state: Res<VoiceState>) {
    if !voice_state.enabled {
        return;
    }

    let ctx = contexts.ctx_mut();

    // render small voice status in corner
    egui::Area::new(egui::Id::new("voice_status"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-10.0, -10.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // mic status
                let mic_icon = if voice_state.muted {
                    "\u{1F507}" // muted speaker
                } else if voice_state.ptt_active {
                    "\u{1F3A4}" // microphone
                } else {
                    "\u{1F3A4}" // microphone (dim)
                };

                let mic_color = if voice_state.muted {
                    egui::Color32::RED
                } else if voice_state.ptt_active {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::GRAY
                };

                ui.label(egui::RichText::new(mic_icon).color(mic_color));

                // headphone status
                if voice_state.deafened {
                    ui.label(egui::RichText::new("\u{1F508}").color(egui::Color32::RED));
                }

                // show who's speaking
                let speaking: Vec<_> = voice_state
                    .players
                    .iter()
                    .filter(|(_, p)| p.speaking)
                    .collect();

                if !speaking.is_empty() {
                    ui.separator();
                    for (seat, _) in speaking.iter().take(3) {
                        ui.label(
                            egui::RichText::new(format!("P{}", seat))
                                .small()
                                .color(egui::Color32::GREEN),
                        );
                    }
                    if speaking.len() > 3 {
                        ui.label(egui::RichText::new(format!("+{}", speaking.len() - 3)).small());
                    }
                }
            });
        });
}

/// render voice settings panel
fn render_voice_settings(
    mut contexts: EguiContexts,
    mut voice_state: ResMut<VoiceState>,
    mut settings: ResMut<VoiceSettings>,
) {
    if !voice_state.show_settings {
        return;
    }

    let ctx = contexts.ctx_mut();

    egui::Window::new("voice settings")
        .collapsible(true)
        .resizable(false)
        .show(ctx, |ui| {
            ui.checkbox(&mut voice_state.enabled, "enable voice chat");

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("input volume:");
                ui.add(egui::Slider::new(&mut settings.input_volume, 0.0..=2.0));
            });

            ui.horizontal(|ui| {
                ui.label("output volume:");
                ui.add(egui::Slider::new(&mut settings.output_volume, 0.0..=2.0));
            });

            ui.separator();

            ui.checkbox(&mut settings.use_vad, "voice activation (instead of PTT)");

            if settings.use_vad {
                ui.horizontal(|ui| {
                    ui.label("sensitivity:");
                    ui.add(egui::Slider::new(&mut settings.vad_threshold, 0.0..=0.1));
                });
            } else {
                ui.label(format!("PTT key: {:?}", settings.ptt_key));
            }

            ui.separator();

            ui.label("player volumes:");
            let players: Vec<_> = voice_state.players.keys().cloned().collect();
            for seat in players {
                if let Some(player) = voice_state.players.get_mut(&seat) {
                    ui.horizontal(|ui| {
                        ui.label(format!("player {}:", seat));
                        ui.add(egui::Slider::new(&mut player.volume, 0.0..=2.0).show_value(false));
                        if ui.checkbox(&mut player.muted_by_us, "mute").changed() {
                            // mute toggled
                        }
                    });
                }
            }

            ui.separator();

            if ui.button("close").clicked() {
                voice_state.show_settings = false;
            }
        });
}

/// jitter buffer for smooth audio playback
#[derive(Default)]
pub struct JitterBuffer {
    /// buffered frames indexed by sequence number
    frames: HashMap<u32, Vec<u8>>,
    /// next expected sequence number
    next_seq: u32,
    /// frames ready to play
    ready: Vec<Vec<u8>>,
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// add a received frame
    pub fn push(&mut self, seq: u32, frame: Vec<u8>) {
        self.frames.insert(seq, frame);
    }

    /// get next frame to play (or silence if missing)
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        if let Some(frame) = self.frames.remove(&self.next_seq) {
            self.next_seq = self.next_seq.wrapping_add(1);
            Some(frame)
        } else if self.frames.len() > JITTER_BUFFER_SIZE {
            // skip to catch up
            if let Some(&min_seq) = self.frames.keys().min() {
                self.next_seq = min_seq;
                return self.frames.remove(&min_seq);
            }
            None
        } else {
            None
        }
    }

    /// clear buffer
    pub fn clear(&mut self) {
        self.frames.clear();
        self.ready.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jitter_buffer() {
        let mut jb = JitterBuffer::new();

        // push out of order
        jb.push(2, vec![2]);
        jb.push(0, vec![0]);
        jb.push(1, vec![1]);

        // should get in order
        assert_eq!(jb.pop(), Some(vec![0]));
        assert_eq!(jb.pop(), Some(vec![1]));
        assert_eq!(jb.pop(), Some(vec![2]));
        assert_eq!(jb.pop(), None);
    }

    #[test]
    fn test_voice_state_transmit() {
        let mut state = VoiceState::default();
        state.enabled = true;
        state.muted = false;

        // not transmitting without PTT
        assert!(!state.should_transmit());

        state.ptt_active = true;
        assert!(state.should_transmit());

        state.muted = true;
        assert!(!state.should_transmit());
    }
}
