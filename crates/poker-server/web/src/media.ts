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

/**
 * A user-facing media error tagged with the step it came from, so the UI can
 * show a targeted message AND a retry control that re-runs exactly that step.
 * `kind` is derived from the DOMException name where possible so the UI can
 * tailor copy (permission vs. device-busy vs. no-device) without string-matching.
 */
export type MediaErrorStep = 'mic' | 'cam' | 'connect'
export type MediaErrorKind =
  | 'denied' // NotAllowedError / SecurityError — user/policy blocked access
  | 'busy' // NotReadableError / AbortError — device in use by another app
  | 'notfound' // NotFoundError / OverconstrainedError — no matching device
  | 'negotiation' // SDP/ICE failure
  | 'unknown'
export interface MediaError {
  step: MediaErrorStep
  kind: MediaErrorKind
  message: string
}

export interface MediaState {
  localStream: () => MediaStream | null
  remoteStream: () => MediaStream | null
  micEnabled: () => boolean
  camEnabled: () => boolean
  connected: () => boolean
  /**
   * The last unrecovered media error (permission denied, device busy, no device,
   * negotiation/ICE failure), or null. Every entry has a `step` so the UI can
   * offer a retry that re-runs just that step. Cleared on a successful retry, on
   * the relevant toggle, and on teardown.
   */
  lastError: () => MediaError | null
  /** clear the current error banner without retrying (user dismissed it). */
  clearError: () => void
  /**
   * Re-run the step that last failed, cleanly. Idempotent and safe to spam:
   * re-requests permission / re-acquires the device / re-negotiates. This is the
   * single "never get stuck" entry point wired to every retry button.
   */
  retry: () => Promise<void>
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
  polite: boolean,
): MediaState {
  const [localStream, setLocalStream] = createSignal<MediaStream | null>(null)
  // Remote media. Today there is exactly one opponent, but a future multiway
  // table has many peers, so remote streams are keyed by peer id in a map and
  // `remoteStream()` is a convenience accessor for "the (single) current peer".
  // The <video> layer should prefer iterating remoteStreams() when it grows to
  // multiway; keeping the map here means media.ts never bakes in "one opponent".
  const [remoteStreams, setRemoteStreams] = createSignal<Map<string, MediaStream>>(new Map())
  // The single default peer key. When signaling grows a per-peer id, thread it
  // through handleSignal/ontrack instead of this constant — nothing else changes.
  const PEER = 'peer'
  const remoteStream = () => remoteStreams().get(PEER) ?? null
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
  // Last unrecovered error + the step to re-run on retry(). See MediaError.
  const [lastError, setLastError] = createSignal<MediaError | null>(null)

  // Map a getUserMedia/RTCPeerConnection failure to a tagged, user-facing error.
  function classifyError(step: MediaErrorStep, e: unknown): MediaError {
    const name = (e as { name?: string })?.name ?? ''
    let kind: MediaErrorKind = 'unknown'
    let message: string
    switch (name) {
      case 'NotAllowedError':
      case 'SecurityError':
        kind = 'denied'
        message =
          step === 'cam'
            ? 'Camera permission was blocked. Allow camera access, then retry.'
            : 'Microphone permission was blocked. Allow mic access, then retry.'
        break
      case 'NotReadableError':
      case 'AbortError':
        kind = 'busy'
        message =
          (step === 'cam' ? 'Camera' : 'Microphone') +
          ' is in use by another app or tab. Close it, then retry.'
        break
      case 'NotFoundError':
      case 'OverconstrainedError':
        kind = 'notfound'
        message =
          step === 'cam'
            ? 'No camera found. Connect one, then retry.'
            : 'No microphone found. Connect one, then retry.'
        break
      default:
        message = `Could not start ${step === 'cam' ? 'camera' : step === 'mic' ? 'microphone' : 'the connection'}. Retry.`
    }
    return { step, kind, message }
  }

  let pc: RTCPeerConnection | null = null
  let makingOffer = false
  let ignoreOffer = false
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
      // Keyed by peer so this extends to a multiway map without changing the
      // signal shape. Fresh Map reference each time → SolidJS re-renders.
      setRemoteStreams((prev) => new Map(prev).set(PEER, stream))
    }

    pc.onconnectionstatechange = () => {
      const st = pc?.connectionState
      console.log('[media] connection state:', st)
      setConnected(st === 'connected')
      if (st === 'connected') {
        // A working connection clears any prior negotiation/ICE error banner.
        if (lastError()?.step === 'connect') setLastError(null)
      } else if (st === 'failed') {
        // ICE/DTLS failed. Surface a retryable error; retry() will re-negotiate.
        setLastError({
          step: 'connect',
          kind: 'negotiation',
          message: 'The direct connection failed (restrictive network). Retry to reconnect.',
        })
      }
    }

    // perfect negotiation pattern
    pc.onnegotiationneeded = async () => {
      try {
        makingOffer = true
        await pc!.setLocalDescription()
        send({ t: '_sdp', d: { sdp: pc!.localDescription!.toJSON() } })
      } catch (e) {
        console.warn('[media] negotiation error:', e)
        setLastError({
          step: 'connect',
          kind: 'negotiation',
          message: 'Could not negotiate the media connection. Retry.',
        })
      } finally {
        makingOffer = false
      }
    }

    return pc
  }

  const VIDEO_CONSTRAINTS: MediaTrackConstraints = { width: 320, height: 240, facingMode: 'user' }

  // Acquire audio and/or video, MERGING into any existing localStream. Each kind
  // is requested separately so a failure is attributable to the exact device
  // (the caller tags the error with 'mic'/'cam'). Re-emits a FRESH MediaStream
  // reference whenever tracks change so the bound preview <video> re-binds.
  async function getLocalMedia(audio: boolean, video: boolean): Promise<MediaStream> {
    let stream = localStream()
    let changed = false

    if (audio && !(stream?.getAudioTracks().length)) {
      const s = await navigator.mediaDevices.getUserMedia({ audio: true })
      if (!stream) stream = new MediaStream()
      s.getAudioTracks().forEach(t => stream!.addTrack(t))
      changed = true
    }
    if (video && !(stream?.getVideoTracks().length)) {
      const s = await navigator.mediaDevices.getUserMedia({ video: VIDEO_CONSTRAINTS })
      if (!stream) stream = new MediaStream()
      s.getVideoTracks().forEach(t => stream!.addTrack(t))
      changed = true
    }
    if (!stream) stream = new MediaStream()
    // New reference on change → the <video> preview re-binds srcObject and paints
    // the newly-added track (the audio-then-video "cam never shows" case locally).
    if (changed) setLocalStream(new MediaStream(stream.getTracks()))
    return localStream()!
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

  // Enable the mic (idempotent). Factored out of toggleMic so retry() can re-run
  // exactly this step after a permission/device failure without a page reload.
  async function enableMic() {
    const stream = await getLocalMedia(true, camEnabled())
    addTracksToPC(stream)
    stream.getAudioTracks().forEach(t => { t.enabled = true })
    setMicEnabled(true)
    if (lastError()?.step === 'mic') setLastError(null)
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
      // disable mic (reversible: track stays but muted, so re-enabling is instant
      // and needs no new permission prompt).
      const stream = localStream()
      stream?.getAudioTracks().forEach(t => { t.enabled = false })
      setMicEnabled(false)
      if (lastError()?.step === 'mic') setLastError(null)
    } else {
      // enable mic
      try {
        await enableMic()
      } catch (e) {
        console.warn('[media] mic access failed:', e)
        // Leave state OFF and surface a tagged, retryable error. retry() re-runs
        // enableMic(); nothing is left half-initialised (no track was added).
        setMicEnabled(false)
        setLastError(classifyError('mic', e))
      }
    }
  }

  // Enable the camera (idempotent). Factored out so retry() can re-run it.
  async function enableCam() {
    const stream = await getLocalMedia(micEnabled(), true)
    // The raw camera track from getUserMedia. This is what we blur; it is also
    // what lands in the PC sender initially (before any blur is applied).
    rawCamTrack = stream.getVideoTracks()[0] ?? null
    if (rawCamTrack) rawCamTrack.enabled = true
    addTracksToPC(stream)
    setCamEnabled(true)
    if (lastError()?.step === 'cam') setLastError(null)
    // If the user had already picked a blur mode, apply it now. This does the
    // replaceTrack swap so the peer receives the processed track from the off.
    if (blurMode() !== 'off') await applyBlurMode(blurMode())
  }

  async function toggleCam() {
    if (!acknowledged()) {
      console.warn('[media] cam toggle blocked: media not acknowledged')
      return
    }
    if (camEnabled()) {
      // disable camera (reversible: track stays live but disabled + blur paused).
      const stream = localStream()
      stream?.getVideoTracks().forEach(t => { t.enabled = false })
      // Stop the blur pipeline (the raw track stays live but disabled).
      blur.setMode('off').catch(() => {})
      setCamEnabled(false)
      if (lastError()?.step === 'cam') setLastError(null)
    } else {
      // enable camera
      try {
        await enableCam()
      } catch (e) {
        console.warn('[media] camera access failed:', e)
        // Leave state OFF and surface a tagged, retryable error. If getUserMedia
        // threw, no track was added; rawCamTrack stays null → nothing to unwind.
        setCamEnabled(false)
        setLastError(classifyError('cam', e))
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
      // Perfect negotiation: exactly one peer is polite. The IMPOLITE peer ignores a
      // colliding offer (keeps its own in-flight offer); the POLITE peer rolls back and
      // accepts. Previously both peers rolled back, so colliding offers annihilated and
      // the later (video) m-line never negotiated → black remote video.
      ignoreOffer = !polite && offerCollision
      if (ignoreOffer) return
      if (offerCollision) {
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
        if (!ignoreOffer) console.warn('[media] ICE error:', e)
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
    setRemoteStreams(new Map())
    pc?.close()
    pc = null
    makingOffer = false
    ignoreOffer = false
    setMicEnabled(false)
    setCamEnabled(false)
    setConnected(false)
    setBlurUnavailable(false)
    setLastError(null)
  }

  function clearError() {
    setLastError(null)
  }

  // Re-run whatever last failed, cleanly. This is the single "never get stuck"
  // entry point behind every retry button. Idempotent: safe to call repeatedly.
  async function retry() {
    const err = lastError()
    if (!err) return
    setLastError(null)
    try {
      if (err.step === 'mic') {
        await enableMic()
      } else if (err.step === 'cam') {
        await enableCam()
      } else {
        // Negotiation / ICE failure. Restart ICE to re-gather + re-offer. If the
        // PC is gone (or too broken), rebuild it and re-add whatever we're
        // sending so a fresh offer/answer runs.
        if (pc && pc.connectionState !== 'closed') {
          try {
            pc.restartIce()
          } catch {
            // Fall through to a full rebuild below.
          }
        }
        if (!pc || pc.connectionState === 'closed' || pc.connectionState === 'failed') {
          const wasCam = camEnabled()
          pc?.close()
          pc = null
          const stream = localStream()
          if (stream) {
            ensurePeerConnection()
            addTracksToPC(stream) // triggers onnegotiationneeded → fresh offer
            if (blurMode() !== 'off' && wasCam) await applyBlurMode(blurMode())
          }
        }
      }
    } catch (e) {
      console.warn('[media] retry failed:', e)
      setLastError(classifyError(err.step, e))
    }
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
    lastError,
    clearError,
    retry,
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
