/**
 * Noise IK encrypted channel between two ZID identities.
 *
 * Protocol: Noise_IK_25519_ChaChaPoly_SHA256
 *
 * Handshake pattern (IK):
 *   <- s                 (responder static known a priori)
 *   ...
 *   -> e, es, s, ss      (initiator sends noise_init 0x01)
 *   <- e, ee, se         (responder sends noise_resp 0x02)
 *
 * After handshake: two CipherState objects (send/recv) with independent keys.
 * Transport: ChaChaPoly1305 with monotonic 8-byte BE counter nonces (0x03 prefix).
 *
 * Wire format:
 *   0x01 noise_init  - [tag][e 32][encrypted s 48][encrypted payload 16+]
 *   0x02 noise_resp  - [tag][e 32][encrypted payload 16+]
 *   0x03 noise_transport - [tag][counter BE 8][ciphertext+tag]
 *
 * The relay routes by (from, to) pubkey pair. It sees the envelope but not
 * the Noise payloads.
 */

import { chacha20poly1305 } from '@noble/ciphers/chacha.js';
import { x25519, edwardsToMontgomeryPub, edwardsToMontgomeryPriv } from '@noble/curves/ed25519';
import { sha256 } from '@noble/hashes/sha256';
import { extract, expand } from '@noble/hashes/hkdf';
import type { ZidChannel } from './types';

// -- constants --

const NOISE_INIT = 0x01;
const NOISE_RESP = 0x02;
const NOISE_TRANSPORT = 0x03;
const PROTOCOL_NAME = 'Noise_IK_25519_ChaChaPoly_SHA256';
const EMPTY = new Uint8Array(0);
const TAG_LEN = 16;

// -- types --

export type SessionKey = {
  pubkey: string; // hex ed25519 public key
  privkey: Uint8Array; // ed25519 seed (32 bytes) - required for x25519 DH
  sign: (data: Uint8Array) => Promise<string>; // returns hex signature
};

interface CipherState {
  k: Uint8Array; // 32-byte symmetric key
  n: bigint; // monotonic counter nonce
}

// -- core Noise functions --

/** h = SHA-256(h || data) */
function mixHash(h: Uint8Array, data: Uint8Array): Uint8Array {
  return sha256(concat(h, data));
}

/** Noise HKDF - extract with ck as salt, expand into N 32-byte outputs */
function noiseHKDF(ck: Uint8Array, ikm: Uint8Array, outputs: 2): [Uint8Array, Uint8Array];
function noiseHKDF(
  ck: Uint8Array,
  ikm: Uint8Array,
  outputs: 3,
): [Uint8Array, Uint8Array, Uint8Array];
function noiseHKDF(ck: Uint8Array, ikm: Uint8Array, outputs: 2 | 3): Uint8Array[] {
  const prk = extract(sha256, ikm, ck);
  const okm = expand(sha256, prk, undefined, 32 * outputs);
  const result: Uint8Array[] = [];
  for (let i = 0; i < outputs; i++) result.push(okm.slice(i * 32, (i + 1) * 32));
  return result;
}

/** mixKey(ck, ikm) -> [new_ck, new_k], resets n to 0 */
function mixKey(ck: Uint8Array, ikm: Uint8Array): [Uint8Array, Uint8Array] {
  return noiseHKDF(ck, ikm, 2);
}

/** 12-byte LE nonce: 4 zero bytes + 8-byte LE counter (Noise spec convention) */
function nonceBytes(n: bigint): Uint8Array {
  const buf = new Uint8Array(12);
  const view = new DataView(buf.buffer);
  view.setUint32(4, Number(n & 0xffffffffn), true);
  view.setUint32(8, Number((n >> 32n) & 0xffffffffn), true);
  return buf;
}

function encryptWithAD(
  k: Uint8Array,
  n: bigint,
  ad: Uint8Array,
  plaintext: Uint8Array,
): Uint8Array {
  return chacha20poly1305(k, nonceBytes(n), ad).encrypt(plaintext);
}

function decryptWithAD(
  k: Uint8Array,
  n: bigint,
  ad: Uint8Array,
  ciphertext: Uint8Array,
): Uint8Array {
  return chacha20poly1305(k, nonceBytes(n), ad).decrypt(ciphertext);
}

