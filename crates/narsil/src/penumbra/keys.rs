//! key management for penumbra syndicates
//!
//! # design principles (inspired by daniel micay's security philosophy)
//!
//! 1. **key isolation**: syndicate keys are INDEPENDENT of personal keys
//!    - compromise of personal key doesn't compromise syndicate share
//!    - compromise of syndicate share doesn't compromise personal key
//!
//! 2. **minimal trust**: members don't need to trust each other's opsec
//!    - DKG generates shares without any party knowing full key
//!    - t-of-n threshold means minority compromise is survivable
//!
//! 3. **forward secrecy via reshare**: periodic resharing means
//!    - old compromised shares become useless
//!    - members can rotate without changing syndicate address
//!
//! # key types
//!
//! ```text
//! personal key (member's own penumbra account)
//!     - used for: identity, P2P auth, receiving distributions
//!     - NOT used for: deriving syndicate shares
//!
//! syndicate spending key (OSST group key)
//!     - created via DKG among members
//!     - split into shares held by each member
//!     - used for: signing syndicate transactions
//!
//! syndicate viewing key (derived from spending key)
//!     - shared with ALL members (not threshold)
//!     - used for: scanning chain for syndicate notes
//!     - cannot sign, only view
//! ```

use alloc::format;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

#[cfg(feature = "decaf377")]
use decaf377::{Element, Fr};

/// unique identifier for a syndicate
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SyndicateId(pub [u8; 32]);

impl SyndicateId {
    /// generate from founding parameters
    pub fn generate(
        founding_members: &[[u8; 32]],  // personal pubkeys
        threshold: u32,
        nonce: &[u8],
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-syndicate-v1");
        hasher.update((founding_members.len() as u32).to_le_bytes());
        for member in founding_members {
            hasher.update(member);
        }
        hasher.update(threshold.to_le_bytes());
        hasher.update(nonce);
        Self(hasher.finalize().into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// member's personal keys (their own penumbra account)
#[derive(Clone)]
pub struct MemberKeys {
    /// member index in syndicate (1-indexed, matches OSST)
    pub index: u32,
    /// personal spending key (for auth, NOT for syndicate)
    /// stored as bytes to avoid coupling to specific key type
    spending_key: [u8; 32],
    /// personal public key (for identification)
    pub pubkey: [u8; 32],
}

impl MemberKeys {
    pub fn new(index: u32, spending_key: [u8; 32], pubkey: [u8; 32]) -> Self {
        Self {
            index,
            spending_key,
            pubkey,
        }
    }

    /// sign a message with personal key (for P2P auth, governance votes)
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        // simplified - real impl would use proper schnorr
        let mut hasher = Sha256::new();
        hasher.update(&self.spending_key);
        hasher.update(message);
        let hash: [u8; 32] = hasher.finalize().into();
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&hash);
        sig[32..].copy_from_slice(&self.pubkey);
        sig
    }

    /// verify a signature from another member
    pub fn verify(pubkey: &[u8; 32], _message: &[u8], signature: &[u8; 64]) -> bool {
        // simplified - real impl would verify schnorr
        signature[32..] == *pubkey
    }
}

impl core::fmt::Debug for MemberKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MemberKeys")
            .field("index", &self.index)
            .field("spending_key", &"[REDACTED]")
            .field("pubkey", &hex_short(&self.pubkey))
            .finish()
    }
}

/// syndicate keys (group keys from DKG)
#[derive(Clone)]
pub struct SyndicateKeys {
    /// syndicate identifier
    pub id: SyndicateId,
    /// group public key (the "address" of the syndicate)
    pub group_pubkey: [u8; 32],
    /// full viewing key (for scanning, shared with all members)
    /// this is derived from spending key but cannot sign
    pub viewing_key: [u8; 32],
    /// threshold required for signing
    pub threshold: u32,
    /// total members
    pub members: u32,
}

impl SyndicateKeys {
    /// create from DKG output
    pub fn from_dkg(
        id: SyndicateId,
        group_pubkey: [u8; 32],
        threshold: u32,
        members: u32,
    ) -> Self {
        // derive viewing key from group pubkey
        // in real penumbra, this would be proper FVK derivation
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-fvk");
        hasher.update(&group_pubkey);
        let viewing_key: [u8; 32] = hasher.finalize().into();

        Self {
            id,
            group_pubkey,
            viewing_key,
            threshold,
            members,
        }
    }

    /// derive address for receiving funds
    pub fn address(&self, diversifier_index: u64) -> SyndicateAddress {
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-addr");
        hasher.update(&self.group_pubkey);
        hasher.update(diversifier_index.to_le_bytes());
        SyndicateAddress {
            inner: hasher.finalize().into(),
            diversifier_index,
        }
    }

    /// check if a note belongs to this syndicate (using viewing key)
    pub fn can_view(&self, note_commitment: &[u8; 32]) -> bool {
        // simplified - real impl would do proper note decryption
        // with the viewing key
        let _ = note_commitment;
        true
    }
}

impl core::fmt::Debug for SyndicateKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SyndicateKeys")
            .field("id", &hex_short(&self.id.0))
            .field("group_pubkey", &hex_short(&self.group_pubkey))
            .field("threshold", &format!("{}/{}", self.threshold, self.members))
            .finish()
    }
}

/// syndicate address (for receiving funds)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyndicateAddress {
    pub inner: [u8; 32],
    pub diversifier_index: u64,
}

impl SyndicateAddress {
    pub fn to_bytes(&self) -> [u8; 40] {
        let mut buf = [0u8; 40];
        buf[..32].copy_from_slice(&self.inner);
        buf[32..].copy_from_slice(&self.diversifier_index.to_le_bytes());
        buf
    }
}

