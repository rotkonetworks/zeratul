//! Network Protocols
//!
//! Implements JAM-style network protocols:
//! - Ticket gossip (for Safrole consensus)
//! - Block announcement and sync
//! - State synchronization

use crate::block::Block;
use commonware_cryptography::sha256::Digest;
use serde::{Deserialize, Serialize};

/// Vote on a block for finality
#[derive(Debug, Clone)]
pub struct Vote {
    /// Block hash being voted on
    pub block_hash: Digest,
    /// Block height
    pub height: u64,
    /// Timeslot
    pub timeslot: u64,
    /// Validator index who signed
    pub validator_index: u32,
    /// BLS signature on block hash (serialized)
    pub signature: Vec<u8>,
}

// Manual Serialize/Deserialize for Vote (Digest doesn't impl serde)
impl Serialize for Vote {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Vote", 5)?;
        state.serialize_field("block_hash", &self.block_hash.as_ref())?;
        state.serialize_field("height", &self.height)?;
        state.serialize_field("timeslot", &self.timeslot)?;
        state.serialize_field("validator_index", &self.validator_index)?;
        state.serialize_field("signature", &self.signature)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Vote {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct VoteHelper {
            block_hash: Vec<u8>,
            height: u64,
            timeslot: u64,
            validator_index: u32,
            signature: Vec<u8>,
        }

        let helper = VoteHelper::deserialize(deserializer)?;

        // Convert Vec<u8> to Digest
        use commonware_codec::Read as _;
        let mut reader: &[u8] = &helper.block_hash;
        let block_hash = Digest::read_cfg(&mut reader, &())
            .map_err(|_| serde::de::Error::custom("Invalid block hash bytes"))?;

        Ok(Vote {
            block_hash,
            height: helper.height,
            timeslot: helper.timeslot,
            validator_index: helper.validator_index,
            signature: helper.signature,
        })
    }
}

/// Finality certificate (aggregated votes)
#[derive(Debug, Clone)]
pub struct FinalityCertificate {
    /// Block hash that was finalized
    pub block_hash: Digest,
    /// Block height
    pub height: u64,
    /// Aggregated BLS signature
    pub aggregate_signature: Vec<u8>,
    /// Bitmap of validators who signed
    pub signers: Vec<u32>,
}

impl Serialize for FinalityCertificate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("FinalityCertificate", 4)?;
        state.serialize_field("block_hash", &self.block_hash.as_ref())?;
        state.serialize_field("height", &self.height)?;
        state.serialize_field("aggregate_signature", &self.aggregate_signature)?;
        state.serialize_field("signers", &self.signers)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for FinalityCertificate {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct CertHelper {
            block_hash: Vec<u8>,
            height: u64,
            aggregate_signature: Vec<u8>,
            signers: Vec<u32>,
        }

        let helper = CertHelper::deserialize(deserializer)?;

        use commonware_codec::Read as _;
        let mut reader: &[u8] = &helper.block_hash;
        let block_hash = Digest::read_cfg(&mut reader, &())
            .map_err(|_| serde::de::Error::custom("Invalid block hash bytes"))?;

        Ok(FinalityCertificate {
            block_hash,
            height: helper.height,
            aggregate_signature: helper.aggregate_signature,
            signers: helper.signers,
        })
    }
}

/// Consensus message (sent over block notification protocol)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsensusMessage {
    /// Block announcement
    BlockAnnounce(BlockAnnounce),
    /// Vote on a block
    Vote(Vote),
    /// Finality certificate
    Finality(FinalityCertificate),
}

/// Network message types
///
/// Note: BlockResponse is omitted because Block doesn't implement Serialize.
/// Use raw Block encoding/decoding via commonware_codec instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkMessage {
    /// Block announcement (UP 0: Unstructured Protocol)
    BlockAnnounce(BlockAnnounce),

    /// Block request (CE 128: Client-Establishing)
    BlockRequest(BlockRequest),

    /// State request (CE 129)
    StateRequest(StateRequest),

    /// State response
    StateResponse(StateResponse),
}

