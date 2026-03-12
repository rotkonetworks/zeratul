//! voice chat for poker table
//!
//! WebRTC for media transport, signaling over iroh QUIC.
//! supports push-to-talk, per-player mute/volume, voice activity detection.
//!
//! native: cpal audio capture → opus encode → WebRTC data channel
//! wasm: browser getUserMedia → WebRTC native

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use std::collections::HashMap;
use tokio::sync::mpsc;

use poker_p2p::{VoiceData, VoiceState as P2PVoiceState, VoiceStateType};
use poker_p2p::{RtcEvent, RtcManager, AudioProcessor, IceConnectionState, MediaKind};

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
            .init_resource::<RtcVoiceState>()
            .add_event::<VoiceEvent>()
            .add_systems(
                Update,
                (
                    handle_ptt_input,
                    poll_rtc_events,
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
    /// WebRTC peer connected
    RtcConnected { seat: u8 },
    /// WebRTC peer disconnected
    RtcDisconnected { seat: u8 },
    /// start voice session with all table players
    StartVoiceSession { seats: Vec<u8> },
    /// stop voice session
    StopVoiceSession,
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
    /// WebRTC connection state
    pub rtc_state: IceConnectionState,
}

impl Default for PlayerVoiceState {
    fn default() -> Self {
        Self {
            speaking: false,
            activity_level: 0.0,
            muted_by_us: false,
            volume: 1.0,
            last_packet: 0.0,
            rtc_state: IceConnectionState::New,
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

    /// count of peers with active WebRTC connections
    pub fn connected_peers(&self) -> usize {
        self.players.values()
            .filter(|p| matches!(p.rtc_state, IceConnectionState::Connected | IceConnectionState::Completed))
            .count()
    }
}

/// WebRTC voice state — bridges RtcManager to the Bevy ECS voice system
#[derive(Resource)]
pub struct RtcVoiceState {
    /// event receiver from RtcManager
    pub event_rx: Option<mpsc::UnboundedReceiver<RtcEvent>>,
    /// signal sender to RtcManager (for forwarding signaling messages)
    pub signal_tx: Option<mpsc::UnboundedSender<poker_p2p::protocol::Message>>,
    /// audio processor for capture/playback mixing
    pub audio_processor: AudioProcessor,
    /// outgoing signaling messages to send via iroh QUIC
    pub outgoing_signals: Vec<poker_p2p::protocol::Message>,
    /// is a voice session active
    pub session_active: bool,
}

impl Default for RtcVoiceState {
    fn default() -> Self {
        Self {
            event_rx: None,
            signal_tx: None,
            audio_processor: AudioProcessor::new(SAMPLE_RATE, SAMPLE_RATE),
            outgoing_signals: Vec::new(),
            session_active: false,
        }
    }
}

impl RtcVoiceState {
    /// initialize WebRTC for a table voice session
    pub fn start_session(&mut self, local_seat: u8, seats: &[u8]) {
        let (mut manager, event_rx, signal_tx) = RtcManager::new(local_seat);

        // request voice connections with all other players
        manager.request_all(seats, MediaKind::Voice);

        self.event_rx = Some(event_rx);
        self.signal_tx = Some(signal_tx);
        self.session_active = true;

        info!("voice: started WebRTC session for seat {} with {} peers",
            local_seat, seats.len().saturating_sub(1));
    }

    /// stop the voice session
    pub fn stop_session(&mut self) {
        self.event_rx = None;
        self.signal_tx = None;
        self.session_active = false;
        self.outgoing_signals.clear();
        info!("voice: stopped WebRTC session");
    }

    /// forward a signaling message from the network to the RtcManager
    pub fn forward_signal(&self, msg: poker_p2p::protocol::Message) {
        if let Some(ref tx) = self.signal_tx {
            let _ = tx.send(msg);
        }
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
    /// auto-join voice when sitting at table
    pub auto_join: bool,
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
            auto_join: true,
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

/// poll RtcManager events and convert to Bevy voice events
fn poll_rtc_events(
    mut rtc_state: ResMut<RtcVoiceState>,
    mut voice_state: ResMut<VoiceState>,
    mut events: EventWriter<VoiceEvent>,
) {
    let Some(ref mut rx) = rtc_state.event_rx else {
        return;
    };

    while let Ok(event) = rx.try_recv() {
        match event {
            RtcEvent::Connected { seat } => {
                let player = voice_state.players.entry(seat).or_default();
                player.rtc_state = IceConnectionState::Connected;
                events.send(VoiceEvent::RtcConnected { seat });
                info!("voice: WebRTC connected to seat {}", seat);
            }
            RtcEvent::Disconnected { seat } => {
                if let Some(player) = voice_state.players.get_mut(&seat) {
                    player.rtc_state = IceConnectionState::Disconnected;
                    player.speaking = false;
                    player.activity_level = 0.0;
                }
                events.send(VoiceEvent::RtcDisconnected { seat });
                info!("voice: WebRTC disconnected from seat {}", seat);
            }
            RtcEvent::AudioReceived { seat, frame, vad } => {
                if !voice_state.deafened {
                    events.send(VoiceEvent::ReceivedVoice(VoiceData {
                        seat,
                        seq: 0,
                        frame,
                        vad,
                    }));
                }
            }
            RtcEvent::IceStateChanged { seat, state } => {
                if let Some(player) = voice_state.players.get_mut(&seat) {
                    player.rtc_state = state;
                }
            }
            RtcEvent::Signal(msg) => {
                // queue for sending via iroh QUIC
                rtc_state.outgoing_signals.push(msg);
            }
            _ => {}
        }
    }
}

/// process voice events
fn process_voice_events(
    mut events: EventReader<VoiceEvent>,
    mut voice_state: ResMut<VoiceState>,
    mut rtc_state: ResMut<RtcVoiceState>,
) {
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
            VoiceEvent::StartVoiceSession { seats } => {
                rtc_state.start_session(voice_state.local_seat, seats);
            }
            VoiceEvent::StopVoiceSession => {
                rtc_state.stop_session();
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
fn render_voice_indicators(
    mut contexts: EguiContexts,
    voice_state: Res<VoiceState>,
    rtc_state: Res<RtcVoiceState>,
) {
    if !voice_state.enabled {
        return;
    }

    let ctx = contexts.ctx_mut();

    // render small voice status in corner
    egui::Area::new(egui::Id::new("voice_status"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-10.0, -10.0))
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_premultiplied(20, 20, 20, 200))
                .rounding(egui::Rounding::same(6.0))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // WebRTC connection indicator
                        let connected = voice_state.connected_peers();
                        if rtc_state.session_active {
                            let conn_color = if connected > 0 {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::YELLOW
                            };
                            ui.label(egui::RichText::new(
                                format!("[{}]", connected)
                            ).small().color(conn_color));
                        }

                        // mic status
                        let mic_icon = if voice_state.muted {
                            "MIC OFF"
                        } else if voice_state.ptt_active {
                            "MIC"
                        } else {
                            "mic"
                        };

                        let mic_color = if voice_state.muted {
                            egui::Color32::RED
                        } else if voice_state.ptt_active {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::GRAY
                        };

                        ui.label(egui::RichText::new(mic_icon).small().color(mic_color));

                        // headphone status
                        if voice_state.deafened {
                            ui.label(egui::RichText::new("DEAF").small().color(egui::Color32::RED));
                        }

                        // show who's speaking
                        let speaking: Vec<_> = voice_state
                            .players
                            .iter()
                            .filter(|(_, p)| p.speaking)
                            .collect();

                        if !speaking.is_empty() {
                            ui.separator();
                            for (seat, player) in speaking.iter().take(3) {
                                // color intensity based on activity level
                                let g = (100.0 + 155.0 * player.activity_level) as u8;
                                ui.label(
                                    egui::RichText::new(format!("P{}", seat))
                                        .small()
                                        .color(egui::Color32::from_rgb(0, g, 0)),
                                );
                            }
                            if speaking.len() > 3 {
                                ui.label(egui::RichText::new(
                                    format!("+{}", speaking.len() - 3)
                                ).small());
                            }
                        }
                    });
                });
        });
}

