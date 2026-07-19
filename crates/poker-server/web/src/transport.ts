/**
 * pluggable transport layer (finagle pattern).
 *
 * the game engine emits ServerMsg locally. the transport sends actions
 * to the peer and receives their actions. the relay is just a pipe.
 *
 * encryption: ephemeral x25519 DH key exchange on connect.
 * all game messages encrypted with AES-256-GCM (Web Crypto).
 * the relay sees opaque base64 blobs. anon or zafu — always encrypted.
 *
 * swappable: WebSocket relay, WebRTC, iroh, nym.
 */

import { createSignal } from 'solid-js'
import type { SessionIdentity } from './identity'
import { signKeyExchange } from './identity'
import { relayBase } from './config'

/** what we send over the wire (opaque to relay) */
export interface WireMessage {
  /** message type tag */
  t: string
  /** JSON payload */
  d: unknown
  /** relay-assigned timestamp (ms since epoch). neutral clock for disputes. */
  relayTs?: number
  /** transport reliability seq (added/stripped by ReliableChannel — internal). */
  _s?: number
  /** piggybacked cumulative ack (highest contiguous seq the sender has delivered). */
  _a?: number
  /** crypto key-epoch id the frame was minted under (stale-key detection). */
  _e?: number
}

/** transport provider interface */
export interface TransportProvider {
  connect(room: string, nick: string): void
  send(msg: WireMessage): void
  /** send a control frame to the SERVER escrow coordinator (staked tables).
   *  returns a delivery handle callers can await/inspect (see {@link ControlDelivery}). */
  sendServer(msg: unknown): ControlDelivery
  disconnect(): void
  readonly connected: () => boolean
  readonly encrypted: () => boolean
  /** register a callback fired when the reliability layer detects unrecoverable
   *  frame loss / reordering it could not repair, so game.ts can void a staked hand.
   *  (game.ts already dedups + gap-detects on its OWN action-seq; this surfaces
   *  the TRANSPORT-level view — e.g. a peer that acks a seq we never got.) */
  onDesync(cb: OnDesync): void
  /** register a HARD-error callback: the crypto handshake did not complete
   *  symmetrically within the timeout, or X25519 init failed. game.ts should
   *  refuse to start / void any staked hand rather than play half-plaintext. */
  onFatal(cb: OnFatal): void
}

/** in-flight handle for a server control frame. `acked()` flips true once the
 *  frame has been written to an OPEN relay socket (the relay forwards `srv`
 *  frames blind and the poker-server does not emit an application ack, so this
 *  is a SOCKET-level delivery guarantee: the frame left this client and will be
 *  retransmitted across reconnects until it does). */
export interface ControlDelivery {
  readonly acked: () => boolean
  /** resolves once the frame has been flushed to an OPEN socket. */
  readonly done: Promise<void>
}

/** transport-level desync/loss notification (distinct from game.ts's engine desync). */
export type OnDesync = (reason: string) => void

/** unrecoverable handshake / crypto failure. */
export type OnFatal = (reason: string) => void

/** callback for incoming peer messages */
export type OnPeerMessage = (msg: WireMessage) => void

/** callback for inbound server control frames (ServerMsg carried in `srv` frames) */
export type OnServerMessage = (msg: unknown) => void

/** callback for room events */
export type OnRoomEvent = (event: 'joined' | 'opponent_joined' | 'opponent_left' | 'opponent_disconnected' | 'opponent_reconnected' | 'error' | 'encrypted', data?: string, seat?: number) => void

// ============================================================================
// Ephemeral encryption (x25519 ECDH → AES-256-GCM)
// ============================================================================

interface SessionCrypto {
  myPublicKey: string   // base64
  sharedKey: CryptoKey | null
  ready: boolean
}

async function generateEphemeralKey(): Promise<{ publicKeyB64: string; keyPair: CryptoKeyPair }> {
  const keyPair = await crypto.subtle.generateKey({ name: 'X25519' }, false, ['deriveBits'])
  const pubRaw = await crypto.subtle.exportKey('raw', keyPair.publicKey)
  return { publicKeyB64: toB64(new Uint8Array(pubRaw)), keyPair }
}

async function deriveSharedKey(myPrivate: CryptoKey, theirPublicB64: string): Promise<CryptoKey> {
  const theirRaw = fromB64(theirPublicB64)
  const theirKey = await crypto.subtle.importKey('raw', theirRaw, { name: 'X25519' }, false, [])
  const bits = await crypto.subtle.deriveBits({ name: 'X25519', public: theirKey }, myPrivate, 256)
  // HKDF to derive AES key from the raw DH output
  const hkdfKey = await crypto.subtle.importKey('raw', bits, 'HKDF', false, ['deriveKey'])
  return crypto.subtle.deriveKey(
    { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(32), info: new TextEncoder().encode('poker-session') },
    hkdfKey,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  )
}

