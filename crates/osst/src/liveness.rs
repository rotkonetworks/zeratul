//! Liveness proofs for custodian participation
//!
//! Ensures custodians are actively running infrastructure by requiring
//! cryptographic proofs of block verification alongside reshare contributions.
//!
//! # Architecture
//!
//! ```text
//! Custodian Node                           On-Chain
//! ┌──────────────────┐                    ┌──────────────────┐
//! │ Verify block N   │                    │ ReshareState     │
//! │ Generate proof   │───contribution────▶│ + LivenessProofs │
//! │ Compute NOMT root│                    │ verify_all()     │
//! └──────────────────┘                    └──────────────────┘
//! ```
//!
//! # Integration with Ligerito
//!
//! Uses the existing `ligerito::verify_sha256()` or `verify_blake2b()`
//! to verify that a custodian correctly processed a recent block.

use alloc::vec::Vec;

use crate::curve::{OsstPoint, OsstScalar};
use crate::error::OsstError;
use crate::reshare::DealerCommitment;

// ============================================================================
// Checkpoint Types
// ============================================================================

/// A checkpoint anchor for liveness proofs
///
/// Represents a known-good block that custodians must prove they've verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointAnchor {
    /// Block height (relay chain or target chain)
    pub height: u64,
    /// Block hash (32 bytes)
    pub block_hash: [u8; 32],
    /// Timestamp (unix seconds)
    pub timestamp: u64,
}

impl CheckpointAnchor {
    pub fn new(height: u64, block_hash: [u8; 32], timestamp: u64) -> Self {
        Self { height, block_hash, timestamp }
    }

    /// Serialize for hashing/signing
    pub fn to_bytes(&self) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0..8].copy_from_slice(&self.height.to_le_bytes());
        buf[8..40].copy_from_slice(&self.block_hash);
        buf[40..48].copy_from_slice(&self.timestamp.to_le_bytes());
        buf
    }

    /// Check if checkpoint is recent enough
    pub fn is_recent(&self, current_height: u64, max_age_blocks: u64) -> bool {
        current_height.saturating_sub(self.height) <= max_age_blocks
    }
}

// ============================================================================
// Liveness Proof
// ============================================================================

/// Proof that custodian verified a checkpoint block
///
/// Contains a Ligerito proof of correct block verification.
#[derive(Clone, Debug)]
pub struct LivenessProof {
    /// The checkpoint being attested
    pub anchor: CheckpointAnchor,
    /// Ligerito proof bytes (from verify_sha256 or verify_blake2b)
    pub ligerito_proof: Vec<u8>,
    /// Custodian's local NOMT state root at this checkpoint
    pub state_root: [u8; 32],
}

impl LivenessProof {
    pub fn new(
        anchor: CheckpointAnchor,
        ligerito_proof: Vec<u8>,
        state_root: [u8; 32],
    ) -> Self {
        Self { anchor, ligerito_proof, state_root }
    }

    /// Estimated proof size for gas/weight estimation
    pub fn byte_size(&self) -> usize {
        48 + 4 + self.ligerito_proof.len() + 32
    }

    /// Serialize for on-chain storage
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.byte_size());
        buf.extend_from_slice(&self.anchor.to_bytes());
        buf.extend_from_slice(&(self.ligerito_proof.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.ligerito_proof);
        buf.extend_from_slice(&self.state_root);
        buf
    }

    /// Deserialize
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, OsstError> {
        if bytes.len() < 48 + 4 + 32 {
            return Err(OsstError::InvalidCommitment);
        }

        let height = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let block_hash: [u8; 32] = bytes[8..40].try_into().unwrap();
        let timestamp = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let anchor = CheckpointAnchor { height, block_hash, timestamp };

        let proof_len = u32::from_le_bytes(bytes[48..52].try_into().unwrap()) as usize;

        if bytes.len() < 52 + proof_len + 32 {
            return Err(OsstError::InvalidCommitment);
        }

        let ligerito_proof = bytes[52..52 + proof_len].to_vec();
        let state_root: [u8; 32] = bytes[52 + proof_len..52 + proof_len + 32]
            .try_into()
            .unwrap();

        Ok(Self { anchor, ligerito_proof, state_root })
    }
}

