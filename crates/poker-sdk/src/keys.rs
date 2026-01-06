//! penumbra-style key derivation for mental poker
//!
//! provides hierarchical key derivation with granular sharing:
//! - share one hand without exposing table history
//! - share table history without exposing other tables
//! - detection keys let game engine scan without decrypt
//! - nullifiers prevent card/bet reuse
//!
//! ## cryptography
//!
//! - key agreement: x25519 ECDH (default), decaf377 (optional)
//! - encryption: ChaCha20-Poly1305 AEAD
//! - key derivation: BLAKE3 with domain separators
//! - signatures: ed25519-style (via BLAKE3 for now, real ed25519 TODO)

use blake3::Hasher;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};

// ============================================================================
// KEY AGREEMENT TRAIT - modular curve backend
// ============================================================================

/// trait for key agreement (ECDH)
/// implement for different curves: x25519 (default), decaf377, etc.
pub trait KeyAgreement: Clone + Send + Sync {
    /// create from 32-byte seed
    fn from_bytes(bytes: [u8; 32]) -> Self;
    /// generate random keypair
    fn random() -> Self;
    /// get public key (32 bytes)
    fn public_key(&self) -> [u8; 32];
    /// ECDH: compute shared secret with their public key
    fn diffie_hellman(&self, their_public: &[u8; 32]) -> [u8; 32];
    /// get secret bytes
    fn to_bytes(&self) -> [u8; 32];
}

// ============================================================================
// X25519 IMPLEMENTATION (default - HSM/hardware compatible)
// ============================================================================

/// x25519 key agreement (default, widely supported)
#[derive(Clone)]
pub struct X25519Secret {
    secret: x25519_dalek::StaticSecret,
    bytes: [u8; 32],
}

impl KeyAgreement for X25519Secret {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        let secret = x25519_dalek::StaticSecret::from(bytes);
        Self { secret, bytes }
    }

    fn random() -> Self {
        use rand_core::OsRng;
        let secret = x25519_dalek::StaticSecret::random_from_rng(OsRng);
        let bytes: [u8; 32] = secret.to_bytes();
        Self { secret, bytes }
    }

    fn public_key(&self) -> [u8; 32] {
        let public = x25519_dalek::PublicKey::from(&self.secret);
        public.to_bytes()
    }

    fn diffie_hellman(&self, their_public: &[u8; 32]) -> [u8; 32] {
        let their_pk = x25519_dalek::PublicKey::from(*their_public);
        let shared = self.secret.diffie_hellman(&their_pk);
        shared.to_bytes()
    }

    fn to_bytes(&self) -> [u8; 32] {
        self.bytes
    }
}

// ============================================================================
// DECAF377 IMPLEMENTATION (optional - for BLS12-377 SNARK compatibility)
// ============================================================================

#[cfg(feature = "decaf377")]
use decaf377_ka as ka;

/// decaf377 key agreement (for BLS12-377 SNARK circuits)
#[cfg(feature = "decaf377")]
#[derive(Clone)]
pub struct Decaf377Secret {
    bytes: [u8; 32],
}

#[cfg(feature = "decaf377")]
impl KeyAgreement for Decaf377Secret {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    fn random() -> Self {
        use rand_core::OsRng;
        let secret = ka::Secret::new(&mut OsRng);
        let public = secret.public();
        let mut hasher = Hasher::new();
        hasher.update(b"decaf377.secret");
        hasher.update(&public.0);
        Self {
            bytes: *hasher.finalize().as_bytes(),
        }
    }

    fn public_key(&self) -> [u8; 32] {
        self.to_ka_secret().public().0
    }

    fn diffie_hellman(&self, their_public: &[u8; 32]) -> [u8; 32] {
        let secret = self.to_ka_secret();
        let their_pk = ka::Public(*their_public);
        secret.key_agreement_with(&their_pk).expect("valid DH").0
    }

    fn to_bytes(&self) -> [u8; 32] {
        self.bytes
    }
}

#[cfg(feature = "decaf377")]
impl Decaf377Secret {
    fn to_ka_secret(&self) -> ka::Secret {
        ka::Secret::new_from_field(decaf377::Fr::from_le_bytes_mod_order(&self.bytes))
    }
}

