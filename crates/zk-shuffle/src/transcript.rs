//! blake2-based transcript with merlin-like api
//!
//! running hash accumulates appended data, challenges derived from cloned state

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use blake2::{Blake2s256, Digest};

/// blake2-based transcript (merlin-like api)
///
/// internally maintains a running hash that accumulates all appended data.
/// challenges are derived by cloning the state and finalizing.
#[derive(Clone)]
pub struct Blake2Transcript {
    state: Blake2s256,
    /// counter for challenge derivation (prevents reuse)
    challenge_counter: u64,
}

impl Blake2Transcript {
    /// create new transcript with domain separator
    pub fn new(domain_sep: &[u8]) -> Self {
        let mut state = Blake2s256::new();
        // domain separation
        state.update(b"blake2-transcript-v1");
        state.update(&(domain_sep.len() as u32).to_le_bytes());
        state.update(domain_sep);
        Self {
            state,
            challenge_counter: 0,
        }
    }

    /// append labeled message to transcript
    pub fn append_message(&mut self, label: &[u8], message: &[u8]) {
        // label length + label + message length + message
        self.state.update(&(label.len() as u32).to_le_bytes());
        self.state.update(label);
        self.state.update(&(message.len() as u32).to_le_bytes());
        self.state.update(message);
    }

    /// append u64 to transcript
    pub fn append_u64(&mut self, label: &[u8], value: u64) {
        self.append_message(label, &value.to_le_bytes());
    }

    /// derive challenge bytes from current transcript state
    ///
    /// uses counter to ensure unique challenges even with same label
    pub fn challenge_bytes(&mut self, label: &[u8], dest: &mut [u8]) {
        // clone state, add label and counter, finalize
        let mut challenge_state = self.state.clone();
        challenge_state.update(b"challenge");
        challenge_state.update(&(label.len() as u32).to_le_bytes());
        challenge_state.update(label);
        challenge_state.update(&self.challenge_counter.to_le_bytes());
        self.challenge_counter += 1;

        let hash = challenge_state.finalize();

        // if dest > 32 bytes, chain hashes
        if dest.len() <= 32 {
            dest.copy_from_slice(&hash[..dest.len()]);
        } else {
            let mut offset = 0;
            let mut seed = hash.to_vec();
            while offset < dest.len() {
                let take = (dest.len() - offset).min(32);
                dest[offset..offset + take].copy_from_slice(&seed[..take]);
                offset += take;
                if offset < dest.len() {
                    let mut h = Blake2s256::new();
                    h.update(&seed);
                    h.update(b"extend");
                    seed = h.finalize().to_vec();
                }
            }
        }

        // fold challenge back into state for forward secrecy
        self.state.update(b"challenge_out");
        self.state.update(dest);
    }
}

/// unified transcript that binds all protocol components
#[derive(Clone)]
pub struct ShuffleTranscript {
    inner: Blake2Transcript,
}

impl ShuffleTranscript {
    /// create new transcript for a game round
    pub fn new(game_id: &[u8], round: u32) -> Self {
        let mut t = Blake2Transcript::new(b"zk-shuffle.game.v1");
        t.append_message(b"game_id", game_id);
        t.append_u64(b"round", round as u64);
        Self { inner: t }
    }

    /// bind the aggregate public key from all players
    pub fn bind_aggregate_key(&mut self, key_bytes: &[u8]) {
        self.inner.append_message(b"aggregate_pk", key_bytes);
    }

    /// bind a player's public key and key ownership proof
    pub fn bind_player_key(&mut self, player_id: u8, pk_bytes: &[u8], proof_bytes: &[u8]) {
        self.inner.append_u64(b"player_id", player_id as u64);
        self.inner.append_message(b"player_pk", pk_bytes);
        self.inner.append_message(b"key_proof", proof_bytes);
    }

    /// bind the initial masked deck
    pub fn bind_initial_deck(&mut self, deck_commitment: &[u8]) {
        self.inner.append_message(b"initial_deck", deck_commitment);
    }

    /// bind a shuffle operation from a player
    pub fn bind_shuffle(&mut self, player_id: u8, shuffled_deck_commitment: &[u8]) {
        self.inner.append_u64(b"shuffler", player_id as u64);
        self.inner.append_message(b"shuffled_deck", shuffled_deck_commitment);
    }

    /// bind ligerito proof commitment (merkle root)
    pub fn bind_ligerito_commitment(&mut self, merkle_root: &[u8; 32]) {
        self.inner.append_message(b"ligerito_root", merkle_root);
    }

    /// bind card reveal tokens
    pub fn bind_reveal_token(&mut self, card_index: usize, token_bytes: &[u8]) {
        self.inner.append_u64(b"card_idx", card_index as u64);
        self.inner.append_message(b"reveal_token", token_bytes);
    }

