//! pss configuration types using osst

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
use osst::SecretShare;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// provider configuration for threshold operations
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// provider index (1-indexed per Shamir convention)
    pub index: u32,
    /// p2p listen port
    pub port: u16,
    /// peer addresses for other providers
    pub peers: Vec<SocketAddr>,
    /// ed25519 signing key (hex)
    pub signing_key: String,
    /// group public key (hex, set after DKG/setup)
    pub group_pubkey: Option<String>,
    /// secret share scalar (hex, private)
    pub share: Option<String>,
}

impl ProviderConfig {
    /// parse share scalar from hex
    pub fn share_scalar(&self) -> Option<Scalar> {
        self.share.as_ref().and_then(|hex_str| {
            let bytes = hex::decode(hex_str).ok()?;
            if bytes.len() != 32 {
                return None;
            }
            let arr: [u8; 32] = bytes.try_into().ok()?;
            Scalar::from_canonical_bytes(arr).into()
        })
    }

    /// parse group pubkey from hex
    pub fn group_pubkey_point(&self) -> Option<RistrettoPoint> {
        self.group_pubkey.as_ref().and_then(|hex_str| {
            let bytes = hex::decode(hex_str).ok()?;
            if bytes.len() != 32 {
                return None;
            }
            let compressed = curve25519_dalek::ristretto::CompressedRistretto::from_slice(&bytes).ok()?;
            compressed.decompress()
        })
    }

    /// create SecretShare from config
    pub fn secret_share(&self) -> Option<SecretShare<Scalar>> {
        let scalar = self.share_scalar()?;
        Some(SecretShare::new(self.index, scalar))
    }
}

/// peer configuration for the provider network
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// total number of providers
    pub num_providers: u32,
    /// threshold (minimum for operations)
    pub threshold: u32,
    /// all provider addresses
    pub providers: Vec<ProviderAddr>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderAddr {
    pub index: u32,
    pub addr: SocketAddr,
    /// ed25519 pubkey (hex)
    pub pubkey: String,
}

impl NetworkConfig {
    /// check if we have quorum
    pub fn has_quorum(&self, count: usize) -> bool {
        count >= self.threshold as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;

    #[test]
    fn test_provider_config_roundtrip() {
        // generate test scalar and point
        let scalar = Scalar::from(42u64);
        let point = RISTRETTO_BASEPOINT_POINT * scalar;

        let config = ProviderConfig {
            index: 1,
            port: 5000,
            peers: vec!["127.0.0.1:5001".parse().unwrap()],
            signing_key: "deadbeef".into(),
            group_pubkey: Some(hex::encode(point.compress().as_bytes())),
            share: Some(hex::encode(scalar.as_bytes())),
        };

        // verify roundtrip
        let recovered_scalar = config.share_scalar().unwrap();
        assert_eq!(recovered_scalar, scalar);

        let recovered_point = config.group_pubkey_point().unwrap();
        assert_eq!(recovered_point, point);
    }
}
