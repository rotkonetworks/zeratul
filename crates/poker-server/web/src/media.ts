/**
 * WebRTC media for voice/video chat during poker.
 *
 * Uses the existing relay for signaling (SDP offer/answer + ICE candidates).
 * Media streams are P2P (not through relay) once ICE connects.
 *
 * PRIVACY / OPT-IN:
 * The poker game itself runs P2P over the ENCRYPTED BLIND RELAY (transport.ts):
 * the two players never open a socket to each other, so no peer IP is leaked.
 * WebRTC media is the exception — it connects the two clients DIRECTLY so that
 * audio/video never touches the operator's servers. That direct connection
 * necessarily reveals each player's IP address to the other.
 *
 * Because of that, media is HARD opt-in: nothing here calls getUserMedia and no
 * RTCPeerConnection is created until the user has explicitly acknowledged the
 * IP-exposure tradeoff (`acknowledge()`). `revoke()` tears everything down and
 * re-arms the gate. Incoming SDP/ICE signaling is ignored until acknowledged, so
 * a remote peer cannot force a connection (or a candidate-gathering IP probe)
 * on us before we consent.
 *
 * ICE / NAT traversal decision (team standing decision — DIRECT P2P, opt-in):
 *   - NO TURN relay. TURN would route media through a server and HIDE IPs, which
 *     is the opposite of the intended tradeoff (and would defeat the "media is
 *     direct" property). If relayed/anonymous media is ever wanted it is a
 *     separate feature with its own consent.
 *   - NO third-party STUN. Public STUN servers (e.g. Google's) would hand a
 *     third party the client's reflexive IP, deanonymizing beyond the peer.
 *     We therefore use an EMPTY iceServers list: host candidates only. Media
 *     connects directly on reachable networks (same LAN / non-symmetric NAT via
 *     peer-reflexive candidates); if both peers are behind restrictive NATs it
 *     may fail to connect — that is the accepted privacy-first default. A
 *     self-hosted STUN could be added later without leaking to a third party.
 */

import { createSignal } from 'solid-js'
import type { WireMessage } from './transport'
import { createVideoBlur, type BlurMode } from './video-blur'

export interface MediaState {
  localStream: () => MediaStream | null
  remoteStream: () => MediaStream | null
  micEnabled: () => boolean
  camEnabled: () => boolean
  connected: () => boolean
  /** true once the user has consented to direct-P2P media + IP exposure */
  acknowledged: () => boolean
  /** record explicit consent; media stays inert until this is called */
  acknowledge: () => void
  /** withdraw consent: stops tracks, closes the peer connection, re-arms the gate */
  revoke: () => void
  /**
   * true when the opponent has started media (an SDP offer arrived) but the
   * local user has NOT yet opted in. Drives the "opponent enabled video —
   * enable yours to connect" prompt. Cleared once we acknowledge or on cleanup.
   */
  incomingPending: () => boolean
  /** dismiss the incoming-media prompt without opting in (stays disconnected) */
  dismissIncoming: () => void
  /**
   * Background-processing mode for the OUTGOING webcam ('off' | 'blur' | 'image').
   * Applied via RTCRtpSender.replaceTrack — no renegotiation. Ignored when the cam
   * is off; re-applied when the cam is (re)enabled.
   */
  blurMode: () => BlurMode
  /**
   * Change the outgoing-webcam background mode. Falls back to the raw stream (and
   * flips blurUnavailable) if the segmentation model can't initialise.
   */
  setBlurMode: (m: BlurMode) => Promise<void>
  /** provide a background image for 'image' mode (same-origin / object URL). */
  setBlurImage: (img: HTMLImageElement | ImageBitmap | null) => void
  /** true if blur was requested but the model failed to init (sending raw). */
  blurUnavailable: () => boolean
  toggleMic: () => Promise<void>
  toggleCam: () => Promise<void>
  handleSignal: (msg: WireMessage) => void
  cleanup: () => void
}

// Direct P2P, no TURN, no third-party STUN. See file header for rationale.
// Host candidates only → the reflexive IP is never disclosed to any third party;
// the only party that learns your IP is the opponent you chose to connect to.
const ICE_SERVERS: RTCIceServer[] = []