/** Encrypt plaintext, mix ciphertext into h. If k is null, pass plaintext in the clear. */
function encryptAndHash(
  k: Uint8Array | null,
  n: bigint,
  h: Uint8Array,
  plaintext: Uint8Array,
): { ct: Uint8Array; h: Uint8Array; n: bigint } {
  if (k === null) {
    return { ct: plaintext, h: mixHash(h, plaintext), n };
  }
  const ct = encryptWithAD(k, n, h, plaintext);
  return { ct, h: mixHash(h, ct), n: n + 1n };
}

/** Decrypt ciphertext, mix it into h. If k is null, treat ciphertext as plaintext. */
function decryptAndHash(
  k: Uint8Array | null,
  n: bigint,
  h: Uint8Array,
  ciphertext: Uint8Array,
): { pt: Uint8Array; h: Uint8Array; n: bigint } {
  if (k === null) {
    return { pt: ciphertext, h: mixHash(h, ciphertext), n };
  }
  const pt = decryptWithAD(k, n, h, ciphertext);
  return { pt, h: mixHash(h, ciphertext), n: n + 1n };
}

/** x25519 Diffie-Hellman */
function dh(priv: Uint8Array, pub: Uint8Array): Uint8Array {
  return x25519.getSharedSecret(priv, pub);
}

// -- helpers --

function concat(...arrays: Uint8Array[]): Uint8Array {
  const len = arrays.reduce((s, a) => s + a.length, 0);
  const out = new Uint8Array(len);
  let off = 0;
  for (const a of arrays) {
    out.set(a, off);
    off += a.length;
  }
  return out;
}

function zeroize(buf: Uint8Array): void {
  buf.fill(0);
}

function unhex(h: string): Uint8Array {
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < h.length; i += 2) bytes[i / 2] = parseInt(h.slice(i, i + 2), 16);
  return bytes;
}

// -- ed25519 to x25519 conversion --

function edPubToX(edPub: Uint8Array): Uint8Array {
  return edwardsToMontgomeryPub(edPub);
}

function edPrivToX(edPriv: Uint8Array): Uint8Array {
  const seed = edPriv.length === 64 ? edPriv.slice(0, 32) : edPriv;
  return edwardsToMontgomeryPriv(seed);
}

// -- Noise symmetric state initialization --

function initSymmetric(): { ck: Uint8Array; h: Uint8Array } {
  // protocol name is > 32 bytes, so hash it per Noise spec
  const h = sha256(new TextEncoder().encode(PROTOCOL_NAME));
  const ck = h.slice();
  return { ck, h };
}

/** split(ck) -> [k1, k2] two independent CipherState keys */
function split(ck: Uint8Array): [Uint8Array, Uint8Array] {
  return noiseHKDF(ck, EMPTY, 2);
}

// -- initiator handshake: -> e, es, s, ss --

function initiatorHandshake(
  localXPriv: Uint8Array,
  localXPub: Uint8Array,
  remoteXPub: Uint8Array,
): {
  message: Uint8Array;
  finish: (respMsg: Uint8Array) => { sendCS: CipherState; recvCS: CipherState };
  cleanup: () => void;
} {
  let { ck, h } = initSymmetric();

  // pre-message: <- s (responder static known)
  h = mixHash(h, remoteXPub);

  // -> e: generate ephemeral x25519, mix pubkey into h
  const ePriv = x25519.utils.randomPrivateKey();
  const ePub = x25519.getPublicKey(ePriv);
  h = mixHash(h, ePub);

  // -> es: DH(e, rs)
  let k: Uint8Array;
  [ck, k] = mixKey(ck, dh(ePriv, remoteXPub));
  let n = 0n;

  // -> s: encrypt our static x25519 pubkey (32 bytes -> 48 bytes with tag)
  const encS = encryptAndHash(k, n, h, localXPub);
  h = encS.h;
  n = encS.n;

  // -> ss: DH(s, rs)
  [ck, k] = mixKey(ck, dh(localXPriv, remoteXPub));
  n = 0n;

  // encrypt empty payload
  const encPayload = encryptAndHash(k, n, h, EMPTY);
  h = encPayload.h;

  // wire: [0x01][ePub 32][encrypted static 48][encrypted payload 16]
  const message = concat(new Uint8Array([NOISE_INIT]), ePub, encS.ct, encPayload.ct);

  // save handshake state for processing the response
  const savedCk = ck.slice();
  const savedH = h.slice();
  const savedEPriv = ePriv.slice();

  function finish(resp: Uint8Array): { sendCS: CipherState; recvCS: CipherState } {
    if (resp[0] !== NOISE_RESP) throw new Error('noise: expected resp message (0x02)');
    const re = resp.slice(1, 33);
    const respCt = resp.slice(33);

    let rh = mixHash(savedH, re);
    let rck: Uint8Array = savedCk;

    // <- ee: DH(e, re)
    [rck] = mixKey(rck, dh(savedEPriv, re));

    // <- se: DH(s, re) - initiator static with responder ephemeral
    let rk: Uint8Array;
    [rck, rk] = mixKey(rck, dh(localXPriv, re));

    // decrypt responder payload (empty)
    const dec = decryptAndHash(rk, 0n, rh, respCt);
    rh = dec.h;

    // split into transport cipher states
    const [k1, k2] = split(rck);

    // zeroize handshake secrets
    zeroize(savedCk);
    zeroize(savedH);
    zeroize(savedEPriv);

    return {
      sendCS: { k: k1, n: 0n },
      recvCS: { k: k2, n: 0n },
    };
  }

  function cleanup(): void {
    zeroize(ePriv);
    zeroize(savedCk);
    zeroize(savedEPriv);
    zeroize(savedH);
  }

  return { message, finish, cleanup };
}

