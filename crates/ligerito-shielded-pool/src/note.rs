//! shielded notes (utxos)
//!
//! a note represents a shielded balance owned by an address

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::keys::Address;
use crate::value::Value;
use crate::NOTE_DOMAIN;

/// random seed for note blinding
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rseed(pub [u8; 32]);

impl Rseed {
    #[cfg(feature = "std")]
    pub fn random<R: rand::RngCore>(rng: &mut R) -> Self {
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }
}

/// a shielded note (the "utxo")
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// value stored in this note
    pub value: Value,
    /// owner address
    pub address: Address,
    /// random seed for blinding/encryption
    pub rseed: Rseed,
}

impl Note {
    /// create a new note
    pub fn new(value: Value, address: Address, rseed: Rseed) -> Self {
        Self { value, address, rseed }
    }

    /// derive note blinding factor from rseed
    fn blinding(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ligerito.note.blinding.v1");
        hasher.update(&self.rseed.0);
        *hasher.finalize().as_bytes()
    }

    /// compute note commitment (published on-chain)
    pub fn commit(&self) -> NoteCommitment {
        let mut hasher = blake3::Hasher::new();
        hasher.update(NOTE_DOMAIN);
        hasher.update(&self.value.to_bytes());
        hasher.update(&self.address.to_bytes());
        hasher.update(&self.blinding());
        NoteCommitment(*hasher.finalize().as_bytes())
    }

    /// encrypt note for the recipient
    pub fn encrypt(&self) -> Vec<u8> {
        // header: diversifier (for recipient to find their notes)
        let mut ciphertext = Vec::with_capacity(32 + 48 + 32);
        ciphertext.extend_from_slice(&self.address.diversifier);

        // derive encryption key from diversifier
        // recipient can derive the same key using their incoming viewing key
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ligerito.note-encryption.v1");
        hasher.update(&self.address.diversifier);
        let key = hasher.finalize();

        // payload: value (48 bytes) + rseed (32 bytes)
        let mut payload = [0u8; 80];
        payload[..48].copy_from_slice(&self.value.to_bytes());
        payload[48..80].copy_from_slice(&self.rseed.0);

        // xor encrypt (simplified - use chacha20poly1305 in production)
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= key.as_bytes()[i % 32];
        }
        ciphertext.extend_from_slice(&payload);

        ciphertext
    }

    /// encode for proofs
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(48 + 36 + 32);
        bytes.extend_from_slice(&self.value.to_bytes());
        bytes.extend_from_slice(&self.address.to_bytes());
        bytes.extend_from_slice(&self.rseed.0);
        bytes
    }
}

/// commitment to a note (what goes in the state tree)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NoteCommitment(pub [u8; 32]);

impl NoteCommitment {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for NoteCommitment {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::SpendKey;
    use crate::value::AssetId;

    #[test]
    fn test_note_commitment() {
        let sk = SpendKey::from_phrase("test", "");
        let addr = sk.address(0);
        let value = Value::new(AssetId::NATIVE, 1000u64.into());
        let rseed = Rseed([1u8; 32]);

        let note = Note::new(value, addr, rseed);
        let commitment = note.commit();

        // same note = same commitment
        let note2 = Note::new(value, addr, rseed);
        assert_eq!(note.commit(), note2.commit());

        // different rseed = different commitment
        let note3 = Note::new(value, addr, Rseed([2u8; 32]));
        assert_ne!(note.commit(), note3.commit());
    }

    #[test]
    fn test_note_encrypt_decrypt() {
        let sk = SpendKey::from_phrase("test", "");
        let vk = sk.view_key();
        let addr = sk.address(0);
        let value = Value::new(AssetId::NATIVE, 1000u64.into());
        let rseed = Rseed([42u8; 32]);

        let note = Note::new(value, addr, rseed);
        let ciphertext = note.encrypt();

        // view key should be able to decrypt
        let decrypted = vk.try_decrypt(&ciphertext);
        assert!(decrypted.is_some());

        let dec = decrypted.unwrap();
        assert_eq!(dec.value_bytes, value.to_bytes());
        assert_eq!(dec.rseed, rseed.0);
    }
}
