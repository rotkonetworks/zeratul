//! webrtc - peer-to-peer media transport
//!
//! manages WebRTC peer connections for voice, video, and screen sharing.
//! signaling happens over the existing iroh QUIC connection.
//!
//! architecture:
//! - iroh QUIC: game protocol (reliable, ordered)
//! - WebRTC: media streams (low latency, tolerates loss)
//!
//! for native: uses webrtc-rs crate
//! for wasm: uses browser WebRTC via web-sys (separate impl)

use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::protocol::*;

/// opus codec parameters
pub const OPUS_SAMPLE_RATE: u32 = 48000;
pub const OPUS_CHANNELS: u8 = 1; // mono
pub const OPUS_FRAME_MS: u32 = 20;
pub const OPUS_FRAME_SAMPLES: usize = (OPUS_SAMPLE_RATE * OPUS_FRAME_MS / 1000) as usize;

/// STUN servers for ICE NAT traversal
pub const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
];

/// events emitted by the WebRTC manager
#[derive(Clone, Debug)]
pub enum RtcEvent {
    /// peer connection established with a seat
    Connected { seat: u8 },
    /// peer connection lost
    Disconnected { seat: u8 },
    /// received audio frame from peer
    AudioReceived {
        seat: u8,
        /// opus-encoded audio frame
        frame: Vec<u8>,
        /// voice activity detected
        vad: bool,
    },
    /// received video frame from peer (future)
    VideoReceived {
        seat: u8,
        frame: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// ICE connection state changed
    IceStateChanged {
        seat: u8,
        state: IceConnectionState,
    },
    /// signaling message to send via iroh QUIC
    Signal(Message),
}

/// ICE connection states
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IceConnectionState {
    New,
    Checking,
    Connected,
    Completed,
    Disconnected,
    Failed,
    Closed,
}

/// per-peer WebRTC connection state
#[derive(Debug)]
pub struct PeerMediaState {
    /// their seat number
    pub seat: u8,
    /// what media they requested
    pub media: MediaKind,
    /// ICE connection state
    pub ice_state: IceConnectionState,
    /// are we the offerer (true) or answerer (false)
    pub is_offerer: bool,
    /// have we completed the SDP exchange
    pub sdp_complete: bool,
    /// pending ICE candidates (received before remote description set)
    pub pending_candidates: Vec<RtcIceCandidate>,
    /// local audio muted
    pub audio_muted: bool,
    /// local video muted
    pub video_muted: bool,
}

impl PeerMediaState {
    pub fn new(seat: u8, media: MediaKind, is_offerer: bool) -> Self {
        Self {
            seat,
            media,
            ice_state: IceConnectionState::New,
            is_offerer,
            sdp_complete: false,
            pending_candidates: Vec::new(),
            audio_muted: false,
            video_muted: true,
        }
    }
}

/// manages WebRTC peer connections for all seats at a table
pub struct RtcManager {
    /// our seat number
    pub local_seat: u8,
    /// peer states indexed by seat
    pub peers: HashMap<u8, PeerMediaState>,
    /// outgoing events (to be processed by the transport layer)
    event_tx: mpsc::UnboundedSender<RtcEvent>,
    /// incoming signaling messages (from iroh QUIC)
    signal_rx: mpsc::UnboundedReceiver<Message>,
    /// STUN server URLs
    pub stun_servers: Vec<String>,
    /// are we accepting media requests
    pub accepting_media: bool,
    /// default media kind for outgoing requests
    pub default_media: MediaKind,
}

impl RtcManager {
    /// create a new RTC manager for a table
    pub fn new(local_seat: u8) -> (Self, mpsc::UnboundedReceiver<RtcEvent>, mpsc::UnboundedSender<Message>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();

        let manager = Self {
            local_seat,
            peers: HashMap::new(),
            event_tx,
            signal_rx,
            stun_servers: DEFAULT_STUN_SERVERS.iter().map(|s| s.to_string()).collect(),
            accepting_media: true,
            default_media: MediaKind::Voice,
        };

        (manager, event_rx, signal_tx)
    }