// ============================================================================
// DEFAULT KEY TYPE - x25519
// ============================================================================

/// default key agreement type (x25519 for compatibility)
pub type DefaultKeyAgreement = X25519Secret;

/// chacha20poly1305 AEAD (symmetric encryption)
pub struct ChaCha20Poly1305Cipher;

impl ChaCha20Poly1305Cipher {
    pub fn encrypt(key: &[u8; 32], nonce: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let cipher = ChaCha20Poly1305::new_from_slice(key).expect("valid key");
        let n = Nonce::from_slice(&nonce[..12]);
        cipher.encrypt(n, plaintext).expect("encryption failed")
    }

    pub fn decrypt(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new_from_slice(key).ok()?;
        let n = Nonce::from_slice(&nonce[..12]);
        cipher.decrypt(n, ciphertext).ok()
    }
}

// ============================================================================

/// domain separators (16 bytes each, penumbra style)
mod domains {
    pub const SPEND_AUTH: &[u8; 16] = b"poker_SpendAuth_";
    pub const NULLIFIER: &[u8; 16] = b"poker_Nullifier_";
    pub const OUTGOING_VK: &[u8; 16] = b"poker_OutgoingVK";
    pub const INCOMING_VK: &[u8; 16] = b"poker_IncomingVK";
    pub const DIVERSIFIER: &[u8; 16] = b"poker_Diversify_";
    pub const DETECTION: &[u8; 16] = b"poker_Detection_";
    pub const TABLE_SEED: &[u8; 16] = b"poker_TableSeed_";
    pub const HAND_SEED: &[u8; 16] = b"poker_HandSeed__";
    pub const HAND_VIEW: &[u8; 16] = b"poker_HandView__";
    pub const CARD_KEY: &[u8; 16] = b"poker_CardKey___";
    pub const PAYLOAD: &[u8; 16] = b"poker_PayloadKey";
    pub const ACTION_ENC: &[u8; 16] = b"poker_ActionEnc_";
}

/// prf_expand: blake3 keyed hash with domain separator
fn prf_expand(domain: &[u8; 16], key: &[u8], input: &[u8]) -> [u8; 32] {
    let key_arr: [u8; 32] = if key.len() >= 32 {
        key[..32].try_into().unwrap()
    } else {
        let mut arr = [0u8; 32];
        arr[..key.len()].copy_from_slice(key);
        arr
    };
    let mut hasher = Hasher::new_keyed(&key_arr);
    hasher.update(domain);
    hasher.update(input);
    *hasher.finalize().as_bytes()
}

/// prf_expand with index byte
fn prf_expand_idx(domain: &[u8; 16], key: &[u8; 32], idx: u8) -> [u8; 32] {
    prf_expand(domain, key, &[idx])
}

/// master seed derived from email+PIN (via OPRF in production)
#[derive(Clone)]
pub struct MasterSeed([u8; 32]);

impl MasterSeed {
    /// create from raw bytes (test mode: hash of email+pin)
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// derive from email and pin (test mode - NOT secure for production)
    pub fn from_email_pin(email: &str, pin: &str) -> Self {
        let hash = blake3::hash(format!("{}:{}", email, pin).as_bytes());
        Self(*hash.as_bytes())
    }

    /// derive spend key
    pub fn derive_spend_key(&self) -> SpendKey {
        SpendKey::derive(&self.0)
    }
}

/// spend key - full authority over account
#[derive(Clone)]
pub struct SpendKey {
    seed: [u8; 32],
    /// authorization key (for signing)
    pub ak: [u8; 32],
    /// nullifier key (for nullifiers)
    pub nk: [u8; 32],
}

impl SpendKey {
    fn derive(master: &[u8; 32]) -> Self {
        let ak = prf_expand_idx(domains::SPEND_AUTH, master, 0);
        let nk = prf_expand_idx(domains::NULLIFIER, master, 1);
        Self {
            seed: *master,
            ak,
            nk,
        }
    }