async function encryptPayload(key: CryptoKey, plaintext: string): Promise<string> {
  const iv = crypto.getRandomValues(new Uint8Array(12))
  const ct = new Uint8Array(await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv },
    key,
    new TextEncoder().encode(plaintext),
  ))
  // pack as iv:ct (both base64)
  return toB64(iv) + '.' + toB64(ct)
}

async function decryptPayload(key: CryptoKey, encrypted: string): Promise<string> {
  const dot = encrypted.indexOf('.')
  if (dot < 0) throw new Error('bad envelope')
  const iv = fromB64(encrypted.slice(0, dot))
  const ct = fromB64(encrypted.slice(dot + 1))
  const plain = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ct)
  return new TextDecoder().decode(plain)
}

function toB64(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes))
}
function fromB64(s: string): Uint8Array {
  return Uint8Array.from(atob(s), c => c.charCodeAt(0))
}

// ============================================================================
// Reliability layer: per-peer transport-level seq + cumulative ack + retransmit
// ============================================================================
//
// The relay is a blind fire-and-forget pipe: a `msg` frame queued but unflushed
// when the socket dies (or sent by the peer during our reconnect gap) is lost
// with no redelivery. This layer sits UNDER the crypto/send path and gives each
// peer channel an exactly-once, in-order stream:
//
//   • every outbound app frame gets a monotonic transport seq `_s` (per channel)
//   • every frame piggybacks our cumulative ack `_a` = highest CONTIGUOUS seq we
//     have delivered from that peer, so the peer can retire its retransmit buffer
//   • un-acked frames stay in `unacked` (a seq→frame ring) and are retransmitted,
//     in order, on reconnect / re-key
//   • inbound frames are delivered in seq order; a gap holds later frames until
//     the missing seq is (re)delivered — head-of-line, never silent drop
//   • a stale duplicate (`_s <= rxContig`) is dropped as an already-applied frame
//   • each frame is tagged with the crypto key-epoch `_e`; a frame minted under a
//     superseded epoch is surfaced (not mis-decrypted) — see KeyEpoch below
//
// It is keyed PER PEER (`ReliableChannel` per peerId), so a future multiway table
// just holds one channel per opponent — nothing here assumes exactly one peer.

/** an outbound frame wrapped with its reliability envelope, kept for retransmit. */
interface OutboundRecord {
  seq: number
  epoch: number
  /** the inner app WireMessage (already sequence-tagged) awaiting ack. */
  msg: WireMessage
}

/**
 * Reliable ordered stream state for a SINGLE peer channel. Pure bookkeeping —
 * it does not touch the socket; the transport pumps it via take/deliver hooks.
 */
class ReliableChannel {
  /** next outbound transport seq to assign. */
  private txSeq = 0
  /** un-acked outbound frames, oldest first (bounded retransmit ring). */
  private unacked: OutboundRecord[] = []
  /** highest CONTIGUOUS inbound seq we have delivered upward. */
  private rxContig = -1
  /** out-of-order inbound frames held until their predecessor arrives. */
  private rxBuffer = new Map<number, WireMessage>()
  /** cap so a malicious/broken peer can't grow buffers without bound. */
  private static readonly MAX_UNACKED = 512
  private static readonly MAX_GAP = 256

  /** stamp an outbound app frame with seq + epoch + our cumulative ack, and
   *  retain it for retransmit until the peer acks it. */
  stamp(msg: WireMessage, epoch: number): WireMessage {
    const seq = this.txSeq++
    const wrapped: WireMessage = { ...msg, _s: seq, _e: epoch, _a: this.rxContig } as WireMessage
    this.unacked.push({ seq, epoch, msg: wrapped })
    if (this.unacked.length > ReliableChannel.MAX_UNACKED) this.unacked.shift()
    return wrapped
  }

  /** re-stamp the un-acked tail under a (possibly new) epoch for retransmit after
   *  reconnect / re-key. Refreshes the piggybacked ack to our latest contiguous rx. */
  tailForRetransmit(epoch: number): WireMessage[] {
    return this.unacked.map(r => {
      r.epoch = epoch
      r.msg = { ...r.msg, _e: epoch, _a: this.rxContig } as WireMessage
      return r.msg
    })
  }

  /** our current cumulative ack (highest contiguous inbound seq). */
  ackSeq(): number { return this.rxContig }

  /** retire everything the peer has acknowledged (cumulative). */
  applyAck(ackedThrough: number): void {
    if (ackedThrough < 0) return
    while (this.unacked.length && this.unacked[0].seq <= ackedThrough) this.unacked.shift()
  }

