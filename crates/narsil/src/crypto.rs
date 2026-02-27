//! end-to-end encryption for syndicate messages
//!
//! QUIC provides transport encryption, but we need E2E encryption because:
//! - relay servers could see plaintext
//! - we want only syndicate members to read messages
//!
//! # approach
//!
//! derive a symmetric group key from the syndicate's shared secret.
//! all members can encrypt/decrypt, but outsiders cannot.
//!
//! ```text
//! syndicate_id + viewing_key → group_encryption_key
//!                                      │
//!                                      ▼
//!                              ChaCha20-Poly1305
//!                                      │
//!                              encrypted message
//! ```
//!
//! # key rotation
//!
//! when syndicate reshares (key rotation), derive new encryption key.
//! old messages remain readable with old key (members can store).

use alloc::vec::Vec;
use sha2::{Digest, Sha256};

/// encryption context for a syndicate
#[derive(Clone)]
pub struct SyndicateCrypto {
    /// syndicate id
    syndicate_id: [u8; 32],
    /// derived encryption key
    encryption_key: [u8; 32],
    /// key epoch (increments on reshare)
    epoch: u64,
}

impl SyndicateCrypto {
    /// create from syndicate viewing key
    pub fn new(syndicate_id: [u8; 32], viewing_key: &[u8; 32], epoch: u64) -> Self {
        let encryption_key = derive_encryption_key(&syndicate_id, viewing_key, epoch);
        Self {
            syndicate_id,
            encryption_key,
            epoch,
        }
    }

    /// get current epoch
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// encrypt message for syndicate
    pub fn encrypt(&self, plaintext: &[u8], nonce: &[u8; 12]) -> Vec<u8> {
        // ChaCha20-Poly1305 encryption
        // simplified - real impl would use proper AEAD
        let mut ciphertext = Vec::with_capacity(plaintext.len() + 16);

        // derive per-message key
        let mut hasher = Sha256::new();
        hasher.update(&self.encryption_key);
        hasher.update(nonce);
        let stream_key: [u8; 32] = hasher.finalize().into();

        // XOR encrypt (simplified - real impl uses ChaCha20)
        for (i, byte) in plaintext.iter().enumerate() {
            let key_byte = stream_key[i % 32];
            ciphertext.push(byte ^ key_byte);
        }

        // append tag (simplified - real impl uses Poly1305)
        let tag = compute_tag(&self.encryption_key, nonce, &ciphertext);
        ciphertext.extend_from_slice(&tag);

        ciphertext
    }

    /// decrypt message from syndicate member
    pub fn decrypt(&self, ciphertext: &[u8], nonce: &[u8; 12]) -> Option<Vec<u8>> {
        if ciphertext.len() < 16 {
            return None;
        }

        let (encrypted, tag) = ciphertext.split_at(ciphertext.len() - 16);

        // verify tag
        let expected_tag = compute_tag(&self.encryption_key, nonce, encrypted);
        if tag != expected_tag {
            return None;
        }

        // derive per-message key
        let mut hasher = Sha256::new();
        hasher.update(&self.encryption_key);
        hasher.update(nonce);
        let stream_key: [u8; 32] = hasher.finalize().into();

        // XOR decrypt
        let mut plaintext = Vec::with_capacity(encrypted.len());
        for (i, byte) in encrypted.iter().enumerate() {
            let key_byte = stream_key[i % 32];
            plaintext.push(byte ^ key_byte);
        }

        Some(plaintext)
    }

    /// rotate to new epoch (after reshare)
    pub fn rotate(&mut self, new_viewing_key: &[u8; 32]) {
        self.epoch += 1;
        self.encryption_key = derive_encryption_key(&self.syndicate_id, new_viewing_key, self.epoch);
    }
}

fn derive_encryption_key(syndicate_id: &[u8; 32], viewing_key: &[u8; 32], epoch: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"narsil-group-key-v1");
    hasher.update(syndicate_id);
    hasher.update(viewing_key);
    hasher.update(epoch.to_le_bytes());
    hasher.finalize().into()
}