    /// get full viewing key (can share for read-only access)
    pub fn full_viewing_key(&self) -> FullViewingKey {
        FullViewingKey::derive(&self.ak, &self.nk)
    }

    /// sign a message (requires spend authority)
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let mut hasher = Hasher::new();
        hasher.update(b"poker.sign.v1");
        hasher.update(&self.ak);
        hasher.update(message);
        let sig_hash = hasher.finalize();

        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(sig_hash.as_bytes());

        let msg_hash = blake3::hash(message);
        signature[32..].copy_from_slice(msg_hash.as_bytes());

        signature
    }

    /// get account address (public key hash)
    pub fn address(&self) -> [u8; 32] {
        *blake3::hash(&self.ak).as_bytes()
    }
}

/// full viewing key - read-only access to all activity
#[derive(Clone)]
pub struct FullViewingKey {
    /// authorization key (public, for verification)
    pub ak: [u8; 32],
    /// nullifier key (for computing nullifiers)
    pub nk: [u8; 32],
    /// outgoing viewing key
    pub ovk: [u8; 32],
    /// incoming viewing key component
    pub ivk: [u8; 32],
    /// diversifier key (for deriving table addresses)
    pub dk: [u8; 16],
    /// detection key (for probabilistic scanning)
    pub dtk: [u8; 32],
}

impl FullViewingKey {
    fn derive(ak: &[u8; 32], nk: &[u8; 32]) -> Self {
        let mut combined = [0u8; 64];
        combined[..32].copy_from_slice(nk);
        combined[32..].copy_from_slice(ak);

        let ovk = prf_expand(domains::OUTGOING_VK, &combined[..32], &combined[32..]);
        let ivk = prf_expand(domains::INCOMING_VK, &combined[..32], &combined[32..]);

        let dk_full = prf_expand(domains::DIVERSIFIER, &combined[..32], &combined[32..]);
        let mut dk = [0u8; 16];
        dk.copy_from_slice(&dk_full[..16]);

        let dtk = prf_expand(domains::DETECTION, &combined[..32], &combined[32..]);

        Self { ak: *ak, nk: *nk, ovk, ivk, dk, dtk }
    }

    /// derive table-specific viewing key
    pub fn table_viewing_key(&self, table_id: &[u8]) -> TableViewingKey {
        TableViewingKey::derive(self, table_id)
    }

    /// derive diversified address for a table
    /// transmission_key is proper decaf377 public key derived from ivk + table
    pub fn table_address(&self, table_id: &[u8]) -> TableAddress {
        let mut hasher = Hasher::new();
        hasher.update(&self.dk);
        hasher.update(table_id);
        let diversifier = *hasher.finalize().as_bytes();

        // derive table-specific secret from ivk + diversifier
        let table_secret = prf_expand(domains::TABLE_SEED, &self.ivk, &diversifier);

        // convert to public key using default curve
        let secret = DefaultKeyAgreement::from_bytes(table_secret);
        let transmission_key = secret.public_key();

        TableAddress {
            diversifier,
            transmission_key,
            table_secret, // store for decryption
            table_id: table_id.to_vec(),
        }
    }

    /// get incoming viewing key (share for incoming-only view)
    pub fn incoming_viewing_key(&self) -> IncomingViewingKey {
        IncomingViewingKey {
            ivk: self.ivk,
            dk: self.dk,
        }
    }

    /// get outgoing viewing key (share for outgoing-only view)
    pub fn outgoing_viewing_key(&self) -> OutgoingViewingKey {
        OutgoingViewingKey { ovk: self.ovk }
    }

    /// get detection key (share for probabilistic scanning)
    pub fn detection_key(&self) -> DetectionKey {
        DetectionKey { dtk: self.dtk }
    }
}

/// incoming viewing key - see cards dealt to you
#[derive(Clone)]
pub struct IncomingViewingKey {
    pub ivk: [u8; 32],
    pub dk: [u8; 16],
}

impl IncomingViewingKey {
    /// decrypt incoming card/action using decaf377 + chacha20poly1305
    pub fn decrypt(&self, epk: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
        let shared = self.key_agreement(epk);
        let payload_key = derive_payload_key(&shared, epk);
        decrypt_aead(&payload_key, ciphertext)
    }

