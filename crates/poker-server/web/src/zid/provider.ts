/**
 * zafu wallet provider detection and session delegation
 */

import type { ZidOptions } from './types'

/** localStorage slot for the persisted session keypair (JWK). */
const SESSION_KEY_STORE = 'zid_session_ed25519_v1'

/** ed25519 session keypair via Web Crypto — PERSISTED across reloads.
 *
 * The session pubkey is pinned on-chain in the escrow deposit memo (`;id:<pubkey>`), and the
 * settlement co-sign must verify against that exact pinned key. A staked match spans the
 * deposit-confirm wait PLUS a full multi-hand bust, so a page reload / reconnect mid-match used to
 * mint a NEW key — breaking the co-sign and forcing the pot into the refund/arbitration fallback
 * (the mechanism that killed the only match that ever cleared DKG). Persisting the key lets a
 * reload restore the same identity so `/settle` can actually finalize.
 *
 * The stored value is a session signing key, NOT a wallet spend key: its only authority is signing
 * game actions and co-signing settlements for rooms where its pubkey is already pinned. Persisting
 * it does not widen the XSS blast radius (a live-page XSS can already sign with the in-memory key).
 */
export async function createSessionKey() {
  let keyPair = await restorePersistedSessionKey()
  if (!keyPair) {
    keyPair = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify'])
    await persistSessionKey(keyPair)
  }
  const pubRaw = new Uint8Array(await crypto.subtle.exportKey('raw', keyPair.publicKey))
  const pubkey = hex(pubRaw)

  return {
    pubkey,
    keyPair,
    sign: async (data: Uint8Array): Promise<string> => {
      const sig = new Uint8Array(await crypto.subtle.sign('Ed25519', keyPair.privateKey, data))
      return hex(sig)
    },
    verify: async (data: Uint8Array, sigHex: string, pubkeyHex: string): Promise<boolean> => {
      const sigBytes = unhex(sigHex)
      const pubBytes = unhex(pubkeyHex)
      const key = await crypto.subtle.importKey('raw', pubBytes, 'Ed25519', false, ['verify'])
      return crypto.subtle.verify('Ed25519', key, sigBytes, data)
    },
  }
}

/** Restore the persisted session keypair from localStorage, or null if none/invalid/unavailable. */
async function restorePersistedSessionKey(): Promise<CryptoKeyPair | null> {
  try {
    const raw = localStorage.getItem(SESSION_KEY_STORE)
    if (!raw) return null
    const jwk = JSON.parse(raw)
    if (!jwk || jwk.kty !== 'OKP' || jwk.crv !== 'Ed25519' || !jwk.d || !jwk.x) return null
    const privateKey = await crypto.subtle.importKey('jwk', jwk, 'Ed25519', true, ['sign'])
    // public half from the same JWK (drop the private scalar `d`)
    const publicKey = await crypto.subtle.importKey(
      'jwk', { kty: jwk.kty, crv: jwk.crv, x: jwk.x }, 'Ed25519', true, ['verify'],
    )
    return { privateKey, publicKey }
  } catch {
    return null // corrupt entry / storage disabled → caller mints a fresh key
  }
}

/** Persist the session keypair (as a JWK) so a reload restores the same on-chain-pinned identity. */
async function persistSessionKey(keyPair: CryptoKeyPair): Promise<void> {
  try {
    const jwk = await crypto.subtle.exportKey('jwk', keyPair.privateKey)
    localStorage.setItem(SESSION_KEY_STORE, JSON.stringify(jwk))
  } catch {
    // private-browsing / storage disabled → fall back to pre-fix ephemeral behavior (reload-unsafe,
    // but no worse than before). Nothing else to do.
  }
}

/** detect zafu/penumbra wallet extension */
export async function detectZafu(): Promise<{ origin: string; provider: any } | null> {
  const providers = (globalThis as any)[Symbol.for('penumbra')]
  if (!providers) return null
  const entries = Object.entries(providers)
  if (!entries.length) return null
  const [origin, provider] = entries[0] as [string, any]
  if (!provider) return null
  return { origin, provider }
}