/// Block announcement
#[derive(Debug, Clone)]
pub struct BlockAnnounce {
    /// Block hash
    pub hash: Digest,

    /// Block height
    pub height: u64,

    /// Timeslot
    pub timeslot: u64,

    /// Parent hash
    pub parent: Digest,
}

// Manual Serialize/Deserialize for BlockAnnounce (Digest doesn't impl serde)
impl Serialize for BlockAnnounce {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("BlockAnnounce", 4)?;
        state.serialize_field("hash", &self.hash.as_ref())?;
        state.serialize_field("height", &self.height)?;
        state.serialize_field("timeslot", &self.timeslot)?;
        state.serialize_field("parent", &self.parent.as_ref())?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for BlockAnnounce {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{self, MapAccess, Visitor};

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field { Hash, Height, Timeslot, Parent }

        struct BlockAnnounceVisitor;

        impl<'de> Visitor<'de> for BlockAnnounceVisitor {
            type Value = BlockAnnounce;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct BlockAnnounce")
            }

            fn visit_map<V: MapAccess<'de>>(self, mut map: V) -> Result<BlockAnnounce, V::Error> {
                let mut hash = None;
                let mut height = None;
                let mut timeslot = None;
                let mut parent = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Hash => {
                            let bytes: Vec<u8> = map.next_value()?;
                            // Digest is [u8; 32], use Read trait
                            use bytes::Buf;
                            use commonware_codec::Read as _;
                            let mut reader: &[u8] = &bytes;
                            hash = Some(Digest::read_cfg(&mut reader, &())
                                .map_err(|_| de::Error::custom("Invalid hash bytes"))?);
                        }
                        Field::Height => height = Some(map.next_value()?),
                        Field::Timeslot => timeslot = Some(map.next_value()?),
                        Field::Parent => {
                            let bytes: Vec<u8> = map.next_value()?;
                            use bytes::Buf;
                            use commonware_codec::Read as _;
                            let mut reader: &[u8] = &bytes;
                            parent = Some(Digest::read_cfg(&mut reader, &())
                                .map_err(|_| de::Error::custom("Invalid parent bytes"))?);
                        }
                    }
                }

                Ok(BlockAnnounce {
                    hash: hash.ok_or_else(|| de::Error::missing_field("hash"))?,
                    height: height.ok_or_else(|| de::Error::missing_field("height"))?,
                    timeslot: timeslot.ok_or_else(|| de::Error::missing_field("timeslot"))?,
                    parent: parent.ok_or_else(|| de::Error::missing_field("parent"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "BlockAnnounce",
            &["hash", "height", "timeslot", "parent"],
            BlockAnnounceVisitor,
        )
    }
}

impl BlockAnnounce {
    /// Create announcement from block
    pub fn from_block(block: &Block) -> Self {
        Self {
            hash: block.digest(),
            height: block.height(),
            timeslot: block.timeslot(),
            parent: block.parent(),
        }
    }
}

/// Block request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockRequest {
    /// Request ID (for matching responses)
    pub request_id: u64,

    /// Requested block hash (as bytes)
    #[serde(with = "digest_serde")]
    pub hash: Digest,

    /// Optional: request by height instead
    pub height: Option<u64>,
}

/// Block response
#[derive(Debug, Clone)]
pub struct BlockResponse {
    /// Request ID
    pub request_id: u64,

    /// Block data (None if not found)
    /// Note: Block doesn't implement Serialize, so this struct can't derive it
    /// Use custom serialization with commonware_codec instead
    pub block: Option<Box<Block>>,
}

/// State request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateRequest {
    /// Request ID
    pub request_id: u64,

    /// State key to fetch
    pub key: Vec<u8>,

    /// Block hash at which to fetch state
    #[serde(with = "digest_serde")]
    pub at_block: Digest,
}