  /**
   * Ingest an inbound wrapped frame. Returns the ordered list of inner app
   * frames ready to deliver (0, 1, or a run flushed after a gap fills), plus a
   * `desync` reason string if the peer acked past what we ever sent OR the gap
   * exceeded MAX_GAP (unrecoverable). Reserved transport frames must NOT be
   * passed here — only app frames carrying `_s`.
   */
  ingest(frame: WireMessage): { deliver: WireMessage[]; ack: number; reAck: boolean; desync?: string } {
    const w = frame as WireMessage & { _s?: number; _a?: number }
    // retire our outbound tail on the peer's piggybacked cumulative ack.
    if (typeof w._a === 'number') {
      if (w._a >= this.txSeq) {
        return { deliver: [], ack: this.rxContig, reAck: false, desync: `peer acked seq ${w._a} but we only sent ${this.txSeq - 1}` }
      }
      this.applyAck(w._a)
    }
    const seq = w._s
    if (typeof seq !== 'number') {
      // un-sequenced app frame (legacy peer / pre-handshake) — pass through as-is.
      return { deliver: [this.strip(frame)], ack: this.rxContig, reAck: false }
    }
    if (seq <= this.rxContig) {
      // already delivered — duplicate/retransmit. Drop, but re-ack so the peer
      // retires it (else it retransmits forever).
      return { deliver: [], ack: this.rxContig, reAck: true }
    }
    if (seq - this.rxContig > ReliableChannel.MAX_GAP) {
      return { deliver: [], ack: this.rxContig, reAck: false, desync: `inbound gap too large: got ${seq}, contiguous at ${this.rxContig}` }
    }
    this.rxBuffer.set(seq, frame)
    // flush the contiguous run starting at rxContig+1.
    const deliver: WireMessage[] = []
    let next = this.rxContig + 1
    while (this.rxBuffer.has(next)) {
      deliver.push(this.strip(this.rxBuffer.get(next)!))
      this.rxBuffer.delete(next)
      this.rxContig = next
      next++
    }
    // if we buffered a frame but could not advance (gap), re-ack so the peer knows
    // our contiguous high-water mark and keeps retransmitting only the missing seq.
    const reAck = deliver.length === 0
    return { deliver, ack: this.rxContig, reAck }
  }

  /** strip the reliability envelope so game.ts sees the unchanged inner frame. */
  private strip(frame: WireMessage): WireMessage {
    const { _s, _a, _e, ...inner } = frame as any
    return inner as WireMessage
  }

  /** whether we are still waiting on any inbound frame (held rxBuffer). */
  hasGap(): boolean { return this.rxBuffer.size > 0 }
}

// ============================================================================
// WebSocket relay transport (same-origin /ws)
// ============================================================================

/** configurable reconnection settings */
export interface ReconnectConfig {
  /** how long to wait for opponent reconnection (seconds) */
  opponentTimeout: number
  /** how quickly to retry our own reconnection (ms) */
  retryDelay: number
  /** max reconnect attempts before giving up */
  maxRetries: number
}

const DEFAULT_RECONNECT: ReconnectConfig = {
  opponentTimeout: 60,
  retryDelay: 2000,
  maxRetries: 10,
}

