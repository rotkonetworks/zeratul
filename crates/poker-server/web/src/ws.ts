/**
 * WebSocket adapter — bridges relay transport + P2P game engine (poker-pvm WASM)
 * to the existing App.tsx which expects createSocket API.
 */

import { createSignal } from 'solid-js'
import type { ServerMsg } from './types'
import { createRelayTransport, getRoomFromUrl, setRoomInUrl } from './transport'
import { createGame, loadWasmEngine } from './game'
import type { WireMessage } from './transport'

export type SendFn = (data: Record<string, unknown>) => void

export function createSocket(onMsg: (msg: ServerMsg) => void) {
  const [connected, setConnected] = createSignal(false)
  let game: ReturnType<typeof createGame> | null = null
  let transport: ReturnType<typeof createRelayTransport> | null = null

  async function connect(name: string) {
    // load poker-pvm WASM engine
    const wasmOk = await loadWasmEngine()
    if (wasmOk) console.log('[poker] WASM engine loaded (deterministic)')
    else console.log('[poker] using JS fallback')

    const room = getRoomFromUrl()
    const isHost = !room

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
            console.log('[ws] opponent_joined, isHost:', isHost)
            onMsg({ type: 'OpponentJoined', seat: isHost ? 1 : 0, name: 'opponent' })
            if (isHost) {
              console.log('[ws] host announcing')
              game?.announce()
            }
            break
          case 'opponent_left':
            onMsg({ type: 'OpponentLeft', seat: isHost ? 1 : 0 })
            break
          case 'error':
            onMsg({ type: 'Error', message: data ?? 'unknown error' })
            break
        }
      },
    )

    game = createGame(transport, isHost, name, {
      onMsg,
      onLog: (t) => console.log('[game]', t),
      onRulesProposed: (_rules, fromSelf) => {
        // auto-accept opponent's rules for demo
        if (!fromSelf) {
          console.log('[game] auto-accepting rules')
          game?.acceptRules()
        }
      },
      onRulesAccepted: () => console.log('[game] rules accepted'),
      onEscrowReady: (addr) => onMsg({ type: 'RoomInfo', code: room ?? '', jury_nodes: 5, jury_threshold: 3, escrow: addr }),
      onDepositConfirmed: () => {},
    })

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

  return { connected, connect, send }
}
