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

/** what we send over the wire (opaque to relay) */
export interface WireMessage {
  /** message type tag */
  t: string
  /** JSON payload */
  d: unknown
  /** relay-assigned timestamp (ms since epoch). neutral clock for disputes. */
  relayTs?: number
}

/** transport provider interface */
export interface TransportProvider {
  connect(room: string, nick: string): void
  send(msg: WireMessage): void
  disconnect(): void
  readonly connected: () => boolean
  readonly encrypted: () => boolean
}

/** callback for incoming peer messages */
export type OnPeerMessage = (msg: WireMessage) => void

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

  function connect(room: string, nick: string) {
    currentNick = nick
    isCreator = !room
    intentionalClose = false
    reconnectAttempts = 0
    doConnect(room)
  }

  function doConnect(room: string) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    // `/p2p`, not `/ws`: prod HAProxy routes `/ws*` to a different relay service.
    const relayUrl = `${proto}//${location.host}/p2p`
    ws = new WebSocket(relayUrl)

    ws.onopen = async () => {
      setConnected(true)
      reconnectAttempts = 0
      console.log('[relay] connected', currentRoom ? '(reconnect)' : '(new)')

      try {
        ephemeral = await generateEphemeralKey()
        console.log('[crypto] ephemeral key generated')
      } catch (e) {
        console.warn('[crypto] X25519 not available:', e)
      }

      if (isCreator && !currentRoom) {
        ws!.send(JSON.stringify({ t: 'create', nick: currentNick }))
      } else {
        const r = currentRoom || room
        hasJoined = false // reset for reconnect
        ws!.send(JSON.stringify({ t: 'join', room: r, nick: currentNick }))
      }
    }

    ws.onmessage = (ev) => {
      try {
        handleRelayMsg(JSON.parse(ev.data))
      } catch {}
    }

    ws.onclose = () => {
      setConnected(false)
      if (intentionalClose) return

      // auto-reconnect if we have a room
      if (currentRoom && reconnectAttempts < config.maxRetries) {
        reconnectAttempts++
        const delay = config.retryDelay * Math.min(reconnectAttempts, 3)
        console.log(`[relay] disconnected, reconnecting in ${delay}ms (attempt ${reconnectAttempts})`)
        // preserve session key — peer might still have it
        setTimeout(() => doConnect(currentRoom!), delay)
      } else {
        console.log('[relay] disconnected, giving up')
        sessionKey = null
        ephemeral = null
        setEncrypted(false)
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
          if (count >= 2) onRoom('opponent_joined')
        } else {
          if (count >= 2) onRoom('opponent_joined')
        }
        break
      }

      case 'msg': {
        if (!hasJoined) break
        const text = msg['text'] as string
        const nick = msg['nick'] as string
        const relayTs = msg['ts'] as number | undefined
        if (nick === currentNick) break
        handlePeerText(text, relayTs)
        break
      }

      case 'system': {
        const text = msg['text'] as string
        if (text.includes('joined')) {
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
          // don't wipe session key yet — they might reconnect
        }
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
          if (sessionKey) {
            console.log('[crypto] re-keyed (opponent reconnected)')
          } else {
            console.log('[crypto] session key derived (AES-256-GCM)')
          }
          const wasRekey = !!sessionKey
          sessionKey = newKey
          setEncrypted(true)
          onRoom('encrypted')
          // M1: only flush pending on initial key exchange, not re-key
          // stale messages from before reconnect could corrupt peer state
          if (!wasRekey) {
            for (const m of pendingMessages) send(m)
          } else {
            if (pendingMessages.length > 0) {
              console.log('[crypto] dropped', pendingMessages.length, 'stale pending messages after re-key')
            }
          }
          pendingMessages = []
        } catch (e) {
          console.warn('[crypto] DH failed:', e)
        }
      }
      return
    }

    // encrypted message
    if (wireMsg.t === '_enc') {
      if (!sessionKey) {
        console.warn('[crypto] encrypted msg but no session key')
        return
      }
      try {
        const plaintext = await decryptPayload(sessionKey, (wireMsg.d as any).p)
        const inner: WireMessage = JSON.parse(plaintext)
        inner.relayTs = relayTs // propagate relay timestamp through encryption
        console.log('[relay] peer (dec):', inner.t)
        onPeer(inner)
      } catch (e) {
        console.warn('[crypto] decrypt failed:', e)
      }
      return
    }

    // plaintext message (pre-encryption or fallback)
    console.log('[relay] peer:', wireMsg.t)
    onPeer(wireMsg)
  }

  /** send raw (unencrypted) through relay */
  function sendRaw(msg: WireMessage) {
    if (!ws || ws.readyState !== WebSocket.OPEN || !currentRoom) return
    ws.send(JSON.stringify({ t: 'msg', text: JSON.stringify(msg) }))
  }

  /** send game message — encrypted if session key available */
  function send(msg: WireMessage) {
    if (!ws || ws.readyState !== WebSocket.OPEN || !currentRoom) return

    if (sessionKey) {
      // encrypt
      const plaintext = JSON.stringify(msg)
      encryptPayload(sessionKey, plaintext).then(encrypted => {
        sendRaw({ t: '_enc', d: { p: encrypted } })
      }).catch(e => {
        // C2: NEVER fall back to plaintext — drop the message
        console.error('[crypto] encrypt failed, message DROPPED:', e)
      })
    } else if (ephemeral) {
      // key exchange in progress, queue
      pendingMessages.push(msg)
    } else {
      // no crypto available, plaintext
      sendRaw(msg)
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
  }

  return { connect, send, disconnect, connected, encrypted }
}

/** get room code from URL path (empty = create new) */
export function getRoomFromUrl(): string {
  const path = location.pathname.replace(/^\/+|\/+$/g, '')
  if (!path || path === 'new') return ''
  return path
}

/** update URL to show room code */
export function setRoomInUrl(room: string) {
  history.replaceState(null, '', '/' + room)
}