// ============================================================================
// Dealer Contribution with Liveness
// ============================================================================

/// Complete dealer contribution for reshare
///
/// Combines reshare commitment with liveness proof.
#[derive(Clone, Debug)]
pub struct DealerContribution<P: OsstPoint> {
    /// Reshare polynomial commitment
    pub commitment: DealerCommitment<P>,
    /// Proof of infrastructure participation
    pub liveness: LivenessProof,
    /// Schnorr signature binding commitment + liveness
    pub signature: ContributionSignature<P::Scalar>,
}

/// Schnorr signature over contribution
#[derive(Clone)]
pub struct ContributionSignature<S: OsstScalar> {
    /// R = g^k
    pub r: [u8; 32],
    /// s = k + e * x
    pub s: S,
}

impl<S: OsstScalar> ContributionSignature<S> {
    pub fn new(r: [u8; 32], s: S) -> Self {
        Self { r, s }
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..32].copy_from_slice(&self.r);
        buf[32..64].copy_from_slice(&self.s.to_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8; 64]) -> Result<Self, OsstError> {
        let r: [u8; 32] = bytes[0..32].try_into().unwrap();
        let s = S::from_canonical_bytes(&bytes[32..64].try_into().unwrap())
            .ok_or(OsstError::InvalidResponse)?;
        Ok(Self { r, s })
    }
}

impl<S: OsstScalar> core::fmt::Debug for ContributionSignature<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ContributionSignature")
            .field("r", &hex_short(&self.r))
            .field("s", &"[SCALAR]")
            .finish()
    }
}

fn hex_short(bytes: &[u8]) -> alloc::string::String {
    use alloc::format;
    if bytes.len() <= 8 {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    } else {
        format!(
            "{}...{}",
            bytes[0..4].iter().map(|b| format!("{:02x}", b)).collect::<alloc::string::String>(),
            bytes[bytes.len()-4..].iter().map(|b| format!("{:02x}", b)).collect::<alloc::string::String>()
        )
    }
}

impl<P: OsstPoint> DealerContribution<P> {
    /// Create new contribution (signature must be computed separately)
    pub fn new(
        commitment: DealerCommitment<P>,
        liveness: LivenessProof,
        signature: ContributionSignature<P::Scalar>,
    ) -> Self {
        Self { commitment, liveness, signature }
    }

    /// Get dealer index
    pub fn dealer_index(&self) -> u32 {
        self.commitment.dealer_index
    }

    /// Compute the message to be signed
    ///
    /// H(commitment || liveness || context)
    pub fn signing_message(
        commitment: &DealerCommitment<P>,
        liveness: &LivenessProof,
        context: &[u8],
    ) -> [u8; 64] {
        use sha2::{Sha512, Digest};

        let mut hasher = Sha512::new();
        hasher.update(b"OSST-CONTRIBUTION-V1");
        hasher.update(&commitment.to_bytes());
        hasher.update(&liveness.to_bytes());
        hasher.update(context);

        hasher.finalize().into()
    }

    /// Sign a contribution
    pub fn sign<R: rand_core::RngCore + rand_core::CryptoRng>(
        commitment: DealerCommitment<P>,
        liveness: LivenessProof,
        secret_key: &P::Scalar,
        context: &[u8],
        rng: &mut R,
    ) -> Self {
        let message = Self::signing_message(&commitment, &liveness, context);

        // Schnorr signature
        let k = P::Scalar::random(rng);
        let r_point = P::generator().mul_scalar(&k);
        let r = r_point.compress();

        // e = H(R || message)
        let e = Self::challenge_hash(&r, &message);

        // s = k + e * x
        let s = k.add(&e.mul(secret_key));

        let signature = ContributionSignature::new(r, s);

        Self { commitment, liveness, signature }
    }