// -- responder handshake: <- e, ee, se --

function responderHandshake(
  localXPriv: Uint8Array,
  localXPub: Uint8Array,
  initMsg: Uint8Array,
): {
  message: Uint8Array;
  sendCS: CipherState;
  recvCS: CipherState;
  remoteXPub: Uint8Array;
} {
  if (initMsg[0] !== NOISE_INIT) throw new Error('noise: expected init message (0x01)');

  let { ck, h } = initSymmetric();

  // pre-message: <- s (our static known to initiator)
  h = mixHash(h, localXPub);

  // -> e: read initiator ephemeral
  const re = initMsg.slice(1, 33);
  h = mixHash(h, re);

  // -> es: DH(s, re) - responder static with initiator ephemeral
  let k: Uint8Array;
  [ck, k] = mixKey(ck, dh(localXPriv, re));
  let n = 0n;

  // -> s: decrypt initiator's static x25519 pubkey
  const encStatic = initMsg.slice(33, 33 + 32 + TAG_LEN);
  const decS = decryptAndHash(k, n, h, encStatic);
  h = decS.h;
  n = decS.n;
  const remoteXPub = decS.pt;

  // -> ss: DH(s, rs) - responder static with initiator static
  [ck, k] = mixKey(ck, dh(localXPriv, remoteXPub));
  n = 0n;

  // decrypt initiator payload (empty)
  const encPayload = initMsg.slice(33 + 32 + TAG_LEN);
  const decPayload = decryptAndHash(k, n, h, encPayload);
  h = decPayload.h;

  // <- e: generate responder ephemeral
  const ePriv = x25519.utils.randomPrivateKey();
  const ePub = x25519.getPublicKey(ePriv);
  h = mixHash(h, ePub);

  // <- ee: DH(e, re)
  [ck] = mixKey(ck, dh(ePriv, re));

  // <- se: DH(e, rs) - responder ephemeral with initiator static
  [ck, k] = mixKey(ck, dh(ePriv, remoteXPub));
  n = 0n;

  // encrypt empty response payload
  const respEnc = encryptAndHash(k, n, h, EMPTY);

  // wire: [0x02][ePub 32][encrypted payload]
  const message = concat(new Uint8Array([NOISE_RESP]), ePub, respEnc.ct);

  // split - responder send = initiator recv and vice versa
  const [initSend, initRecv] = split(ck);

  // zeroize handshake secrets
  zeroize(ePriv);
  zeroize(ck);

  return {
    message,
    sendCS: { k: initRecv, n: 0n },
    recvCS: { k: initSend, n: 0n },
    remoteXPub,
  };
}

// -- transport encryption (0x03 prefix) --

function encryptTransport(cs: CipherState, plaintext: Uint8Array): Uint8Array {
  const ct = chacha20poly1305(cs.k, nonceBytes(cs.n)).encrypt(plaintext);
  // wire: [0x03][8-byte BE counter][ciphertext + tag]
  const counter = new Uint8Array(8);
  new DataView(counter.buffer).setBigUint64(0, cs.n, false);
  cs.n += 1n;
  return concat(new Uint8Array([NOISE_TRANSPORT]), counter, ct);
}

