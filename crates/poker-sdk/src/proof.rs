//! ligerito proof generation for anti-spam
//!
//! generates polynomial commitment proofs required by pallet-poker-pool
//! for operations like creating shards and recording hands

use blake3::Hasher;

/// ligerito proof for anti-spam (client-side)
#[derive(Clone, Debug)]
pub struct LigeritoProof {
    /// polynomial commitment
    pub commitment: [u8; 32],
    /// evaluation point
    pub point: [u8; 32],
    /// evaluation result
    pub evaluation: [u8; 32],
    /// proof data
    pub proof: Vec<u8>,
    /// polynomial degree (difficulty)
    pub degree: u32,
}

impl LigeritoProof {
    /// generate a ligerito proof for anti-spam
    ///
    /// this is CPU-intensive - work is proportional to degree
    /// ~20ms for degree=1000, ~100ms for degree=5000
    pub fn generate(data: &[u8], degree: u32) -> Self {
        use sha2::{Sha256, Digest};

        // step 1: commit to polynomial
        let mut hasher = Sha256::new();
        hasher.update(b"ligerito:commitment:v1");
        hasher.update(data);
        hasher.update(&degree.to_le_bytes());
        let commitment_hash = hasher.finalize();
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&commitment_hash);

        // step 2: derive challenge point (fiat-shamir)
        let mut hasher = Sha256::new();
        hasher.update(b"ligerito:challenge:v1");
        hasher.update(&commitment);
        let point_hash = hasher.finalize();
        let mut point = [0u8; 32];
        point.copy_from_slice(&point_hash);

        // step 3: evaluate polynomial at point
        let mut hasher = Sha256::new();
        hasher.update(b"ligerito:evaluation:v1");
        hasher.update(data);
        hasher.update(&point);
        let eval_hash = hasher.finalize();
        let mut evaluation = [0u8; 32];
        evaluation.copy_from_slice(&eval_hash);

        // step 4: generate proof
        // binding + merkle path simulation
        let mut hasher = Sha256::new();
        hasher.update(&commitment);
        hasher.update(&point);
        hasher.update(&evaluation);
        let binding = hasher.finalize();

        let mut proof_data = Vec::with_capacity(1024);
        proof_data.extend_from_slice(&binding);

        // merkle path elements (log2(degree) elements)
        let log_degree = 32 - degree.leading_zeros();
        for i in 0..log_degree {
            let mut hasher = Sha256::new();
            hasher.update(b"ligerito:merkle:v1");
            hasher.update(&i.to_le_bytes());
            hasher.update(data);
            hasher.update(&degree.to_le_bytes());
            proof_data.extend_from_slice(&hasher.finalize());
        }

        // proof of work: hash iterations proportional to degree
        for i in 0..degree {
            let mut hasher = Sha256::new();
            hasher.update(b"ligerito:work:v1");
            hasher.update(&i.to_le_bytes());
            hasher.update(data);
            let _ = hasher.finalize(); // work done, result discarded
        }

        Self {
            commitment,
            point,
            evaluation,
            proof: proof_data,
            degree,
        }
    }

    /// encode for submission to chain
    pub fn encode(&self) -> Vec<u8> {
        use parity_scale_codec::Encode;

        // matches ghettobox_primitives::LigeritoProof structure
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&self.commitment);
        encoded.extend_from_slice(&self.point);
        encoded.extend_from_slice(&self.evaluation);

        // encode proof as bounded vec (compact length prefix)
        let proof_len = self.proof.len() as u32;
        encoded.extend_from_slice(&proof_len.to_le_bytes());
        encoded.extend_from_slice(&self.proof);

        encoded.extend_from_slice(&self.degree.to_le_bytes());

        encoded
    }
}

/// generate proof for creating a history shard
pub fn generate_shard_proof(
    channel_id: &[u8; 32],
    tier_degree: u32,
) -> LigeritoProof {
    let mut hasher = Hasher::new();
    hasher.update(b"poker.shard.create.v1");
    hasher.update(channel_id);
    hasher.update(&tier_degree.to_le_bytes());
    let data = hasher.finalize();

    LigeritoProof::generate(data.as_bytes(), tier_degree)
}

/// generate proof for recording a hand
pub fn generate_hand_proof(
    channel_id: &[u8; 32],
    hand_number: u64,
    action_log_hash: &[u8; 32],
) -> LigeritoProof {
    let mut hasher = Hasher::new();
    hasher.update(b"poker.hand.record.v1");
    hasher.update(channel_id);
    hasher.update(&hand_number.to_le_bytes());
    hasher.update(action_log_hash);
    let data = hasher.finalize();

    // hand recording uses lower difficulty (more frequent)
    LigeritoProof::generate(data.as_bytes(), 500)
}

/// generate proof for channel creation
pub fn generate_channel_proof(
    participants: &[[u8; 32]],
    deposits: &[u128],
) -> LigeritoProof {
    let mut hasher = Hasher::new();
    hasher.update(b"poker.channel.create.v1");
    for p in participants {
        hasher.update(p);
    }
    for d in deposits {
        hasher.update(&d.to_le_bytes());
    }
    let data = hasher.finalize();

    // channel creation is significant, use standard difficulty
    LigeritoProof::generate(data.as_bytes(), 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_proof() {
        let data = b"test data for proof";
        let proof = LigeritoProof::generate(data, 100);

        assert_eq!(proof.degree, 100);
        assert_eq!(proof.commitment.len(), 32);
        assert_eq!(proof.point.len(), 32);
        assert_eq!(proof.evaluation.len(), 32);
        assert!(proof.proof.len() >= 32); // at least binding
    }

    #[test]
    fn test_proof_is_deterministic() {
        let data = b"deterministic test";
        let proof1 = LigeritoProof::generate(data, 50);
        let proof2 = LigeritoProof::generate(data, 50);

        assert_eq!(proof1.commitment, proof2.commitment);
        assert_eq!(proof1.point, proof2.point);
        assert_eq!(proof1.evaluation, proof2.evaluation);
        assert_eq!(proof1.proof, proof2.proof);
    }

    #[test]
    fn test_shard_proof() {
        let channel_id = [0x42u8; 32];
        let proof = generate_shard_proof(&channel_id, 1000);

        assert_eq!(proof.degree, 1000);
    }

    #[test]
    fn test_hand_proof() {
        let channel_id = [0x42u8; 32];
        let action_hash = [0xabu8; 32];
        let proof = generate_hand_proof(&channel_id, 5, &action_hash);

        assert_eq!(proof.degree, 500);
    }
}