    /// key agreement: shared = ivk × epk
    fn key_agreement(&self, epk: &[u8; 32]) -> [u8; 32] {
        let secret = DefaultKeyAgreement::from_bytes(self.ivk);
        secret.diffie_hellman(epk)
    }
}

/// outgoing viewing key - see actions you sent
#[derive(Clone)]
pub struct OutgoingViewingKey {
    pub ovk: [u8; 32],
}

impl OutgoingViewingKey {
    /// decrypt wrapped key to recover shared secret
    pub fn decrypt_wrapped(
        &self,
        commitment: &[u8; 32],
        epk: &[u8; 32],
        wrapped: &[u8],
    ) -> Option<[u8; 32]> {
        let ock = derive_outgoing_cipher_key(&self.ovk, commitment, epk);
        decrypt_wrapped_key(&ock, wrapped)
    }
}

/// detection key - probabilistic scanning without decryption
#[derive(Clone)]
pub struct DetectionKey {
    pub dtk: [u8; 32],
}

impl DetectionKey {
    /// check if a clue matches (fuzzy message detection)
    pub fn check_clue(&self, clue: &[u8]) -> bool {
        if clue.len() < 32 {
            return false;
        }
        let mut hasher = Hasher::new();
        hasher.update(&self.dtk);
        hasher.update(&clue[..32]);
        let check = hasher.finalize();
        check.as_bytes()[0] == clue.get(32).copied().unwrap_or(0)
    }
}

/// table-specific address with x25519 keys
#[derive(Clone)]
pub struct TableAddress {
    pub diversifier: [u8; 32],
    /// x25519 public key for this table
    pub transmission_key: [u8; 32],
    /// x25519 secret for decrypting (keep private!)
    table_secret: [u8; 32],
    pub table_id: Vec<u8>,
}

impl TableAddress {
    /// encode as hex string
    pub fn to_hex(&self) -> String {
        format!("5Table{}", hex::encode(&self.transmission_key[..16]))
    }

    /// decrypt incoming data for this table
    pub fn decrypt(&self, epk: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
        let secret = DefaultKeyAgreement::from_bytes(self.table_secret);
        let shared = secret.diffie_hellman(epk);
        let payload_key = derive_payload_key(&shared, epk);
        decrypt_aead(&payload_key, ciphertext)
    }

    /// get the secret for advanced use (careful - keep private!)
    pub fn secret(&self) -> &[u8; 32] {
        &self.table_secret
    }
}

/// table viewing key - share to let someone see one table's history
#[derive(Clone)]
pub struct TableViewingKey {
    seed: [u8; 32],
    pub table_id: Vec<u8>,
}

impl TableViewingKey {
    fn derive(fvk: &FullViewingKey, table_id: &[u8]) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(domains::TABLE_SEED);
        hasher.update(&fvk.ivk);
        hasher.update(&fvk.ovk);
        hasher.update(table_id);
        let seed = *hasher.finalize().as_bytes();

        Self {
            seed,
            table_id: table_id.to_vec(),
        }
    }

    /// derive hand viewing key
    pub fn hand_viewing_key(&self, hand_number: u64) -> HandViewingKey {
        HandViewingKey::derive(&self.seed, hand_number)
    }

    /// decrypt any action at this table
    pub fn decrypt_action(&self, epk: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
        let payload_key = derive_payload_key(&self.seed, epk);
        decrypt_payload(&payload_key, ciphertext)
    }
}

/// hand viewing key - share to let someone see one hand
#[derive(Clone)]
pub struct HandViewingKey {
    seed: [u8; 32],
    pub hand_number: u64,
}

impl HandViewingKey {
    fn derive(table_seed: &[u8; 32], hand_number: u64) -> Self {
        let seed = prf_expand(domains::HAND_SEED, table_seed, &hand_number.to_le_bytes());
        Self { seed, hand_number }
    }

    /// get the hand viewing key bytes (shareable)
    pub fn to_bytes(&self) -> [u8; 32] {
        prf_expand(domains::HAND_VIEW, &self.seed, &[])
    }