    /// request media session with a specific peer
    pub fn request_media(&mut self, target_seat: u8, media: MediaKind) {
        let state = PeerMediaState::new(target_seat, media, true);
        self.peers.insert(target_seat, state);

        let _ = self.event_tx.send(RtcEvent::Signal(Message::RtcMediaRequest(
            RtcMediaRequest {
                seat: self.local_seat,
                media,
                dtls_fingerprint: Vec::new(), // filled by actual WebRTC impl
            },
        )));
    }

    /// request media sessions with all players at table
    pub fn request_all(&mut self, seats: &[u8], media: MediaKind) {
        for &seat in seats {
            if seat != self.local_seat {
                self.request_media(seat, media);
            }
        }
    }

    /// handle incoming signaling message from iroh QUIC
    pub fn handle_signal(&mut self, msg: Message) {
        match msg {
            Message::RtcMediaRequest(req) => self.on_media_request(req),
            Message::RtcMediaResponse(resp) => self.on_media_response(resp),
            Message::RtcOffer(offer) => self.on_sdp_offer(offer),
            Message::RtcAnswer(answer) => self.on_sdp_answer(answer),
            Message::RtcIceCandidate(candidate) => self.on_ice_candidate(candidate),
            _ => {} // ignore non-RTC messages
        }
    }

    fn on_media_request(&mut self, req: RtcMediaRequest) {
        if !self.accepting_media {
            let _ = self.event_tx.send(RtcEvent::Signal(Message::RtcMediaResponse(
                RtcMediaResponse {
                    seat: self.local_seat,
                    accepted: false,
                    reason: Some("not accepting media".into()),
                    dtls_fingerprint: Vec::new(),
                },
            )));
            return;
        }

        // accept and create peer state (we are the answerer)
        let state = PeerMediaState::new(req.seat, req.media, false);
        self.peers.insert(req.seat, state);

        let _ = self.event_tx.send(RtcEvent::Signal(Message::RtcMediaResponse(
            RtcMediaResponse {
                seat: self.local_seat,
                accepted: true,
                reason: None,
                dtls_fingerprint: Vec::new(),
            },
        )));

        // the offerer (requester) will send the SDP offer next
    }

    fn on_media_response(&mut self, resp: RtcMediaResponse) {
        if !resp.accepted {
            self.peers.remove(&resp.seat);
            tracing::info!("peer {} rejected media: {:?}", resp.seat, resp.reason);
            return;
        }

        // peer accepted, now create and send SDP offer
        // (actual SDP creation happens in platform-specific code)
        if let Some(peer) = self.peers.get_mut(&resp.seat) {
            peer.is_offerer = true;
            // signal that we need to create an offer
            let _ = self.event_tx.send(RtcEvent::IceStateChanged {
                seat: resp.seat,
                state: IceConnectionState::New,
            });
        }
    }

    fn on_sdp_offer(&mut self, offer: RtcSessionDescription) {
        if offer.target != self.local_seat && offer.target != 0 {
            return;
        }

        if let Some(peer) = self.peers.get_mut(&offer.seat) {
            // store that we received the offer, platform code will create answer
            peer.sdp_complete = false;

            // flush pending ICE candidates that arrived before the offer
            let pending = std::mem::take(&mut peer.pending_candidates);
            for candidate in pending {
                let _ = self.event_tx.send(RtcEvent::Signal(Message::RtcIceCandidate(candidate)));
            }
        }
    }

    fn on_sdp_answer(&mut self, answer: RtcSessionDescription) {
        if answer.target != self.local_seat && answer.target != 0 {
            return;
        }

        if let Some(peer) = self.peers.get_mut(&answer.seat) {
            peer.sdp_complete = true;

            // flush pending ICE candidates
            let pending = std::mem::take(&mut peer.pending_candidates);
            for candidate in pending {
                let _ = self.event_tx.send(RtcEvent::Signal(Message::RtcIceCandidate(candidate)));
            }
        }
    }

