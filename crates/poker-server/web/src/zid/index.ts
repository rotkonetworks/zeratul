/**
 * @zafu/zid — zafu identity SDK
 *
 * one-line wallet connection, session signing, e2ee channels.
 *
 * ```typescript
 * import { zid } from '@zafu/zid'
 *
 * const me = await zid.connect()
 * const sig = await me.sign(data)
 * const ch = await me.channel(peerPubkey)
 * ch.send('hello')
 * ```
 */

export { zid } from './zid'
export type { ZidIdentity, ZidChannel, ZidOptions } from './types'