/// DKG ceremony for creating syndicate
///
/// this is a simplified version - real impl would use
/// proper verifiable DKG (e.g., Pedersen DKG or FROST DKG)
pub struct DkgCeremony {
    /// syndicate being formed
    pub syndicate_id: SyndicateId,
    /// participating members (personal pubkeys)
    pub members: Vec<[u8; 32]>,
    /// threshold
    pub threshold: u32,
}

impl DkgCeremony {
    pub fn new(members: Vec<[u8; 32]>, threshold: u32, nonce: &[u8]) -> Self {
        let syndicate_id = SyndicateId::generate(&members, threshold, nonce);
        Self {
            syndicate_id,
            members,
            threshold,
        }
    }

    /// number of members
    pub fn member_count(&self) -> u32 {
        self.members.len() as u32
    }

    // DKG rounds would go here...
    // round 1: each party generates polynomial, broadcasts commitments
    // round 2: each party sends encrypted shares to others
    // round 3: each party verifies received shares
    // output: group pubkey + individual shares
}

/// member's share of the syndicate spending key
///
/// this wraps an OSST SecretShare for the syndicate
#[derive(Clone)]
pub struct SyndicateShare {
    /// which syndicate this share is for
    pub syndicate_id: SyndicateId,
    /// member index (matches OSST index)
    pub member_index: u32,
    /// the actual OSST share (encrypted at rest with personal key)
    share_bytes: [u8; 32],
}

impl SyndicateShare {
    pub fn new(syndicate_id: SyndicateId, member_index: u32, share_bytes: [u8; 32]) -> Self {
        Self {
            syndicate_id,
            member_index,
            share_bytes,
        }
    }

    /// get share bytes (for creating OSST SecretShare)
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.share_bytes
    }

    /// encrypt share for storage (with personal key)
    pub fn encrypt(&self, personal_key: &[u8; 32]) -> [u8; 64] {
        // simplified - real impl would use proper AEAD
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-share-key");
        hasher.update(personal_key);
        hasher.update(&self.syndicate_id.0);
        let key: [u8; 32] = hasher.finalize().into();

        let mut encrypted = [0u8; 64];
        for (i, (s, k)) in self.share_bytes.iter().zip(key.iter()).enumerate() {
            encrypted[i] = s ^ k;
        }
        encrypted[32..].copy_from_slice(&self.syndicate_id.0);
        encrypted
    }

    /// decrypt share from storage
    pub fn decrypt(
        encrypted: &[u8; 64],
        personal_key: &[u8; 32],
        member_index: u32,
    ) -> Option<Self> {
        let syndicate_id = SyndicateId(encrypted[32..].try_into().ok()?);

        let mut hasher = Sha256::new();
        hasher.update(b"narsil-share-key");
        hasher.update(personal_key);
        hasher.update(&syndicate_id.0);
        let key: [u8; 32] = hasher.finalize().into();

        let mut share_bytes = [0u8; 32];
        for (i, (e, k)) in encrypted[..32].iter().zip(key.iter()).enumerate() {
            share_bytes[i] = e ^ k;
        }

        Some(Self {
            syndicate_id,
            member_index,
            share_bytes,
        })
    }
}

impl core::fmt::Debug for SyndicateShare {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SyndicateShare")
            .field("syndicate_id", &hex_short(&self.syndicate_id.0))
            .field("member_index", &self.member_index)
            .field("share_bytes", &"[REDACTED]")
            .finish()
    }
}

fn hex_short(bytes: &[u8]) -> alloc::string::String {
    use alloc::format;
    if bytes.len() <= 4 {
        hex::encode(bytes)
    } else {
        format!("{}..{}", hex::encode(&bytes[..2]), hex::encode(&bytes[bytes.len()-2..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syndicate_id_deterministic() {
        let members = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
        let id1 = SyndicateId::generate(&members, 2, b"nonce");
        let id2 = SyndicateId::generate(&members, 2, b"nonce");
        assert_eq!(id1, id2);

        // different nonce = different id
        let id3 = SyndicateId::generate(&members, 2, b"other");
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_syndicate_address_derivation() {
        let id = SyndicateId([0u8; 32]);
        let keys = SyndicateKeys::from_dkg(id, [1u8; 32], 2, 3);

        let addr0 = keys.address(0);
        let addr1 = keys.address(1);

        // different diversifiers = different addresses
        assert_ne!(addr0.inner, addr1.inner);
    }

    #[test]
    fn test_share_encryption_roundtrip() {
        let syndicate_id = SyndicateId([42u8; 32]);
        let personal_key = [99u8; 32];
        let share = SyndicateShare::new(syndicate_id, 1, [123u8; 32]);

        let encrypted = share.encrypt(&personal_key);
        let decrypted = SyndicateShare::decrypt(&encrypted, &personal_key, 1).unwrap();

        assert_eq!(share.syndicate_id, decrypted.syndicate_id);
        assert_eq!(share.share_bytes, *decrypted.as_bytes());
    }

    #[test]
    fn test_share_wrong_key_fails() {
        let syndicate_id = SyndicateId([42u8; 32]);
        let personal_key = [99u8; 32];
        let wrong_key = [11u8; 32];
        let share = SyndicateShare::new(syndicate_id, 1, [123u8; 32]);

        let encrypted = share.encrypt(&personal_key);
        let decrypted = SyndicateShare::decrypt(&encrypted, &wrong_key, 1).unwrap();

        // decryption "succeeds" but gives wrong bytes (xor is symmetric)
        assert_ne!(share.share_bytes, *decrypted.as_bytes());
    }
}
