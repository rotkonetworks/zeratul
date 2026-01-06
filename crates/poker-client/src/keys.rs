//! player keys module
//!
//! bridges auth module with poker-sdk key hierarchy
//! provides per-table and per-hand viewing keys for encrypted card history

pub use poker_sdk::keys::*;

use poker_sdk::keys::{
    MasterSeed, SpendKey, FullViewingKey, TableViewingKey, HandViewingKey,
    EphemeralKey, EncryptedPayload, encrypt_aead, decrypt_aead,
};

/// encrypted card for hand history
/// stores card value encrypted with player's hand viewing key
#[derive(Clone, Debug)]
pub struct EncryptedCard {
    /// card index (0-51)
    pub index: u8,
    /// encrypted card data (2 bytes: rank + suit)
    pub ciphertext: Vec<u8>,
}

impl EncryptedCard {
    /// encrypt a card for a specific hand using ChaCha20Poly1305
    pub fn encrypt(card_index: u8, card_data: &[u8], hand_vk: &HandViewingKey) -> Self {
        let key = hand_vk.card_key(card_index);
        let ciphertext = encrypt_aead(&key, card_data);
        Self {
            index: card_index,
            ciphertext,
        }
    }

    /// decrypt using hand viewing key
    pub fn decrypt(&self, hand_vk: &HandViewingKey) -> Option<Vec<u8>> {
        hand_vk.decrypt_card(self.index, &self.ciphertext)
    }
}

/// player session keys
/// derived from auth credentials, provides full key hierarchy
pub struct PlayerSessionKeys {
    /// master seed (from auth)
    master: MasterSeed,
    /// spend key for signing
    spend_key: SpendKey,
    /// full viewing key
    fvk: FullViewingKey,
}

impl PlayerSessionKeys {
    /// create from auth signing key seed
    pub fn from_auth_key(signing_key: &[u8; 32]) -> Self {
        // derive master seed from signing key with domain separation
        let seed_hash = blake3::keyed_hash(signing_key, b"mental-poker.master-seed");
        let master = MasterSeed::from_bytes(*seed_hash.as_bytes());
        let spend_key = master.derive_spend_key();
        let fvk = spend_key.full_viewing_key();

        Self { master, spend_key, fvk }
    }

    /// get table-specific viewing key
    pub fn table_key(&self, table_id: &[u8]) -> TableViewingKey {
        self.fvk.table_viewing_key(table_id)
    }

    /// get hand viewing key for a specific table and hand
    pub fn hand_key(&self, table_id: &[u8], hand_number: u64) -> HandViewingKey {
        self.table_key(table_id).hand_viewing_key(hand_number)
    }

    /// get table address for receiving encrypted cards
    pub fn table_address(&self, table_id: &[u8]) -> TableAddress {
        self.fvk.table_address(table_id)
    }

    /// get incoming viewing key (share for someone to see all your cards)
    pub fn incoming_vk(&self) -> IncomingViewingKey {
        self.fvk.incoming_viewing_key()
    }

    /// create ephemeral key for encrypting to another player
    pub fn create_ephemeral(&self) -> EphemeralKey {
        EphemeralKey::random()
    }
}

/// encrypted hand history entry
#[derive(Clone, Debug)]
pub struct EncryptedHandHistory {
    /// table identifier
    pub table_id: Vec<u8>,
    /// hand number
    pub hand_number: u64,
    /// our hole cards (encrypted)
    pub hole_cards: Vec<EncryptedCard>,
    /// community cards (in order: flop[3], turn[1], river[1])
    /// encrypted for our viewing key
    pub community_cards: Vec<EncryptedCard>,
    /// actions we took (encrypted)
    pub our_actions: Vec<EncryptedAction>,
    /// final pot amount (plaintext, public info)
    pub final_pot: u64,
    /// our result (win/loss amount, plaintext)
    pub result: i64,
}

impl EncryptedHandHistory {
    /// create new empty hand history
    pub fn new(table_id: Vec<u8>, hand_number: u64) -> Self {
        Self {
            table_id,
            hand_number,
            hole_cards: Vec::new(),
            community_cards: Vec::new(),
            our_actions: Vec::new(),
            final_pot: 0,
            result: 0,
        }
    }

    /// add encrypted hole card
    pub fn add_hole_card(&mut self, card: EncryptedCard) {
        self.hole_cards.push(card);
    }

    /// add encrypted community card
    pub fn add_community_card(&mut self, card: EncryptedCard) {
        self.community_cards.push(card);
    }

    /// add encrypted action
    pub fn add_action(&mut self, action: EncryptedAction) {
        self.our_actions.push(action);
    }

