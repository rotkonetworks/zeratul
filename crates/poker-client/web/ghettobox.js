// ghettobox SDK bridge for poker client
// extension-free web3 identity

import { init, GhettoboxStorage, WimProver, toBase64, fromBase64 } from '@ghettobox/sdk';

let storage = null;
let prover = null;
let currentSession = null;

// operator endpoints (would be configured per-environment)
const OPERATORS = [
  'https://op1.ghettobox.io',
  'https://op2.ghettobox.io',
  'https://op3.ghettobox.io',
];

/**
 * initialize the SDK
 */
async function ensureInit() {
  if (prover) return;

  await init();
  storage = new GhettoboxStorage('poker', 'anonymous');
  await storage.open();
}

/**
 * derive identity commitment from email
 * uses OPRF so operators never see the actual email
 */
async function deriveIdentity(email) {
  await ensureInit();

  // blind the email for OPRF
  const emailBytes = new TextEncoder().encode(email.toLowerCase().trim());

  // in production: send blinded element to operators, get responses
  // for now: simulate with local hash
  const identityHash = await crypto.subtle.digest('SHA-256', emailBytes);
  return toBase64(new Uint8Array(identityHash));
}

/**
 * derive encryption key from PIN via OPRF
 */
async function deriveKeyFromPin(pin) {
  await ensureInit();

  const pinBytes = new TextEncoder().encode(pin);

  // in production: OPRF with operators
  // for now: simulate with PBKDF2
  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    pinBytes,
    'PBKDF2',
    false,
    ['deriveBits']
  );

  const salt = new TextEncoder().encode('ghettobox-poker-v1');
  const derived = await crypto.subtle.deriveBits(
    { name: 'PBKDF2', salt, iterations: 100000, hash: 'SHA-256' },
    keyMaterial,
    256
  );

  return new Uint8Array(derived);
}

/**
 * generate a new keypair
 */
async function generateKeypair() {
  const keypair = await crypto.subtle.generateKey(
    { name: 'ECDSA', namedCurve: 'P-256' },
    true,
    ['sign', 'verify']
  );

  const publicKey = await crypto.subtle.exportKey('raw', keypair.publicKey);
  const privateKey = await crypto.subtle.exportKey('pkcs8', keypair.privateKey);

  return {
    publicKey: new Uint8Array(publicKey),
    privateKey: new Uint8Array(privateKey),
  };
}

/**
 * encrypt private key with PIN-derived key
 */
async function encryptPrivateKey(privateKey, pinKey) {
  const iv = crypto.getRandomValues(new Uint8Array(12));

  const aesKey = await crypto.subtle.importKey(
    'raw',
    pinKey,
    'AES-GCM',
    false,
    ['encrypt']
  );

  const encrypted = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv },
    aesKey,
    privateKey
  );

  // prepend IV to ciphertext
  const result = new Uint8Array(iv.length + encrypted.byteLength);
  result.set(iv);
  result.set(new Uint8Array(encrypted), iv.length);

  return result;
}

/**
 * decrypt private key with PIN-derived key
 */
async function decryptPrivateKey(encryptedKey, pinKey) {
  const iv = encryptedKey.slice(0, 12);
  const ciphertext = encryptedKey.slice(12);

  const aesKey = await crypto.subtle.importKey(
    'raw',
    pinKey,
    'AES-GCM',
    false,
    ['decrypt']
  );

  const decrypted = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv },
    aesKey,
    ciphertext
  );

  return new Uint8Array(decrypted);
}

/**
 * compute address from public key
 */
function computeAddress(publicKey) {
  // use first 20 bytes of hash as address (ethereum style)
  return crypto.subtle.digest('SHA-256', publicKey).then(hash => {
    const bytes = new Uint8Array(hash).slice(0, 20);
    return '0x' + Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
  });
}

/**
 * register new account with email + PIN
 */
