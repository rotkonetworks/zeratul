/**
 * WebRTC media for voice/video chat during poker.
 *
 * Uses the existing relay for signaling (SDP offer/answer + ICE candidates).
 * Media streams are P2P (not through relay) once ICE connects.
 *
 * WARNING: WebRTC is direct peer-to-peer. Enabling voice/video reveals
 * your IP address to the other player. Game messages stay encrypted through
 * the relay, but media bypasses it. A future version may route media through
 * a TURN relay to prevent IP leaks.
 *
 * Opt-in: player must click mic/camera to enable.
 * The relay never sees audio/video data - only signaling messages.
 */

import { createSignal } from 'solid-js'
import type { WireMessage } from './transport'

export interface MediaState {
  localStream: () => MediaStream | null
  remoteStream: () => MediaStream | null
  micEnabled: () => boolean
  camEnabled: () => boolean
  connected: () => boolean
  toggleMic: () => Promise<void>
  toggleCam: () => Promise<void>
  handleSignal: (msg: WireMessage) => void
  cleanup: () => void
}

const ICE_SERVERS = [
  { urls: 'stun:stun.l.google.com:19302' },
  { urls: 'stun:stun1.l.google.com:19302' },
]

export function createMedia(
  send: (msg: WireMessage) => void,
): MediaState {
  const [localStream, setLocalStream] = createSignal<MediaStream | null>(null)
  const [remoteStream, setRemoteStream] = createSignal<MediaStream | null>(null)
  const [micEnabled, setMicEnabled] = createSignal(false)
  const [camEnabled, setCamEnabled] = createSignal(false)
  const [connected, setConnected] = createSignal(false)

  let pc: RTCPeerConnection | null = null
  let makingOffer = false

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
      if (e.streams[0]) {
        setRemoteStream(e.streams[0])
      } else {
        const stream = remoteStream() || new MediaStream()
        stream.addTrack(e.track)
        setRemoteStream(stream)
      }
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

  async function toggleMic() {
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
    if (camEnabled()) {
      // disable camera
      const stream = localStream()
      stream?.getVideoTracks().forEach(t => { t.enabled = false })
      setCamEnabled(false)
    } else {
      // enable camera
      try {
        const stream = await getLocalMedia(micEnabled(), true)
        addTracksToPC(stream)
        stream.getVideoTracks().forEach(t => { t.enabled = true })
        setCamEnabled(true)
      } catch (e) {
        console.warn('[media] camera access denied:', e)
      }
    }
  }

  // handle incoming WebRTC signaling messages
  async function handleSignal(msg: WireMessage) {
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

  function cleanup() {
    localStream()?.getTracks().forEach(t => t.stop())
    setLocalStream(null)
    setRemoteStream(null)
    pc?.close()
    pc = null
    setMicEnabled(false)
    setCamEnabled(false)
    setConnected(false)
  }

  return {
    localStream,
    remoteStream,
    micEnabled,
    camEnabled,
    connected,
    toggleMic,
    toggleCam,
    handleSignal,
    cleanup,
  }
}