    fn on_ice_candidate(&mut self, candidate: RtcIceCandidate) {
        if candidate.target != self.local_seat && candidate.target != 0 {
            return;
        }

        if let Some(peer) = self.peers.get_mut(&candidate.seat) {
            if !peer.sdp_complete {
                // buffer candidates until remote description is set
                peer.pending_candidates.push(candidate);
            }
            // platform code will add the candidate to the peer connection
        }
    }

    /// update ICE state for a peer (called by platform-specific code)
    pub fn set_ice_state(&mut self, seat: u8, state: IceConnectionState) {
        if let Some(peer) = self.peers.get_mut(&seat) {
            peer.ice_state = state;
        }
        let _ = self.event_tx.send(RtcEvent::IceStateChanged { seat, state });

        match state {
            IceConnectionState::Connected | IceConnectionState::Completed => {
                let _ = self.event_tx.send(RtcEvent::Connected { seat });
            }
            IceConnectionState::Disconnected | IceConnectionState::Failed | IceConnectionState::Closed => {
                let _ = self.event_tx.send(RtcEvent::Disconnected { seat });
            }
            _ => {}
        }
    }

    /// emit received audio event (called by platform-specific code)
    pub fn on_audio_received(&self, seat: u8, frame: Vec<u8>, vad: bool) {
        let _ = self.event_tx.send(RtcEvent::AudioReceived { seat, frame, vad });
    }

    /// send audio frame to a peer (returns opus bytes to be sent via WebRTC track)
    pub fn send_audio(&self, _frame: &[u8]) -> Option<Vec<u8>> {
        // platform-specific code handles actual sending via WebRTC data channel or RTP
        // this is a pass-through that lets the manager track state
        Some(_frame.to_vec())
    }

    /// disconnect a specific peer
    pub fn disconnect_peer(&mut self, seat: u8) {
        self.peers.remove(&seat);
        let _ = self.event_tx.send(RtcEvent::Disconnected { seat });
    }

    /// disconnect all peers
    pub fn disconnect_all(&mut self) {
        let seats: Vec<u8> = self.peers.keys().copied().collect();
        for seat in seats {
            self.disconnect_peer(seat);
        }
    }

    /// toggle audio mute for a specific peer
    pub fn set_peer_muted(&mut self, seat: u8, muted: bool) {
        if let Some(peer) = self.peers.get_mut(&seat) {
            peer.audio_muted = muted;
        }
    }

    /// get connected peer count
    pub fn connected_count(&self) -> usize {
        self.peers.values()
            .filter(|p| matches!(p.ice_state, IceConnectionState::Connected | IceConnectionState::Completed))
            .count()
    }

    /// poll for incoming signaling messages and process them
    pub fn poll(&mut self) {
        while let Ok(msg) = self.signal_rx.try_recv() {
            self.handle_signal(msg);
        }
    }
}

/// audio processor - handles opus encode/decode and resampling
pub struct AudioProcessor {
    /// sample rate for capture
    pub capture_rate: u32,
    /// sample rate for playback
    pub playback_rate: u32,
    /// capture buffer (accumulated samples before encoding)
    capture_buffer: Vec<f32>,
    /// playback buffers per seat
    playback_buffers: HashMap<u8, Vec<f32>>,
}

impl AudioProcessor {
    pub fn new(capture_rate: u32, playback_rate: u32) -> Self {
        Self {
            capture_rate,
            playback_rate,
            capture_buffer: Vec::with_capacity(OPUS_FRAME_SAMPLES),
            playback_buffers: HashMap::new(),
        }
    }

    /// push captured samples, returns opus frames when enough accumulated
    pub fn push_capture(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        self.capture_buffer.extend_from_slice(samples);

        let mut frames = Vec::new();
        while self.capture_buffer.len() >= OPUS_FRAME_SAMPLES {
            let frame: Vec<f32> = self.capture_buffer.drain(..OPUS_FRAME_SAMPLES).collect();
            frames.push(frame);
        }
        frames
    }

    /// push decoded audio for a peer's playback buffer
    pub fn push_playback(&mut self, seat: u8, samples: Vec<f32>) {
        self.playback_buffers.entry(seat).or_default().extend(samples);
    }