fn compute_tag(key: &[u8; 32], nonce: &[u8; 12], data: &[u8]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(b"narsil-tag");
    hasher.update(key);
    hasher.update(nonce);
    hasher.update(data);
    let hash: [u8; 32] = hasher.finalize().into();
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&hash[..16]);
    tag
}

/// encrypted message envelope
#[derive(Clone, Debug)]
pub struct EncryptedMessage {
    /// key epoch used for encryption
    pub epoch: u64,
    /// nonce (unique per message)
    pub nonce: [u8; 12],
    /// ciphertext with auth tag
    pub ciphertext: Vec<u8>,
}

impl EncryptedMessage {
    /// create by encrypting plaintext
    pub fn seal(crypto: &SyndicateCrypto, plaintext: &[u8], nonce: [u8; 12]) -> Self {
        let ciphertext = crypto.encrypt(plaintext, &nonce);
        Self {
            epoch: crypto.epoch(),
            nonce,
            ciphertext,
        }
    }

    /// decrypt (returns None if auth fails)
    pub fn open(&self, crypto: &SyndicateCrypto) -> Option<Vec<u8>> {
        if self.epoch != crypto.epoch() {
            // TODO: support decrypting old epochs if stored
            return None;
        }
        crypto.decrypt(&self.ciphertext, &self.nonce)
    }

    /// serialize for transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + 12 + 4 + self.ciphertext.len());
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        buf.extend_from_slice(&self.nonce);
        buf.extend_from_slice(&(self.ciphertext.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.ciphertext);
        buf
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 + 12 + 4 {
            return None;
        }
        let epoch = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let nonce: [u8; 12] = bytes[8..20].try_into().ok()?;
        let len = u32::from_le_bytes(bytes[20..24].try_into().ok()?) as usize;
        if bytes.len() < 24 + len {
            return None;
        }
        let ciphertext = bytes[24..24 + len].to_vec();
        Some(Self { epoch, nonce, ciphertext })
    }
}

/// generate random nonce (requires RNG)
pub fn generate_nonce<R: rand_core::RngCore>(rng: &mut R) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    nonce
}

/// signed message wrapper
///
/// wraps any message with sender signature for authenticity
#[derive(Clone, Debug)]
pub struct SignedMessage {
    /// sender's public key
    pub sender: [u8; 32],
    /// message content (may be encrypted)
    pub content: Vec<u8>,
    /// signature over (sender || content)
    pub signature: [u8; 64],
}

impl SignedMessage {
    /// create signed message (caller provides signature)
    pub fn new(sender: [u8; 32], content: Vec<u8>, signature: [u8; 64]) -> Self {
        Self { sender, content, signature }
    }

    /// get signing payload
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(32 + self.content.len());
        payload.extend_from_slice(&self.sender);
        payload.extend_from_slice(&self.content);
        payload
    }

    /// serialize for transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32 + 4 + self.content.len() + 64);
        buf.extend_from_slice(&self.sender);
        buf.extend_from_slice(&(self.content.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.content);
        buf.extend_from_slice(&self.signature);
        buf
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 + 4 + 64 {
            return None;
        }
        let sender: [u8; 32] = bytes[0..32].try_into().ok()?;
        let content_len = u32::from_le_bytes(bytes[32..36].try_into().ok()?) as usize;
        if bytes.len() < 36 + content_len + 64 {
            return None;
        }
        let content = bytes[36..36 + content_len].to_vec();
        let signature: [u8; 64] = bytes[36 + content_len..36 + content_len + 64]
            .try_into()
            .ok()?;
        Some(Self { sender, content, signature })
    }
}

/// per-member encryption context
///
/// used for direct messages to specific members via their mailboxes.
/// derives a shared key using ECDH-like construction.
#[derive(Clone)]
pub struct MemberCrypto {
    /// our secret key (for signing and key derivation)
    my_secret: [u8; 32],
    /// our public key
    my_pubkey: [u8; 32],
    /// syndicate context for domain separation
    syndicate_id: [u8; 32],
}

impl MemberCrypto {
    /// create from secret key
    pub fn new(my_secret: [u8; 32], syndicate_id: [u8; 32]) -> Self {
        // derive pubkey (simplified - real impl uses curve ops)
        let my_pubkey = derive_pubkey(&my_secret);
        Self {
            my_secret,
            my_pubkey,
            syndicate_id,
        }
    }

