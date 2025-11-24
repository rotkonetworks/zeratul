//! DKG Protocol over QUIC (CE 200-202)
//!
//! Custom stream protocols for Golden DKG over JAMNP-S.
//!
//! ## Protocols
//!
//! - **CE 200**: DKG Broadcast (validator → all validators)
//! - **CE 201**: DKG Request (validator → validator, for missing broadcasts)
//! - **CE 202**: DKG Complete (validator → all validators, announce completion)

use anyhow::{bail, Result};
use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::governance::EpochIndex;
use super::streams::{StreamKind, StreamHandler};
use super::crypto_compat::BlsPublicKey;

/// CE 200: DKG Broadcast
///
/// Protocol:
/// ```
/// Validator -> Validator
/// --> Epoch ++ Sender (encoded) ++ BroadcastMsg (encoded)
/// --> FIN
/// <-- FIN
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DKGBroadcast {
    pub epoch: EpochIndex,
    /// Sender public key (parity-scale-codec encoded)
    pub sender_bytes: Vec<u8>,
    /// BroadcastMsg (opaque bytes - TODO: Add Encode impl to golden-rs)
    pub bmsg_bytes: Vec<u8>,
}

impl DKGBroadcast {
    pub fn new(epoch: EpochIndex, sender: &BlsPublicKey, bmsg_bytes: Vec<u8>) -> Self {
        Self {
            epoch,
            sender_bytes: sender.encode_bytes(),
            bmsg_bytes,
        }
    }

    pub fn decode_sender(&self) -> Result<BlsPublicKey> {
        BlsPublicKey::from_encoded(&self.sender_bytes)
    }
}

/// CE 201: DKG Request
///
/// Protocol:
/// ```
/// Validator -> Validator
/// --> Epoch ++ len++[Missing Validator (encoded)]
/// --> FIN
/// <-- len++[Epoch ++ Sender (encoded) ++ BroadcastMsg (encoded)]
/// <-- FIN
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DKGRequest {
    pub epoch: EpochIndex,
    /// Missing validator public keys (each parity-scale-codec encoded)
    pub missing_validators_bytes: Vec<Vec<u8>>,
}

impl DKGRequest {
    pub fn new(epoch: EpochIndex, missing: Vec<BlsPublicKey>) -> Self {
        Self {
            epoch,
            missing_validators_bytes: missing.iter().map(|pk| pk.encode_bytes()).collect(),
        }
    }

    pub fn decode_missing(&self) -> Result<Vec<BlsPublicKey>> {
        self.missing_validators_bytes
            .iter()
            .map(|bytes| BlsPublicKey::from_encoded(bytes))
            .collect()
    }
}

/// CE 202: DKG Complete
///
/// Protocol:
/// ```
/// Validator -> Validator
/// --> Epoch ++ Group Pubkey (encoded)
/// --> FIN
/// <-- FIN
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DKGComplete {
    pub epoch: EpochIndex,
    /// Group public key (parity-scale-codec encoded)
    pub group_pubkey_bytes: Vec<u8>,
}

impl DKGComplete {
    pub fn new(epoch: EpochIndex, group_pubkey: &BlsPublicKey) -> Self {
        Self {
            epoch,
            group_pubkey_bytes: group_pubkey.encode_bytes(),
        }
    }

    pub fn decode_group_pubkey(&self) -> Result<BlsPublicKey> {
        BlsPublicKey::from_encoded(&self.group_pubkey_bytes)
    }
}

/// DKG protocol handler
pub struct DKGProtocol {
    // TODO: Add DKGManager reference
}

impl DKGProtocol {
    pub fn new() -> Self {
        Self {}
    }

    /// Handle CE 200: DKG Broadcast
    pub fn handle_broadcast(&self, msg: DKGBroadcast) -> Result<()> {
        // TODO TODO TODO: Forward to DKGManager
        //
        // 1. Decode sender public key
        // 2. Decode BroadcastMsg (need custom deserializer)
        // 3. Call dkg_manager.handle_message(DKGMessage::Broadcast { ... })
        // 4. Return success

        Ok(())
    }

    /// Handle CE 201: DKG Request
    pub fn handle_request(&self, msg: DKGRequest) -> Result<Vec<DKGBroadcast>> {
        // TODO TODO TODO: Return requested broadcasts
        //
        // 1. Decode missing validator list
        // 2. Lookup broadcasts in DKGManager
        // 3. Encode and return

        Ok(Vec::new())
    }

    /// Handle CE 202: DKG Complete
    pub fn handle_complete(&self, msg: DKGComplete) -> Result<()> {
        // TODO TODO TODO: Process completion announcement
        //
        // 1. Decode group pubkey
        // 2. Verify we're on the same epoch
        // 3. Log completion (optimization - not required for correctness)

        Ok(())
    }
}

impl StreamHandler for DKGProtocol {
    fn handle_stream(&self, kind: StreamKind, data: Vec<u8>) -> Result<Vec<u8>> {
        match kind {
            StreamKind::DKGBroadcast => {
                let msg: DKGBroadcast = bincode::deserialize(&data)?;
                self.handle_broadcast(msg)?;
                Ok(Vec::new()) // No response for broadcast
            }
            StreamKind::DKGRequest => {
                let msg: DKGRequest = bincode::deserialize(&data)?;
                let broadcasts = self.handle_request(msg)?;
                Ok(bincode::serialize(&broadcasts)?)
            }
            StreamKind::DKGComplete => {
                let msg: DKGComplete = bincode::deserialize(&data)?;
                self.handle_complete(msg)?;
                Ok(Vec::new()) // No response for completion
            }
            _ => bail!("Invalid stream kind for DKG protocol"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::bls12381::primitives::group::{Element, G1};

    #[test]
    fn test_dkg_broadcast_encoding() {
        let epoch = 42;
        let inner_key = commonware_cryptography::bls12381::PublicKey::from(G1::one());
        let pubkey = BlsPublicKey::from(inner_key);
        let bmsg_bytes = vec![1, 2, 3, 4];

        let broadcast = DKGBroadcast::new(epoch, &pubkey, bmsg_bytes.clone());

        // Should be serializable
        let encoded = bincode::serialize(&broadcast).unwrap();
        let decoded: DKGBroadcast = bincode::deserialize(&encoded).unwrap();

        assert_eq!(decoded.epoch, epoch);
        assert_eq!(decoded.bmsg_bytes, bmsg_bytes);

        // Should be able to decode sender
        let sender = decoded.decode_sender().unwrap();
        assert_eq!(sender, pubkey);
    }

    #[test]
    fn test_dkg_request() {
        let epoch = 42;
        let inner_key = commonware_cryptography::bls12381::PublicKey::from(G1::one());
        let pubkey = BlsPublicKey::from(inner_key);
        let missing = vec![pubkey];

        let request = DKGRequest::new(epoch, missing.clone());

        // Should be serializable
        let encoded = bincode::serialize(&request).unwrap();
        let decoded: DKGRequest = bincode::deserialize(&encoded).unwrap();

        assert_eq!(decoded.epoch, epoch);

        // Should be able to decode missing validators
        let decoded_missing = decoded.decode_missing().unwrap();
        assert_eq!(decoded_missing.len(), missing.len());
    }
}