    /// drain mixed playback samples (all peers mixed together)
    pub fn drain_playback(&mut self, count: usize) -> Vec<f32> {
        let mut mixed = vec![0.0f32; count];
        let mut active_sources = 0u32;

        for (_, buffer) in self.playback_buffers.iter_mut() {
            let available = buffer.len().min(count);
            if available == 0 {
                continue;
            }
            active_sources += 1;
            for i in 0..available {
                mixed[i] += buffer[i];
            }
            buffer.drain(..available);
        }

        // normalize to prevent clipping when mixing multiple sources
        if active_sources > 1 {
            let scale = 1.0 / (active_sources as f32).sqrt();
            for sample in &mut mixed {
                *sample *= scale;
            }
        }

        // soft clip
        for sample in &mut mixed {
            *sample = sample.clamp(-1.0, 1.0);
        }

        mixed
    }

    /// voice activity detection (simple energy-based)
    pub fn detect_voice_activity(samples: &[f32], threshold: f32) -> bool {
        if samples.is_empty() {
            return false;
        }
        let energy: f32 = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
        energy.sqrt() > threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtc_manager_creation() {
        let (manager, _event_rx, _signal_tx) = RtcManager::new(1);
        assert_eq!(manager.local_seat, 1);
        assert_eq!(manager.peers.len(), 0);
        assert!(manager.accepting_media);
    }

    #[test]
    fn test_request_media() {
        let (mut manager, mut event_rx, _signal_tx) = RtcManager::new(1);

        manager.request_media(2, MediaKind::Voice);

        assert!(manager.peers.contains_key(&2));
        let peer = &manager.peers[&2];
        assert_eq!(peer.seat, 2);
        assert_eq!(peer.media, MediaKind::Voice);
        assert!(peer.is_offerer);

        // should have emitted a signal event
        let event = event_rx.try_recv().unwrap();
        assert!(matches!(event, RtcEvent::Signal(Message::RtcMediaRequest(_))));
    }

    #[test]
    fn test_media_request_accepted() {
        let (mut manager, mut event_rx, _signal_tx) = RtcManager::new(2);

        // simulate receiving a media request from seat 1
        manager.handle_signal(Message::RtcMediaRequest(RtcMediaRequest {
            seat: 1,
            media: MediaKind::Voice,
            dtls_fingerprint: Vec::new(),
        }));

        assert!(manager.peers.contains_key(&1));
        let peer = &manager.peers[&1];
        assert!(!peer.is_offerer); // we're the answerer

        // should have emitted acceptance
        let event = event_rx.try_recv().unwrap();
        match event {
            RtcEvent::Signal(Message::RtcMediaResponse(resp)) => {
                assert!(resp.accepted);
                assert_eq!(resp.seat, 2);
            }
            _ => panic!("expected RtcMediaResponse"),
        }
    }

    #[test]
    fn test_media_request_rejected() {
        let (mut manager, mut event_rx, _signal_tx) = RtcManager::new(2);
        manager.accepting_media = false;

        manager.handle_signal(Message::RtcMediaRequest(RtcMediaRequest {
            seat: 1,
            media: MediaKind::Voice,
            dtls_fingerprint: Vec::new(),
        }));

        assert!(!manager.peers.contains_key(&1));

        let event = event_rx.try_recv().unwrap();
        match event {
            RtcEvent::Signal(Message::RtcMediaResponse(resp)) => {
                assert!(!resp.accepted);
            }
            _ => panic!("expected rejection"),
        }
    }

    #[test]
    fn test_ice_candidate_buffering() {
        let (mut manager, _event_rx, _signal_tx) = RtcManager::new(1);

        // create peer state
        manager.peers.insert(2, PeerMediaState::new(2, MediaKind::Voice, false));

        // send ICE candidate before SDP is complete
        let candidate = RtcIceCandidate {
            seat: 2,
            target: 1,
            candidate: "candidate:1 1 UDP 2122262783 192.168.1.1 12345 typ host".into(),
            sdp_m_line_index: 0,
            sdp_mid: Some("audio".into()),
        };
        manager.handle_signal(Message::RtcIceCandidate(candidate));

        // should be buffered
        assert_eq!(manager.peers[&2].pending_candidates.len(), 1);
    }

    #[test]
    fn test_audio_processor_capture() {
        let mut proc = AudioProcessor::new(48000, 48000);

        // push less than one frame
        let half = vec![0.5f32; OPUS_FRAME_SAMPLES / 2];
        let frames = proc.push_capture(&half);
        assert!(frames.is_empty());

        // push another half → should get one frame
        let frames = proc.push_capture(&half);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), OPUS_FRAME_SAMPLES);
    }