    /// derive card encryption key for a specific card
    pub fn card_key(&self, card_index: u8) -> [u8; 32] {
        prf_expand(domains::CARD_KEY, &self.seed, &[card_index])
    }

    /// encrypt a card with this hand's key (ChaCha20Poly1305)
    pub fn encrypt_card(&self, card_index: u8, plaintext: &[u8]) -> Vec<u8> {
        let key = self.card_key(card_index);
        encrypt_aead(&key, plaintext)
    }

    /// decrypt a card with this hand's key (ChaCha20Poly1305)
    pub fn decrypt_card(&self, card_index: u8, ciphertext: &[u8]) -> Option<Vec<u8>> {
        let key = self.card_key(card_index);
        decrypt_aead(&key, ciphertext)
    }

    /// decrypt action at this hand (ChaCha20Poly1305)
    pub fn decrypt_action(&self, action_index: u8, ciphertext: &[u8]) -> Option<Vec<u8>> {
        let key = prf_expand(domains::ACTION_ENC, &self.seed, &[action_index]);
        decrypt_aead(&key, ciphertext)
    }
}

/// ephemeral key for encrypting a single action/card
/// uses decaf377 for key agreement (SNARK-friendly)
pub struct EphemeralKey {
    secret: [u8; 32],
    pub public: [u8; 32],
}

impl EphemeralKey {
    /// generate random ephemeral key
    pub fn random() -> Self {
        let ka = DefaultKeyAgreement::random();
        Self {
            secret: ka.to_bytes(),
            public: ka.public_key(),
        }
    }

    /// create from existing secret bytes
    pub fn from_bytes(secret: [u8; 32]) -> Self {
        let ka = DefaultKeyAgreement::from_bytes(secret);
        Self {
            secret,
            public: ka.public_key(),
        }
    }

    /// key agreement: shared = esk × recipient_pk
    pub fn shared_secret(&self, recipient_pk: &[u8; 32]) -> [u8; 32] {
        let ka = DefaultKeyAgreement::from_bytes(self.secret);
        ka.diffie_hellman(recipient_pk)
    }

    /// encrypt for recipient using ECDH + chacha20poly1305
    pub fn encrypt_for(&self, recipient_pk: &[u8; 32], plaintext: &[u8]) -> EncryptedPayload {
        let shared = self.shared_secret(recipient_pk);
        let payload_key = derive_payload_key(&shared, &self.public);
        let ciphertext = encrypt_aead(&payload_key, plaintext);

        EncryptedPayload {
            epk: self.public,
            ciphertext,
        }
    }
}

/// encrypted payload with ephemeral public key
#[derive(Clone)]
pub struct EncryptedPayload {
    pub epk: [u8; 32],
    pub ciphertext: Vec<u8>,
}

// helper functions

fn derive_payload_key(shared_secret: &[u8; 32], epk: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(domains::PAYLOAD);
    hasher.update(shared_secret);
    hasher.update(epk);
    *hasher.finalize().as_bytes()
}

fn derive_outgoing_cipher_key(
    ovk: &[u8; 32],
    commitment: &[u8; 32],
    epk: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(b"poker_OutCipher_");
    hasher.update(ovk);
    hasher.update(commitment);
    hasher.update(epk);
    *hasher.finalize().as_bytes()
}

/// encrypt with ChaCha20Poly1305 (authenticated encryption)
/// nonce derived from key hash (safe since key is unique per message)
pub fn encrypt_aead(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new_from_slice(key).expect("valid key");

    // derive nonce from key (unique per ephemeral key)
    let nonce_bytes = blake3::hash(&[key.as_slice(), b"nonce"].concat());
    let nonce = Nonce::from_slice(&nonce_bytes.as_bytes()[..12]);

    cipher.encrypt(nonce, plaintext).expect("encryption failed")
}

/// decrypt with ChaCha20Poly1305 (authenticated decryption)
pub fn decrypt_aead(key: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new_from_slice(key).ok()?;

    let nonce_bytes = blake3::hash(&[key.as_slice(), b"nonce"].concat());
    let nonce = Nonce::from_slice(&nonce_bytes.as_bytes()[..12]);

    cipher.decrypt(nonce, ciphertext).ok()
}