export function createMedia(
  send: (msg: WireMessage) => void,
): MediaState {
  const [localStream, setLocalStream] = createSignal<MediaStream | null>(null)
  const [remoteStream, setRemoteStream] = createSignal<MediaStream | null>(null)
  const [micEnabled, setMicEnabled] = createSignal(false)
  const [camEnabled, setCamEnabled] = createSignal(false)
  const [connected, setConnected] = createSignal(false)
  // Consent gate. Nothing that could leak the local IP (getUserMedia →
  // addTrack → ICE gathering, or accepting a remote offer) runs until true.
  const [acknowledged, setAcknowledged] = createSignal(false)
  // Set when a remote SDP offer arrives while we have NOT opted in. We do not
  // touch it (no PC, no ICE, no IP leak) — we only raise this flag so the UI can
  // surface a consent prompt. The peer's repeated offers just re-arm the flag.
  const [incomingPending, setIncomingPending] = createSignal(false)
  // Outgoing webcam background processing (blur / image). See video-blur.ts.
  const [blurMode, setBlurMode] = createSignal<BlurMode>('off')
  const [blurUnavailable, setBlurUnavailable] = createSignal(false)

  let pc: RTCPeerConnection | null = null
  let makingOffer = false
  // The pipeline that turns the raw camera track into a blurred/image one.
  const blur = createVideoBlur()
  // The UNPROCESSED camera track. `localStream` (what the <video> preview + the
  // RTCRtpSender hold) may carry the PROCESSED track instead; we keep the raw one
  // here so we can revert to it, restart processing, or stop it on teardown.
  let rawCamTrack: MediaStreamTrack | null = null

  function ensurePeerConnection() {
    if (pc) return pc

    pc = new RTCPeerConnection({ iceServers: ICE_SERVERS })

    pc.onicecandidate = (e) => {
      if (e.candidate) {
        send({ t: '_ice', d: { candidate: e.candidate.toJSON() } })
      }
    }

    pc.ontrack = (e) => {
      console.log('[media] remote track:', e.track.kind)
      // Rebuild the remote stream from ALL current receiver tracks. When a track
      // is added by a later renegotiation (e.g. video turned on after an
      // audio-only call is already up), the browser fires ontrack with the SAME
      // MediaStream object in e.streams[0] it used for the earlier audio track.
      // Reusing that reference makes setRemoteStream a no-op for the `===` signal,
      // so the bound <video> never re-attaches and the new video track is never
      // painted (the classic "mic works, cam never shows" bug). Building a fresh
      // MediaStream every time yields a new reference → the signal fires → the
      // <video> element re-binds srcObject and renders audio + video together.
      const stream = new MediaStream()
      for (const r of pc!.getReceivers()) {
        if (r.track) stream.addTrack(r.track)
      }
      setRemoteStream(stream)
    }

    pc.onconnectionstatechange = () => {
      console.log('[media] connection state:', pc?.connectionState)
      setConnected(pc?.connectionState === 'connected')
    }

    // perfect negotiation pattern
    pc.onnegotiationneeded = async () => {
      try {
        makingOffer = true
        await pc!.setLocalDescription()
        send({ t: '_sdp', d: { sdp: pc!.localDescription!.toJSON() } })
      } catch (e) {
        console.warn('[media] negotiation error:', e)
      } finally {
        makingOffer = false
      }
    }

    return pc
  }

  async function getLocalMedia(audio: boolean, video: boolean): Promise<MediaStream> {
    const existing = localStream()
    if (existing) {
      // update existing tracks
      if (audio && !existing.getAudioTracks().length) {
        const s = await navigator.mediaDevices.getUserMedia({ audio: true })
        s.getAudioTracks().forEach(t => existing.addTrack(t))
      }
      if (video && !existing.getVideoTracks().length) {
        const s = await navigator.mediaDevices.getUserMedia({ video: { width: 320, height: 240, facingMode: 'user' } })
        s.getVideoTracks().forEach(t => existing.addTrack(t))
      }
      return existing
    }

    const stream = await navigator.mediaDevices.getUserMedia({
      audio: audio,
      video: video ? { width: 320, height: 240, facingMode: 'user' } : false,
    })
    setLocalStream(stream)
    return stream
  }

  function addTracksToPC(stream: MediaStream) {
    const conn = ensurePeerConnection()
    const existingSenders = conn.getSenders()
    for (const track of stream.getTracks()) {
      const exists = existingSenders.some(s => s.track?.id === track.id)
      if (!exists) {
        conn.addTrack(track, stream)
      }
    }
  }

  // The RTCRtpSender currently carrying video (if any).
  function videoSender(): RTCRtpSender | null {
    return pc?.getSenders().find(s => s.track?.kind === 'video') ?? null
  }

  // Swap the video track the PEER receives WITHOUT renegotiation, and swap the
  // same track into localStream so the local <video> preview mirrors what we send.
  async function swapVideoTrack(next: MediaStreamTrack) {
    const sender = videoSender()
    if (sender && sender.track?.id !== next.id) {
      try { await sender.replaceTrack(next) } catch (e) { console.warn('[media] replaceTrack failed:', e) }
    }
    const stream = localStream()
    if (stream) {
      const cur = stream.getVideoTracks()[0]
      if (cur && cur.id !== next.id) {
        stream.removeTrack(cur)
        stream.addTrack(next)
        // Re-emit a fresh MediaStream reference so the bound <video> re-binds.
        setLocalStream(stream)
      }
    }
  }

  // Apply the requested blur mode to the outgoing webcam. Safe to call when the
  // cam is off (no-op) or when the model is unavailable (reverts to raw + flag).
  async function applyBlurMode(mode: BlurMode) {
    setBlurMode(mode)
    setBlurUnavailable(false)
    // No camera running: remember the choice; it's applied when the cam is on.
    if (!camEnabled() || !rawCamTrack) return

    if (mode === 'off') {
      await blur.setMode('off')
      await swapVideoTrack(rawCamTrack)
      return
    }
    try {
      await blur.setMode(mode, rawCamTrack)
      const out = blur.outputTrack()
      if (out) await swapVideoTrack(out)
      else throw new Error('no processed track')
    } catch {
      // Model failed to init / too weak: keep sending the RAW track, surface flag.
      setBlurUnavailable(true)
      await swapVideoTrack(rawCamTrack)
    }
  }

  async function toggleMic() {
    // Consent gate: refuse to touch the mic / create a peer connection until the
    // user has acknowledged the IP-exposure tradeoff. The UI must call
    // acknowledge() first; this is a defensive backstop.
    if (!acknowledged()) {
      console.warn('[media] mic toggle blocked: media not acknowledged')
      return
    }
    if (micEnabled()) {
      // disable mic
      const stream = localStream()
      stream?.getAudioTracks().forEach(t => { t.enabled = false })
      setMicEnabled(false)
    } else {
      // enable mic
      try {
        const stream = await getLocalMedia(true, camEnabled())
        addTracksToPC(stream)
        stream.getAudioTracks().forEach(t => { t.enabled = true })
        setMicEnabled(true)
      } catch (e) {
        console.warn('[media] mic access denied:', e)
      }
    }
  }

  async function toggleCam() {
    if (!acknowledged()) {
      console.warn('[media] cam toggle blocked: media not acknowledged')
      return
    }
    if (camEnabled()) {
      // disable camera
      const stream = localStream()
      stream?.getVideoTracks().forEach(t => { t.enabled = false })
      // Stop the blur pipeline (the raw track stays live but disabled).
      blur.setMode('off').catch(() => {})
      setCamEnabled(false)
    } else {
      // enable camera
      try {
        const stream = await getLocalMedia(micEnabled(), true)
        // The raw camera track from getUserMedia. This is what we blur; it is also
        // what lands in the PC sender initially (before any blur is applied).
        rawCamTrack = stream.getVideoTracks()[0] ?? null
        if (rawCamTrack) rawCamTrack.enabled = true
        addTracksToPC(stream)
        setCamEnabled(true)
        // If the user had already picked a blur mode, apply it now. This does the
        // replaceTrack swap so the peer receives the processed track from the off.
        if (blurMode() !== 'off') await applyBlurMode(blurMode())
      } catch (e) {
        console.warn('[media] camera access denied:', e)
      }
    }
  }

  // handle incoming WebRTC signaling messages.
  // Ignored entirely until the user has opted in: otherwise a remote offer would
  // create an RTCPeerConnection and start ICE gathering — leaking our IP to the
  // peer before we ever consented. We only ever connect after our own opt-in.
  async function handleSignal(msg: WireMessage) {
    if (!acknowledged()) {
      // Not opted in: do NOT create a peer connection or add the candidate —
      // that would start ICE and leak our IP to the peer before we consented.
      // Instead, surface an incoming-media prompt when the peer sends an OFFER
      // (their attempt to start media). ICE candidates arriving before consent
      // are just dropped; they're meaningless without a peer connection.
      if (msg.t === '_sdp' && (msg.d as { sdp?: RTCSessionDescriptionInit })?.sdp?.type === 'offer') {
        setIncomingPending(true)
      }
      console.warn('[media] ignoring signaling before opt-in:', msg.t)
      return
    }
    if (msg.t === '_sdp') {
      const d = msg.d as { sdp: RTCSessionDescriptionInit }
      const conn = ensurePeerConnection()
      const desc = new RTCSessionDescription(d.sdp)

      const offerCollision = desc.type === 'offer' && (makingOffer || conn.signalingState !== 'stable')
      if (offerCollision) {
        // polite peer: rollback and accept
        await conn.setLocalDescription({ type: 'rollback' })
      }

      await conn.setRemoteDescription(desc)

      if (desc.type === 'offer') {
        await conn.setLocalDescription()
        send({ t: '_sdp', d: { sdp: conn.localDescription!.toJSON() } })
      }
    }

    if (msg.t === '_ice') {
      const d = msg.d as { candidate: RTCIceCandidateInit }
      const conn = ensurePeerConnection()
      try {
        await conn.addIceCandidate(new RTCIceCandidate(d.candidate))
      } catch (e) {
        console.warn('[media] ICE error:', e)
      }
    }
  }

  // Fully stop media and close the peer connection, without touching the
  // consent flag. Shared by revoke() and cleanup().
  function teardown() {
    // Stop the blur pipeline + its canvas-capture track, and the raw camera track
    // (which may have been swapped out of localStream and so is not stopped below).
    blur.stop()
    rawCamTrack?.stop()
    rawCamTrack = null
    localStream()?.getTracks().forEach(t => t.stop())
    setLocalStream(null)
    setRemoteStream(null)
    pc?.close()
    pc = null
    makingOffer = false
    setMicEnabled(false)
    setCamEnabled(false)
    setConnected(false)
    setBlurUnavailable(false)
  }

  function acknowledge() {
    setAcknowledged(true)
    // We're consenting now; the prompt is no longer relevant. If the peer had
    // already offered, our own toggle will (re)negotiate a fresh connection.
    setIncomingPending(false)
  }

  // User withdrew consent: stop everything AND re-arm the gate, so any later
  // mic/cam use requires acknowledging again.
  function revoke() {
    teardown()
    setAcknowledged(false)
    setIncomingPending(false)
  }

  // Dismiss the "opponent enabled video" prompt without opting in. We stay
  // disconnected — no PC, no ICE, no IP leak.
  function dismissIncoming() {
    setIncomingPending(false)
  }

  function cleanup() {
    teardown()
    setAcknowledged(false)
    setIncomingPending(false)
  }

  return {
    localStream,
    remoteStream,
    micEnabled,
    camEnabled,
    connected,
    acknowledged,
    acknowledge,
    revoke,
    incomingPending,
    dismissIncoming,
    blurMode,
    setBlurMode: applyBlurMode,
    setBlurImage: (img) => blur.setBackgroundImage(img),
    blurUnavailable,
    toggleMic,
    toggleCam,
    handleSignal,
    cleanup,
  }
}