    /// Verify contribution signature
    pub fn verify_signature(&self, public_key: &P, context: &[u8]) -> bool {
        let message = Self::signing_message(&self.commitment, &self.liveness, context);

        // Decompress R
        let r_point = match P::decompress(&self.signature.r) {
            Some(p) => p,
            None => return false,
        };

        // e = H(R || message)
        let e = Self::challenge_hash(&self.signature.r, &message);

        // Verify: g^s == R + Y^e
        let lhs = P::generator().mul_scalar(&self.signature.s);
        let rhs = r_point.add(&public_key.mul_scalar(&e));

        lhs == rhs
    }

    fn challenge_hash(r: &[u8; 32], message: &[u8; 64]) -> P::Scalar {
        use sha2::{Sha512, Digest};

        let mut hasher = Sha512::new();
        hasher.update(r);
        hasher.update(message);

        let hash: [u8; 64] = hasher.finalize().into();
        P::Scalar::from_bytes_wide(&hash)
    }
}

// ============================================================================
// Liveness Verifier Trait
// ============================================================================

/// Trait for verifying Ligerito proofs
///
/// Implement this to connect to your on-chain Ligerito verifier.
pub trait LivenessVerifier {
    /// Verify a Ligerito proof for a checkpoint
    fn verify_ligerito_proof(
        &self,
        anchor: &CheckpointAnchor,
        proof: &[u8],
        state_root: &[u8; 32],
    ) -> bool;

    /// Get the current checkpoint anchor
    fn current_anchor(&self) -> CheckpointAnchor;

    /// Maximum age of valid checkpoints (in blocks)
    fn max_checkpoint_age(&self) -> u64;
}

/// Batch verifier for multiple contributions
pub struct ContributionVerifier<'a, P: OsstPoint, V: LivenessVerifier> {
    verifier: &'a V,
    context: &'a [u8],
    _marker: core::marker::PhantomData<P>,
}

impl<'a, P: OsstPoint, V: LivenessVerifier> ContributionVerifier<'a, P, V> {
    pub fn new(verifier: &'a V, context: &'a [u8]) -> Self {
        Self {
            verifier,
            context,
            _marker: core::marker::PhantomData,
        }
    }

    /// Verify a single contribution
    pub fn verify(
        &self,
        contribution: &DealerContribution<P>,
        public_key: &P,
    ) -> Result<(), ContributionError> {
        let current = self.verifier.current_anchor();

        // Check checkpoint is recent
        if !contribution.liveness.anchor.is_recent(
            current.height,
            self.verifier.max_checkpoint_age(),
        ) {
            return Err(ContributionError::CheckpointTooOld);
        }

        // Verify Schnorr signature
        if !contribution.verify_signature(public_key, self.context) {
            return Err(ContributionError::InvalidSignature);
        }

        // Verify Ligerito proof
        if !self.verifier.verify_ligerito_proof(
            &contribution.liveness.anchor,
            &contribution.liveness.ligerito_proof,
            &contribution.liveness.state_root,
        ) {
            return Err(ContributionError::InvalidLigerito);
        }

        Ok(())
    }

    /// Verify multiple contributions, returning valid ones
    pub fn verify_batch(
        &self,
        contributions: &[DealerContribution<P>],
        public_keys: &[P],
    ) -> Vec<usize> {
        contributions
            .iter()
            .zip(public_keys.iter())
            .enumerate()
            .filter_map(|(i, (contrib, pk))| {
                self.verify(contrib, pk).ok().map(|_| i)
            })
            .collect()
    }
}