    /// get our public key
    pub fn pubkey(&self) -> &[u8; 32] {
        &self.my_pubkey
    }

    /// derive shared key with another member
    ///
    /// this is a simplified construction. in production, use proper ECDH
    /// where: shared = my_secret * their_pubkey = their_secret * my_pubkey
    pub fn derive_shared_key(&self, their_pubkey: &[u8; 32]) -> [u8; 32] {
        // for this simplified version, we use sorted pubkeys to ensure
        // both parties derive the same key
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-shared-key-v1");
        hasher.update(&self.syndicate_id);

        // sort pubkeys for symmetric derivation
        if self.my_pubkey < *their_pubkey {
            hasher.update(&self.my_pubkey);
            hasher.update(their_pubkey);
        } else {
            hasher.update(their_pubkey);
            hasher.update(&self.my_pubkey);
        }

        hasher.finalize().into()
    }

    /// encrypt message for specific recipient
    pub fn encrypt_for(&self, recipient: &[u8; 32], plaintext: &[u8], nonce: &[u8; 12]) -> Vec<u8> {
        let shared_key = self.derive_shared_key(recipient);
        encrypt_with_key(&shared_key, plaintext, nonce)
    }

    /// decrypt message from specific sender
    pub fn decrypt_from(
        &self,
        sender: &[u8; 32],
        ciphertext: &[u8],
        nonce: &[u8; 12],
    ) -> Option<Vec<u8>> {
        let shared_key = self.derive_shared_key(sender);
        decrypt_with_key(&shared_key, ciphertext, nonce)
    }

    /// sign a message
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        // simplified signature - real impl uses Ed25519 or similar
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-sig-v1");
        hasher.update(&self.my_secret);
        hasher.update(message);
        let hash: [u8; 32] = hasher.finalize().into();

        let mut signature = [0u8; 64];
        signature[..32].copy_from_slice(&hash);
        signature[32..].copy_from_slice(&self.my_pubkey);
        signature
    }

    /// verify a signature (returns true if valid)
    pub fn verify(sender: &[u8; 32], _message: &[u8], signature: &[u8; 64]) -> bool {
        // extract claimed pubkey from signature
        let claimed_pubkey: [u8; 32] = signature[32..64].try_into().unwrap_or([0u8; 32]);
        if &claimed_pubkey != sender {
            return false;
        }

        // for simplified signature, we can't verify without secret
        // real impl would use public key verification
        // this is just a placeholder showing the structure
        !signature.iter().all(|&b| b == 0)
    }

    /// create signed message
    pub fn sign_message(&self, content: Vec<u8>) -> SignedMessage {
        let mut payload = Vec::with_capacity(32 + content.len());
        payload.extend_from_slice(&self.my_pubkey);
        payload.extend_from_slice(&content);
        let signature = self.sign(&payload);
        SignedMessage::new(self.my_pubkey, content, signature)
    }
}

fn derive_pubkey(secret: &[u8; 32]) -> [u8; 32] {
    // simplified - real impl uses curve point multiplication
    let mut hasher = Sha256::new();
    hasher.update(b"narsil-pubkey-v1");
    hasher.update(secret);
    hasher.finalize().into()
}

fn encrypt_with_key(key: &[u8; 32], plaintext: &[u8], nonce: &[u8; 12]) -> Vec<u8> {
    let mut ciphertext = Vec::with_capacity(plaintext.len() + 16);

    // derive stream key
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(nonce);
    let stream_key: [u8; 32] = hasher.finalize().into();

    // XOR encrypt
    for (i, byte) in plaintext.iter().enumerate() {
        let key_byte = stream_key[i % 32];
        ciphertext.push(byte ^ key_byte);
    }

    // append tag
    let tag = compute_tag(key, nonce, &ciphertext);
    ciphertext.extend_from_slice(&tag);

    ciphertext
}

