//! frostito escrow: 2-of-3 nested FROST for heads-up poker
//!
//! player A + player B + jury. the jury's share is born distributed
//! via interleaved DKG among jury nodes. s₃ never exists as a scalar.
//!
//! # flow
//!
//! 1. `EscrowTable::create()` - interleaved DKG, produces escrow address
//! 2. players deposit to escrow address
//! 3. play poker (off-chain via poker-p2p engine)
//! 4. happy path: `settle()` - both players sign, funds released
//! 5. dispute: `dispute()` - OSST authorization → nested FROST sign → funds released
//!
//! # on-chain
//!
//! the escrow address is a standard pallas public key. the chain sees
//! standard schnorr signatures. no evidence of multisig, jury, or nesting.

use osst::curve::{OsstPoint, OsstScalar};
use osst::nested;
use osst::frost;
use osst::{SecretShare, Contribution, verify as osst_verify};
use pasta_curves::pallas::{Point, Scalar};
use sha2::{Sha512, Digest};

/// a heads-up poker escrow table
pub struct EscrowTable {
    /// player A's outer FROST share (index 1)
    pub player_a: SecretShare<Scalar>,
    /// player B's outer FROST share (index 2)
    pub player_b: SecretShare<Scalar>,
    /// jury node shares (index 3, split among nodes)
    pub jury_shares: Vec<SecretShare<Scalar>>,
    /// jury threshold
    pub jury_threshold: u32,
    /// jury's verification share g^{s₃}
    pub jury_pubkey: Point,
    /// outer group public key (the escrow address)
    pub group_pubkey: Point,
    /// escrow address bytes
    pub address: [u8; 32],
}

/// result of a signing operation
pub struct SignResult {
    /// the schnorr signature (R, z)
    pub signature: frost::Signature<Point>,
    /// whether OSST authorization passed
    pub osst_verified: bool,
}

impl EscrowTable {
    /// create a new escrow table via interleaved DKG.
    /// s₃ (jury's share) is born distributed — never materialized.
    pub fn create(jury_n: u32, jury_threshold: u32) -> Result<Self, osst::OsstError> {
        use osst::redpallas::zcash as redpallas;
        let mut rng = rand::thread_rng();

        let (player_a, player_b, jury_network, group_pubkey) =
            redpallas::setup_escrow(jury_n, jury_threshold, &mut rng)?;

        let address = redpallas::derive_address_bytes(&group_pubkey);

        Ok(Self {
            player_a,
            player_b,
            jury_shares: jury_network.node_shares,
            jury_threshold,
            jury_pubkey: jury_network.outer_verification_share,
            group_pubkey,
            address,
        })
    }

    /// happy path: player A + player B sign cooperatively.
    /// no jury involvement. standard 2-of-3 FROST.
    pub fn settle(&self, message: &[u8]) -> Option<frost::Signature<Point>> {
        let mut rng = rand::thread_rng();

        let (nonces_a, commits_a) = frost::commit::<Point, _>(self.player_a.index, &mut rng);
        let (nonces_b, commits_b) = frost::commit::<Point, _>(self.player_b.index, &mut rng);

        let package = frost::SigningPackage::new(
            message.to_vec(),
            vec![commits_a, commits_b],
        ).ok()?;

        let sig_a = frost::sign::<Point>(&package, nonces_a, &self.player_a, &self.group_pubkey).ok()?;
        let sig_b = frost::sign::<Point>(&package, nonces_b, &self.player_b, &self.group_pubkey).ok()?;

        let signature = frost::aggregate::<Point>(
            &package, &[sig_a, sig_b], &self.group_pubkey, None,
        ).ok()?;

        if frost::verify_signature(&self.group_pubkey, message, &signature) {
            Some(signature)
        } else {
            None
        }
    }