    #[test]
    fn test_audio_processor_mixing() {
        let mut proc = AudioProcessor::new(48000, 48000);

        // two sources at 0.5
        proc.push_playback(1, vec![0.5; 100]);
        proc.push_playback(2, vec![0.5; 100]);

        let mixed = proc.drain_playback(100);
        assert_eq!(mixed.len(), 100);

        // mixed value should be 0.5 + 0.5 scaled by 1/sqrt(2)
        let expected = (0.5 + 0.5) / 2.0f32.sqrt();
        assert!((mixed[0] - expected).abs() < 0.01);
    }

    #[test]
    fn test_vad() {
        // silence → no voice
        assert!(!AudioProcessor::detect_voice_activity(&vec![0.0; 960], 0.01));

        // loud signal → voice
        assert!(AudioProcessor::detect_voice_activity(&vec![0.5; 960], 0.01));

        // quiet signal → depends on threshold
        let quiet: Vec<f32> = (0..960).map(|i| (i as f32 * 0.01).sin() * 0.005).collect();
        assert!(!AudioProcessor::detect_voice_activity(&quiet, 0.01));
    }

    #[test]
    fn test_connected_count() {
        let (mut manager, _event_rx, _signal_tx) = RtcManager::new(1);

        manager.peers.insert(2, PeerMediaState::new(2, MediaKind::Voice, true));
        manager.peers.insert(3, PeerMediaState::new(3, MediaKind::Voice, true));

        // none connected yet
        assert_eq!(manager.connected_count(), 0);

        manager.set_ice_state(2, IceConnectionState::Connected);
        assert_eq!(manager.connected_count(), 1);

        manager.set_ice_state(3, IceConnectionState::Completed);
        assert_eq!(manager.connected_count(), 2);

        manager.set_ice_state(2, IceConnectionState::Disconnected);
        assert_eq!(manager.connected_count(), 1);
    }

    #[test]
    fn test_disconnect_all() {
        let (mut manager, _event_rx, _signal_tx) = RtcManager::new(1);

        manager.peers.insert(2, PeerMediaState::new(2, MediaKind::Voice, true));
        manager.peers.insert(3, PeerMediaState::new(3, MediaKind::Voice, true));

        manager.disconnect_all();
        assert!(manager.peers.is_empty());
    }

    #[test]
    fn test_signaling_protocol_roundtrip() {
        // verify the new message types encode/decode correctly
        let offer = Message::RtcOffer(RtcSessionDescription {
            seat: 1,
            target: 2,
            sdp: "v=0\r\no=- 123 2 IN IP4 127.0.0.1\r\n".into(),
        });
        let encoded = offer.encode_to_vec();
        let decoded = Message::decode_from_slice(&encoded).unwrap();
        match decoded {
            Message::RtcOffer(desc) => {
                assert_eq!(desc.seat, 1);
                assert_eq!(desc.target, 2);
                assert!(desc.sdp.starts_with("v=0"));
            }
            _ => panic!("wrong message type"),
        }

        let candidate = Message::RtcIceCandidate(RtcIceCandidate {
            seat: 1,
            target: 2,
            candidate: "candidate:1 1 UDP 2122262783 10.0.0.1 54321 typ host".into(),
            sdp_m_line_index: 0,
            sdp_mid: Some("0".into()),
        });
        let encoded = candidate.encode_to_vec();
        let decoded = Message::decode_from_slice(&encoded).unwrap();
        match decoded {
            Message::RtcIceCandidate(c) => {
                assert_eq!(c.seat, 1);
                assert!(c.candidate.contains("54321"));
            }
            _ => panic!("wrong message type"),
        }
    }
}
