// message.rs — ephemeral ed25519-signed message envelopes
//
// every FROST round message is wrapped in a SignedMessage.
// the ed25519 key is ephemeral (per-session), not long-lived.
// FROST identifiers derive from ephemeral pubkeys.

use ed25519_consensus::{SigningKey, VerificationKey, Signature};
use rand_core::{CryptoRng, RngCore};
use serde::{Serialize, Deserialize};

use crate::Identifier;

/// generate a fresh ephemeral ed25519 identity for a session
pub fn ephemeral_identity(rng: &mut (impl RngCore + CryptoRng)) -> SigningKey {
    SigningKey::new(rng)
}

/// derive FROST identifier from ephemeral ed25519 verification key
pub fn identifier_from_vk(vk: &VerificationKey) -> Result<Identifier, String> {
    Identifier::derive(vk.as_bytes().as_slice())
        .map_err(|e| format!("derive frost identifier: {}", e))
}

/// ed25519-signed message envelope. ephemeral pubkey, not long-lived.
#[derive(Clone, Serialize, Deserialize)]
pub struct SignedMessage {
    pub pk: Vec<u8>,
    pub sig: Vec<u8>,
    pub payload: Vec<u8>,
}

impl SignedMessage {
    pub fn sign(sk: &SigningKey, payload: &[u8]) -> Self {
        let sig = sk.sign(payload);
        Self {
            pk: sk.verification_key().to_bytes().to_vec(),
            sig: sig.to_bytes().to_vec(),
            payload: payload.to_vec(),
        }
    }

    pub fn verify(&self) -> Result<(VerificationKey, &[u8]), String> {
        let pk_bytes: [u8; 32] = self.pk.as_slice().try_into()
            .map_err(|_| "invalid ed25519 pubkey length")?;
        let sig_bytes: [u8; 64] = self.sig.as_slice().try_into()
            .map_err(|_| "invalid ed25519 signature length")?;
        let vk = VerificationKey::try_from(pk_bytes)
            .map_err(|e| format!("invalid ed25519 pubkey: {}", e))?;
        let sig = Signature::from(sig_bytes);
        vk.verify(&sig, &self.payload)
            .map_err(|_| "ed25519 signature verification failed".to_string())?;
        Ok((vk, &self.payload))
    }
}