export async function ghettobox_register(email, pin) {
  await ensureInit();

  // validate PIN
  if (pin.length < 4 || pin.length > 8) {
    throw new Error('PIN must be 4-8 digits');
  }
  if (!/^\d+$/.test(pin)) {
    throw new Error('PIN must be digits only');
  }

  // derive identity and key
  const identity = await deriveIdentity(email);
  const pinKey = await deriveKeyFromPin(pin);

  // check if already registered
  const existing = await storage.getSecret(identity);
  if (existing) {
    throw new Error('account already exists - use login instead');
  }

  // generate new keypair
  const { publicKey, privateKey } = await generateKeypair();
  const address = await computeAddress(publicKey);

  // encrypt private key
  const encryptedKey = await encryptPrivateKey(privateKey, pinKey);

  // in production: split encryptedKey into shards, send to operators
  // for now: store locally
  await storage.putSecret({
    commitment: identity,
    label: email,
    threshold: 1,
    totalShards: 1,
    operatorIds: ['local'],
    createdAt: Date.now(),
    recoveryCount: 0,
  });

  // store encrypted key in IndexedDB
  localStorage.setItem(`ghettobox:${identity}:key`, toBase64(encryptedKey));
  localStorage.setItem(`ghettobox:${identity}:pub`, toBase64(publicKey));

  // create session
  currentSession = {
    identity,
    email,
    address,
    publicKey,
    privateKey, // kept in memory for session
    expiresAt: Date.now() + 3600_000, // 1 hour
  };

  console.log('registered:', address);
  return address;
}

/**
 * login with email + PIN
 */
export async function ghettobox_login(email, pin) {
  await ensureInit();

  // derive identity and key
  const identity = await deriveIdentity(email);
  const pinKey = await deriveKeyFromPin(pin);

  // check if registered
  const secret = await storage.getSecret(identity);
  if (!secret) {
    throw new Error('account not found - register first');
  }

  // get encrypted key
  const encryptedKeyB64 = localStorage.getItem(`ghettobox:${identity}:key`);
  const publicKeyB64 = localStorage.getItem(`ghettobox:${identity}:pub`);

  if (!encryptedKeyB64 || !publicKeyB64) {
    throw new Error('key data missing - may need recovery');
  }

  const encryptedKey = fromBase64(encryptedKeyB64);
  const publicKey = fromBase64(publicKeyB64);

  // decrypt private key
  let privateKey;
  try {
    privateKey = await decryptPrivateKey(encryptedKey, pinKey);
  } catch (e) {
    throw new Error('wrong PIN');
  }

  const address = await computeAddress(publicKey);

  // create session
  currentSession = {
    identity,
    email,
    address,
    publicKey,
    privateKey,
    expiresAt: Date.now() + 3600_000,
  };

  console.log('logged in:', address);
  return address;
}

/**
 * sign a message with current session
 */
export async function ghettobox_sign(payload) {
  if (!currentSession) {
    throw new Error('not logged in');
  }

  if (Date.now() > currentSession.expiresAt) {
    currentSession = null;
    throw new Error('session expired');
  }

  // import private key
  const privateKey = await crypto.subtle.importKey(
    'pkcs8',
    currentSession.privateKey,
    { name: 'ECDSA', namedCurve: 'P-256' },
    false,
    ['sign']
  );

  // sign
  const signature = await crypto.subtle.sign(
    { name: 'ECDSA', hash: 'SHA-256' },
    privateKey,
    payload
  );

  return new Uint8Array(signature);
}

/**
 * get current timestamp (for Rust bridge)
 */
export function js_timestamp() {
  return BigInt(Date.now());
}

/**
 * check if logged in
 */
export function is_logged_in() {
  return currentSession !== null && Date.now() < currentSession.expiresAt;
}

/**
 * get current address
 */
export function get_address() {
  return currentSession?.address ?? null;
}

/**
 * logout
 */
export function logout() {
  currentSession = null;
}