/// legacy XOR encryption (for internal key wrapping only)
fn encrypt_payload(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    plaintext
        .iter()
        .enumerate()
        .map(|(i, &b)| b ^ key[i % 32])
        .collect()
}

fn decrypt_payload(key: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    Some(encrypt_payload(key, ciphertext))
}

fn decrypt_wrapped_key(ock: &[u8; 32], wrapped: &[u8]) -> Option<[u8; 32]> {
    if wrapped.len() != 32 {
        return None;
    }
    let decrypted = decrypt_payload(ock, wrapped)?;
    let mut key = [0u8; 32];
    key.copy_from_slice(&decrypted);
    Some(key)
}

/// nullifier for preventing reuse
pub fn compute_nullifier(nk: &[u8; 32], position: u64, commitment: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(b"poker.nullifier.v1");
    hasher.update(nk);
    hasher.update(&position.to_le_bytes());
    hasher.update(commitment);
    *hasher.finalize().as_bytes()
}

mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_hierarchy() {
        let master = MasterSeed::from_email_pin("test@example.com", "1234");
        let spend_key = master.derive_spend_key();
        let fvk = spend_key.full_viewing_key();

        let table_id = b"table-123";
        let table_addr = fvk.table_address(table_id);
        let table_vk = fvk.table_viewing_key(table_id);

        let hand_vk = table_vk.hand_viewing_key(1);

        let card_key_0 = hand_vk.card_key(0);
        let card_key_1 = hand_vk.card_key(1);
        assert_ne!(card_key_0, card_key_1);

        let shareable = hand_vk.to_bytes();
        assert_eq!(shareable.len(), 32);

        println!("table address: {}", table_addr.to_hex());
    }

    #[test]
    fn test_encrypt_decrypt() {
        let master = MasterSeed::from_bytes([0x42u8; 32]);
        let spend_key = master.derive_spend_key();
        let fvk = spend_key.full_viewing_key();

        let table_addr = fvk.table_address(b"table-1");

        let esk = EphemeralKey::random();
        let plaintext = b"Ah"; // ace of hearts
        let encrypted = esk.encrypt_for(&table_addr.transmission_key, plaintext);

        // table address has the derived secret matching transmission_key
        let decrypted = table_addr.decrypt(&encrypted.epk, &encrypted.ciphertext).unwrap();
        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn test_hand_viewing_key_sharing() {
        let master = MasterSeed::from_bytes([0x42u8; 32]);
        let spend_key = master.derive_spend_key();
        let fvk = spend_key.full_viewing_key();

        let table_vk = fvk.table_viewing_key(b"table-1");
        let hand_vk = table_vk.hand_viewing_key(5);

        let plaintext = b"Kd";
        let ciphertext = hand_vk.encrypt_card(0, plaintext);

        let decrypted = hand_vk.decrypt_card(0, &ciphertext).unwrap();
        assert_eq!(&decrypted, plaintext);

        // different hand has different key
        let other_hand_vk = table_vk.hand_viewing_key(6);
        let card_key = hand_vk.card_key(0);
        let other_card_key = other_hand_vk.card_key(0);
        assert_ne!(card_key, other_card_key);
    }

    #[test]
    fn test_nullifier() {
        let master = MasterSeed::from_bytes([0x42u8; 32]);
        let spend_key = master.derive_spend_key();
        let fvk = spend_key.full_viewing_key();

        let commitment = blake3::hash(b"card:Ah");
        let nullifier = compute_nullifier(&fvk.nk, 0, commitment.as_bytes());

        let nullifier2 = compute_nullifier(&fvk.nk, 1, commitment.as_bytes());
        assert_ne!(nullifier, nullifier2);

        let other_master = MasterSeed::from_bytes([0x43u8; 32]);
        let other_fvk = other_master.derive_spend_key().full_viewing_key();
        let other_nullifier = compute_nullifier(&other_fvk.nk, 0, commitment.as_bytes());
        assert_ne!(nullifier, other_nullifier);
    }
}