    /// dispute: OSST authorization → nested FROST sign.
    /// the disputing player (A or B) + jury produce the signature.
    /// s₃ is never reconstructed.
    pub fn dispute(
        &self,
        message: &[u8],
        disputing_player: DisputingPlayer,
    ) -> Option<SignResult> {
        let mut rng = rand::thread_rng();
        let jury_index = 3u32;

        let player_share = match disputing_player {
            DisputingPlayer::A => &self.player_a,
            DisputingPlayer::B => &self.player_b,
        };

        // phase 1: OSST authorization
        let active_jury: Vec<u32> = self.jury_shares[..self.jury_threshold as usize]
            .iter()
            .map(|s| s.index)
            .collect();

        let contributions: Vec<Contribution<Point>> = active_jury
            .iter()
            .map(|&k| self.jury_shares[(k - 1) as usize].contribute::<Point, _>(&mut rng, message))
            .collect();

        let osst_verified = osst_verify(
            &self.jury_pubkey, &contributions, self.jury_threshold, message,
        ).unwrap_or(false);

        if !osst_verified {
            return None;
        }

        // phase 2: inner commitment round (with inner binding)
        let mut inner_nonces = Vec::new();
        let mut inner_commitments = Vec::new();
        for &k in &active_jury {
            let (nonces, commits) = nested::inner_commit::<Point, _>(k, &mut rng);
            inner_nonces.push(nonces);
            inner_commitments.push(commits);
        }

        // aggregate with inner binding factors
        let r_nested = nested::aggregate_inner_commitments(&inner_commitments, message);

        // phase 3: outer FROST round 1
        let (player_nonces, player_commits) =
            frost::commit::<Point, _>(player_share.index, &mut rng);

        let jury_outer_commits = frost::SigningCommitments {
            index: jury_index,
            hiding: r_nested,
            binding: Point::identity(),
        };

        let outer_package = frost::SigningPackage::new(
            message.to_vec(),
            vec![player_commits, jury_outer_commits],
        ).ok()?;

        // player signs
        let player_sig = frost::sign::<Point>(
            &outer_package, player_nonces, player_share, &self.group_pubkey,
        ).ok()?;

        // compute outer params
        let outer_indices = outer_package.signer_indices();
        let outer_lambda = osst::compute_lagrange_coefficients::<Scalar>(&outer_indices).ok()?;
        let nested_pos = outer_indices.iter().position(|&i| i == jury_index)?;

        let outer_gc = {
            let mut r = Point::identity();
            for &idx in &outer_indices {
                let c = outer_package.get_commitments(idx)?;
                let rho = outer_binding_factor(idx, message, &outer_package);
                r = r.add(&c.hiding).add(&c.binding.mul_scalar(&rho));
            }
            r
        };

        let outer_challenge = {
            let mut h = Sha512::new();
            h.update(b"frost-challenge-v1");
            h.update(OsstPoint::compress(&outer_gc));
            h.update(OsstPoint::compress(&self.group_pubkey));
            h.update(message);
            Scalar::from_bytes_wide(&h.finalize().into())
        };

        let params = nested::InnerSigningParams {
            outer_challenge,
            outer_lambda: outer_lambda[nested_pos],
        };

        // phase 4: inner holders sign — s₃ never reconstructed
        let mut inner_sigs = Vec::new();
        for (nonces, &k) in inner_nonces.into_iter().zip(active_jury.iter()) {
            let sig = nested::inner_sign::<Point>(
                nonces,
                &self.jury_shares[(k - 1) as usize],
                &params,
                &inner_commitments,
                &active_jury,
                message,
            ).ok()?;
            inner_sigs.push(sig);
        }

        let z_nested = nested::aggregate_inner_shares(&inner_sigs);
        let jury_sig = frost::SignatureShare {
            index: jury_index,
            response: z_nested,
        };

        // phase 5: outer aggregation
        let signature = frost::aggregate::<Point>(
            &outer_package,
            &[player_sig, jury_sig],
            &self.group_pubkey,
            None,
        ).ok()?;

        if frost::verify_signature(&self.group_pubkey, message, &signature) {
            Some(SignResult { signature, osst_verified })
        } else {
            None
        }
    }

    /// verify a signature against this table's escrow address
    pub fn verify(&self, message: &[u8], signature: &frost::Signature<Point>) -> bool {
        frost::verify_signature(&self.group_pubkey, message, signature)
    }
}

/// which player is disputing
#[derive(Debug, Clone, Copy)]
pub enum DisputingPlayer {
    A,
    B,
}

fn outer_binding_factor(
    index: u32,
    message: &[u8],
    package: &frost::SigningPackage<Point>,
) -> Scalar {
    let mut encoded = Vec::new();
    for idx in package.signer_indices() {
        let c = package.get_commitments(idx).unwrap();
        encoded.extend_from_slice(&c.index.to_le_bytes());
        encoded.extend_from_slice(&c.hiding.compress());
        encoded.extend_from_slice(&c.binding.compress());
    }
    let mut h = Sha512::new();
    h.update(b"frost-binding-v1");
    h.update(index.to_le_bytes());
    h.update((message.len() as u64).to_le_bytes());
    h.update(message);
    h.update(&encoded);
    Scalar::from_bytes_wide(&h.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escrow_happy_path() {
        let table = EscrowTable::create(5, 3).unwrap();
        let message = b"settle: A=1200 B=800";

        let sig = table.settle(message).expect("happy path should produce signature");
        assert!(table.verify(message, &sig));
    }

    #[test]
    fn test_escrow_dispute_player_a() {
        let table = EscrowTable::create(5, 3).unwrap();
        let message = b"dispute: A=1500 B=500 (jury ruled for A)";

        let result = table.dispute(message, DisputingPlayer::A)
            .expect("dispute should produce signature");

        assert!(result.osst_verified);
        assert!(table.verify(message, &result.signature));
    }

    #[test]
    fn test_escrow_dispute_player_b() {
        let table = EscrowTable::create(5, 3).unwrap();
        let message = b"dispute: A=500 B=1500 (jury ruled for B)";

        let result = table.dispute(message, DisputingPlayer::B)
            .expect("dispute should produce signature");

        assert!(result.osst_verified);
        assert!(table.verify(message, &result.signature));
    }

    #[test]
    fn test_escrow_wrong_message_fails() {
        let table = EscrowTable::create(5, 3).unwrap();
        let message = b"settle: A=1000 B=1000";
        let wrong = b"settle: A=2000 B=0";

        let sig = table.settle(message).unwrap();
        assert!(!table.verify(wrong, &sig));
    }

    #[test]
    fn test_escrow_address_stable() {
        let table = EscrowTable::create(5, 3).unwrap();
        assert_ne!(table.address, [0u8; 32]);

        // settle and dispute produce different signatures for different messages
        // but both verify against the same escrow address
        let sig1 = table.settle(b"msg1").unwrap();
        let sig2 = table.settle(b"msg2").unwrap();
        assert!(table.verify(b"msg1", &sig1));
        assert!(table.verify(b"msg2", &sig2));
        assert!(!table.verify(b"msg1", &sig2));
    }
}
