//! BEEFY types for ZK verification
//!
//! These types are designed for efficient ZK circuit representation while
//! maintaining compatibility with Polkadot's BEEFY protocol.

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

use codec::{Decode, Encode};
use scale_info::TypeInfo;

/// BEEFY validator set ID (increments each epoch)
pub type ValidatorSetId = u64;

/// Block number type
pub type BlockNumber = u32;

/// BLS public key (48 bytes compressed BLS12-381 G1)
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct BlsPublicKey(pub [u8; 48]);

/// BLS signature (96 bytes compressed BLS12-381 G2)
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct BlsSignature(pub [u8; 96]);

/// Aggregated BLS signature (same format, but aggregated from multiple signatures)
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct AggregateBlsSignature(pub [u8; 96]);

/// A validator in the BEEFY authority set
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct Validator {
    /// BLS public key for BEEFY signing
    pub bls_public_key: BlsPublicKey,
    /// Stake weight (for threshold calculation)
    pub weight: u64,
}

/// The BEEFY authority set
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct AuthoritySet {
    /// Unique identifier for this set
    pub id: ValidatorSetId,
    /// List of validators with their weights
    pub validators: Vec<Validator>,
    /// Total stake (sum of all weights)
    pub total_stake: u128,
}

impl AuthoritySet {
    /// Calculate the merkle root of the authority set
    pub fn merkle_root(&self) -> [u8; 32] {
        // Simple implementation: hash the encoded validators
        use blake2::{Blake2b512, Digest};
        let encoded = self.validators.encode();
        let hash = Blake2b512::digest(&encoded);
        let mut root = [0u8; 32];
        root.copy_from_slice(&hash[..32]);
        root
    }

    /// Calculate threshold for >2/3 majority
    pub fn threshold(&self) -> u128 {
        // Need strictly more than 2/3
        (self.total_stake * 2 / 3) + 1
    }
}

/// BEEFY commitment - what validators sign
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct Commitment {
    /// The payload being committed to (typically MMR root)
    pub payload: Vec<u8>,
    /// Block number this commitment is for
    pub block_number: BlockNumber,
    /// Validator set that should sign this
    pub validator_set_id: ValidatorSetId,
}

impl Commitment {
    /// Get the signing message (what validators actually sign)
    pub fn signing_message(&self) -> Vec<u8> {
        self.encode()
    }
}

/// Witness for ZK verification of BEEFY finality
///
/// This is the input to the ZK circuit. It contains all the information
/// needed to verify that a block has been finalized by BEEFY.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct BeefyWitness {
    /// The commitment being attested to
    pub commitment: Commitment,

    /// Bit vector indicating which validators signed
    /// `signed_by[i] = true` means validator `i` signed
    pub signed_by: Vec<bool>,

    /// Aggregated BLS signature from all signers
    pub aggregate_signature: AggregateBlsSignature,

    /// The authority set (for stake calculation)
    /// In production, this would be a merkle proof instead
    pub authority_set: AuthoritySet,
}

impl BeefyWitness {
    /// Calculate the total stake that signed
    pub fn signed_stake(&self) -> u128 {
        self.signed_by
            .iter()
            .zip(self.authority_set.validators.iter())
            .filter_map(|(signed, validator)| {
                if *signed {
                    Some(validator.weight as u128)
                } else {
                    None
                }
            })
            .sum()
    }

    /// Check if we have enough signatures (>2/3 stake)
    pub fn has_supermajority(&self) -> bool {
        self.signed_stake() >= self.authority_set.threshold()
    }

    /// Get the public keys of validators who signed
    pub fn signer_public_keys(&self) -> Vec<&BlsPublicKey> {
        self.signed_by
            .iter()
            .zip(self.authority_set.validators.iter())
            .filter_map(|(signed, validator)| {
                if *signed {
                    Some(&validator.bls_public_key)
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Compact proof of BEEFY finality (output of ZK prover)
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct BeefyFinalityProof {
    /// The block number that was finalized
    pub block_number: BlockNumber,

    /// The payload (MMR root) that was committed to
    pub payload: Vec<u8>,

    /// Validator set ID that finalized this
    pub validator_set_id: ValidatorSetId,

    /// Merkle root of the authority set (for binding)
    pub authority_set_root: [u8; 32],

    /// The Ligerito proof (opaque bytes)
    pub zk_proof: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_authority_set() -> AuthoritySet {
        let validators = (0..4)
            .map(|i| Validator {
                bls_public_key: BlsPublicKey([i as u8; 48]),
                weight: 100,
            })
            .collect();

        AuthoritySet {
            id: 1,
            validators,
            total_stake: 400,
        }
    }

    #[test]
    fn test_authority_set_threshold() {
        let set = mock_authority_set();
        // 2/3 of 400 = 266.67, so threshold should be 267
        assert_eq!(set.threshold(), 267);
    }

    #[test]
    fn test_signed_stake_calculation() {
        let authority_set = mock_authority_set();

        let witness = BeefyWitness {
            commitment: Commitment {
                payload: vec![1, 2, 3],
                block_number: 100,
                validator_set_id: 1,
            },
            signed_by: vec![true, true, true, false], // 3 out of 4 signed
            aggregate_signature: AggregateBlsSignature([0u8; 96]),
            authority_set,
        };

        assert_eq!(witness.signed_stake(), 300);
        assert!(witness.has_supermajority()); // 300 >= 267
    }

    #[test]
    fn test_insufficient_signatures() {
        let authority_set = mock_authority_set();

        let witness = BeefyWitness {
            commitment: Commitment {
                payload: vec![1, 2, 3],
                block_number: 100,
                validator_set_id: 1,
            },
            signed_by: vec![true, true, false, false], // only 2 signed
            aggregate_signature: AggregateBlsSignature([0u8; 96]),
            authority_set,
        };

        assert_eq!(witness.signed_stake(), 200);
        assert!(!witness.has_supermajority()); // 200 < 267
    }
}