/// State response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateResponse {
    /// Request ID
    pub request_id: u64,

    /// State value (None if not found)
    pub value: Option<Vec<u8>>,

    /// Merkle proof (for light clients)
    pub proof: Option<Vec<Vec<u8>>>,
}

/// Serde helper for Digest
mod digest_serde {
    use commonware_codec::Read as CodecRead;
    use commonware_cryptography::sha256::Digest;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(digest: &Digest, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(digest.as_ref())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Digest, D::Error> {
        use bytes::Buf;
        use commonware_codec::Read as _;
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        let mut reader: &[u8] = &bytes;
        Digest::read_cfg(&mut reader, &()).map_err(serde::de::Error::custom)
    }
}

// Ticket gossip removed - using TLE-based leader selection instead

/// Block synchronization handler
pub struct BlockSync {
    /// Pending block requests
    pending_requests: std::collections::HashMap<u64, BlockRequest>,

    /// Next request ID
    next_request_id: u64,
}

impl BlockSync {
    /// Create new block sync handler
    pub fn new() -> Self {
        Self {
            pending_requests: std::collections::HashMap::new(),
            next_request_id: 0,
        }
    }

    /// Request a block by hash
    pub fn request_block(&mut self, hash: Digest) -> BlockRequest {
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let request = BlockRequest {
            request_id,
            hash,
            height: None,
        };

        self.pending_requests.insert(request_id, request.clone());

        request
    }

    /// Request a block by height
    pub fn request_block_by_height(&mut self, height: u64) -> BlockRequest {
        use commonware_cryptography::Hasher;
        use commonware_cryptography::Sha256;

        let request_id = self.next_request_id;
        self.next_request_id += 1;

        // Create a placeholder hash (zeros)
        let mut hasher = Sha256::new();
        hasher.update(b"placeholder");
        let placeholder_hash = hasher.finalize();

        let request = BlockRequest {
            request_id,
            hash: placeholder_hash,
            height: Some(height),
        };

        self.pending_requests.insert(request_id, request.clone());

        request
    }

    /// Handle block response
    ///
    /// Returns the block if request was valid
    pub fn handle_block_response(&mut self, response: BlockResponse) -> Option<Block> {
        // Remove pending request
        self.pending_requests.remove(&response.request_id)?;

        // Return block if present
        response.block.map(|b| *b)
    }

    /// Get pending request count
    pub fn pending_count(&self) -> usize {
        self.pending_requests.len()
    }
}

impl Default for BlockSync {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ticket gossip tests removed - using TLE-based leader selection

    #[test]
    fn test_block_sync_request() {
        use commonware_cryptography::Hasher;
        use commonware_cryptography::Sha256;

        let mut sync = BlockSync::new();

        let mut hasher = Sha256::new();
        hasher.update(b"test");
        let hash = hasher.finalize();

        let request = sync.request_block(hash);

        assert_eq!(request.request_id, 0);
        assert_eq!(request.hash, hash);
        assert_eq!(sync.pending_count(), 1);

        // Second request gets new ID
        let request2 = sync.request_block(hash);
        assert_eq!(request2.request_id, 1);
        assert_eq!(sync.pending_count(), 2);
    }

    #[test]
    fn test_block_sync_response() {
        use commonware_cryptography::Hasher;
        use commonware_cryptography::Sha256;

        let mut sync = BlockSync::new();

        let mut hasher = Sha256::new();
        hasher.update(b"test");
        let hash = hasher.finalize();

        let request = sync.request_block(hash);

        // Create response
        let block = Block::genesis();
        let response = BlockResponse {
            request_id: request.request_id,
            block: Some(Box::new(block.clone())),
        };

        // Handle response
        let received_block = sync.handle_block_response(response);
        assert!(received_block.is_some());
        assert_eq!(received_block.unwrap().digest(), block.digest());

        // Request should be removed
        assert_eq!(sync.pending_count(), 0);
    }
}