/// Contribution verification errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContributionError {
    /// Checkpoint is too old
    CheckpointTooOld,
    /// Schnorr signature invalid
    InvalidSignature,
    /// Ligerito proof invalid
    InvalidLigerito,
    /// Dealer index mismatch
    IndexMismatch,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use crate::reshare::Dealer;
    use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
    use rand::rngs::OsRng;

    /// Mock liveness verifier for testing
    struct MockVerifier {
        current_height: u64,
        max_age: u64,
    }

    impl LivenessVerifier for MockVerifier {
        fn verify_ligerito_proof(
            &self,
            _anchor: &CheckpointAnchor,
            proof: &[u8],
            _state_root: &[u8; 32],
        ) -> bool {
            // Accept any non-empty proof in tests
            !proof.is_empty()
        }

        fn current_anchor(&self) -> CheckpointAnchor {
            CheckpointAnchor::new(self.current_height, [0u8; 32], 0)
        }

        fn max_checkpoint_age(&self) -> u64 {
            self.max_age
        }
    }

    #[test]
    fn test_contribution_sign_verify() {
        let mut rng = OsRng;

        // Generate dealer key
        let secret = Scalar::random(&mut rng);
        let public: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        // Create dealer and commitment
        let dealer: Dealer<RistrettoPoint> = Dealer::new(1, Scalar::random(&mut rng), 3, &mut rng);
        let commitment = dealer.commitment().clone();

        // Create liveness proof
        let anchor = CheckpointAnchor::new(100, [1u8; 32], 1234567890);
        let liveness = LivenessProof::new(anchor, vec![1, 2, 3, 4], [2u8; 32]);

        // Sign contribution
        let context = b"test-epoch-42";
        let contribution = DealerContribution::sign(
            commitment,
            liveness,
            &secret,
            context,
            &mut rng,
        );

        // Verify signature
        assert!(contribution.verify_signature(&public, context));

        // Wrong context should fail
        assert!(!contribution.verify_signature(&public, b"wrong-context"));

        // Wrong public key should fail
        let wrong_public: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&Scalar::random(&mut rng));
        assert!(!contribution.verify_signature(&wrong_public, context));
    }

    #[test]
    fn test_contribution_verifier() {
        let mut rng = OsRng;

        let verifier = MockVerifier {
            current_height: 100,
            max_age: 10,
        };

        let secret = Scalar::random(&mut rng);
        let public: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let dealer: Dealer<RistrettoPoint> = Dealer::new(1, Scalar::random(&mut rng), 3, &mut rng);
        let commitment = dealer.commitment().clone();

        // Recent checkpoint - should pass
        let anchor = CheckpointAnchor::new(95, [1u8; 32], 0);
        let liveness = LivenessProof::new(anchor, vec![1, 2, 3], [0u8; 32]);
        let context = b"epoch-1";

        let contribution = DealerContribution::sign(
            commitment.clone(),
            liveness,
            &secret,
            context,
            &mut rng,
        );

        let cv = ContributionVerifier::<RistrettoPoint, _>::new(&verifier, context);
        assert!(cv.verify(&contribution, &public).is_ok());

        // Old checkpoint - should fail
        let old_anchor = CheckpointAnchor::new(50, [1u8; 32], 0);
        let old_liveness = LivenessProof::new(old_anchor, vec![1, 2, 3], [0u8; 32]);

        let old_contribution = DealerContribution::sign(
            commitment,
            old_liveness,
            &secret,
            context,
            &mut rng,
        );

        assert_eq!(
            cv.verify(&old_contribution, &public),
            Err(ContributionError::CheckpointTooOld)
        );
    }

    #[test]
    fn test_checkpoint_serialization() {
        let anchor = CheckpointAnchor::new(12345, [0xab; 32], 1700000000);
        let bytes = anchor.to_bytes();

        assert_eq!(bytes.len(), 48);
        assert_eq!(u64::from_le_bytes(bytes[0..8].try_into().unwrap()), 12345);
    }

    #[test]
    fn test_liveness_proof_serialization() {
        let anchor = CheckpointAnchor::new(100, [1u8; 32], 123);
        let proof = LivenessProof::new(anchor.clone(), vec![1, 2, 3, 4, 5], [2u8; 32]);

        let bytes = proof.to_bytes();
        let recovered = LivenessProof::from_bytes(&bytes).unwrap();

        assert_eq!(recovered.anchor, anchor);
        assert_eq!(recovered.ligerito_proof, vec![1, 2, 3, 4, 5]);
        assert_eq!(recovered.state_root, [2u8; 32]);
    }
}