fn decrypt_with_key(key: &[u8; 32], ciphertext: &[u8], nonce: &[u8; 12]) -> Option<Vec<u8>> {
    if ciphertext.len() < 16 {
        return None;
    }

    let (encrypted, tag) = ciphertext.split_at(ciphertext.len() - 16);

    // verify tag
    let expected_tag = compute_tag(key, nonce, encrypted);
    if tag != expected_tag {
        return None;
    }

    // derive stream key
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(nonce);
    let stream_key: [u8; 32] = hasher.finalize().into();

    // XOR decrypt
    let mut plaintext = Vec::with_capacity(encrypted.len());
    for (i, byte) in encrypted.iter().enumerate() {
        let key_byte = stream_key[i % 32];
        plaintext.push(byte ^ key_byte);
    }

    Some(plaintext)
}

/// encrypted direct message (to specific recipient)
#[derive(Clone, Debug)]
pub struct DirectMessage {
    /// sender's public key
    pub sender: [u8; 32],
    /// recipient's public key
    pub recipient: [u8; 32],
    /// nonce
    pub nonce: [u8; 12],
    /// encrypted content with auth tag
    pub ciphertext: Vec<u8>,
}

impl DirectMessage {
    /// create encrypted message
    pub fn seal(
        crypto: &MemberCrypto,
        recipient: &[u8; 32],
        plaintext: &[u8],
        nonce: [u8; 12],
    ) -> Self {
        let ciphertext = crypto.encrypt_for(recipient, plaintext, &nonce);
        Self {
            sender: *crypto.pubkey(),
            recipient: *recipient,
            nonce,
            ciphertext,
        }
    }

    /// decrypt message (returns None if not for us or auth fails)
    pub fn open(&self, crypto: &MemberCrypto) -> Option<Vec<u8>> {
        if &self.recipient != crypto.pubkey() {
            return None;
        }
        crypto.decrypt_from(&self.sender, &self.ciphertext, &self.nonce)
    }

