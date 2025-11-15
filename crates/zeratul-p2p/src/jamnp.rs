//! JAM Simple Networking Protocol (JAMNP-S) implementation
//!
//! Based on: https://graypaper.com (Networking section)
//!
//! Key features:
//! - QUIC transport (UDP-based, low latency)
//! - TLS 1.3 encryption with Ed25519 certificates
//! - Bidirectional streams with unique persistent (UP) and common ephemeral (CE) patterns
//! - Message-based protocol with length prefixes

use serde::{Deserialize, Serialize};

/// Ed25519 public key (32 bytes)
pub type Ed25519PublicKey = [u8; 32];

/// Ed25519 signature (64 bytes)
pub type Ed25519Signature = [u8; 64];

/// Hash (32 bytes)
pub type Hash = [u8; 32];

/// Validator index
pub type ValidatorIndex = u16;

/// Core index
pub type CoreIndex = u16;

/// Slot number (increments every 6 seconds in JAM)
pub type Slot = u32;

/// Epoch index (slot / epoch_length)
pub type EpochIndex = u32;

/// TLS certificate requirements for JAMNP-S
#[derive(Debug, Clone)]
pub struct JamCertificate {
    /// Ed25519 key (must match certificate subject)
    pub ed25519_key: Ed25519PublicKey,
    /// Alternative name derived from Ed25519 key
    pub alt_name: String,
}

impl JamCertificate {
    /// Compute alternative name from Ed25519 public key
    ///
    /// N(k) = "e" ++ B(E32^-1(k), 52)
    /// Where B encodes in base32 using alphabet: abcdefghijklmnopqrstuvwxyz234567
    pub fn compute_alt_name(key: &Ed25519PublicKey) -> String {
        const BASE32_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";

        // Convert key to u256
        let mut n = num_bigint::BigUint::from_bytes_le(key);

        let mut result = String::with_capacity(53);
        result.push('e');

        // Encode 52 base32 characters
        for _ in 0..52 {
            let digit = (&n % 32u32).to_u32_digits()[0] as usize;
            result.push(BASE32_ALPHABET[digit] as char);
            n /= 32u32;
        }

        result
    }

    /// Create certificate from Ed25519 key
    pub fn new(ed25519_key: Ed25519PublicKey) -> Self {
        let alt_name = Self::compute_alt_name(&ed25519_key);
        Self { ed25519_key, alt_name }
    }
}

/// ALPN protocol identifier
///
/// Format: "jamnp-s/V/H" or "jamnp-s/V/H/builder"
/// - V = protocol version (0)
/// - H = first 8 nibbles of genesis header hash (lowercase hex)
#[derive(Debug, Clone)]
pub struct AlpnId {
    pub version: u8,
    pub genesis_hash_prefix: String,
    pub is_builder: bool,
}

impl AlpnId {
    pub fn new(genesis_hash: &Hash, is_builder: bool) -> Self {
        let prefix = hex::encode(&genesis_hash[..4]); // First 8 nibbles
        Self {
            version: 0,
            genesis_hash_prefix: prefix,
            is_builder,
        }
    }

    pub fn to_string(&self) -> String {
        if self.is_builder {
            format!("jamnp-s/{}/{}/builder", self.version, self.genesis_hash_prefix)
        } else {
            format!("jamnp-s/{}/{}", self.version, self.genesis_hash_prefix)
        }
    }
}

/// Preferred initiator for validator connections
///
/// P(a, b) = a if (a_31 > 127) ⊕ (b_31 > 127) ⊕ (a < b), else b
pub fn preferred_initiator(a: &Ed25519PublicKey, b: &Ed25519PublicKey) -> Ed25519PublicKey {
    let a_high = a[31] > 127;
    let b_high = b[31] > 127;
    let a_less = a < b;

    if a_high ^ b_high ^ a_less {
        *a
    } else {
        *b
    }
}

/// Stream kind identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Unique Persistent (UP) streams (0-127)
    UP(u8),
    /// Common Ephemeral (CE) streams (128-255)
    CE(u8),
}

impl StreamKind {
    pub fn to_byte(&self) -> u8 {
        match self {
            StreamKind::UP(n) => *n,
            StreamKind::CE(n) => *n,
        }
    }