    /// get a challenge for the next protocol step
    pub fn challenge(&mut self, label: &'static [u8]) -> [u8; 32] {
        let mut challenge = [0u8; 32];
        self.inner.challenge_bytes(label, &mut challenge);
        challenge
    }

    /// get a challenge as u64 (for smaller domains)
    pub fn challenge_u64(&mut self, label: &'static [u8]) -> u64 {
        let bytes = self.challenge(label);
        u64::from_le_bytes(bytes[..8].try_into().unwrap())
    }

    /// fork transcript for sub-protocol (e.g., ligerito internal)
    pub fn fork(&self, label: &'static [u8]) -> Self {
        let mut forked = self.inner.clone();
        forked.append_message(b"fork", label);
        Self { inner: forked }
    }

    /// get raw bytes for external consumption (e.g., ligerito transcript seed)
    pub fn get_seed(&mut self, label: &'static [u8], dest: &mut [u8]) {
        self.inner.challenge_bytes(label, dest);
    }

    /// get inner transcript for direct manipulation
    pub fn inner(&mut self) -> &mut Blake2Transcript {
        &mut self.inner
    }
}

/// adapter to use shuffle transcript with ligerito's transcript trait
pub struct LigeritoTranscriptAdapter {
    transcript: ShuffleTranscript,
}

impl LigeritoTranscriptAdapter {
    pub fn new(transcript: ShuffleTranscript) -> Self {
        Self { transcript }
    }

    pub fn into_inner(self) -> ShuffleTranscript {
        self.transcript
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake2_transcript_determinism() {
        let mut t1 = Blake2Transcript::new(b"test-domain");
        let mut t2 = Blake2Transcript::new(b"test-domain");

        t1.append_message(b"data", b"hello");
        t2.append_message(b"data", b"hello");

        let mut c1 = [0u8; 32];
        let mut c2 = [0u8; 32];
        t1.challenge_bytes(b"chal", &mut c1);
        t2.challenge_bytes(b"chal", &mut c2);

        assert_eq!(c1, c2);
    }

    #[test]
    fn test_blake2_transcript_binding() {
        let mut t1 = Blake2Transcript::new(b"test");
        let mut t2 = Blake2Transcript::new(b"test");

        t1.append_message(b"x", b"a");
        t2.append_message(b"x", b"b");

        let mut c1 = [0u8; 32];
        let mut c2 = [0u8; 32];
        t1.challenge_bytes(b"c", &mut c1);
        t2.challenge_bytes(b"c", &mut c2);

        assert_ne!(c1, c2);
    }

    #[test]
    fn test_challenge_counter() {
        let mut t = Blake2Transcript::new(b"test");
        t.append_message(b"data", b"x");

        let mut c1 = [0u8; 32];
        let mut c2 = [0u8; 32];
        t.challenge_bytes(b"same_label", &mut c1);
        t.challenge_bytes(b"same_label", &mut c2);

        // same label but different challenges due to counter
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_transcript_determinism() {
        let mut t1 = ShuffleTranscript::new(b"game123", 1);
        let mut t2 = ShuffleTranscript::new(b"game123", 1);

        t1.bind_aggregate_key(b"pk_bytes");
        t2.bind_aggregate_key(b"pk_bytes");

        let c1 = t1.challenge(b"test_label");
        let c2 = t2.challenge(b"test_label");

        assert_eq!(c1, c2, "same inputs should produce same challenges");
    }

    #[test]
    fn test_transcript_binding() {
        let mut t1 = ShuffleTranscript::new(b"game123", 1);
        let mut t2 = ShuffleTranscript::new(b"game123", 1);

        t1.bind_aggregate_key(b"pk_a");
        t2.bind_aggregate_key(b"pk_b"); // different key

        let c1 = t1.challenge(b"test_label");
        let c2 = t2.challenge(b"test_label");

        assert_ne!(c1, c2, "different inputs should produce different challenges");
    }

    #[test]
    fn test_fork() {
        let t = ShuffleTranscript::new(b"game", 1);
        let mut forked = t.fork(b"ligerito_fork");

        // forked transcript should work independently
        let c = forked.challenge(b"inner_label");
        assert_ne!(c, [0u8; 32]);
    }

    #[test]
    fn test_large_challenge() {
        let mut t = Blake2Transcript::new(b"test");
        t.append_message(b"x", b"data");

        let mut large = [0u8; 64];
        t.challenge_bytes(b"big", &mut large);

        // should have non-zero bytes throughout
        assert_ne!(&large[..32], &[0u8; 32]);
        assert_ne!(&large[32..], &[0u8; 32]);
    }
}
