//! Block structure for state transition blockchain
//!
//! Each block contains:
//! - Standard blockchain metadata (parent, height, timestamp)
//! - List of AccidentalComputer proofs (state transitions)
//! - State root commitment from NOMT
//! - Safrole consensus fields (timeslot, seals, epoch markers)

use bytes::{Buf, BufMut};
use commonware_codec::{varint::UInt, EncodeSize, Error, Read, ReadExt, Write};
use commonware_cryptography::{sha256::Digest, Committable, Digestible, Hasher, Sha256};
use serde::{Deserialize, Serialize};
use zeratul_circuit::AccidentalComputerProof;

use commonware_cryptography::bls12381::PublicKey;

/// BLS signature (threshold or individual)
pub type BlsSignature = Vec<u8>;

/// A block in the state transition blockchain
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The parent block's digest
    pub parent: Digest,

    /// The height of the block in the blockchain
    pub height: u64,

    /// Timeslot index (configurable slot duration)
    pub timeslot: u64,

    /// The timestamp of the block (in milliseconds since the Unix epoch)
    pub timestamp: u64,

    /// State root after applying all transactions in this block
    pub state_root: [u8; 32],

    /// List of state transition proofs (transactions)
    pub proofs: Vec<AccidentalComputerProof>,

    /// Author's BLS public key (from Golden DKG)
    pub author_pubkey: Vec<u8>,

    /// BLS signature proving authorship
    /// (Can be individual or partial threshold signature)
    pub author_signature: BlsSignature,

    /// Pre-computed digest of the block
    digest: Digest,
}

impl Block {
    /// Compute the digest of a block (excluding signature for verification)
    fn compute_digest(
        parent: &Digest,
        height: u64,
        timeslot: u64,
        timestamp: u64,
        state_root: &[u8; 32],
        proofs: &[AccidentalComputerProof],
        author_pubkey: &[u8],
    ) -> Digest {
        let mut hasher = Sha256::new();
        hasher.update(parent);
        hasher.update(&height.to_be_bytes());
        hasher.update(&timeslot.to_be_bytes());
        hasher.update(&timestamp.to_be_bytes());
        hasher.update(state_root);
        hasher.update(author_pubkey);

        // Hash all proofs
        for proof in proofs {
            hasher.update(&proof.zoda_commitment);
            hasher.update(&proof.sender_commitment_old);
            hasher.update(&proof.sender_commitment_new);
            hasher.update(&proof.receiver_commitment_old);
            hasher.update(&proof.receiver_commitment_new);
        }

        hasher.finalize()
    }

    /// Create a new block
    pub fn new(
        parent: Digest,
        height: u64,
        timeslot: u64,
        timestamp: u64,
        state_root: [u8; 32],
        proofs: Vec<AccidentalComputerProof>,
        author_pubkey: Vec<u8>,
        author_signature: BlsSignature,
    ) -> Self {
        let digest = Self::compute_digest(
            &parent,
            height,
            timeslot,
            timestamp,
            &state_root,
            &proofs,
            &author_pubkey,
        );
        Self {
            parent,
            height,
            timeslot,
            timestamp,
            state_root,
            proofs,
            author_pubkey,
            author_signature,
            digest,
        }
    }

    /// Create a genesis block
    pub fn genesis() -> Self {
        let genesis_parent = {
            let mut hasher = Sha256::new();
            hasher.update(b"zeratul-genesis");
            hasher.finalize()
        };

        Self::new(
            genesis_parent,
            0,
            0,        // Timeslot 0
            0,        // Timestamp 0
            [0u8; 32], // Initial state root
            vec![],    // No proofs in genesis
            vec![],    // Genesis author (empty)
            vec![],    // No signature in genesis
        )
    }

    /// Create a simple block (for backward compatibility)
    pub fn new_simple(
        parent: Digest,
        height: u64,
        timestamp: u64,
        state_root: [u8; 32],
        proofs: Vec<AccidentalComputerProof>,
    ) -> Self {
        // For simple blocks, timeslot = timestamp (1ms granularity)
        let timeslot = timestamp;

        Self::new(
            parent,
            height,
            timeslot,
            timestamp,
            state_root,
            proofs,
            vec![],    // Placeholder author
            vec![],    // No signature
        )
    }
}

