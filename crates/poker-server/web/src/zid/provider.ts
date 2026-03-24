/**
 * zafu wallet provider detection and session delegation
 */

import type { ZidOptions } from './types'

/** ed25519 session keypair via Web Crypto */
export async function createSessionKey() {
  const keyPair = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify'])
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

// hex helpers
function hex(bytes: Uint8Array): string {
  return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('')
}
function unhex(h: string): Uint8Array {
  const bytes = new Uint8Array(h.length / 2)
  for (let i = 0; i < h.length; i += 2) bytes[i / 2] = parseInt(h.slice(i, i + 2), 16)
  return bytes
}
