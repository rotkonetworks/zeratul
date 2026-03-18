/**
 * pluggable transport layer (finagle pattern).
 *
 * the game engine emits ServerMsg locally. the transport sends actions
 * to the peer and receives their actions. the relay is just a pipe.
 *
 * swappable: WebSocket relay, WebRTC, iroh, nym.
 */

import { createSignal } from 'solid-js'
import type { ServerMsg } from './types'

/** what we send over the wire (opaque to relay) */
export interface WireMessage {
  /** message type tag */
  t: string
  /** JSON payload */
  d: unknown
}

/** transport provider interface */
export interface TransportProvider {
  connect(room: string, nick: string): void
  send(msg: WireMessage): void
  disconnect(): void
  readonly connected: () => boolean
}

/** callback for incoming peer messages */
export type OnPeerMessage = (msg: WireMessage) => void

/** callback for room events */
export type OnRoomEvent = (event: 'joined' | 'opponent_joined' | 'opponent_left' | 'error', data?: string) => void

/** WebSocket relay transport (relay.zk.bot) */
export function createRelayTransport(
  onPeer: OnPeerMessage,
  onRoom: OnRoomEvent,
): TransportProvider {
  const [connected, setConnected] = createSignal(false)
  let ws: WebSocket | null = null
  let currentRoom: string | null = null
  let currentNick = 'anon'
  let isCreator = false
  let hasJoined = false

  function connect(room: string, nick: string) {
    currentNick = nick

    // if room is empty, we're creating; otherwise joining
    isCreator = !room
    const relayUrl = 'wss://relay.zk.bot/ws'

    ws = new WebSocket(relayUrl)

    ws.onopen = () => {
      setConnected(true)
      if (isCreator) {
        // create room
        ws!.send(JSON.stringify({ t: 'create', nick }))
      } else {
        // join existing room
        currentRoom = room
        ws!.send(JSON.stringify({ t: 'join', room, nick }))
      }
    }

    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data)
        handleRelayMsg(msg)
      } catch {}
    }

    ws.onclose = () => {
      setConnected(false)
      // reconnect after 3s
      setTimeout(() => { if (currentRoom) connect(currentRoom, currentNick) }, 3000)
    }
  }

  function handleRelayMsg(msg: Record<string, unknown>) {
    console.log('[relay]', msg['t'], msg)
    switch (msg['t']) {
      case 'created':
        currentRoom = msg['room'] as string
        // auto-join the room we created
        ws?.send(JSON.stringify({ t: 'join', room: currentRoom, nick: currentNick }))
        break

      case 'joined': {
        currentRoom = msg['room'] as string
        const count = msg['count'] as number
        if (!hasJoined) {
          // this is US joining
          hasJoined = true
          onRoom('joined', currentRoom)
          if (count >= 2) {
            // opponent was already here
            onRoom('opponent_joined')
          }
        } else {
          // we already joined — this is a replay of someone else joining
          if (count >= 2) {
            onRoom('opponent_joined')
          }
        }
        break
      }

      case 'msg': {
        // ignore replayed messages from before we joined
        if (!hasJoined) break
        const text = msg['text'] as string
        const nick = msg['nick'] as string
        // ignore own messages (relay echoes everything)
        if (nick === currentNick) break
        console.log('[relay] peer:', nick, text.slice(0, 60))
        try {
          const wireMsg: WireMessage = JSON.parse(text)
          onPeer(wireMsg)
        } catch { /* not structured */ }
        break
      }

      case 'system':
        const text = msg['text'] as string
        if (text.includes('joined')) {
          onRoom('opponent_joined')
        } else if (text.includes('left') || text.includes('closed')) {
          onRoom('opponent_left')
        }
        break

      case 'error':
        onRoom('error', msg['msg'] as string)
        break
    }
  }

  function send(msg: WireMessage) {
    if (!ws || ws.readyState !== WebSocket.OPEN || !currentRoom) return
    // pack as relay message: nick\0json inside the relay payload
    const payload = JSON.stringify(msg)
    ws.send(JSON.stringify({ t: 'msg', text: payload }))
  }

  function disconnect() {
    ws?.send(JSON.stringify({ t: 'part' }))
    ws?.close()
    ws = null
    currentRoom = null
  }

  return { connect, send, disconnect, connected }
}

/** get room code from URL path (empty = create new) */
export function getRoomFromUrl(): string {
  const path = location.pathname.replace(/^\/+|\/+$/g, '')
  // 'new' means create, not join
  if (!path || path === 'new') return ''
  return path
}

/** update URL to show room code */
export function setRoomInUrl(room: string) {
  history.replaceState(null, '', '/' + room)
}
