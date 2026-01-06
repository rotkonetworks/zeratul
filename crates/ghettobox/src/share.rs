//! share types and serialization
//!
//! shares can be encoded as:
//! - hex string
//! - base64 string
//! - bip39-like words (for human backup)

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// user share that must be stored separately (email, paper, etc)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    /// version for future compatibility
    pub version: u8,
    /// the actual share bytes
    #[serde(with = "hex_bytes")]
    pub data: Vec<u8>,
    /// optional checksum for verification
    #[serde(with = "hex_bytes")]
    pub checksum: Vec<u8>,
}

impl Share {
    /// create a new share from raw bytes
    pub fn new(data: Vec<u8>) -> Self {
        let checksum = crate::crypto::mac(&data, &[b"ghettobox:share_checksum:v1"]);
        Self {
            version: 1,
            data,
            checksum: checksum[..4].to_vec(),
        }
    }

    /// verify the share checksum
    pub fn verify(&self) -> Result<()> {
        let expected = crate::crypto::mac(&self.data, &[b"ghettobox:share_checksum:v1"]);
        if self.checksum == expected[..4] {
            Ok(())
        } else {
            Err(Error::ShareVerificationFailed)
        }
    }

    /// encode as hex string
    pub fn to_hex(&self) -> String {
        hex::encode(&self.to_bytes())
    }

    /// decode from hex string
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes = hex::decode(s).map_err(|_| Error::InvalidShareFormat)?;
        Self::from_bytes(&bytes)
    }

    /// encode as base64
    pub fn to_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.to_bytes())
    }

    /// decode from base64
    pub fn from_base64(s: &str) -> Result<Self> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| Error::InvalidShareFormat)?;
        Self::from_bytes(&bytes)
    }

    /// encode as words (for human-friendly backup)
    pub fn to_words(&self) -> String {
        bytes_to_words(&self.data)
    }

    /// decode from words
    pub fn from_words(words: &str) -> Result<Self> {
        let data = words_to_bytes(words)?;
        let share = Self::new(data);
        Ok(share)
    }

    /// serialize to bytes
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + 2 + self.data.len() + self.checksum.len());
        bytes.push(self.version);
        bytes.extend_from_slice(&(self.data.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&self.data);
        bytes.extend_from_slice(&self.checksum);
        bytes
    }

    /// deserialize from bytes
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 7 {
            return Err(Error::InvalidShareFormat);
        }

        let version = bytes[0];
        let data_len = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;

        if bytes.len() < 3 + data_len + 4 {
            return Err(Error::InvalidShareFormat);
        }

        let data = bytes[3..3 + data_len].to_vec();
        let checksum = bytes[3 + data_len..3 + data_len + 4].to_vec();

        let share = Self {
            version,
            data,
            checksum,
        };

        share.verify()?;
        Ok(share)
    }
}

/// bip39-like wordlist subset (256 words for 8 bits per word)
const WORDLIST: [&str; 256] = [
    "abandon", "ability", "able", "about", "above", "absent", "absorb", "abstract",
    "absurd", "abuse", "access", "accident", "account", "accuse", "achieve", "acid",
    "acoustic", "acquire", "across", "act", "action", "actor", "actress", "actual",
    "adapt", "add", "addict", "address", "adjust", "admit", "adult", "advance",
    "advice", "aerobic", "affair", "afford", "afraid", "again", "age", "agent",
    "agree", "ahead", "aim", "air", "airport", "aisle", "alarm", "album",
    "alcohol", "alert", "alien", "all", "alley", "allow", "almost", "alone",
    "alpha", "already", "also", "alter", "always", "amateur", "amazing", "among",
    "amount", "amused", "analyst", "anchor", "ancient", "anger", "angle", "angry",
    "animal", "ankle", "announce", "annual", "another", "answer", "antenna", "antique",
    "anxiety", "any", "apart", "apology", "appear", "apple", "approve", "april",
    "arch", "arctic", "area", "arena", "argue", "arm", "armed", "armor",
    "army", "around", "arrange", "arrest", "arrive", "arrow", "art", "artefact",
    "artist", "artwork", "ask", "aspect", "assault", "asset", "assist", "assume",
    "asthma", "athlete", "atom", "attack", "attend", "attitude", "attract", "auction",
    "audit", "august", "aunt", "author", "auto", "autumn", "average", "avocado",
    "avoid", "awake", "aware", "away", "awesome", "awful", "awkward", "axis",
    "baby", "bachelor", "bacon", "badge", "bag", "balance", "balcony", "ball",
    "bamboo", "banana", "banner", "bar", "barely", "bargain", "barrel", "base",
    "basic", "basket", "battle", "beach", "bean", "beauty", "because", "become",
    "beef", "before", "begin", "behave", "behind", "believe", "below", "belt",
    "bench", "benefit", "best", "betray", "better", "between", "beyond", "bicycle",
    "bid", "bike", "bind", "biology", "bird", "birth", "bitter", "black",
    "blade", "blame", "blanket", "blast", "bleak", "bless", "blind", "blood",
    "blossom", "blouse", "blue", "blur", "blush", "board", "boat", "body",
    "boil", "bomb", "bone", "bonus", "book", "boost", "border", "boring",
    "borrow", "boss", "bottom", "bounce", "box", "boy", "bracket", "brain",
    "brand", "brass", "brave", "bread", "breeze", "brick", "bridge", "brief",
    "bright", "bring", "brisk", "broccoli", "broken", "bronze", "broom", "brother",
    "brown", "brush", "bubble", "buddy", "budget", "buffalo", "build", "bulb",
    "bulk", "bullet", "bundle", "bunker", "burden", "burger", "burst", "bus",
    "business", "busy", "butter", "buyer", "buzz", "cabbage", "cabin", "cable",
];

/// convert bytes to words (1 word per byte)
fn bytes_to_words(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| WORDLIST[b as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// convert words to bytes
fn words_to_bytes(words: &str) -> Result<Vec<u8>> {
    words
        .split_whitespace()
        .map(|word| {
            WORDLIST
                .iter()
                .position(|&w| w == word.to_lowercase())
                .map(|i| i as u8)
                .ok_or(Error::InvalidShareFormat)
        })
        .collect()
}

/// hex serialization helper for serde
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        hex::decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_share_roundtrip_hex() {
        let share = Share::new(vec![1, 2, 3, 4, 5]);
        let hex = share.to_hex();
        let recovered = Share::from_hex(&hex).unwrap();
        assert_eq!(share.data, recovered.data);
    }

    #[test]
    fn test_share_roundtrip_base64() {
        let share = Share::new(vec![1, 2, 3, 4, 5]);
        let b64 = share.to_base64();
        let recovered = Share::from_base64(&b64).unwrap();
        assert_eq!(share.data, recovered.data);
    }

    #[test]
    fn test_share_roundtrip_words() {
        let share = Share::new(vec![0, 127, 255, 42]);
        let words = share.to_words();
        let recovered = Share::from_words(&words).unwrap();
        assert_eq!(share.data, recovered.data);
    }

    #[test]
    fn test_checksum_verification() {
        let mut share = Share::new(vec![1, 2, 3]);
        share.data[0] = 99; // tamper with data
        assert!(share.verify().is_err());
    }
}
