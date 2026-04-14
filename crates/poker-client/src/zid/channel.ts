/**
 * e2ee channel between two zid identities
 *
 * x25519 ECDH key exchange → AES-256-GCM encrypted messages
 * relay-agnostic: works over WebSocket, WebRTC, or any transport
 */

import type { ZidChannel } from './types'

type SessionKey = {
  pubkey: string
  sign: (data: Uint8Array) => Promise<string>
}

/** create an e2ee channel to a peer via relay WebSocket */
export async function createChannel(
  session: SessionKey,
  peerPubkey: string,
  relayUrl?: string,
): Promise<ZidChannel> {
  // generate ephemeral x25519 key pair for this channel
  const dh = await crypto.subtle.generateKey(
    { name: 'X25519' } as any,
    true,
    ['deriveBits'],
  )

  const ourDhPub = new Uint8Array(await crypto.subtle.exportKey('raw', dh.publicKey))

  // sign our DH pubkey with our session key (authenticated key exchange)
  const dhSig = await session.sign(ourDhPub)

  // message handlers
  const handlers: ((data: Uint8Array) => void)[] = []
  let sharedKey: CryptoKey | null = null
  let ws: WebSocket | null = null

  // connect to relay
  const url = relayUrl || `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws/zid`
  ws = new WebSocket(url)

  ws.onopen = () => {
    // send key exchange: our session pubkey + DH pubkey + signature
    ws?.send(JSON.stringify({
      type: 'keyex',
      from: session.pubkey,
      to: peerPubkey,
      dhPub: hex(ourDhPub),
      sig: dhSig,
    }))
  }

  ws.onmessage = async (ev) => {
    try {
      const msg = JSON.parse(ev.data)

      if (msg.type === 'keyex' && msg.from === peerPubkey) {
        // verify peer signed their DH pubkey with their session key (prevents relay MitM)
        const peerDhBytes = unhex(msg.dhPub)
        const peerSessionKey = await crypto.subtle.importKey(
          'raw', unhex(peerPubkey), 'Ed25519', false, ['verify'],
        )
        const sigValid = await crypto.subtle.verify(
          'Ed25519', peerSessionKey, unhex(msg.sig), peerDhBytes,
        )
        if (!sigValid) {
          console.error('zid: DH signature verification failed — possible MitM')
          return
        }

        // derive shared secret
        const peerDhPub = await crypto.subtle.importKey(
          'raw', peerDhBytes,
          { name: 'X25519' } as any,
          false, [],
        )
        const sharedBits = new Uint8Array(
          await crypto.subtle.deriveBits(
            { name: 'X25519', public: peerDhPub } as any,
            dh.privateKey, 256,
          ),
        )
        // derive AES key — info binds to both session pubkeys to prevent unknown-key-share
        const sortedPubkeys = [session.pubkey, peerPubkey].sort().join(':')
        const info = new TextEncoder().encode(`zid-e2ee:${sortedPubkeys}`)
        const keyMaterial = await crypto.subtle.importKey('raw', sharedBits, 'HKDF', false, ['deriveKey'])
        sharedKey = await crypto.subtle.deriveKey(
          { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(32), info },
          keyMaterial,
          { name: 'AES-GCM', length: 256 },
          false,
          ['encrypt', 'decrypt'],
        )
      }

      if (msg.type === 'enc' && msg.from === peerPubkey && sharedKey) {
        const iv = unhex(msg.iv)
        const ct = unhex(msg.ct)
        const plain = new Uint8Array(await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, sharedKey, ct))
        for (const h of handlers) h(plain)
      }
    } catch (e) {
      console.error('zid channel error:', e)
    }
  }

  return {
    peer: peerPubkey,

    send: async (data: string | Uint8Array) => {
      if (!sharedKey || !ws) return
      const plain = typeof data === 'string' ? new TextEncoder().encode(data) : data
      const iv = crypto.getRandomValues(new Uint8Array(12))
      const ct = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, sharedKey, plain))
      ws.send(JSON.stringify({
        type: 'enc',
        from: session.pubkey,
        to: peerPubkey,
        iv: hex(iv),
        ct: hex(ct),
      }))
    },

    on: (event: 'message', handler: (data: Uint8Array) => void) => {
      if (event === 'message') handlers.push(handler)
    },

    close: () => {
      ws?.close()
      ws = null
      sharedKey = null
    },
  }
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('')
}
function unhex(h: string): Uint8Array {
  const bytes = new Uint8Array(h.length / 2)
  for (let i = 0; i < h.length; i += 2) bytes[i / 2] = parseInt(h.slice(i, i + 2), 16)
  return bytes
}
