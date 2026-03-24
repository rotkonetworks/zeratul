/**
 * zid — the simplest possible identity SDK
 *
 * detects zafu wallet → requests session → signs actions → opens e2ee channels
 * falls back to ephemeral browser keys if no wallet
 */

import type { ZidIdentity, ZidChannel, ZidOptions } from './types'
import { createSessionKey, detectZafu, requestDelegation } from './provider'
import { createChannel } from './channel'

/** the zid singleton */
export const zid = {
  /**
   * connect to zafu wallet or generate ephemeral identity.
   *
   * ```typescript
   * const me = await zid.connect({ appName: 'poker.zk.bot' })
   * console.log(me.pubkey) // ed25519 hex
   * ```
   */
  async connect(opts: ZidOptions = {}): Promise<ZidIdentity> {
    const session = await createSessionKey()

    if (!opts.ephemeral) {
      const zafu = await detectZafu()
      if (zafu) {
        const delegation = await requestDelegation(zafu, session.pubkey, opts)
        if (delegation) {
          const name = localStorage.getItem('zid_name') || delegation.walletPubkey.slice(0, 8)
          return {
            pubkey: session.pubkey,
            network: delegation.network,
            name,
            sign: session.sign,
            verify: session.verify,
            channel: (peer: string) => createChannel(session, peer, opts.relayUrl),
            mode: 'zafu',
            walletPubkey: delegation.walletPubkey,
            delegation: delegation.signature,
            disconnect: () => { /* clear session */ },
          }
        }
      }
    }

    // ephemeral mode — no wallet
    const name = localStorage.getItem('zid_name') || session.pubkey.slice(0, 8)
    return {
      pubkey: session.pubkey,
      network: 'none',
      name,
      sign: session.sign,
      verify: session.verify,
      channel: (peer: string) => createChannel(session, peer, opts.relayUrl),
      mode: 'ephemeral',
      disconnect: () => { /* clear session */ },
    }
  },

  /** set display name (persisted in localStorage) */
  setName(name: string) {
    localStorage.setItem('zid_name', name)
  },

  /** get stored display name */
  getName(): string | null {
    return localStorage.getItem('zid_name')
  },
}
