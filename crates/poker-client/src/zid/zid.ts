/**
 * zid — the simplest possible identity SDK
 *
 * detects zafu wallet → requests session → signs actions → opens e2ee channels
 * falls back to ephemeral browser keys if no wallet
 *
 * contacts live in zid (localStorage), not in the wallet.
 * zafu provides richer contacts — zid uses them when available, works without.
 */

import type { ZidIdentity, ZidOptions, PickContactsOptions, InvitePayload, InviteResult, IncomingInvite } from './types'
import { createSessionKey, detectZafu, requestDelegation, pickContacts as providerPickContacts, sendInvite, listenInvites } from './provider'
import { createChannel } from './channel'
import { getContactRefs, resolveHandle, upsertContact } from './contacts'

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
    const appOrigin = globalThis.location?.origin || opts.appName || 'unknown'

    if (!opts.ephemeral) {
      const zafu = await detectZafu()
      if (zafu) {
        const delegation = await requestDelegation(zafu, session.pubkey, opts)
        if (delegation) {
          const appKey = opts.appName ? `zid_name:${opts.appName}` : 'zid_name'
          const name = localStorage.getItem(appKey) || localStorage.getItem('zid_name') || delegation.walletPubkey.slice(0, 8)
          return {
            pubkey: session.pubkey,
            network: delegation.network,
            name,
            sign: session.sign,
            verify: session.verify,
            channel: (peer: string) => createChannel(session, peer, opts.relayUrl),
            pickContacts: async (pickOpts?: PickContactsOptions) => {
              // try zafu wallet picker first
              const result = await providerPickContacts(zafu, { ...pickOpts, appName: opts.appName })
              if (result && result.length > 0) return result
              // fallback to zid local contacts
              return getContactRefs(appOrigin)
            },
            invite: async (handle: string, payload: InvitePayload) => {
              // try zafu-routed invite first (e2ee via wallet)
              const result = await sendInvite(zafu, handle, payload, { appName: opts.appName, relayUrl: opts.relayUrl })
              if (result.sent) return result
              // fallback: resolve handle locally and send via zid channel
              const pubkey = resolveHandle(handle, appOrigin)
              if (pubkey) {
                const ch = await createChannel(session, pubkey, opts.relayUrl)
                ch.send(JSON.stringify({ type: 'zid:invite', payload, from: name, appOrigin }))
                // don't close channel immediately — recipient needs time to receive
                setTimeout(() => ch.close(), 30_000)
                return { sent: true }
              }
              return { sent: false }
            },
            onInvite: (handler: (invite: IncomingInvite) => void) => {
              return listenInvites(zafu, handler)
            },
            mode: 'zafu',
            walletPubkey: delegation.walletPubkey,
            delegation: delegation.signature,
            disconnect: () => { /* clear session */ },
          }
        }
      }
    }

    // ephemeral mode — no wallet, zid-only contacts
    const ephAppKey = opts.appName ? `zid_name:${opts.appName}` : 'zid_name'
    const name = localStorage.getItem(ephAppKey) || localStorage.getItem('zid_name') || session.pubkey.slice(0, 8)
    return {
      pubkey: session.pubkey,
      network: 'none',
      name,
      sign: session.sign,
      verify: session.verify,
      channel: (peer: string) => createChannel(session, peer, opts.relayUrl),
      pickContacts: async (pickOpts?: PickContactsOptions) => {
        return getContactRefs(appOrigin)
      },
      invite: async (handle: string, payload: InvitePayload) => {
        const pubkey = resolveHandle(handle, appOrigin)
        if (pubkey) {
          const ch = await createChannel(session, pubkey, opts.relayUrl)
          ch.send(JSON.stringify({ type: 'zid:invite', payload, from: name, appOrigin }))
          setTimeout(() => ch.close(), 30_000)
          return { sent: true }
        }
        return { sent: false }
      },
      mode: 'ephemeral',
      disconnect: () => { /* clear session */ },
    }
  },

  /** set display name — per-app to prevent cross-app correlation */
  setName(name: string, appName?: string) {
    const key = appName ? `zid_name:${appName}` : 'zid_name'
    localStorage.setItem(key, name)
  },

  /** get stored display name for this app */
  getName(appName?: string): string | null {
    const key = appName ? `zid_name:${appName}` : 'zid_name'
    return localStorage.getItem(key)
  },

  /** add a contact (call when you interact with someone — auto-builds social graph) */
  addContact: upsertContact,

  /** get contacts for this app */
  async getContacts(appName?: string) {
    const origin = globalThis.location?.origin || appName || 'unknown'
    return getContactRefs(origin)
  },
}
