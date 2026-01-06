//! state channel signing
//!
//! signs state transitions for mental poker
//!
//! ## privacy analysis
//!
//! **what leaks:**
//! - identity: email → deterministic address (in test mode)
//!   - production OPRF hides email from operators
//!   - but address is linkable across sessions
//!
//! - actions: all poker actions visible to other players
//!   - this is inherent to the game (you see what they do)
//!   - timing reveals thinking patterns
//!
//! - disputes: if channel goes on-chain, full history is public
//!   - all bets, folds, raises visible to everyone
//!   - chip balances become public
//!
//! - network: IP addresses in P2P connections
//!   - could use tor but latency hurts poker
//!
//! **what's protected:**
//! - cards: encrypted until reveal (mental poker protocol)
//! - PIN: never sent anywhere, derives keys locally
//! - cross-table: no automatic linkage between tables
//!   (unless same address used, which is default)
//!
//! **future improvements:**
//! - WIM signing: sign inside ZK, key never in JS memory
//! - ephemeral addresses: derive fresh address per table
//! - tor integration: hide IP from other players
//! - zk disputes: prove winner without revealing hands

use crate::auth::{AuthState, AuthMode};
use parity_scale_codec::Encode;

/// sign a message using current auth session
pub fn sign_message(auth: &AuthState, message: &[u8]) -> Result<[u8; 64], SigningError> {
    match auth.mode {
        AuthMode::Test => {
            let key = auth.test_signing_key.ok_or(SigningError::NotLoggedIn)?;
            sign_with_blake3_key(&key, message)
        }
        AuthMode::Ghettobox => {
            // TODO: implement WIM signing when ready
            Err(SigningError::NotImplemented)
        }
    }
}

/// sign using blake3-derived ed25519-like key (test mode)
/// NOT SECURE - for testing only
fn sign_with_blake3_key(key: &[u8; 32], message: &[u8]) -> Result<[u8; 64], SigningError> {
    // derive signing key and public key
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mental-poker.test-sign.v1");
    hasher.update(key);
    hasher.update(message);
    let sig_hash = hasher.finalize();

    // create 64-byte "signature" (not real crypto, just for testing)
    let mut signature = [0u8; 64];
    signature[..32].copy_from_slice(sig_hash.as_bytes());

    // second half is hash of message for verification
    let msg_hash = blake3::hash(message);
    signature[32..].copy_from_slice(msg_hash.as_bytes());

    Ok(signature)
}

/// verify a test mode signature
pub fn verify_signature(
    pubkey: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> bool {
    // in test mode, just verify message hash matches
    let msg_hash = blake3::hash(message);
    signature[32..] == msg_hash.as_bytes()[..]
}

/// create a signed state transition
pub fn sign_state_transition<S: Encode>(
    auth: &AuthState,
    state: &S,
    nonce: u64,
) -> Result<SignedTransition, SigningError> {
    // encode state with nonce
    let mut encoded = state.encode();
    encoded.extend_from_slice(&nonce.to_le_bytes());

    // hash the encoded state
    let state_hash = blake3::hash(&encoded);

    // sign the hash
    let signature = sign_message(auth, state_hash.as_bytes())?;

    Ok(SignedTransition {
        state_hash: *state_hash.as_bytes(),
        nonce,
        signature,
        signer: auth.account_address.clone().unwrap_or_default(),
    })
}

/// signed transition ready to send to other players
#[derive(Clone, Debug)]
pub struct SignedTransition {
    pub state_hash: [u8; 32],
    pub nonce: u64,
    pub signature: [u8; 64],
    pub signer: String,
}

/// signing errors
#[derive(Clone, Debug)]
pub enum SigningError {
    NotLoggedIn,
    NotImplemented,
    InvalidKey,
}

impl std::fmt::Display for SigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoggedIn => write!(f, "not logged in"),
            Self::NotImplemented => write!(f, "ghettobox signing not yet implemented"),
            Self::InvalidKey => write!(f, "invalid signing key"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_verify() {
        let key = [0x42u8; 32];
        let message = b"test message";

        let sig = sign_with_blake3_key(&key, message).unwrap();

        // derive pubkey same way as in auth
        let pubkey = *blake3::hash(&key).as_bytes();

        assert!(verify_signature(&pubkey, message, &sig));
        assert!(!verify_signature(&pubkey, b"wrong message", &sig));
    }
}