impl Write for Block {
    fn write(&self, writer: &mut impl BufMut) {
        self.parent.write(writer);
        UInt(self.height).write(writer);
        UInt(self.timeslot).write(writer);
        UInt(self.timestamp).write(writer);
        writer.put_slice(&self.state_root);

        // Write author fields
        UInt(self.author_pubkey.len() as u64).write(writer);
        writer.put_slice(&self.author_pubkey);
        UInt(self.author_signature.len() as u64).write(writer);
        writer.put_slice(&self.author_signature);

        // Write number of proofs
        UInt(self.proofs.len() as u64).write(writer);

        // Write each proof as JSON (temporary - should use binary encoding)
        for proof in &self.proofs {
            let json = serde_json::to_vec(proof).unwrap();
            UInt(json.len() as u64).write(writer);
            writer.put_slice(&json);
        }
    }
}

impl Read for Block {
    type Cfg = ();

    fn read_cfg(reader: &mut impl Buf, _: &Self::Cfg) -> Result<Self, Error> {
        let parent = Digest::read(reader)?;
        let height = UInt::read(reader)?.into();
        let timeslot = UInt::read(reader)?.into();
        let timestamp = UInt::read(reader)?.into();

        let mut state_root = [0u8; 32];
        reader.copy_to_slice(&mut state_root);

        // Read author fields
        let author_pubkey_len: u64 = UInt::read(reader)?.into();
        let mut author_pubkey = vec![0u8; author_pubkey_len as usize];
        reader.copy_to_slice(&mut author_pubkey);

        let author_sig_len: u64 = UInt::read(reader)?.into();
        let mut author_signature = vec![0u8; author_sig_len as usize];
        reader.copy_to_slice(&mut author_signature);

        // Read number of proofs
        let num_proofs: u64 = UInt::read(reader)?.into();
        let mut proofs = Vec::with_capacity(num_proofs as usize);

        // Read each proof
        for _ in 0..num_proofs {
            let json_len: u64 = UInt::read(reader)?.into();
            let mut json = vec![0u8; json_len as usize];
            reader.copy_to_slice(&mut json);
            let proof = serde_json::from_slice(&json)
                .map_err(|_| Error::Invalid("AccidentalComputerProof", "Failed to deserialize proof"))?;
            proofs.push(proof);
        }

        // Pre-compute the digest
        let digest = Self::compute_digest(
            &parent,
            height,
            timeslot,
            timestamp,
            &state_root,
            &proofs,
            &author_pubkey,
        );
        Ok(Self {
            parent,
            height,
            timeslot,
            timestamp,
            state_root,
            proofs,
            author_pubkey,
            author_signature,
            digest,
        })
    }
}

impl EncodeSize for Block {
    fn encode_size(&self) -> usize {
        let mut size = self.parent.encode_size()
            + UInt(self.height).encode_size()
            + UInt(self.timeslot).encode_size()
            + UInt(self.timestamp).encode_size()
            + 32 // state_root
            + UInt(self.author_pubkey.len() as u64).encode_size()
            + self.author_pubkey.len()
            + UInt(self.author_signature.len() as u64).encode_size()
            + self.author_signature.len()
            + UInt(self.proofs.len() as u64).encode_size();

        // Add size of each proof (as JSON)
        for proof in &self.proofs {
            let json = serde_json::to_vec(proof).unwrap();
            size += UInt(json.len() as u64).encode_size() + json.len();
        }

        size
    }
}

impl Digestible for Block {
    type Digest = Digest;

    fn digest(&self) -> Digest {
        self.digest
    }
}

impl Committable for Block {
    type Commitment = Digest;

    fn commitment(&self) -> Digest {
        self.digest
    }
}

impl commonware_consensus::Block for Block {
    fn parent(&self) -> Digest {
        self.parent
    }

    fn height(&self) -> u64 {
        self.height
    }
}

impl Block {
    /// Get timeslot (for consensus time tracking)
    pub fn timeslot(&self) -> u64 {
        self.timeslot
    }

    /// Get author's BLS public key
    pub fn author_key(&self) -> &[u8] {
        &self.author_pubkey
    }

    /// Get author's signature
    pub fn author_signature(&self) -> &[u8] {
        &self.author_signature
    }

    /// Get block digest (hash)
    pub fn digest(&self) -> Digest {
        self.digest
    }

    /// Get parent hash
    pub fn parent(&self) -> Digest {
        self.parent
    }

    /// Get block height
    pub fn height(&self) -> u64 {
        self.height
    }
}
