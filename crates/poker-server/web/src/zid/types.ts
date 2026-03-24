/** connected zafu identity */
export interface ZidIdentity {
  /** hex-encoded ed25519 public key (session key) */
  pubkey: string
  /** active network ('penumbra' | 'zcash' | 'polkadot' | ...) */
  network: string
  /** display name (first 8 chars of pubkey, or custom) */
  name: string
  /** sign arbitrary bytes with session key. returns hex signature */
  sign: (data: Uint8Array) => Promise<string>
  /** verify a signature. returns true if valid */
  verify: (data: Uint8Array, sig: string, pubkey: string) => Promise<boolean>
  /** open an e2ee channel to a peer */
  channel: (peerPubkey: string) => Promise<ZidChannel>
  /** whether connected via zafu extension or browser-generated key */
  mode: 'zafu' | 'ephemeral'
  /** zafu wallet pubkey (if mode === 'zafu') */
  walletPubkey?: string
  /** delegation signature proving wallet authorized this session */
  delegation?: string
  /** disconnect and clear session */
  disconnect: () => void
}

/** encrypted channel between two zid identities */
export interface ZidChannel {
  /** peer's public key */
  peer: string
  /** send encrypted message */
  send: (data: string | Uint8Array) => void
  /** receive callback */
  on: (event: 'message', handler: (data: Uint8Array) => void) => void
  /** close channel */
  close: () => void
}

/** options for zid.connect() */
export interface ZidOptions {
  /** app name shown in wallet approval popup */
  appName?: string
  /** preferred network (default: wallet's active network) */
  network?: string
  /** request trading mode (auto-sign without popups) */
  tradingMode?: boolean
  /** trading mode session duration in minutes (default: 60) */
  sessionMinutes?: number
  /** custom WebSocket URL for e2ee relay (default: same origin /ws/zid) */
  relayUrl?: string
  /** skip zafu detection, use ephemeral key */
  ephemeral?: boolean
}
