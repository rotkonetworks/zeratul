/**
 * Runtime endpoint configuration — the "pick your node" layer.
 *
 * The client's ONLY backend dependency is a blind relay (the `/p2p`, `/lobby`, `/ws`, `/zid`
 * WebSocket endpoints). Historically every caller derived that from `location.host`, which
 * hard-couples the static bundle to whatever host served it. This module lets the relay origin
 * be chosen at runtime (à la the Polkadot-JS Apps node selector) and persisted in localStorage,
 * so the exact same bundle can be hosted anywhere — an IPFS gateway, Netlify, a laptop — and
 * pointed at any relay. With no override set it falls back to same-origin: byte-for-byte the
 * old behavior.
 */

const RELAY_KEY = 'poker_relay_base'

/** Curated relays offered in the settings dropdown. Community relays can be appended here. */
export const RELAY_PRESETS: { name: string; url: string }[] = [
  { name: 'zkbtc.org (official)', url: 'wss://zkbtc.org' },
]

/** Normalize any user-entered endpoint to a bare ws(s) origin (no trailing slash / path). */
function toWsOrigin(raw: string): string {
  let s = raw.trim().replace(/\/+$/, '')
  if (s.startsWith('http://')) s = 'ws://' + s.slice(7)
  else if (s.startsWith('https://')) s = 'wss://' + s.slice(8)
  else if (!s.startsWith('ws://') && !s.startsWith('wss://')) {
    // bare host[:port] → infer protocol from how the page itself was loaded
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    s = `${proto}//${s}`
  }
  // strip any path the user may have pasted — callers append their own (`/p2p`, `/lobby`, …)
  try {
    const u = new URL(s)
    return `${u.protocol}//${u.host}`
  } catch {
    return s
  }
}

/** Same-origin default: the ws(s) origin of whatever host served this bundle. */
function sameOrigin(): string {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}`
}

/** The configured relay ws origin, or same-origin if the user hasn't overridden it. */
export function relayBase(): string {
  const override = localStorage.getItem(RELAY_KEY)
  return override && override.trim() ? toWsOrigin(override) : sameOrigin()
}

/** The raw stored override ('' when using the default same-origin relay). */
export function relayOverride(): string {
  return localStorage.getItem(RELAY_KEY) ?? ''
}

/** True when no override is set (running against the host that served the bundle). */
export function isDefaultRelay(): boolean {
  const v = localStorage.getItem(RELAY_KEY)
  return !(v && v.trim())
}

/** Persist (or clear, when null/empty) the relay override. Caller reloads to apply. */
export function setRelayBase(url: string | null): void {
  if (url && url.trim()) localStorage.setItem(RELAY_KEY, toWsOrigin(url))
  else localStorage.removeItem(RELAY_KEY)
}
