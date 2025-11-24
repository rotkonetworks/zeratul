//! Common network types

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use super::crypto_compat::Ed25519PublicKey;

/// Validator identifier (Ed25519 public key)
pub type ValidatorId = Ed25519PublicKey;

/// Peer identifier (same as validator ID for now)
pub type PeerId = Ed25519PublicKey;

/// Network message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Sender peer ID
    pub from: PeerId,

    /// Message payload
    pub payload: Vec<u8>,

    /// Stream kind (UP 0, CE 128, etc.)
    pub stream_kind: u8,
}

/// Validator endpoint (IPv6 + port as per JAM spec)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ValidatorEndpoint {
    /// IPv6 address (first 16 bytes of validator metadata)
    pub address: [u8; 16],

    /// Port (bytes 16-17 of validator metadata, little-endian)
    pub port: u16,
}

impl ValidatorEndpoint {
    /// Convert to SocketAddr (IPv6)
    pub fn to_socket_addr(&self) -> SocketAddr {
        let addr = std::net::Ipv6Addr::from(self.address);
        SocketAddr::V6(std::net::SocketAddrV6::new(addr, self.port, 0, 0))
    }

    /// Parse from validator metadata (first 18 bytes)
    pub fn from_metadata(metadata: &[u8]) -> Option<Self> {
        if metadata.len() < 18 {
            return None;
        }

        let mut address = [0u8; 16];
        address.copy_from_slice(&metadata[0..16]);

        // Little-endian port
        let port = u16::from_le_bytes([metadata[16], metadata[17]]);

        Some(Self { address, port })
    }
}

/// Alternative name for Ed25519 public key (JAM spec)
///
/// Given an Ed25519 public key k (32 bytes), compute:
/// N(k) = "e" + B(E32^-1(k), 52)
///
/// Where B encodes a 256-bit integer as base32 (52 chars)
pub fn alternative_name(pubkey: &[u8; 32]) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";

    // Convert public key bytes to big integer
    let mut n = num_bigint::BigUint::from_bytes_be(pubkey.as_ref());

    // Encode as base32 (52 characters)
    let mut result = Vec::with_capacity(53);
    result.push(b'e'); // Prefix

    for _ in 0..52 {
        let digit = (&n % 32u32).to_u32_digits();
        let idx = if digit.is_empty() { 0 } else { digit[0] as usize };
        result.push(ALPHABET[idx % 32]);
        n /= 32u32;
    }

    String::from_utf8(result).expect("base32 is valid ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validator_endpoint_conversion() {
        let metadata = [
            // IPv6: ::1 (localhost)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            // Port: 9000 (little-endian)
            0x28, 0x23, // 9000 = 0x2328
        ];

        let endpoint = ValidatorEndpoint::from_metadata(&metadata).unwrap();
        let addr = endpoint.to_socket_addr();

        assert_eq!(endpoint.port, 9000);
        println!("Address: {}", addr);
    }

    #[test]
    fn test_alternative_name() {
        // Test with dummy key
        let mut key_bytes = [0u8; 32];
        key_bytes[31] = 42; // Some non-zero value

        let name = alternative_name(&key_bytes);

        // Should start with 'e' and be 53 chars total
        assert_eq!(name.len(), 53);
        assert!(name.starts_with('e'));
        println!("Alternative name: {}", name);
    }
}