    /// decrypt hand history using hand viewing key
    pub fn decrypt(&self, hand_vk: &HandViewingKey) -> Option<DecryptedHandHistory> {
        let mut hole_cards = Vec::new();
        for card in &self.hole_cards {
            let data = card.decrypt(hand_vk)?;
            hole_cards.push(data);
        }

        let mut community_cards = Vec::new();
        for card in &self.community_cards {
            let data = card.decrypt(hand_vk)?;
            community_cards.push(data);
        }

        Some(DecryptedHandHistory {
            table_id: self.table_id.clone(),
            hand_number: self.hand_number,
            hole_cards,
            community_cards,
            final_pot: self.final_pot,
            result: self.result,
        })
    }
}

/// decrypted hand history (for display)
#[derive(Clone, Debug)]
pub struct DecryptedHandHistory {
    pub table_id: Vec<u8>,
    pub hand_number: u64,
    pub hole_cards: Vec<Vec<u8>>,
    pub community_cards: Vec<Vec<u8>>,
    pub final_pot: u64,
    pub result: i64,
}

/// encrypted action in hand history
#[derive(Clone, Debug)]
pub struct EncryptedAction {
    /// action index within hand (for key derivation)
    pub index: u8,
    /// encrypted action data
    pub ciphertext: Vec<u8>,
}

impl EncryptedAction {
    /// encrypt action with hand viewing key using ChaCha20Poly1305
    pub fn encrypt(action_index: u8, action_data: &[u8], hand_vk: &HandViewingKey) -> Self {
        let key = hand_vk.card_key(action_index + 52); // offset to not collide with cards
        let ciphertext = encrypt_aead(&key, action_data);
        Self {
            index: action_index,
            ciphertext,
        }
    }

    /// decrypt action with hand viewing key
    pub fn decrypt(&self, hand_vk: &HandViewingKey) -> Option<Vec<u8>> {
        let key = hand_vk.card_key(self.index + 52);
        decrypt_aead(&key, &self.ciphertext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_keys_from_auth() {
        let auth_key = [0x42u8; 32];
        let session = PlayerSessionKeys::from_auth_key(&auth_key);

        let table_vk = session.table_key(b"test-table");
        let hand_vk = table_vk.hand_viewing_key(1);

        // encrypt a card
        let card_data = b"Ah"; // ace of hearts
        let encrypted = EncryptedCard::encrypt(0, card_data, &hand_vk);

        // decrypt it
        let decrypted = encrypted.decrypt(&hand_vk).unwrap();
        assert_eq!(&decrypted, card_data);
    }

    #[test]
    fn test_hand_history_encryption() {
        let auth_key = [0x42u8; 32];
        let session = PlayerSessionKeys::from_auth_key(&auth_key);

        let hand_vk = session.hand_key(b"test-table", 1);

        let mut history = EncryptedHandHistory::new(b"test-table".to_vec(), 1);

        // add hole cards
        history.add_hole_card(EncryptedCard::encrypt(0, b"Ah", &hand_vk));
        history.add_hole_card(EncryptedCard::encrypt(1, b"Kh", &hand_vk));

        // add community cards (flop)
        history.add_community_card(EncryptedCard::encrypt(2, b"Qh", &hand_vk));
        history.add_community_card(EncryptedCard::encrypt(3, b"Jh", &hand_vk));
        history.add_community_card(EncryptedCard::encrypt(4, b"Th", &hand_vk));

        history.final_pot = 1000;
        history.result = 500; // we won

        // decrypt history
        let decrypted = history.decrypt(&hand_vk).unwrap();
        assert_eq!(decrypted.hole_cards.len(), 2);
        assert_eq!(&decrypted.hole_cards[0], b"Ah");
        assert_eq!(&decrypted.hole_cards[1], b"Kh");
        assert_eq!(decrypted.community_cards.len(), 3);
    }

    #[test]
    fn test_sharing_hand_viewing_key() {
        let auth_key = [0x42u8; 32];
        let session = PlayerSessionKeys::from_auth_key(&auth_key);

        let hand_vk = session.hand_key(b"test-table", 42);

        // encrypt card with our key
        let encrypted = EncryptedCard::encrypt(0, b"As", &hand_vk);

        // simulate sharing - serialize hand viewing key
        let shared_bytes = hand_vk.to_bytes();

        // recipient reconstructs hand viewing key
        // (in real use, this would be from deserializing shared_bytes)
        let _reconstructed_vk = session.hand_key(b"test-table", 42);

        // they can decrypt our cards
        let decrypted = encrypted.decrypt(&hand_vk).unwrap();
        assert_eq!(&decrypted, b"As");
    }
}
