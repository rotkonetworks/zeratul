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
    const ws = new WebSocket(`${proto}//${location.host}/${room}/ws`)

    ws.onopen = () => {
      setConnected(true)
      // pubkey gates reconnect server-side: a later reload with a different pubkey is
      // refused, defeating name-based seat hijack. Falls back to name-only if undefined.
      const join: Record<string, unknown> = { type: 'Join', name }
      if (pubkey) join.pubkey = pubkey
      if (zcashAddress) join.zcash_address = zcashAddress
      ws.send(JSON.stringify(join))
    }

    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data)
        onMsg(msg)
      } catch {}
    }

    ws.onclose = () => setConnected(false)

    return {
      send: (data: Record<string, unknown>) => {
        if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(data))
      },
      close: () => ws.close(),
    }
  }

  let directWs: { send: (data: Record<string, unknown>) => void; close: () => void } | null = null

  async function connect(name: string, customRules?: { smallBlind: number; bigBlind: number; buyin: number }) {
    // try direct server mode first (centralized WebSocket)
    const directRoom = getRoomFromUrl()
    if (directRoom) {
      onMsg({ type: 'Status', phase: 'connecting', message: 'connecting to table...' })
      // resolve zid identity BEFORE the WS so we can pass pubkey on Join. Pubkey gates
      // server-side reconnect so the seat can't be hijacked by a same-name attacker.
      // identity() also exposes the pubkey to App.tsx for the UI badge.
      let pubkey: string | undefined
      try {
        const zidIdentity = await zid.connect({ appName: 'poker.zk.bot', tradingMode: true })
        if (name) zid.setName(name)
        const sess: SessionIdentity = {
          ...zidIdentity,
          sessionPubKey: zidIdentity.pubkey,
          nick: zidIdentity.name,
        }
        setIdentity(sess)
        // walletPubkey is the persistent zafu identity; session pubkey is regenerated every connect()
        pubkey = zidIdentity.walletPubkey || zidIdentity.pubkey
      } catch (e) {
        console.warn('[ws] zid.connect failed, joining anon:', e)
      }
      const ws = await connectDirect(name, pubkey)
      if (ws) {
        directWs = ws
        return
      }
    }

    // fallback: P2P relay mode
    onMsg({ type: 'Status', phase: 'connecting', message: 'loading game engine...' })
    const wasmOk = await loadWasmEngine()
    if (!wasmOk) onMsg({ type: 'Error', message: 'game engine failed to load — using fallback' })

    const room = getRoomFromUrl()
    const isHost = !room

    // create session identity via zid SDK (tries zafu, falls back to ephemeral)
    const zidIdentity = await zid.connect({ appName: 'poker.zk.bot', tradingMode: true })
    if (name) zid.setName(name)
    const sess: SessionIdentity = {
      ...zidIdentity,
      sessionPubKey: zidIdentity.pubkey,
      nick: zidIdentity.name,
    }
    setIdentity(sess)
    if (zidIdentity.mode === 'zafu') name = zidIdentity.name

    transport = createRelayTransport(
      (msg: WireMessage) => {
        // media signaling filter
        if (msg.t === '_sdp' || msg.t === '_ice') {
          media?.handleSignal(msg)
          return
        }
        // chat filter
        if (msg.t === 'chat') {
          const text = (msg.d as any)?.text
          if (text) onMsg({ type: 'Chat', from: 'opp', text })
          return
        }
        game?.onPeerMessage(msg)
      },
      (event, data) => {
        switch (event) {
          case 'joined':
            setConnected(true)
            console.log('[ws] joined room:', data, 'isHost:', isHost)
            if (data) {
              setRoomInUrl(data)
              onMsg({ type: 'RoomInfo', code: data, jury_nodes: 5, jury_threshold: 3, escrow: '' })
              onMsg({ type: 'InviteLink', url: '/' + data })
            }
            if (!isHost) {
              console.log('[ws] guest announcing')
              game?.announce()
            }
            break
          case 'opponent_joined':
            console.log('[ws] opponent_joined, isHost:', isHost, 'announced:', announced)
            onMsg({ type: 'OpponentJoined', seat: isHost ? 1 : 0, name: 'opponent' })
            if (isHost && !announced) {
              announced = true
              console.log('[ws] host announcing (once)')
              game?.announce()
            }
            break
          case 'opponent_left':
            onMsg({ type: 'OpponentLeft', seat: isHost ? 1 : 0 })
            break
          case 'opponent_disconnected':
            onMsg({ type: 'OpponentDisconnected', seat: isHost ? 1 : 0, reconnect_secs: parseInt(data ?? '60') })
            game?.pauseTimer()
            break
          case 'opponent_reconnected':
            onMsg({ type: 'OpponentReconnected', seat: isHost ? 1 : 0 })
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

    // media: WebRTC voice/video (opt-in, signaling through encrypted relay)
    media = createMedia((msg) => transport!.send(msg))

    game = createGame(transport, isHost, name, {
      onMsg,
      onLog: (t) => console.log('[game]', t),
      onRulesProposed: (rules, fromSelf) => {
        onMsg({ type: 'RulesProposed', buyin: rules.buyin, smallBlind: rules.smallBlind, bigBlind: rules.bigBlind, fromSelf })
        // guest auto-accepts host's rules (they chose to join this table)
        if (!fromSelf && !isHost) {
          setTimeout(() => game?.acceptRules(), 500)
        }
      },
      onRulesAccepted: () => {
        onMsg({ type: 'RulesAccepted' })
      },
      onEscrowReady: (addr) => onMsg({ type: 'RoomInfo', code: room ?? '', jury_nodes: 5, jury_threshold: 3, escrow: addr }),
      onDepositConfirmed: () => {},
      onTimerTick: (s) => { if (s >= 0) onMsg({ type: 'TimerTick', secondsLeft: s }) },
    }, sess, customRules)

    onMsg({ type: 'Seated', seat: isHost ? 0 : 1, name })
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
    }
  }

  return { connected, connect, send, identity, encrypted: encSignal, media: () => media }
}
