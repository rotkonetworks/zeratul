/**
 * WebSocket adapter — bridges relay transport + P2P game engine (poker-pvm WASM)
 * to the existing App.tsx which expects createSocket API.
 *
 * identity: zafu wallet (ed25519 delegation) or anonymous (ephemeral ed25519).
 * encryption: ephemeral x25519 DH → AES-256-GCM. all messages encrypted.
 * signing: session key signs every action (non-repudiable log for disputes).
 */

import { createSignal } from 'solid-js'
import type { ServerMsg } from './types'
import { createRelayTransport, getRoomFromUrl, setRoomInUrl } from './transport'
import { createGame, loadWasmEngine } from './game'
import { zid } from './zid'
import type { ZidIdentity } from './zid'
import { signKeyExchange } from './identity'
// SessionIdentity is now ZidIdentity
type SessionIdentity = ZidIdentity & { sessionPubKey: string; nick: string }
import type { WireMessage } from './transport'
import { createMedia } from './media'
import type { MediaState } from './media'

export type SendFn = (data: Record<string, unknown>) => void

export function createSocket(onMsg: (msg: ServerMsg) => void) {
  const [connected, setConnected] = createSignal(false)
  const [encSignal, setEncSignal] = createSignal(false)
  const [identity, setIdentity] = createSignal<SessionIdentity | null>(null)
  let game: ReturnType<typeof createGame> | null = null
  let transport: ReturnType<typeof createRelayTransport> | null = null
  let announced = false
  let media: MediaState | null = null

  /** direct WebSocket to server (centralized mode, not P2P) */
  async function connectDirect(name: string, pubkey?: string, zcashAddress?: string) {
    const room = getRoomFromUrl()
    if (!room) return

    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    let ws: WebSocket
    let intentionalClose = false
    let reconnectAttempts = 0
    const maxRetries = 10

    function open() {
      ws = new WebSocket(`${proto}//${location.host}/${room}/ws`)

      ws.onopen = () => {
        setConnected(true)
        reconnectAttempts = 0
        // pubkey gates reconnect server-side: a later reload with a different pubkey is
        // refused, defeating name-based seat hijack. Falls back to name-only if undefined.
        // re-sent verbatim on every (re)connect — the server matches name+pubkey to the
        // disconnected seat and replays game/deposit/settlement state.
        const join: Record<string, unknown> = { type: 'Join', name }
        if (pubkey) join.pubkey = pubkey
        if (zcashAddress) join.zcash_address = zcashAddress
        ws.send(JSON.stringify(join))
      }

      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(ev.data)
          // terminal states: the room is being torn down server-side, so stop reconnecting
          // (a reconnect after cleanup would spawn a fresh empty room).
          if (msg.type === 'PayoutComplete' || msg.type === 'PayoutFailed') intentionalClose = true
          onMsg(msg)
        } catch {}
      }

      ws.onclose = () => {
        setConnected(false)
        if (intentionalClose) return
        if (reconnectAttempts < maxRetries) {
          reconnectAttempts++
          const delay = Math.min(reconnectAttempts, 5) * 1000
          console.log(`[ws] disconnected, reconnecting in ${delay}ms (attempt ${reconnectAttempts}/${maxRetries})`)
          setTimeout(open, delay)
        } else {
          console.log('[ws] reconnect attempts exhausted')
          onMsg({ type: 'Error', message: 'connection lost — reload to rejoin' })
        }
      }
    }

    open()

    return {
      send: (data: Record<string, unknown>) => {
        if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(data))
      },
      close: () => { intentionalClose = true; ws.close() },
    }
  }

  let directWs: { send: (data: Record<string, unknown>) => void; close: () => void } | null = null

  async function connect(name: string, customRules?: { smallBlind: number; bigBlind: number; buyin: number }) {
    // P2P over the blind relay: both clients run the engine + mental-poker
    // ceremony locally and exchange only ciphertext through the server relay.
    // the operator never sees cards, and the two players never open a socket to
    // each other (no peer IP leak). the server engine at /{code}/ws is unused.
    onMsg({ type: 'Status', phase: 'connecting', message: 'loading game engine...' })
    const wasmOk = await loadWasmEngine()
    if (!wasmOk) onMsg({ type: 'Error', message: 'game engine failed to load — using fallback' })

    const room = getRoomFromUrl()

    // session identity via zid SDK (zafu wallet, or ephemeral ed25519)
    const zidIdentity = await zid.connect({ appName: 'poker.zk.bot', tradingMode: true })
    if (name) zid.setName(name)
    const sess: SessionIdentity = {
      ...zidIdentity,
      sessionPubKey: zidIdentity.pubkey,
      nick: zidIdentity.name,
    }
    setIdentity(sess)
    if (zidIdentity.mode === 'zafu') name = zidIdentity.name

    // host/guest is decided by the relay-assigned seat (join order), NOT the URL:
    // whoever joins the relay room first is seat 0 (host). the game is built once,
    // as soon as we learn our seat. (falls back to URL if the relay omits a seat.)
    let mySeat = -1

    function buildGame(isHost: boolean) {
      if (game) return
      // media: WebRTC voice/video (opt-in, direct P2P, signaling over the relay)
      media = createMedia((msg) => transport!.send(msg))
      game = createGame(transport!, isHost, name, {
        onMsg,
        onLog: (t) => console.log('[game]', t),
        onRulesProposed: (rules, fromSelf) => {
          onMsg({ type: 'RulesProposed', buyin: rules.buyin, smallBlind: rules.smallBlind, bigBlind: rules.bigBlind, fromSelf })
          // guest auto-accepts host's rules (they chose to join this table)
          if (!fromSelf && !isHost) setTimeout(() => game?.acceptRules(), 500)
        },
        onRulesAccepted: () => onMsg({ type: 'RulesAccepted' }),
        onEscrowReady: (addr) => onMsg({ type: 'RoomInfo', code: room ?? '', jury_nodes: 5, jury_threshold: 3, escrow: addr }),
        onDepositConfirmed: () => {},
        onTimerTick: (s) => { if (s >= 0) onMsg({ type: 'TimerTick', secondsLeft: s }) },
      }, sess, customRules)
    }

    transport = createRelayTransport(
      (msg: WireMessage) => {
        // media signaling filter
        if (msg.t === '_sdp' || msg.t === '_ice') { media?.handleSignal(msg); return }
        // chat filter
        if (msg.t === 'chat') {
          const text = (msg.d as any)?.text
          if (text) onMsg({ type: 'Chat', from: 'opp', text })
          return
        }
        game?.onPeerMessage(msg)
      },
      (event, data, seat) => {
        switch (event) {
          case 'joined':
            setConnected(true)
            if (mySeat < 0) {
              mySeat = typeof seat === 'number' ? seat : (room ? 1 : 0)
              console.log('[ws] joined room:', data, 'seat:', mySeat)
              buildGame(mySeat === 0)
              onMsg({ type: 'Seated', seat: mySeat, name })
              // guest (joined second) announces immediately; host announces when
              // the opponent arrives (opponent_joined below).
              if (mySeat === 1) game?.announce()
            }
            if (data) {
              setRoomInUrl(data)
              onMsg({ type: 'RoomInfo', code: data, jury_nodes: 5, jury_threshold: 3, escrow: '' })
              onMsg({ type: 'InviteLink', url: '/' + data })
            }
            break
          case 'opponent_joined':
            console.log('[ws] opponent_joined, mySeat:', mySeat, 'announced:', announced)
            onMsg({ type: 'OpponentJoined', seat: mySeat === 0 ? 1 : 0, name: 'opponent' })
            if (mySeat === 0 && !announced) {
              announced = true
              console.log('[ws] host announcing (once)')
              game?.announce()
            }
            break
          case 'opponent_left':
            onMsg({ type: 'OpponentLeft', seat: mySeat === 0 ? 1 : 0 })
            break
          case 'opponent_disconnected':
            onMsg({ type: 'OpponentDisconnected', seat: mySeat === 0 ? 1 : 0, reconnect_secs: parseInt(data ?? '60') })
            game?.pauseTimer()
            break
          case 'opponent_reconnected':
            onMsg({ type: 'OpponentReconnected', seat: mySeat === 0 ? 1 : 0 })
            game?.resumeTimer()
            break
          case 'encrypted':
            setEncSignal(true)
            onMsg({ type: 'Status', phase: 'encrypting', message: 'encrypted channel established' })
            break
          case 'error':
            onMsg({ type: 'Error', message: data ?? 'unknown error' })
            break
        }
      },
      sess, // pass identity for authenticated key exchange
    )

    transport.connect(room, name)
  }

  const send: SendFn = (data) => {
    // direct server mode: send JSON to server WebSocket
    if (directWs) {
      directWs.send(data)
      return
    }
    // P2P relay mode: route through game engine
    if (data['type'] === 'Action') {
      game?.act((data['action'] as string).toLowerCase(), (data['amount'] as number) || 0)
    } else if (data['type'] === 'StartHand') {
      game?.dealHand()
    } else if (data['type'] === 'ProposeRules') {
      game?.proposeRules(data as any)
    } else if (data['type'] === 'AcceptRules') {
      game?.acceptRules()
    } else if (data['type'] === 'Chat') {
      transport?.send({ t: 'chat', d: { text: data['text'] } })
    } else if (data['type'] === 'Leave') {
      // explicit leave: drop the relay socket so the opponent is notified
      transport?.disconnect()
    }
  }

  return { connected, connect, send, identity, encrypted: encSignal, media: () => media }
}
