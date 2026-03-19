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
import { createSessionIdentity, signKeyExchange } from './identity'
import type { SessionIdentity } from './identity'
import type { WireMessage } from './transport'

export type SendFn = (data: Record<string, unknown>) => void

export function createSocket(onMsg: (msg: ServerMsg) => void) {
  const [connected, setConnected] = createSignal(false)
  const [encSignal, setEncSignal] = createSignal(false)
  const [identity, setIdentity] = createSignal<SessionIdentity | null>(null)
  let game: ReturnType<typeof createGame> | null = null
  let transport: ReturnType<typeof createRelayTransport> | null = null
  let announced = false

  async function connect(name: string, customRules?: { smallBlind: number; bigBlind: number; buyin: number }) {
    // load WASM engines (game + shuffle)
    const wasmOk = await loadWasmEngine()
    if (wasmOk) console.log('[poker] WASM engine loaded')
    else console.log('[poker] JS fallback')

    const room = getRoomFromUrl()
    const isHost = !room

    // create session identity (tries zafu delegation, falls back to anon)
    const sess = await createSessionIdentity(room)
    setIdentity(sess)
    // zafu overrides name with wallet identity; anon keeps user's typed name
    if (sess.mode === 'zafu') name = sess.nick

    transport = createRelayTransport(
      (msg: WireMessage) => game?.onPeerMessage(msg),
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
            break
          case 'error':
            onMsg({ type: 'Error', message: data ?? 'unknown error' })
            break
        }
      },
      sess, // pass identity for authenticated key exchange
    )

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
    if (data['type'] === 'Action') {
      game?.act((data['action'] as string).toLowerCase(), (data['amount'] as number) || 0)
    } else if (data['type'] === 'StartHand') {
      game?.dealHand()
    } else if (data['type'] === 'ProposeRules') {
      game?.proposeRules(data as any)
    } else if (data['type'] === 'AcceptRules') {
      game?.acceptRules()
    }
  }

  return { connected, connect, send, identity, encrypted: encSignal }
}
