//! pseudonymous mailbox addressing for relay coordination
//!
//! members derive mailbox addresses from their viewing keys. relays see
//! activity patterns but cannot link mailboxes to penumbra addresses.
//!
//! # addressing scheme
//!
//! ```text
//! mailbox_id = sha256(viewing_key || domain_separator || syndicate_id)
//! ```
//!
//! the domain separator prevents cross-protocol attacks. syndicate_id
//! provides isolation between syndicates.
//!
//! # polling model
//!
//! members poll their mailboxes periodically. messages are encrypted to
//! recipients and signed by senders. relays cannot read content.

use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::wire::Hash32;

/// domain separator for mailbox derivation
const MAILBOX_DOMAIN: &[u8] = b"narsil-mailbox-v1";

/// domain separator for syndicate broadcast topic
const BROADCAST_DOMAIN: &[u8] = b"narsil-broadcast-v1";

/// pseudonymous mailbox identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MailboxId(pub Hash32);

impl MailboxId {
    /// derive mailbox id from viewing key and syndicate
    ///
    /// this creates a pseudonymous address that:
    /// - is deterministically derived (same inputs = same mailbox)
    /// - cannot be linked to viewing key without knowledge of key
    /// - is unique per syndicate (same key in different syndicates = different mailbox)
    pub fn derive(viewing_key: &[u8], syndicate_id: &Hash32) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(viewing_key);
        hasher.update(MAILBOX_DOMAIN);
        hasher.update(syndicate_id);
        Self(hasher.finalize().into())
    }

    /// derive a rotated mailbox id for forward secrecy
    ///
    /// use epoch to rotate mailboxes periodically. old mailboxes become
    /// unreadable without the rotation secret.
    pub fn derive_rotated(viewing_key: &[u8], syndicate_id: &Hash32, epoch: u64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(viewing_key);
        hasher.update(MAILBOX_DOMAIN);
        hasher.update(syndicate_id);
        hasher.update(&epoch.to_le_bytes());
        Self(hasher.finalize().into())
    }

    /// get the raw bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// create from raw bytes
    pub fn from_bytes(bytes: Hash32) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for MailboxId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// broadcast topic for syndicate-wide messages
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BroadcastTopic(pub Hash32);

impl BroadcastTopic {
    /// derive broadcast topic from syndicate id
    ///
    /// all syndicate members subscribe to this topic for proposals,
    /// votes, and contributions. relays see subscription but not content.
    pub fn derive(syndicate_id: &Hash32) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(BROADCAST_DOMAIN);
        hasher.update(syndicate_id);
        Self(hasher.finalize().into())
    }

    /// get the raw bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8]> for BroadcastTopic {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// address book entry for a syndicate member
#[derive(Clone, Debug)]
pub struct MemberAddress {
    /// member's public key (for signatures and encryption)
    pub pubkey: Hash32,
    /// member's mailbox id (derived, not stored long-term)
    pub mailbox: MailboxId,
    /// shares owned (cached from registry)
    pub share_count: u8,
}

impl MemberAddress {
    /// create address entry
    pub fn new(pubkey: Hash32, mailbox: MailboxId, share_count: u8) -> Self {
        Self {
            pubkey,
            mailbox,
            share_count,
        }
    }
}

/// message routing for a syndicate
#[derive(Clone, Debug)]
pub struct SyndicateRouter {
    /// syndicate identifier
    pub syndicate_id: Hash32,
    /// broadcast topic for syndicate
    pub broadcast: BroadcastTopic,
    /// known member addresses
    pub members: Vec<MemberAddress>,
}

impl SyndicateRouter {
    /// create router for syndicate
    pub fn new(syndicate_id: Hash32) -> Self {
        Self {
            syndicate_id,
            broadcast: BroadcastTopic::derive(&syndicate_id),
            members: Vec::new(),
        }
    }

    /// add member to router
    pub fn add_member(&mut self, pubkey: Hash32, viewing_key: &[u8], share_count: u8) {
        let mailbox = MailboxId::derive(viewing_key, &self.syndicate_id);
        self.members.push(MemberAddress::new(pubkey, mailbox, share_count));
    }

    /// find member by pubkey
    pub fn find_by_pubkey(&self, pubkey: &Hash32) -> Option<&MemberAddress> {
        self.members.iter().find(|m| &m.pubkey == pubkey)
    }

    /// find member by mailbox
    pub fn find_by_mailbox(&self, mailbox: &MailboxId) -> Option<&MemberAddress> {
        self.members.iter().find(|m| &m.mailbox == mailbox)
    }

    /// get all mailboxes for direct messaging
    pub fn all_mailboxes(&self) -> Vec<MailboxId> {
        self.members.iter().map(|m| m.mailbox).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_derivation() {
        let viewing_key = [1u8; 32];
        let syndicate_id = [2u8; 32];

        let mailbox = MailboxId::derive(&viewing_key, &syndicate_id);

        // deterministic
        let mailbox2 = MailboxId::derive(&viewing_key, &syndicate_id);
        assert_eq!(mailbox, mailbox2);

        // different viewing key = different mailbox
        let other_key = [3u8; 32];
        let other_mailbox = MailboxId::derive(&other_key, &syndicate_id);
        assert_ne!(mailbox, other_mailbox);

        // different syndicate = different mailbox
        let other_syndicate = [4u8; 32];
        let other_mailbox = MailboxId::derive(&viewing_key, &other_syndicate);
        assert_ne!(mailbox, other_mailbox);
    }

    #[test]
    fn test_mailbox_rotation() {
        let viewing_key = [1u8; 32];
        let syndicate_id = [2u8; 32];

        let epoch0 = MailboxId::derive_rotated(&viewing_key, &syndicate_id, 0);
        let epoch1 = MailboxId::derive_rotated(&viewing_key, &syndicate_id, 1);

        assert_ne!(epoch0, epoch1);

        // same epoch = same mailbox
        let epoch0_again = MailboxId::derive_rotated(&viewing_key, &syndicate_id, 0);
        assert_eq!(epoch0, epoch0_again);
    }

    #[test]
    fn test_broadcast_topic() {
        let syndicate_id = [1u8; 32];
        let topic = BroadcastTopic::derive(&syndicate_id);

        // deterministic
        let topic2 = BroadcastTopic::derive(&syndicate_id);
        assert_eq!(topic, topic2);

        // different syndicate = different topic
        let other_syndicate = [2u8; 32];
        let other_topic = BroadcastTopic::derive(&other_syndicate);
        assert_ne!(topic, other_topic);
    }

    #[test]
    fn test_syndicate_router() {
        let syndicate_id = [1u8; 32];
        let mut router = SyndicateRouter::new(syndicate_id);

        let alice_key = [2u8; 32];
        let alice_viewing = [3u8; 32];
        router.add_member(alice_key, &alice_viewing, 30);

        let bob_key = [4u8; 32];
        let bob_viewing = [5u8; 32];
        router.add_member(bob_key, &bob_viewing, 70);

        // find by pubkey
        let alice = router.find_by_pubkey(&alice_key).unwrap();
        assert_eq!(alice.share_count, 30);

        // find by mailbox
        let alice_mailbox = MailboxId::derive(&alice_viewing, &syndicate_id);
        let alice_by_mailbox = router.find_by_mailbox(&alice_mailbox).unwrap();
        assert_eq!(alice_by_mailbox.pubkey, alice_key);

        // all mailboxes
        let mailboxes = router.all_mailboxes();
        assert_eq!(mailboxes.len(), 2);
    }
}