/** request delegation from zafu wallet */
export async function requestDelegation(
  zafu: { origin: string; provider: any },
  sessionPubkey: string,
  opts: ZidOptions = {},
): Promise<{ walletPubkey: string; signature: string; network: string } | null> {
  try {
    // connect with timeout
    await Promise.race([
      zafu.provider.connect(),
      new Promise((_, rej) => setTimeout(() => rej(new Error('timeout')), 3000)),
    ])

    const appName = opts.appName || globalThis.location?.hostname || 'zid-app'
    const delegationMsg = `zid:delegate:${sessionPubkey}:${appName}`
    const challengeHex = hex(new TextEncoder().encode(delegationMsg))

    const extId = zafu.origin.replace('chrome-extension://', '').replace(/\/$/, '')

    const resp: any = await new Promise((resolve, reject) => {
      chrome.runtime.sendMessage(extId, {
        type: 'zafu_sign',
        challengeHex,
        statement: `Authorize ${appName}\nSession: ${sessionPubkey.slice(0, 16)}...`,
        tradingMode: opts.tradingMode,
        sessionMinutes: opts.sessionMinutes || 60,
      }, (r: any) => {
        if (chrome.runtime.lastError) reject(chrome.runtime.lastError)
        else resolve(r)
      })
    })

    if (resp?.success && resp.publicKey && resp.signature) {
      return {
        walletPubkey: resp.publicKey,
        signature: resp.signature,
        network: resp.network || 'penumbra',
      }
    }
    return null
  } catch {
    return null
  }
}

/** pick contacts from zafu address book (opens extension picker UI) */
export async function pickContacts(
  zafu: { origin: string; provider: any },
  opts: { purpose?: string; max?: number; appName?: string } = {},
): Promise<{ handle: string; displayName: string }[] | null> {
  try {
    const extId = zafu.origin.replace('chrome-extension://', '').replace(/\/$/, '')
    const resp: any = await new Promise((resolve, reject) => {
      chrome.runtime.sendMessage(extId, {
        type: 'zafu_pick_contacts',
        purpose: opts.purpose || `${opts.appName || 'App'} wants to pick contacts`,
        max: opts.max || 1,
        appOrigin: globalThis.location?.origin || opts.appName || 'unknown',
      }, (r: any) => {
        if (chrome.runtime.lastError) reject(chrome.runtime.lastError)
        else resolve(r)
      })
    })

    if (resp?.success && Array.isArray(resp.contacts)) {
      return resp.contacts // [{ handle, displayName }] — handles are app-scoped BLAKE2b
    }
    return null
  } catch {
    return null
  }
}

/** send an invite to a contact via their opaque handle.
 *  zafu resolves handle→pubkey internally, delivers via e2ee channel. */
export async function sendInvite(
  zafu: { origin: string; provider: any },
  handle: string,
  payload: { type: string; data: Record<string, unknown>; ttl?: number },
  opts: { appName?: string; relayUrl?: string } = {},
): Promise<{ sent: boolean; delivered?: boolean }> {
  try {
    const extId = zafu.origin.replace('chrome-extension://', '').replace(/\/$/, '')
    const resp: any = await new Promise((resolve, reject) => {
      chrome.runtime.sendMessage(extId, {
        type: 'zafu_send_invite',
        handle,
        payload,
        appOrigin: globalThis.location?.origin || opts.appName || 'unknown',
        relayUrl: opts.relayUrl,
      }, (r: any) => {
        if (chrome.runtime.lastError) reject(chrome.runtime.lastError)
        else resolve(r)
      })
    })

    return { sent: resp?.sent ?? false, delivered: resp?.delivered }
  } catch {
    return { sent: false }
  }
}

/** subscribe to incoming invites via zafu extension */
export function listenInvites(
  zafu: { origin: string; provider: any },
  handler: (invite: {
    appOrigin: string; type: string; data: Record<string, unknown>;
    fromName: string; accept: () => void; decline: () => void;
  }) => void,
): () => void {
  const extId = zafu.origin.replace('chrome-extension://', '').replace(/\/$/, '')
  const listener = (msg: any, sender: any) => {
    if (sender.id !== extId) return
    if (msg?.type !== 'zafu_incoming_invite') return
    handler({
      appOrigin: msg.appOrigin,
      type: msg.inviteType,
      data: msg.data,
      fromName: msg.fromName,
      accept: () => {
        chrome.runtime.sendMessage(extId, {
          type: 'zafu_invite_response', id: msg.inviteId, accepted: true,
        })
      },
      decline: () => {
        chrome.runtime.sendMessage(extId, {
          type: 'zafu_invite_response', id: msg.inviteId, accepted: false,
        })
      },
    })
  }
  chrome.runtime.onMessage.addListener(listener)
  return () => chrome.runtime.onMessage.removeListener(listener)
}

// hex helpers
function hex(bytes: Uint8Array): string {
  return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('')
}
function unhex(h: string): Uint8Array {
  const bytes = new Uint8Array(h.length / 2)
  for (let i = 0; i < h.length; i += 2) bytes[i / 2] = parseInt(h.slice(i, i + 2), 16)
  return bytes
}
