/**
 * Session identity for poker.
 *
 * two modes:
 *   1. zafu wallet: ed25519 identity from extension, delegates to ephemeral session key
 *   2. anon: ephemeral ed25519 session key only (no delegation)
 *
 * the session key signs:
 *   - the x25519 encryption pubkey (authenticated key exchange)
 *   - every game action (non-repudiable action log)
 *
 * the delegation signature proves: zafu identity → session key (one popup)
 * the action signature proves: session key → specific action (no popup)
 * the jury verifies: zafu → session → action (chain of trust)
 */

/** session identity — either zafu-delegated or anonymous */
export interface SessionIdentity {
  /** 'zafu' or 'anon' */
  mode: 'zafu' | 'anon'
  /** session ed25519 public key (hex) */
  sessionPubKey: string
  /** zafu ed25519 public key (hex), only in zafu mode */
  zafuPubKey?: string
  /** delegation signature: zafu signs "delegate:{sessionPubKey}:{room}" */
  delegation?: string
  /** sign arbitrary bytes with session key */
  sign: (data: Uint8Array) => Promise<string>
  /** verify a signature from this identity */
  verify: (data: Uint8Array, sig: string, pubkey: string) => Promise<boolean>
  /** human-readable nick */
  nick: string
}

/** generate ephemeral ed25519 session keypair via Web Crypto */
async function generateSessionKey(): Promise<{
  pubHex: string
  sign: (data: Uint8Array) => Promise<string>
  verify: (data: Uint8Array, sig: string, pubkey: string) => Promise<boolean>
}> {
  const keyPair = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify'])
  const pubRaw = new Uint8Array(await crypto.subtle.exportKey('raw', keyPair.publicKey))
  const pubHex = bytesToHex(pubRaw)

  return {
    pubHex,
    sign: async (data: Uint8Array) => {
      const sig = new Uint8Array(await crypto.subtle.sign('Ed25519', keyPair.privateKey, data))
      return bytesToHex(sig)
    },
    verify: async (data: Uint8Array, sigHex: string, pubkeyHex: string) => {
      const sigBytes = hexToBytes(sigHex)
      const pubBytes = hexToBytes(pubkeyHex)
      const importedKey = await crypto.subtle.importKey('raw', pubBytes, 'Ed25519', false, ['verify'])
      return crypto.subtle.verify('Ed25519', importedKey, sigBytes, data)
    },
  }
}

/** detect zafu extension and request delegation signature */
async function zafuDelegate(sessionPubHex: string, room: string): Promise<{
  zafuPubKey: string
  delegation: string
  nick: string
} | null> {
  try {
    const providers = (window as any)[Symbol.for('penumbra')]
    if (!providers) return null
    const entries = Object.entries(providers)
    if (!entries.length) return null
    const [origin, provider] = entries[0] as [string, any]
    if (!provider) return null

    await Promise.race([
      provider.connect(),
      new Promise((_, reject) => setTimeout(() => reject(new Error('zafu timeout')), 2000))
    ])

    // the challenge is the delegation message itself
    const delegationMsg = `delegate:${sessionPubHex}:${room || 'new'}`
    const challengeHex = bytesToHex(new TextEncoder().encode(delegationMsg))

    const extId = origin.replace('chrome-extension://', '').replace(/\/$/, '')

    const resp: any = await new Promise((resolve, reject) => {
      chrome.runtime.sendMessage(extId, {
        type: 'zafu_sign',
        challengeHex,
        statement: `Authorize poker session\n${sessionPubHex.slice(0, 16)}...`,
      }, (r: any) => {
        if (chrome.runtime.lastError) reject(chrome.runtime.lastError)
        else resolve(r)
      })
    })

    if (resp?.success && resp.publicKey && resp.signature) {
      const nick = 'zid' + resp.publicKey.slice(0, 5)
      return {
        zafuPubKey: resp.publicKey,
        delegation: resp.signature,
        nick,
      }
    }
    return null
  } catch {
    return null
  }
}

/** create session identity (tries zafu first, falls back to anon) */
export async function createSessionIdentity(room: string): Promise<SessionIdentity> {
  const session = await generateSessionKey()

  // try zafu delegation
  const zafu = await zafuDelegate(session.pubHex, room)

  if (zafu) {
    console.log(`[identity] zafu delegated: ${zafu.nick} → session ${session.pubHex.slice(0, 12)}...`)
    return {
      mode: 'zafu',
      sessionPubKey: session.pubHex,
      zafuPubKey: zafu.zafuPubKey,
      delegation: zafu.delegation,
      sign: session.sign,
      verify: session.verify,
      nick: zafu.nick,
    }
  }

  // anon mode
  const nick = 'anon' + session.pubHex.slice(0, 5)
  console.log(`[identity] anon session: ${nick}`)
  return {
    mode: 'anon',
    sessionPubKey: session.pubHex,
    sign: session.sign,
    verify: session.verify,
    nick,
  }
}

/** sign a game action: seat|action|amount|seq → sig */
export async function signAction(
  identity: SessionIdentity,
  seat: number,
  action: string,
  amount: number,
  seq: number,
): Promise<string> {
  const msg = `${seat}|${action}|${amount}|${seq}`
  return identity.sign(new TextEncoder().encode(msg))
}

/** sign an x25519 public key for authenticated key exchange */
export async function signKeyExchange(
  identity: SessionIdentity,
  x25519PubB64: string,
): Promise<string> {
  const msg = `keyex:${x25519PubB64}`
  return identity.sign(new TextEncoder().encode(msg))
}

// helpers
function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('')
}
function hexToBytes(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2)
  for (let i = 0; i < hex.length; i += 2) bytes[i / 2] = parseInt(hex.slice(i, i + 2), 16)
  return bytes
}
