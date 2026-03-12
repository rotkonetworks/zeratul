import { createSignal } from 'solid-js'
import type { ServerMsg } from './types'

export type SendFn = (data: Record<string, unknown>) => void

export function createSocket(onMsg: (msg: ServerMsg) => void) {
  const [connected, setConnected] = createSignal(false)
  let ws: WebSocket | null = null

  function connect(name: string) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    ws = new WebSocket(`${proto}//${location.host}/ws`)
    ws.onopen = () => {
      setConnected(true)
      ws!.send(JSON.stringify({ type: 'Join', name }))
    }
    ws.onmessage = (e) => {
      try { onMsg(JSON.parse(e.data)) } catch {}
    }
    ws.onclose = () => setConnected(false)
  }

  const send: SendFn = (data) => {
    ws?.send(JSON.stringify(data))
  }

  return { connected, connect, send }
}