    /// serialize
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32 + 32 + 12 + 4 + self.ciphertext.len());
        buf.extend_from_slice(&self.sender);
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&self.nonce);
        buf.extend_from_slice(&(self.ciphertext.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.ciphertext);
        buf
    }

    /// deserialize
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 + 32 + 12 + 4 {
            return None;
        }
        let sender: [u8; 32] = bytes[0..32].try_into().ok()?;
        let recipient: [u8; 32] = bytes[32..64].try_into().ok()?;
        let nonce: [u8; 12] = bytes[64..76].try_into().ok()?;
        let len = u32::from_le_bytes(bytes[76..80].try_into().ok()?) as usize;
        if bytes.len() < 80 + len {
            return None;
        }
        let ciphertext = bytes[80..80 + len].to_vec();
        Some(Self { sender, recipient, nonce, ciphertext })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let crypto = SyndicateCrypto::new([1u8; 32], &[2u8; 32], 0);
        let plaintext = b"hello syndicate members!";
        let nonce = [3u8; 12];

        let ciphertext = crypto.encrypt(plaintext, &nonce);
        let decrypted = crypto.decrypt(&ciphertext, &nonce).unwrap();

        assert_eq!(plaintext.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_wrong_key_fails() {
        let crypto1 = SyndicateCrypto::new([1u8; 32], &[2u8; 32], 0);
        let crypto2 = SyndicateCrypto::new([1u8; 32], &[3u8; 32], 0); // different viewing key

        let plaintext = b"secret message";
        let nonce = [4u8; 12];

        let ciphertext = crypto1.encrypt(plaintext, &nonce);
        let result = crypto2.decrypt(&ciphertext, &nonce);

        assert!(result.is_none()); // auth tag should fail
    }

    #[test]
    fn test_envelope_roundtrip() {
        let crypto = SyndicateCrypto::new([1u8; 32], &[2u8; 32], 0);
        let plaintext = b"message in envelope";
        let nonce = [5u8; 12];

        let envelope = EncryptedMessage::seal(&crypto, plaintext, nonce);

        // serialize/deserialize
        let bytes = envelope.to_bytes();
        let recovered = EncryptedMessage::from_bytes(&bytes).unwrap();

        // decrypt
        let decrypted = recovered.open(&crypto).unwrap();
        assert_eq!(plaintext.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_epoch_mismatch() {
        let mut crypto = SyndicateCrypto::new([1u8; 32], &[2u8; 32], 0);
        let plaintext = b"old message";
        let nonce = [6u8; 12];

        let envelope = EncryptedMessage::seal(&crypto, plaintext, nonce);

        // rotate key
        crypto.rotate(&[7u8; 32]);

        // old message should fail (different epoch)
        let result = envelope.open(&crypto);
        assert!(result.is_none());
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let crypto = SyndicateCrypto::new([1u8; 32], &[2u8; 32], 0);
        let plaintext = b"authentic message";
        let nonce = [8u8; 12];

        let mut ciphertext = crypto.encrypt(plaintext, &nonce);

        // tamper with ciphertext
        if !ciphertext.is_empty() {
            ciphertext[0] ^= 0xFF;
        }

        let result = crypto.decrypt(&ciphertext, &nonce);
        assert!(result.is_none()); // auth tag should fail
    }

    #[test]
    fn test_signed_message_roundtrip() {
        let syndicate_id = [1u8; 32];
        let alice = MemberCrypto::new([2u8; 32], syndicate_id);

        let content = b"hello from alice".to_vec();
        let signed = alice.sign_message(content.clone());

        assert_eq!(signed.sender, *alice.pubkey());
        assert_eq!(signed.content, content);

        // serialize roundtrip
        let bytes = signed.to_bytes();
        let recovered = SignedMessage::from_bytes(&bytes).unwrap();
        assert_eq!(recovered.sender, signed.sender);
        assert_eq!(recovered.content, signed.content);
        assert_eq!(recovered.signature, signed.signature);
    }

    #[test]
    fn test_direct_message_roundtrip() {
        let syndicate_id = [1u8; 32];
        let alice = MemberCrypto::new([2u8; 32], syndicate_id);
        let bob = MemberCrypto::new([3u8; 32], syndicate_id);

        let plaintext = b"secret for bob only";
        let nonce = [4u8; 12];

        // alice encrypts for bob
        let msg = DirectMessage::seal(&alice, bob.pubkey(), plaintext, nonce);

        // bob can decrypt
        let decrypted = msg.open(&bob).unwrap();
        assert_eq!(decrypted.as_slice(), plaintext);

        // alice cannot decrypt (wrong recipient)
        assert!(msg.open(&alice).is_none());
    }

    #[test]
    fn test_direct_message_wrong_sender() {
        let syndicate_id = [1u8; 32];
        let alice = MemberCrypto::new([2u8; 32], syndicate_id);
        let bob = MemberCrypto::new([3u8; 32], syndicate_id);
        let carol = MemberCrypto::new([4u8; 32], syndicate_id);

        let plaintext = b"from alice to bob";
        let nonce = [5u8; 12];

        // alice encrypts for bob
        let msg = DirectMessage::seal(&alice, bob.pubkey(), plaintext, nonce);

        // bob can decrypt
        assert!(msg.open(&bob).is_some());

        // carol cannot decrypt (not the recipient)
        assert!(msg.open(&carol).is_none());
    }

    #[test]
    fn test_direct_message_serialization() {
        let syndicate_id = [1u8; 32];
        let alice = MemberCrypto::new([2u8; 32], syndicate_id);
        let bob = MemberCrypto::new([3u8; 32], syndicate_id);

        let plaintext = b"test message";
        let nonce = [6u8; 12];

        let msg = DirectMessage::seal(&alice, bob.pubkey(), plaintext, nonce);
        let bytes = msg.to_bytes();
        let recovered = DirectMessage::from_bytes(&bytes).unwrap();

        // bob can still decrypt
        let decrypted = recovered.open(&bob).unwrap();
        assert_eq!(decrypted.as_slice(), plaintext);
    }

    #[test]
    fn test_member_crypto_pubkey_derivation() {
        let syndicate_id = [1u8; 32];
        let alice1 = MemberCrypto::new([2u8; 32], syndicate_id);
        let alice2 = MemberCrypto::new([2u8; 32], syndicate_id);

        // same secret = same pubkey
        assert_eq!(alice1.pubkey(), alice2.pubkey());

        let bob = MemberCrypto::new([3u8; 32], syndicate_id);
        // different secret = different pubkey
        assert_ne!(alice1.pubkey(), bob.pubkey());
    }
}
