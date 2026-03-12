//! state channel signing
//!
//! signs state transitions for mental poker using ed25519 via AuthState
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

use crate::auth::AuthState;
use parity_scale_codec::Encode;

/// sign a message using current auth session (ed25519 for both test and production)
pub fn sign_message(auth: &AuthState, message: &[u8]) -> Result<[u8; 64], SigningError> {
    auth.sign(message).ok_or(SigningError::NotLoggedIn)
}

/// verify an ed25519 signature
pub fn verify_signature(
    pubkey: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> bool {
    use ed25519_dalek::{VerifyingKey, Verifier, Signature};
    let Ok(vk) = VerifyingKey::from_bytes(pubkey) else { return false };
    let sig = Signature::from_bytes(signature);
    vk.verify(message, &sig).is_ok()
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
    InvalidKey,
}

impl std::fmt::Display for SigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoggedIn => write!(f, "not logged in"),
            Self::InvalidKey => write!(f, "invalid signing key"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ed25519_sign_verify() {
        use ed25519_dalek::{SigningKey, Signer as _};
        let key = SigningKey::from_bytes(&[0x42u8; 32]);
        let message = b"test message";

        let sig = key.sign(message);
        let pubkey = key.verifying_key().to_bytes();

        assert!(verify_signature(&pubkey, message, &sig.to_bytes()));
        assert!(!verify_signature(&pubkey, b"wrong message", &sig.to_bytes()));
    }
}