export function createRelayTransport(
  onPeer: OnPeerMessage,
  onRoom: OnRoomEvent,
  sessionIdentity?: SessionIdentity,
  reconnectConfig?: Partial<ReconnectConfig>,
  onServer?: OnServerMessage,
): TransportProvider {
  const config = { ...DEFAULT_RECONNECT, ...reconnectConfig }
  const [connected, setConnected] = createSignal(false)
  const [encrypted, setEncrypted] = createSignal(false)
  let ws: WebSocket | null = null
  let currentRoom: string | null = null
  let currentNick = 'anon'
  let isCreator = false
  let hasJoined = false
  let reconnectAttempts = 0
  let intentionalClose = false
  let opponentSeen = false
  let peerSessionPub: string | null = null // lock to first peer's identity

  // session encryption state
  let ephemeral: { publicKeyB64: string; keyPair: CryptoKeyPair } | null = null
  let sessionKey: CryptoKey | null = null
  let pendingMessages: WireMessage[] = []

  // ---- reliability + crypto-epoch state (see ReliableChannel) --------------
  // one reliable stream per peer. keyed by the peer's session pubkey; anon peers
  // (no session identity) share the DEFAULT_PEER key. Generalises to N peers: a
  // multiway table just populates more entries — nothing below assumes one peer.
  const DEFAULT_PEER = '_anon'
  const channels = new Map<string, ReliableChannel>()
  function channelFor(peer: string | null): ReliableChannel {
    const id = peer || DEFAULT_PEER
    let ch = channels.get(id)
    if (!ch) { ch = new ReliableChannel(); channels.set(id, ch) }
    return ch
  }
  // monotonic crypto key-epoch: bumped on every (re)derive so a frame minted
  // under a superseded key is detectable rather than mis-decrypted/dropped.
  let keyEpoch = 0
  // inbound `_enc` frames that arrived BEFORE sessionKey derived — replayed in
  // order once the key is ready (fix #2: never silently drop the peer's first
  // encrypted frames during async key derivation).
  let earlyEnc: { payload: string; relayTs?: number }[] = []
  // outbound SERVER control frames awaiting an OPEN socket (fix #5): queue while
  // closed, flush on reconnect, retry until written. Each carries its delivery handle.
  interface PendingControl { msg: unknown; resolve: () => void; delivered: { v: boolean } }
  let controlQueue: PendingControl[] = []
  // symmetric-handshake watchdog (fix #3): once a peer is present, BOTH sides
  // must derive a session key within this budget or we raise a hard fatal error.
  const HANDSHAKE_TIMEOUT_MS = 15000
  let handshakeTimer: ReturnType<typeof setTimeout> | null = null
  let handshakeFailed = false
  let onDesyncCb: OnDesync | null = null
  let onFatalCb: OnFatal | null = null

  function clearHandshakeTimer() { if (handshakeTimer) { clearTimeout(handshakeTimer); handshakeTimer = null } }
  /** arm the symmetric-encryption deadline. Called when a peer is first seen.
   *  If sessionKey is not derived by the deadline, the handshake is asymmetric
   *  (one side plaintext / one side never keyed) → surface a HARD error so
   *  game.ts refuses to start / voids a staked hand instead of degrading. */
  function armHandshakeWatchdog() {
    if (handshakeTimer || sessionKey || handshakeFailed) return
    handshakeTimer = setTimeout(() => {
      handshakeTimer = null
      if (!sessionKey && !handshakeFailed) {
        handshakeFailed = true
        console.error('[crypto] symmetric handshake did not complete within', HANDSHAKE_TIMEOUT_MS, 'ms — refusing to run half-plaintext')
        onFatalCb?.('encryption handshake failed — channel is not symmetrically encrypted')
        onRoom('error', 'encryption handshake failed')
      }
    }, HANDSHAKE_TIMEOUT_MS)
  }

  function connect(room: string, nick: string) {
    // Idempotency guard: never run two parallel relay sockets. If a socket is still live
    // from a prior connect() (double-click "sit down", a remount), close it FIRST. An
    // orphaned socket stays open on the relay STILL HOLDING ITS SEAT — the new socket then
    // sees it as a phantom "opponent" (or is rejected "room full"), deadlocking the table.
    // No-op on the normal first connect (ws is null).
    if (ws && ws.readyState !== WebSocket.CLOSED) {
      intentionalClose = true
      try { ws.send(JSON.stringify({ t: 'part' })) } catch {}
      try { ws.close() } catch {}
    }
    ws = null
    currentRoom = null
    hasJoined = false
    currentNick = nick
    isCreator = !room
    intentionalClose = false
    reconnectAttempts = 0
    doConnect(room)
  }

  function doConnect(room: string) {
    // `/p2p`, not `/ws`: prod HAProxy routes `/ws*` to a different relay service.
    // relayBase() is the user-selectable relay origin (default: same host that served us).
    const relayUrl = `${relayBase()}/p2p`
    ws = new WebSocket(relayUrl)

    // relay-unreachable watchdog: if the socket doesn't open promptly, treat the relay as
    // down and surface an error (the browser's own connect timeout can be 30s+ of silence).
    let openTimer: ReturnType<typeof setTimeout> | null = setTimeout(() => {
      if (ws && ws.readyState !== WebSocket.OPEN) {
        console.warn('[relay] connect timeout —', relayUrl, 'unreachable')
        try { ws.close() } catch {}
      }
    }, 8000)
    const clearOpenTimer = () => { if (openTimer) { clearTimeout(openTimer); openTimer = null } }

    ws.onerror = () => { console.warn('[relay] websocket error on', relayUrl) }

    ws.onopen = async () => {
      clearOpenTimer()
      setConnected(true)
      reconnectAttempts = 0
      console.log('[relay] connected', currentRoom ? '(reconnect)' : '(new)')

      try {
        ephemeral = await generateEphemeralKey()
        console.log('[crypto] ephemeral key generated')
      } catch (e) {
        // fix #3: X25519 init failure must fail LOUDLY. A staked table cannot run
        // one-directional plaintext, so we surface a fatal error instead of silently
        // proceeding unencrypted (which would let one side encrypt while we can't).
        console.error('[crypto] X25519 init FAILED — cannot establish encrypted channel:', e)
        ephemeral = null
        handshakeFailed = true
        onFatalCb?.('X25519 unavailable — cannot encrypt this table')
        onRoom('error', 'encryption unavailable on this device')
      }

      if (isCreator && !currentRoom) {
        ws!.send(JSON.stringify({ t: 'create', nick: currentNick }))
      } else {
        const r = currentRoom || room
        hasJoined = false // reset for reconnect
        ws!.send(JSON.stringify({ t: 'join', room: r, nick: currentNick }))
      }
      // fix #5: socket is OPEN again — flush any control frames that queued while
      // it was down. (Peer game frames are retransmitted after re-key, below.)
      flushControlQueue()
    }

    ws.onmessage = (ev) => {
      try {
        handleRelayMsg(JSON.parse(ev.data))
      } catch {}
    }

    ws.onclose = () => {
      clearOpenTimer()
      setConnected(false)
      if (intentionalClose) return

      // auto-reconnect if we have an established room
      if (currentRoom && reconnectAttempts < config.maxRetries) {
        reconnectAttempts++
        const delay = config.retryDelay * Math.min(reconnectAttempts, 3)
        console.log(`[relay] disconnected, reconnecting in ${delay}ms (attempt ${reconnectAttempts})`)
        // preserve session key — peer might still have it
        setTimeout(() => doConnect(currentRoom!), delay)
      } else if (!currentRoom) {
        // the INITIAL connection never succeeded → the relay is unreachable. Surface it loudly
        // so the UI can prompt the user to pick another relay, instead of hanging on "connecting".
        console.warn('[relay] initial connect failed —', `${relayBase()}/p2p`, 'unreachable')
        onRoom('error', `relay unreachable: ${relayBase()}`)
      } else {
        console.log('[relay] disconnected after exhausting retries')
        sessionKey = null
        ephemeral = null
        setEncrypted(false)
        onRoom('error', 'lost connection to relay')
      }
    }
  }

  function handleRelayMsg(msg: Record<string, unknown>) {
    console.log('[relay]', msg['t'], msg['t'] === 'msg' ? '' : msg)
    switch (msg['t']) {
      case 'created':
        currentRoom = msg['room'] as string
        ws?.send(JSON.stringify({ t: 'join', room: currentRoom, nick: currentNick }))
        break

      case 'joined': {
        currentRoom = msg['room'] as string
        const count = msg['count'] as number
        const seat = msg['seat'] as number | undefined
        if (!hasJoined) {
          hasJoined = true
          onRoom('joined', currentRoom, seat)
          // send our ephemeral public key + session identity signature
          if (ephemeral) {
            if (sessionIdentity) {
              signKeyExchange(sessionIdentity, ephemeral.publicKeyB64).then(sig => {
                sendRaw({ t: '_keyex', d: {
                  pk: ephemeral!.publicKeyB64,
                  sessionPub: sessionIdentity!.sessionPubKey,
                  sig,
                  mode: sessionIdentity!.mode,
                  zafuPub: sessionIdentity!.zafuPubKey,
                  delegation: sessionIdentity!.delegation,
                }})
              })
            } else {
              sendRaw({ t: '_keyex', d: { pk: ephemeral.publicKeyB64 } })
            }
          }
          if (count >= 2) { armHandshakeWatchdog(); onRoom('opponent_joined') }
        } else {
          if (count >= 2) { armHandshakeWatchdog(); onRoom('opponent_joined') }
        }
        break
      }

      case 'msg': {
        if (!hasJoined) break
        const text = msg['text'] as string
        const relayTs = msg['ts'] as number | undefined
        // NOTE: no self-echo suppression here. The relay server forwards a `msg` frame ONLY to
        // the OTHER peers (it skips the sender's own channel — `same_channel` guard server-side),
        // so a `msg` we receive is always from the peer. Filtering on `nick === currentNick`
        // used to silently DROP real peer frames whenever both nicks were empty/equal (two
        // anon players) — wedging the ceremony. The encrypted-payload path additionally binds
        // to the peer's session key (peerSessionPub), so identity is enforced cryptographically.
        handlePeerText(text, relayTs)
        break
      }

      case 'system': {
        const text = msg['text'] as string
        if (text.includes('joined')) {
          armHandshakeWatchdog() // fix #3: a peer is present → require symmetric crypto
          if (opponentSeen) {
            onRoom('opponent_reconnected')
          } else {
            opponentSeen = true
            onRoom('opponent_joined')
          }
          // re-send key exchange in case they missed it
          if (ephemeral && sessionIdentity) {
            signKeyExchange(sessionIdentity, ephemeral.publicKeyB64).then(sig => {
              sendRaw({ t: '_keyex', d: {
                pk: ephemeral!.publicKeyB64,
                sessionPub: sessionIdentity!.sessionPubKey,
                sig,
                mode: sessionIdentity!.mode,
                zafuPub: sessionIdentity!.zafuPubKey,
                delegation: sessionIdentity!.delegation,
              }})
            })
          } else if (ephemeral) {
            sendRaw({ t: '_keyex', d: { pk: ephemeral.publicKeyB64 } })
          }
        } else if (text.includes('left') || text.includes('closed')) {
          // opponent disconnected — give them time to reconnect
          onRoom('opponent_disconnected', String(config.opponentTimeout))
          // Reset the peer crypto lock so a NEW opponent (or the same one reconnecting with a
          // fresh ephemeral) can complete key exchange. Without this the session stays locked to
          // the departed peer's session key → the newcomer's keyex is rejected ("unknown peer")
          // and their frames fail to decrypt. Whoever (re)joins re-sends their keyex regardless.
          peerSessionPub = null
          sessionKey = null
          setEncrypted(false)
          opponentSeen = false
          // reliability/crypto reset for a fresh peer: drop the departed peer's
          // channel (its seq space does not carry to a newcomer) and disarm the
          // handshake watchdog until the next peer appears. earlyEnc is cleared so
          // a newcomer's frames are not replayed under a stale (null) key.
          // (2-seat table: clear() is fine; a multiway table would delete only the
          //  departed peer's channel by id.)
          channels.clear()
          earlyEnc = []
          handshakeFailed = false
          clearHandshakeTimer()
        }
        break
      }

      case 'srv': {
        // server-originated escrow control frame (RoomInfo / DepositStatus /
        // PayoutSigningRequest / PayoutComplete …). Distinct from opaque `_enc`
        // peer frames — the server only ever emits these for staked tables.
        const inner = msg['msg']
        if (inner) onServer?.(inner)
        break
      }

      case 'error':
        onRoom('error', msg['msg'] as string)
        break
    }
  }

  async function handlePeerText(text: string, relayTs?: number) {
    // try to parse as wire message
    let wireMsg: WireMessage
    try {
      wireMsg = JSON.parse(text)
    } catch {
      return
    }
    wireMsg.relayTs = relayTs

    // key exchange message (unencrypted, special)
    if (wireMsg.t === '_keyex') {
      const d = wireMsg.d as any
      const theirPk = d.pk as string

      // verify the x25519 pubkey is signed by the sender's session key
      if (d.sig && d.sessionPub && sessionIdentity) {
        // lock to first peer — reject keyex from different session keys
        if (peerSessionPub && d.sessionPub !== peerSessionPub) {
          console.warn('[crypto] keyex from unknown peer', d.sessionPub?.slice(0, 12), '— ignoring (locked to', peerSessionPub.slice(0, 12), ')')
          return
        }
        try {
          const msg = `keyex:${theirPk}`
          const valid = await sessionIdentity.verify(
            new TextEncoder().encode(msg), d.sig, d.sessionPub,
          )
          if (!valid) {
            console.warn('[crypto] keyex signature INVALID — possible MITM')
            return
          }
          peerSessionPub = d.sessionPub // lock to this peer
          console.log('[crypto] keyex verified:', d.mode, d.sessionPub?.slice(0, 12))
        } catch (e) {
          console.warn('[crypto] keyex sig verification error:', e)
        }
      }

      if (ephemeral) {
        try {
          const newKey = await deriveSharedKey(ephemeral.keyPair.privateKey, theirPk)
          const wasRekey = !!sessionKey
          if (wasRekey) {
            console.log('[crypto] re-keyed (opponent reconnected)')
          } else {
            console.log('[crypto] session key derived (AES-256-GCM)')
          }
          sessionKey = newKey
          // fix #4: every re-derive advances the key-epoch. Outbound frames are
          // stamped with it; the peer surfaces (does not mis-decrypt) a frame that
          // arrives under a superseded epoch.
          keyEpoch++
          setEncrypted(true)
          clearHandshakeTimer()          // fix #3: symmetric handshake completed on our side
          handshakeFailed = false
          onRoom('encrypted')

          // fix #2: replay inbound `_enc` frames that arrived before the key was
          // ready, IN ORDER, instead of dropping them.
          const buffered = earlyEnc
          earlyEnc = []
          for (const e of buffered) await decryptAndDeliver(e.payload, e.relayTs)

          const ch = channelFor(peerSessionPub)
          if (!wasRekey) {
            // fix #1/#4: flush any app frames queued during the initial handshake.
            // These have not been sequence-stamped yet, so route them through send()
            // which stamps + buffers them for retransmit.
            const queued = pendingMessages
            pendingMessages = []
            for (const m of queued) send(m)
          } else {
            // fix #4: DO NOT blind-drop un-sent actions on re-key. Any pendingMessages
            // (queued mid-handshake) still need to go; hand them to send() so they are
            // stamped under the NEW epoch, then retransmit the un-acked tail.
            const queued = pendingMessages
            pendingMessages = []
            for (const m of queued) send(m)
          }
          // fix #1: retransmit the un-acked outbound tail under the current epoch so
          // frames in flight when the old socket died are redelivered exactly once
          // (the peer dedups on `_s <= rxContig`).
          for (const w of ch.tailForRetransmit(keyEpoch)) sendEnvelope(w)
        } catch (e) {
          console.warn('[crypto] DH failed:', e)
        }
      }
      return
    }

    // bare cumulative-ack from the peer (fix #1): retire our un-acked outbound tail.
    if (wireMsg.t === '_ack') {
      const a = (wireMsg as WireMessage & { _a?: number })._a
      if (typeof a === 'number') channelFor(peerSessionPub).applyAck(a)
      return
    }

    // encrypted message
    if (wireMsg.t === '_enc') {
      const payload = (wireMsg.d as any).p as string
      if (!sessionKey) {
        // fix #2: the peer's first `_enc` frames can arrive while OUR key derivation
        // is still in flight (async). BUFFER them and replay in order once the key
        // derives, instead of warn+drop (which silently lost the opening frames).
        console.log('[crypto] _enc before session key — buffering for replay')
        earlyEnc.push({ payload, relayTs })
        return
      }
      await decryptAndDeliver(payload, relayTs)
      return
    }

    // plaintext peer frame (any non-reserved tag: game action, media signaling,
    // chat — they ALL flow through the encrypted reliable stream once keyed).
    // fix #3: if we HAVE an ephemeral key and expect encryption, a plaintext frame
    // from the peer means the channel is asymmetric (they are sending clear while we
    // would encrypt). We must NOT silently accept it and run half-plaintext on a
    // table with money at stake — raise a hard fatal so game.ts can void/refuse.
    if (ephemeral && !sessionKey && !handshakeFailed) {
      console.error('[crypto] peer sent PLAINTEXT game frame while we expect encryption — asymmetric channel, refusing')
      handshakeFailed = true
      clearHandshakeTimer()
      onFatalCb?.('peer is not encrypting — channel is one-way plaintext')
      onRoom('error', 'peer channel not encrypted')
      return
    }
    // no ephemeral at all (crypto unavailable both sides / free-play) → plaintext ok.
    deliverInbound(wireMsg, relayTs)
  }

  /** decrypt an `_enc` payload and route the inner frame through the reliability
   *  channel (ordered, exactly-once). */
  async function decryptAndDeliver(payload: string, relayTs?: number) {
    if (!sessionKey) return
    try {
      const plaintext = await decryptPayload(sessionKey, payload)
      const inner: WireMessage = JSON.parse(plaintext)
      inner.relayTs = relayTs // propagate relay timestamp through encryption
      // a bare `_ack` may ride INSIDE the encrypted envelope — retire our tail and
      // stop (it is not an app frame and must not enter the ordered stream).
      if (inner.t === '_ack') {
        const a = (inner as WireMessage & { _a?: number })._a
        if (typeof a === 'number') channelFor(peerSessionPub).applyAck(a)
        return
      }
      deliverInbound(inner, relayTs)
    } catch (e) {
      console.warn('[crypto] decrypt failed:', e)
    }
  }

  /** feed an inbound app frame through the peer's ReliableChannel: retire our
   *  acked tail, reorder, dedup, and deliver the in-order run to onPeer. Surfaces
   *  a transport-level desync (not an engine desync) via onDesync. */
  function deliverInbound(frame: WireMessage, relayTs?: number) {
    // fix #4: explicit key-epoch detection. A frame minted under a superseded key
    // cannot even decrypt under the current one (distinct AES key per epoch), so a
    // frame reaching here whose `_e` predates ours indicates a re-key crossing —
    // log it. The reliability seq still dedups/reorders it correctly.
    const fe = (frame as WireMessage & { _e?: number })._e
    if (typeof fe === 'number' && fe < keyEpoch) {
      console.warn('[crypto] frame from stale key-epoch', fe, '(current', keyEpoch, ') — reordered across re-key')
    }
    const ch = channelFor(peerSessionPub)
    const { deliver, ack, reAck, desync } = ch.ingest(frame)
    if (desync) {
      console.warn('[reliability] transport desync:', desync)
      onDesyncCb?.(desync)
    }
    for (const inner of deliver) {
      if (relayTs !== undefined && inner.relayTs === undefined) inner.relayTs = relayTs
      console.log('[relay] peer:', inner.t)
      onPeer(inner)
    }
    // Ack home so the peer retires its retransmit buffer: after we advance our
    // contiguous rx (delivered ≥ 1), or on a duplicate / held-gap (reAck) so a
    // retransmitting peer sees our high-water mark instead of resending forever.
    if (deliver.length > 0 || reAck) sendAck(ch, ack)
  }

  /** send a bare cumulative-ack frame (no app payload) so the peer retires its
   *  un-acked tail during quiet periods. Reserved transport tag `_ack`. */
  function sendAck(_ch: ReliableChannel, ackThrough: number) {
    // ack rides inside a reserved frame; it is itself un-sequenced (not reliable —
    // a lost ack is harmless, the next frame re-carries the cumulative value).
    sendEnvelope({ t: '_ack', d: {}, _a: ackThrough } as WireMessage)
  }

  /** send raw (unencrypted) through relay */
  function sendRaw(msg: WireMessage) {
    if (!ws || ws.readyState !== WebSocket.OPEN || !currentRoom) return
    ws.send(JSON.stringify({ t: 'msg', text: JSON.stringify(msg) }))
  }

  /** send a control frame to the SERVER escrow coordinator (staked tables).
   *  wrapped as `{t:'srv',msg}` — NOT a peer `msg` frame, so it never reaches
   *  the opponent and is never encrypted. the server demuxes on `t == "srv"`.
   *
   *  fix #5: a dropped Settlement/DkgComplete → escrow never gets a co-sign →
   *  refund. So we QUEUE the frame if the socket is not OPEN and flush it on
   *  reconnect, retrying until it is written. Returns a ControlDelivery the caller
   *  can inspect so it never marks something "sent" that wasn't put on the wire.
   *  (The poker-server does not emit an application ack for `srv` frames — this is
   *  a socket-level delivery guarantee, not an end-to-end one.) */
  function sendServer(msg: unknown): ControlDelivery {
    const delivered = { v: false }
    let resolveDone!: () => void
    const done = new Promise<void>(r => { resolveDone = r })
    const entry: PendingControl = { msg, delivered, resolve: resolveDone }
    controlQueue.push(entry)
    flushControlQueue()
    return { acked: () => delivered.v, done }
  }

  /** write any queued server control frames to an OPEN socket; leave them queued
   *  (for reconnect retransmit) if it is not. */
  function flushControlQueue() {
    if (!ws || ws.readyState !== WebSocket.OPEN || !currentRoom) return
    const pending = controlQueue
    controlQueue = []
    for (const c of pending) {
      try {
        ws.send(JSON.stringify({ t: 'srv', msg: c.msg }))
        c.delivered.v = true
        c.resolve()
      } catch (e) {
        // put it back at the head for the next flush (socket died mid-write).
        console.warn('[srv] control frame send failed, requeueing:', e)
        controlQueue.unshift(c)
        break
      }
    }
  }

  /** send game message — stamped for reliable ordered delivery, encrypted if a
   *  session key is available. This is the PUBLIC api game.ts calls. */
  function send(msg: WireMessage) {
    if (sessionKey) {
      // stamp with transport seq + epoch + piggybacked ack, retain for retransmit,
      // then put it on the wire.
      const wrapped = channelFor(peerSessionPub).stamp(msg, keyEpoch)
      sendEnvelope(wrapped)
    } else if (ephemeral) {
      // key exchange in progress — queue; flushed (and stamped) once the key derives.
      pendingMessages.push(msg)
    } else {
      // no crypto available anywhere (free-play, both sides plaintext): still stamp
      // so ordering/dedup holds, then send in the clear.
      const wrapped = channelFor(peerSessionPub).stamp(msg, keyEpoch)
      sendEnvelope(wrapped)
    }
  }

  /** put an already-stamped wrapped frame on the wire: encrypt if we have a key,
   *  else plaintext. Used by send(), retransmit, and ack frames. If the socket is
   *  not OPEN the frame is NOT lost — it stays in the channel's un-acked ring and
   *  is retransmitted on reconnect (fix #1). */
  function sendEnvelope(wrapped: WireMessage) {
    if (sessionKey) {
      const plaintext = JSON.stringify(wrapped)
      encryptPayload(sessionKey, plaintext).then(enc => {
        sendRaw({ t: '_enc', d: { p: enc } })
      }).catch(e => {
        // C2: NEVER fall back to plaintext. The frame is still in the un-acked ring
        // (if sequenced), so a later retransmit re-attempts it — not silently lost.
        console.error('[crypto] encrypt failed, will retransmit on next epoch:', e)
      })
    } else {
      sendRaw(wrapped)
    }
  }

  function disconnect() {
    intentionalClose = true
    ws?.send(JSON.stringify({ t: 'part' }))
    ws?.close()
    ws = null
    currentRoom = null
    sessionKey = null
    setEncrypted(false)
    clearHandshakeTimer()
    channels.clear()
    earlyEnc = []
    // fail any control frames that never made it to an OPEN socket so callers
    // awaiting delivery are not left hanging (their .done resolves; .acked stays false).
    for (const c of controlQueue) c.resolve()
    controlQueue = []
  }

  function onDesync(cb: OnDesync) { onDesyncCb = cb }
  function onFatal(cb: OnFatal) { onFatalCb = cb }

  return { connect, send, sendServer, disconnect, connected, encrypted, onDesync, onFatal }
}

/** reserved first-path-segments that are app routes, never room codes. */
const RESERVED_SEGMENTS = new Set(['new', 't', 'tournaments', 'watch', 'lobby', 'settings', 'play'])

/** get the room code from the URL path ('' = create new / not a room). Parses the FIRST segment
 *  only, so `/CODE/spectate` yields `CODE` (not the old garbage `CODE/spectate`), and reserved app
 *  routes (`/t/…`, `/watch/…`) are not mistaken for rooms. `/play/<code>` unwraps to `<code>`. */
export function getRoomFromUrl(): string {
  const segs = location.pathname.replace(/^\/+|\/+$/g, '').split('/').filter(Boolean)
  if (segs.length === 0) return ''
  if (segs[0] === 'play' || segs[0] === 'watch') return segs[1] || '' // /play/<code>, /watch/<code>
  if (RESERVED_SEGMENTS.has(segs[0])) return ''                       // /t, /new, /settings, …
  return segs[0]                                                       // bare /<code> (+ ignore any suffix)
}

/** update URL to show room code */
export function setRoomInUrl(room: string) {
  history.replaceState(null, '', '/' + room)
}