/// render voice settings panel
fn render_voice_settings(
    mut contexts: EguiContexts,
    mut voice_state: ResMut<VoiceState>,
    mut settings: ResMut<VoiceSettings>,
    rtc_state: Res<RtcVoiceState>,
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

            // WebRTC status
            if rtc_state.session_active {
                let connected = voice_state.connected_peers();
                let total = voice_state.players.len();
                ui.horizontal(|ui| {
                    ui.label("WebRTC:");
                    ui.label(egui::RichText::new(
                        format!("{}/{} connected", connected, total)
                    ).color(if connected > 0 { egui::Color32::GREEN } else { egui::Color32::YELLOW }));
                });
            } else {
                ui.label(egui::RichText::new("WebRTC: inactive").color(egui::Color32::GRAY));
            }

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

            ui.checkbox(&mut settings.auto_join, "auto-join voice on table sit");

            ui.separator();

            ui.label("player volumes:");
            let players: Vec<_> = voice_state.players.keys().cloned().collect();
            for seat in players {
                if let Some(player) = voice_state.players.get_mut(&seat) {
                    ui.horizontal(|ui| {
                        // connection state indicator
                        let state_icon = match player.rtc_state {
                            IceConnectionState::Connected | IceConnectionState::Completed => {
                                egui::RichText::new("*").color(egui::Color32::GREEN)
                            }
                            IceConnectionState::Checking => {
                                egui::RichText::new("~").color(egui::Color32::YELLOW)
                            }
                            IceConnectionState::Disconnected | IceConnectionState::Failed => {
                                egui::RichText::new("x").color(egui::Color32::RED)
                            }
                            _ => egui::RichText::new("-").color(egui::Color32::GRAY),
                        };
                        ui.label(state_icon);

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

    #[test]
    fn test_connected_peers_count() {
        let mut state = VoiceState::default();

        state.players.insert(1, PlayerVoiceState {
            rtc_state: IceConnectionState::Connected,
            ..Default::default()
        });
        state.players.insert(2, PlayerVoiceState {
            rtc_state: IceConnectionState::Checking,
            ..Default::default()
        });
        state.players.insert(3, PlayerVoiceState {
            rtc_state: IceConnectionState::Completed,
            ..Default::default()
        });

        assert_eq!(state.connected_peers(), 2); // seats 1 and 3
    }
}
