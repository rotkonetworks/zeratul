import { createSignal, For, Show } from 'solid-js'
import { RELAY_PRESETS, relayBase, relayOverride, isDefaultRelay, setRelayBase } from './config'

/**
 * Relay endpoint picker — the "which node am I on" panel, à la Polkadot-JS Apps.
 *
 * The client's only backend is a blind relay; this lets the user point the (portable, static)
 * bundle at any relay and persists the choice. Applying reloads the page so every WebSocket
 * re-establishes against the new origin — same approach Polkadot-JS uses on a network switch.
 */
export function Settings(props: { connected: boolean; onClose: () => void }) {
  // "" is the sentinel for the same-origin default (the host that served the bundle).
  const initial = isDefaultRelay() ? '' : relayOverride()
  const presetMatch = RELAY_PRESETS.find(p => p.url === initial)
  const [choice, setChoice] = createSignal<string>(
    initial === '' ? '__default__' : presetMatch ? presetMatch.url : '__custom__',
  )
  const [custom, setCustom] = createSignal(presetMatch || initial === '' ? '' : initial)

  const currentOrigin = () => new URL(relayBase()).host

  function apply() {
    const c = choice()
    if (c === '__default__') setRelayBase(null)
    else if (c === '__custom__') {
      const v = custom().trim()
      if (!v) return
      setRelayBase(v)
    } else {
      setRelayBase(c) // a preset url
    }
    // reload so transport/lobby/zid sockets all re-dial the new relay cleanly
    location.reload()
  }

  const dirty = () => {
    const c = choice()
    if (c === '__default__') return !isDefaultRelay()
    if (c === '__custom__') return custom().trim() !== '' && custom().trim() !== relayOverride()
    return c !== relayOverride()
  }

  return (
    <div
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
      onClick={props.onClose}
    >
      <div
        class="w-full max-w-md bg-zec-dark border border-white/12 rounded-lg p-5 text-white"
        onClick={(e) => e.stopPropagation()}
      >
        <div class="flex items-center justify-between mb-1">
          <h2 class="text-14px text-zec-yellow tracking-wide">settings · relay</h2>
          <button class="text-neutral-500 hover:text-white text-16px leading-none" onClick={props.onClose}>×</button>
        </div>
        <p class="text-10px text-neutral-500 mb-4 leading-relaxed">
          zk.poker is a static client — its only network link is a blind relay for matchmaking and
          encrypted peer messages. Point it at any relay; your choice is saved on this device.
        </p>

        <div class="flex items-center gap-2 mb-4 text-10px">
          <span class={`w-2 h-2 rounded-full ${props.connected ? 'bg-green-500' : 'bg-amber-500/70'}`} />
          <span class="text-neutral-400">
            {props.connected ? 'live connection to' : 'idle — connects when you sit at a table ·'} <span class="font-mono text-neutral-200">{currentOrigin()}</span>
          </span>
        </div>

        <div class="flex flex-col gap-1.5">
          {/* same-origin default */}
          <label class="flex items-center gap-2 px-3 py-2 rounded border border-white/8 hover:border-white/20 cursor-pointer">
            <input type="radio" name="relay" checked={choice() === '__default__'} onChange={() => setChoice('__default__')} />
            <span class="flex-1 text-12px">this host <span class="text-neutral-500">(default · {location.host})</span></span>
          </label>

          {/* curated presets — hide any that resolve to the current host, since "this host"
              above already offers it (avoids a duplicate e.g. zkbtc.org shown twice). */}
          <For each={RELAY_PRESETS.filter(p => { try { return new URL(p.url).host !== location.host } catch { return true } })}>{(p) => (
            <label class="flex items-center gap-2 px-3 py-2 rounded border border-white/8 hover:border-white/20 cursor-pointer">
              <input type="radio" name="relay" checked={choice() === p.url} onChange={() => setChoice(p.url)} />
              <span class="flex-1 text-12px">{p.name} <span class="font-mono text-neutral-500 text-10px">{p.url}</span></span>
            </label>
          )}</For>

          {/* custom */}
          <label class="flex items-center gap-2 px-3 py-2 rounded border border-white/8 hover:border-white/20 cursor-pointer">
            <input type="radio" name="relay" checked={choice() === '__custom__'} onChange={() => setChoice('__custom__')} />
            <span class="flex-1 text-12px">custom relay</span>
          </label>
          <Show when={choice() === '__custom__'}>
            <input
              class="ml-6 px-3 py-1.5 bg-black/40 border border-white/12 rounded text-12px font-mono text-neutral-200 placeholder:text-neutral-600 outline-none focus:border-zec-yellow/50"
              placeholder="wss://relay.example.org  or  relay.example.org:3000"
              value={custom()}
              onInput={(e) => setCustom(e.currentTarget.value)}
            />
          </Show>
        </div>

        <div class="flex items-center justify-end gap-2 mt-5">
          <button class="text-11px px-3 py-1.5 text-neutral-500 hover:text-neutral-300" onClick={props.onClose}>cancel</button>
          <button
            class="text-11px px-4 py-1.5 rounded bg-zec-yellow text-black font-medium disabled:opacity-40 disabled:cursor-not-allowed"
            disabled={!dirty()}
            onClick={apply}
          >save &amp; reconnect</button>
        </div>
      </div>
    </div>
  )
}