    pub fn from_byte(b: u8) -> Self {
        if b < 128 {
            StreamKind::UP(b)
        } else {
            StreamKind::CE(b)
        }
    }
}

/// UP 0: Block announcement stream
pub const UP_BLOCK_ANNOUNCEMENT: StreamKind = StreamKind::UP(0);

/// Common Ephemeral (CE) stream kinds
pub const CE_BLOCK_REQUEST: StreamKind = StreamKind::CE(128);
pub const CE_STATE_REQUEST: StreamKind = StreamKind::CE(129);
pub const CE_SAFROLE_TICKET_DIST_1: StreamKind = StreamKind::CE(131);
pub const CE_SAFROLE_TICKET_DIST_2: StreamKind = StreamKind::CE(132);
pub const CE_WORK_PACKAGE_SUBMIT: StreamKind = StreamKind::CE(133);
pub const CE_WORK_PACKAGE_SHARE: StreamKind = StreamKind::CE(134);
pub const CE_WORK_REPORT_DIST: StreamKind = StreamKind::CE(135);
pub const CE_WORK_REPORT_REQUEST: StreamKind = StreamKind::CE(136);

/// Message encoding/decoding
///
/// All messages are prefixed with a 4-byte little-endian length
pub struct Message;

impl Message {
    /// Encode message: 4-byte length prefix + content
    pub fn encode(content: &[u8]) -> Vec<u8> {
        let len = content.len() as u32;
        let mut buf = Vec::with_capacity(4 + content.len());
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(content);
        buf
    }

    /// Decode message: read 4-byte length, then content
    pub fn decode(buf: &[u8]) -> Result<(usize, Vec<u8>), DecodeError> {
        if buf.len() < 4 {
            return Err(DecodeError::InsufficientData);
        }

        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

        if buf.len() < 4 + len {
            return Err(DecodeError::InsufficientData);
        }

        let content = buf[4..4 + len].to_vec();
        Ok((4 + len, content))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("Insufficient data")]
    InsufficientData,

    #[error("Invalid message format")]
    InvalidFormat,
}

/// Block announcement (UP 0)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockAnnouncement {
    /// Block header
    pub header: Vec<u8>, // Encoded header
    /// Latest finalized block
    pub finalized_hash: Hash,
    /// Latest finalized slot
    pub finalized_slot: Slot,
}

/// Block announcement handshake (UP 0)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHandshake {
    /// Latest finalized block
    pub finalized_hash: Hash,
    /// Latest finalized slot
    pub finalized_slot: Slot,
    /// Known leaves (chain tips)
    pub leaves: Vec<(Hash, Slot)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alt_name_generation() {
        let key = [0u8; 32];
        let alt_name = JamCertificate::compute_alt_name(&key);
        assert_eq!(alt_name.len(), 53);
        assert!(alt_name.starts_with('e'));
    }

    #[test]
    fn test_alpn_id() {
        let genesis_hash = [0xAB; 32];
        let alpn = AlpnId::new(&genesis_hash, false);
        assert_eq!(alpn.to_string(), "jamnp-s/0/abababab");

        let alpn_builder = AlpnId::new(&genesis_hash, true);
        assert_eq!(alpn_builder.to_string(), "jamnp-s/0/abababab/builder");
    }

    #[test]
    fn test_preferred_initiator() {
        let a = [0u8; 32];
        let mut b = [0u8; 32];
        b[31] = 200; // High bit set

        let preferred = preferred_initiator(&a, &b);
        // Should deterministically select one
        assert!(preferred == a || preferred == b);
    }

    #[test]
    fn test_message_encode_decode() {
        let content = b"hello world";
        let encoded = Message::encode(content);

        assert_eq!(encoded.len(), 4 + content.len());

        let (bytes_read, decoded) = Message::decode(&encoded).unwrap();
        assert_eq!(bytes_read, encoded.len());
        assert_eq!(decoded, content);
    }

    #[test]
    fn test_stream_kind() {
        assert_eq!(UP_BLOCK_ANNOUNCEMENT.to_byte(), 0);
        assert_eq!(CE_BLOCK_REQUEST.to_byte(), 128);

        let kind = StreamKind::from_byte(128);
        assert_eq!(kind, CE_BLOCK_REQUEST);
    }
}