function decryptTransport(cs: CipherState, msg: Uint8Array): Uint8Array {
  if (msg[0] !== NOISE_TRANSPORT) throw new Error('noise: expected transport message (0x03)');
  const wireN = new DataView(msg.buffer, msg.byteOffset + 1, 8).getBigUint64(0, false);
  if (wireN !== cs.n) {
    throw new Error(`noise: counter mismatch (expected ${cs.n}, got ${wireN})`);
  }
  const ct = msg.slice(9);
  const pt = chacha20poly1305(cs.k, nonceBytes(cs.n)).decrypt(ct);
  cs.n += 1n;
  return pt;
}

// -- public API --

/** Create a Noise IK encrypted channel to a peer via relay WebSocket. */
export async function createNoiseChannel(
  session: SessionKey,
  peerPubkey: string,
  relayUrl?: string,
): Promise<ZidChannel> {
  const localXPub = edPubToX(unhex(session.pubkey));
  const localXPriv = edPrivToX(session.privkey);
  const remoteXPub = edPubToX(unhex(peerPubkey));

  const handlers: ((data: Uint8Array) => void)[] = [];
  let sendCS: CipherState | null = null;
  let recvCS: CipherState | null = null;
  let ws: WebSocket | null = null;

  // initiator = lexicographically smaller pubkey
  const isInitiator = session.pubkey < peerPubkey;

  // `/zid`, not `/ws/zid`: HAProxy routes `/ws*` to the FROST relay.
  const url =
    relayUrl || `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/zid`;

  const handshakeComplete = new Promise<void>((resolve, reject) => {
    ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';

    function transportHandler(ev: MessageEvent): void {
      if (typeof ev.data === 'string') return;
      try {
        const data = new Uint8Array(ev.data as ArrayBuffer);
        if (data[0] === NOISE_TRANSPORT && recvCS) {
          const pt = decryptTransport(recvCS, data);
          for (const h of handlers) h(pt);
        }
      } catch (e) {
        console.error('noise: transport decrypt error', e);
      }
    }

    ws.onopen = () => {
      // announce to relay so it knows our routing pair
      ws?.send(JSON.stringify({ type: 'announce', from: session.pubkey, to: peerPubkey }));

      if (isInitiator) {
        const hs = initiatorHandshake(localXPriv, localXPub, remoteXPub);
        ws?.send(hs.message);

        ws!.onmessage = ev => {
          if (typeof ev.data === 'string') return;
          try {
            const data = new Uint8Array(ev.data as ArrayBuffer);
            if (data[0] === NOISE_RESP) {
              const result = hs.finish(data);
              sendCS = result.sendCS;
              recvCS = result.recvCS;
              ws!.onmessage = transportHandler;
              zeroize(localXPriv);
              resolve();
            }
          } catch (e) {
            hs.cleanup();
            reject(e);
          }
        };
      }
    };

    // responder path - listen for init before we get promoted to initiator handler
    ws.onmessage = ev => {
      if (typeof ev.data === 'string') return;
      const data = new Uint8Array(ev.data as ArrayBuffer);
      if (data[0] === NOISE_INIT && !isInitiator) {
        try {
          const result = responderHandshake(localXPriv, localXPub, data);
          sendCS = result.sendCS;
          recvCS = result.recvCS;
          ws?.send(result.message);
          ws!.onmessage = transportHandler;
          zeroize(localXPriv);
          resolve();
        } catch (e) {
          reject(e);
        }
      }
    };

    ws.onerror = () => reject(new Error('noise: WebSocket error'));
    ws.onclose = () => {
      if (!sendCS) reject(new Error('noise: connection closed during handshake'));
    };
  });

  await handshakeComplete;

  return {
    peer: peerPubkey,

    send(data: string | Uint8Array): void {
      if (!sendCS || !ws) return;
      const plain = typeof data === 'string' ? new TextEncoder().encode(data) : data;
      ws.send(encryptTransport(sendCS, plain));
    },

    on(event: 'message', handler: (data: Uint8Array) => void): void {
      if (event === 'message') handlers.push(handler);
    },

    close(): void {
      if (sendCS) {
        zeroize(sendCS.k);
        sendCS = null;
      }
      if (recvCS) {
        zeroize(recvCS.k);
        recvCS = null;
      }
      ws?.close();
      ws = null;
    },
  };
}
