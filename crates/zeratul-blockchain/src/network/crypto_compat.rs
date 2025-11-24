//! Cryptography Compatibility Layer
//!
//! Wrapper types for commonware cryptography types that add Serialize/Deserialize
//! and other trait implementations needed for networking.

use anyhow::Result;
use commonware_cryptography::ed25519;
use commonware_cryptography::bls12381;
use parity_scale_codec::{Encode, Decode};
use serde::{Deserialize, Serialize, Serializer, Deserializer};
use ed25519_consensus::{VerificationKey, SigningKey};

/// Wrapper for Ed25519 public key with Serialize/Deserialize
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ed25519PublicKey(ed25519::PublicKey);

impl Ed25519PublicKey {
    pub fn inner(&self) -> &ed25519::PublicKey {
        &self.0
    }

    pub fn into_inner(self) -> ed25519::PublicKey {
        self.0
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            anyhow::bail!("Ed25519 public key must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        // Use ed25519_consensus as intermediate (commonware can convert from it)
        let vk = VerificationKey::try_from(arr)
            .map_err(|e| anyhow::anyhow!("Invalid Ed25519 public key: {}", e))?;
        let key = ed25519::PublicKey::from(vk);
        Ok(Self(key))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl From<ed25519::PublicKey> for Ed25519PublicKey {
    fn from(key: ed25519::PublicKey) -> Self {
        Self(key)
    }
}

impl Serialize for Ed25519PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(self.0.as_ref())
    }
}

impl<'de> Deserialize<'de> for Ed25519PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        Self::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// Wrapper for Ed25519 private key
#[derive(Clone)]
pub struct Ed25519PrivateKey(ed25519::PrivateKey);

impl Ed25519PrivateKey {
    pub fn inner(&self) -> &ed25519::PrivateKey {
        &self.0
    }

    pub fn into_inner(self) -> ed25519::PrivateKey {
        self.0
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            anyhow::bail!("Ed25519 private key seed must be 32 bytes");
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        // Use ed25519_consensus as intermediate
        let sk = SigningKey::from(arr);
        let key = ed25519::PrivateKey::from(sk);
        Ok(Self(key))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }

    pub fn public_key(&self) -> Ed25519PublicKey {
        // Derive public key from private key using ed25519_consensus
        let bytes = self.0.as_ref();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(bytes);
        let sk = SigningKey::from(seed);
        let vk = sk.verification_key();
        Ed25519PublicKey::from_bytes(vk.as_ref()).unwrap()
    }

    /// Sign a message and return the signature bytes
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let bytes = self.0.as_ref();
        let mut seed = [0u8; 32];
        seed.copy_from_slice(bytes);
        let sk = SigningKey::from(seed);
        let signature = sk.sign(message);
        signature.to_bytes().to_vec()
    }
}

impl From<ed25519::PrivateKey> for Ed25519PrivateKey {
    fn from(key: ed25519::PrivateKey) -> Self {
        Self(key)
    }
}

/// Wrapper for BLS12-381 public key with parity-scale-codec
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlsPublicKey(bls12381::PublicKey);

impl BlsPublicKey {
    pub fn inner(&self) -> &bls12381::PublicKey {
        &self.0
    }

    pub fn into_inner(self) -> bls12381::PublicKey {
        self.0
    }

    pub fn from_encoded(bytes: &[u8]) -> Result<Self> {
        // TODO TODO TODO: Implement proper BLS12-381 G1 point deserialization
        // BLS public keys are G1 points (48 bytes compressed, 96 bytes uncompressed)
        // Need to:
        // 1. Deserialize bytes to G1 point using blst or arkworks
        // 2. Wrap in commonware PublicKey
        // For now, return error
        anyhow::bail!("BLS public key deserialization not yet implemented - need G1 point serialization")
    }

    pub fn encode_bytes(&self) -> Vec<u8> {
        // TODO TODO TODO: Implement proper BLS12-381 G1 point serialization
        // BLS public keys are G1 points (48 bytes compressed recommended)
        // Need to:
        // 1. Extract G1 point from PublicKey
        // 2. Serialize to compressed bytes using blst or arkworks
        // For now, return placeholder
        Vec::new()
    }
}

impl From<bls12381::PublicKey> for BlsPublicKey {
    fn from(key: bls12381::PublicKey) -> Self {
        Self(key)
    }
}

impl Serialize for BlsPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&self.encode_bytes())
    }
}

impl<'de> Deserialize<'de> for BlsPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        Self::from_encoded(&bytes).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ed25519_roundtrip() {
        let bytes = [42u8; 32];
        let key = Ed25519PublicKey::from_bytes(&bytes).unwrap();

        let serialized = bincode::serialize(&key).unwrap();
        let deserialized: Ed25519PublicKey = bincode::deserialize(&serialized).unwrap();

        assert_eq!(key, deserialized);
    }

    #[test]
    fn test_bls_roundtrip() {
        use commonware_cryptography::bls12381::primitives::group::{Element, G1};

        let inner = bls12381::PublicKey::from(G1::one());
        let key = BlsPublicKey::from(inner);

        let serialized = bincode::serialize(&key).unwrap();
        let deserialized: BlsPublicKey = bincode::deserialize(&serialized).unwrap();

        assert_eq!(key, deserialized);
    }
}
