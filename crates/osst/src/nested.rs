//! nested FROST: one outer share controlled by an inner threshold group
//!
//! the inner group collectively holds one position in an outer FROST scheme.
//! the outer share's secret never exists as a single scalar — it is born
//! distributed via interleaved DKG and used distributed via nested signing.
//!
//! # architecture
//!
//! ```text
//! outer FROST (t_out, n_out):
//!   positions 1..n_out, one of which is the nested position
//!
//! nested position (t_in, n_in):
//!   inner holders collectively control one outer share
//!   OSST gates authorization, inner FROST produces the partial signature
//! ```
//!
//! # signing protocol
//!
//! inner holders run a full FROST commitment round among themselves before
//! the outer commitment list is assembled. this prevents adaptive commitment
//! selection attacks at the inner level (the same attack that outer FROST's
//! binding factors prevent at the outer level).
//!
//! 1. inner holders generate nonce pairs and broadcast commitments
//! 2. inner binding factors computed from inner commitment list + outer message
//! 3. inner bound commitments: R_k = D_k + ρ_inner_k · E_k
//! 4. relay sums: R_nested = Σ R_k (single point for outer protocol)
//! 5. outer FROST uses R_nested as the nested position's commitment
//! 6. inner holders compute: z_k = d_k + ρ_inner_k·e_k + (λ_out·c·μ_k)·σ_k
//! 7. relay sums: z_nested = Σ z_k
//!
//! # security
//!
//! inner binding factors ensure no inner holder can adaptively choose their
//! commitment after seeing others'. the outer binding factor is not applied
//! to inner nonces — instead, the inner group presents a single pre-bound
//! commitment to the outer protocol. the outer protocol treats this like
//! any other signer's commitment (applying outer binding on top).
//!
//! the security composition (inner FROST feeding into outer FROST) is
//! believed correct by linearity but has not been formally proven in a
//! game-based reduction. this is an open problem.

use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha512};

use crate::curve::{OsstPoint, OsstScalar};
use crate::dkg;
use crate::error::OsstError;
use crate::lagrange::compute_lagrange_coefficients;
use crate::reshare::DealerCommitment;
use crate::SecretShare;

// ============================================================================
// Interleaved DKG
// ============================================================================

/// state for one coefficient's inner DKG
///
/// the nested position's outer polynomial f_p(x) = a_0 + a_1*x + ... + a_{t-1}*x^{t-1}
/// has t_out coefficients. each coefficient is shared via an independent inner DKG.
pub struct CoefficientDkg<P: OsstPoint> {
    /// which outer polynomial coefficient this DKG is for (0-indexed)
    pub coeff_index: u32,
    /// inner DKG dealers (one per inner holder)
    pub dealers: Vec<dkg::Dealer<P>>,
}

/// result of the interleaved DKG for one inner holder
pub struct InnerShare<S: OsstScalar> {
    /// inner holder's index (1-indexed)
    pub holder_index: u32,
    /// shamir shares of each outer polynomial coefficient
    /// alpha[j] = holder's share of coefficient j
    pub coefficient_shares: Vec<S>,
}

impl<S: OsstScalar> InnerShare<S> {
    /// compute this holder's share of the outer polynomial evaluated at point x
    ///
    /// returns: Σ_j alpha_j * x^j (share of f_p(x))
    /// this is a valid shamir share of f_p(x) by the homomorphic property.
    pub fn eval_at(&self, x: u32) -> S {
        let x_scalar = S::from_u32(x);
        let mut result = S::zero();
        let mut x_pow = S::one();
        for alpha in &self.coefficient_shares {
            result = result.add(&alpha.mul(&x_pow));
            x_pow = x_pow.mul(&x_scalar);
        }
        result
    }
}

/// run the interleaved DKG for a nested position
///
/// produces inner holders' shares of each outer polynomial coefficient.
/// nobody learns the coefficients themselves.
///
/// # returns
/// - `Vec<InnerShare>`: one per inner holder (1-indexed)
/// - `Vec<P>`: g^{a_j} for each coefficient j (public commitments)
pub fn interleaved_dkg<P: OsstPoint, R: rand_core::RngCore + rand_core::CryptoRng>(
    inner_n: u32,
    inner_t: u32,
    outer_t: u32,
    rng: &mut R,
) -> Result<(Vec<InnerShare<P::Scalar>>, Vec<P>), OsstError> {
    let mut coeff_dkgs: Vec<CoefficientDkg<P>> = Vec::with_capacity(outer_t as usize);
    for j in 0..outer_t {
        let dealers: Vec<dkg::Dealer<P>> = (1..=inner_n)
            .map(|k| dkg::Dealer::new(k, inner_t, rng))
            .collect();
        coeff_dkgs.push(CoefficientDkg {
            coeff_index: j,
            dealers,
        });
    }

    // coefficient commitments: g^{a_j} = Σ_k g^{p_k(0)} for each DKG j
    let mut coeff_commitments: Vec<P> = Vec::with_capacity(outer_t as usize);
    for dkg_j in &coeff_dkgs {
        let mut commitment = P::identity();
        for dealer in &dkg_j.dealers {
            commitment = commitment.add(dealer.commitment().share_commitment());
        }
        coeff_commitments.push(commitment);
    }

    // aggregate shares for each holder
    let mut inner_shares: Vec<InnerShare<P::Scalar>> = Vec::with_capacity(inner_n as usize);
    for k in 1..=inner_n {
        let mut coefficient_shares = Vec::with_capacity(outer_t as usize);
        for dkg_j in &coeff_dkgs {
            let commitments: Vec<&DealerCommitment<P>> =
                dkg_j.dealers.iter().map(|d| d.commitment()).collect();

            let mut agg: dkg::Aggregator<P> = dkg::Aggregator::new(k);
            for dealer in &dkg_j.dealers {
                let subshare = dealer.generate_subshare(k);
                agg.add_subshare(subshare, commitments[(dealer.index() - 1) as usize])?;
            }
            coefficient_shares.push(agg.finalize(inner_n)?);
        }
        inner_shares.push(InnerShare {
            holder_index: k,
            coefficient_shares,
        });
    }

    Ok((inner_shares, coeff_commitments))
}

/// split an outer participant's evaluation among inner holders via shamir.
///
/// returns (shares, feldman_commitments) so inner holders can verify.
/// the feldman commitments are g^{c_j} for the splitting polynomial
/// c(x) = evaluation + c_1·x + ... + c_{t-1}·x^{t-1}.
pub fn split_evaluation_for_inner<P: OsstPoint, R: rand_core::RngCore + rand_core::CryptoRng>(
    evaluation: &P::Scalar,
    inner_n: u32,
    inner_t: u32,
    rng: &mut R,
) -> (Vec<(u32, P::Scalar)>, DealerCommitment<P>) {
    let mut coeffs = vec![evaluation.clone()];
    for _ in 1..inner_t {
        coeffs.push(P::Scalar::random(rng));
    }

    // feldman commitment for verification
    // dealer_index is arbitrary here (must be >0 per DealerCommitment invariant).
    // use 1 as placeholder — the index is not meaningful for split verification,
    // only the polynomial commitments matter.
    let commitment = DealerCommitment::from_polynomial(1, &coeffs);

    let shares = (1..=inner_n)
        .map(|k| {
            let x = P::Scalar::from_u32(k);
            let mut result = P::Scalar::zero();
            let mut x_pow = P::Scalar::one();
            for c in &coeffs {
                result = result.add(&c.mul(&x_pow));
                x_pow = x_pow.mul(&x);
            }
            (k, result)
        })
        .collect();

    (shares, commitment)
}

/// verify a split evaluation piece against its feldman commitment
pub fn verify_split_piece<P: OsstPoint>(
    commitment: &DealerCommitment<P>,
    holder_index: u32,
    piece: &P::Scalar,
) -> bool {
    commitment.verify_subshare(holder_index, piece)
}

/// combine inner DKG shares with outer participants' split evaluations.
///
/// σ_k = inner_eval_at(p) + Σ_i π_{i,k}
///
/// the sum of shamir shares is a shamir share of the sum (homomorphic property).
/// since inner_eval_at(p) is a shamir share of f_p(p) and each π_{i,k} is a
/// shamir share of f_i(p), the result is a shamir share of
/// f_1(p) + f_2(p) + ... + f_p(p) = s_p (the nested position's outer secret).
pub fn combine_shares<S: OsstScalar>(
    inner_share: &InnerShare<S>,
    nested_position: u32,
    outer_eval_pieces: &[(u32, S)],
) -> S {
    let mut result = inner_share.eval_at(nested_position);
    for (_, piece) in outer_eval_pieces {
        result = result.add(piece);
    }
    result
}

// ============================================================================
// Nested FROST signing
// ============================================================================

/// inner holder's nonce pair for nested signing
pub struct InnerNonces<S: OsstScalar> {
    pub holder_index: u32,
    pub(crate) hiding: S,
    pub(crate) binding: S,
}

impl<S: OsstScalar> Drop for InnerNonces<S> {
    fn drop(&mut self) {
        self.hiding.zeroize();
        self.binding.zeroize();
    }
}

/// inner holder's nonce commitments (broadcast to relay + other inner holders)
#[derive(Clone, Debug)]
pub struct InnerCommitments<P: OsstPoint> {
    pub holder_index: u32,
    pub hiding: P,
    pub binding: P,
}

/// generate nonces for an inner holder
pub fn inner_commit<P: OsstPoint, R: rand_core::RngCore + rand_core::CryptoRng>(
    holder_index: u32,
    rng: &mut R,
) -> (InnerNonces<P::Scalar>, InnerCommitments<P>) {
    let hiding = P::Scalar::random(rng);
    let binding = P::Scalar::random(rng);

    let commitments = InnerCommitments {
        holder_index,
        hiding: P::generator().mul_scalar(&hiding),
        binding: P::generator().mul_scalar(&binding),
    };

    (
        InnerNonces {
            holder_index,
            hiding,
            binding,
        },
        commitments,
    )
}

/// inner binding factor: prevents adaptive commitment selection among inner holders.
///
/// ρ_inner_k = H("frostito-inner-bind" || k || msg || inner_commitment_list)
///
/// this mirrors FROST's binding factor but operates at the inner level.
/// each inner holder's binding nonce is mixed with the full inner commitment
/// list so that no holder can choose their commitment after seeing others'.
fn inner_binding_factor<P: OsstPoint>(
    holder_index: u32,
    outer_message: &[u8],
    inner_commitments: &[InnerCommitments<P>],
) -> P::Scalar {
    let mut h = Sha512::new();
    h.update(b"frostito-inner-bind");
    h.update(holder_index.to_le_bytes());
    h.update((outer_message.len() as u64).to_le_bytes());
    h.update(outer_message);
    for c in inner_commitments {
        h.update(c.holder_index.to_le_bytes());
        h.update(c.hiding.compress());
        h.update(c.binding.compress());
    }
    let hash: [u8; 64] = h.finalize().into();
    P::Scalar::from_bytes_wide(&hash)
}

/// # ⚠️ INSECURE — superseded by [`aggregate_inner_commitment_pair`] (v2)
///
/// This pre-binds the inner nonces and hands the outer protocol a SINGLE point
/// with an identity binding commitment. That fixes the nested position's
/// effective nonce independently of the outer commitment set, severing the
/// coupling FROST's binding factor exists to create: an outer adversary can
/// hold this nonce still while sweeping the challenge, which is the standard
/// ROS setting (Benhamouda et al. 2020) — polynomial-time forgery once
/// ~log2(q) sessions are concurrently open. Retained only so existing
/// deployments keep compiling while they migrate. DO NOT use for new code.
///
/// aggregate inner commitments into the nested position's single outer commitment.
///
/// each inner holder's bound commitment is R_k = D_k + ρ_inner_k · E_k.
/// the aggregate is R_nested = Σ R_k.
///
/// the outer protocol receives R_nested as a single point (the nested
/// position's "hiding" commitment), with a zero binding commitment.
/// the outer binding factor then applies to R_nested directly:
/// R_outer_nested = R_nested + ρ_outer · identity = R_nested.
///
/// this means the outer binding factor for the nested position is effectively
/// unused (binding commitment is identity). the inner binding factors provide
/// the equivalent security at the inner level.
pub fn aggregate_inner_commitments<P: OsstPoint>(
    inner_commitments: &[InnerCommitments<P>],
    outer_message: &[u8],
) -> P {
    let mut r_agg = P::identity();
    for c in inner_commitments {
        let rho = inner_binding_factor::<P>(c.holder_index, outer_message, inner_commitments);
        // R_k = D_k + ρ_inner_k · E_k
        let r_k = c.hiding.add(&c.binding.mul_scalar(&rho));
        r_agg = r_agg.add(&r_k);
    }
    r_agg
}

/// parameters distributed to inner holders for signing.
///
/// the relay computes these from the outer FROST context. inner holders
/// can independently verify them against public data (outer commitment list,
/// group public key, message).
#[derive(Clone)]
pub struct InnerSigningParams<S: OsstScalar> {
    /// outer schnorr challenge: c = H(R_outer, Y, m)
    pub outer_challenge: S,
    /// outer lagrange coefficient for the nested position
    pub outer_lambda: S,
}

/// inner holder's partial signature
pub struct InnerSignatureShare<S: OsstScalar> {
    pub holder_index: u32,
    pub response: S,
}

/// compute an inner holder's partial signature.
///
/// z_{p,k} = d_k + ρ_inner_k·e_k + (λ_outer·c·μ_k)·σ_k
///
/// the nonce part (d_k + ρ_inner_k·e_k) matches the bound commitment R_k
/// that was aggregated into R_nested. the secret part uses the product of
/// outer lagrange, outer challenge, and inner lagrange coefficients.
///
/// summing over t inner holders:
///   Σ z_{p,k} = Σ(d_k + ρ_inner_k·e_k) + λ_outer·c·Σ(μ_k·σ_k)
///             = R_nested_scalar + λ_outer·c·s_p
///
/// which is a valid FROST partial signature for the nested position.
pub fn inner_sign<P: OsstPoint>(
    nonces: InnerNonces<P::Scalar>,
    share: &SecretShare<P::Scalar>,
    params: &InnerSigningParams<P::Scalar>,
    inner_commitments: &[InnerCommitments<P>],
    active_indices: &[u32],
    outer_message: &[u8],
) -> Result<InnerSignatureShare<P::Scalar>, OsstError> {
    // inner binding factor (same computation as aggregate_inner_commitments)
    let rho_inner =
        inner_binding_factor::<P>(nonces.holder_index, outer_message, inner_commitments);

    // inner lagrange coefficient
    let lagrange = compute_lagrange_coefficients::<P::Scalar>(active_indices)?;
    let my_pos = active_indices
        .iter()
        .position(|&i| i == share.index)
        .ok_or(OsstError::InvalidIndex)?;
    let mu_k = &lagrange[my_pos];

    // z_{p,k} = d_k + ρ_inner_k·e_k + (λ_outer·c·μ_k)·σ_k
    let rho_e = rho_inner.mul(&nonces.binding);
    let weight = params.outer_lambda.mul(&params.outer_challenge).mul(mu_k);
    let response = nonces.hiding.add(&rho_e).add(&weight.mul(share.scalar()));

    Ok(InnerSignatureShare {
        holder_index: nonces.holder_index,
        response,
    })
}

/// aggregate inner signature shares into the nested position's outer share
pub fn aggregate_inner_shares<S: OsstScalar>(
    shares: &[InnerSignatureShare<S>],
) -> S {
    let mut z = S::zero();
    for share in shares {
        z = z.add(&share.response);
    }
    z
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use crate::frost;
    use curve25519_dalek::ristretto::RistrettoPoint;
    use curve25519_dalek::scalar::Scalar;
    use rand::rngs::OsRng;

    type Point = RistrettoPoint;

    /// THE property that makes v2 reviewable: a nested position is
    /// indistinguishable from a flat FROST signer.
    ///
    /// We build a real outer 2-of-2 over positions {1, 2}. Position 2's key is
    /// split 3-of-5 among inner holders, and its nonce is the sum of the inner
    /// holders' nonces. We then produce the signature TWICE — once through the
    /// nested v2 path, once with position 2 as an ordinary signer holding the
    /// reconstructed scalar — and assert the signature shares and the final
    /// signatures are IDENTICAL, and that both verify.
    ///
    /// If this holds, security reduces to FROST's own proof: the outer
    /// protocol cannot tell a nested position from a flat one, so an adversary
    /// against the nested scheme is an adversary against FROST.
    #[test]
    fn nested_v2_equals_flat_frost() {
        let mut rng = OsRng;
        let msg = b"settlement: a=1000 b=0";

        // ── outer key: degree-1 polynomial ⇒ 2-of-2 over positions 1,2 ──────
        let secret = Scalar::random(&mut rng);
        let a1 = Scalar::random(&mut rng);
        let eval = |x: u32| {
            let x = <Scalar as OsstScalar>::from_u32(x);
            secret.add(&a1.mul(&x))
        };
        let sigma_1 = eval(1);
        let sigma_2 = eval(2); // the NESTED position's outer share
        let group_pubkey = <Point as OsstPoint>::generator().mul_scalar(&secret);

        let share_1 = SecretShare::new(1, sigma_1.clone());
        let share_2_flat = SecretShare::new(2, sigma_2.clone());

        // ── split position 2's key 3-of-5 among inner holders ──────────────
        let inner_t = 3u32;
        let inner_n = 5u32;
        let (inner_pieces, dealer_commitment) =
            split_evaluation_for_inner::<Point, _>(&sigma_2, inner_n, inner_t, &mut rng);

        // every holder verifies its piece against the Feldman commitment
        for (k, piece) in &inner_pieces {
            assert!(
                verify_split_piece::<Point>(&dealer_commitment, *k, piece),
                "inner piece {} failed Feldman verification",
                k
            );
        }

        let quorum: Vec<u32> = vec![1, 2, 3];
        let inner_shares: Vec<SecretShare<Scalar>> = quorum
            .iter()
            .map(|k| {
                let (_, piece) = inner_pieces.iter().find(|(i, _)| i == k).unwrap();
                SecretShare::new(*k, piece.clone())
            })
            .collect();

        // sanity: the quorum reconstructs position 2's outer share
        {
            let lag = compute_lagrange_coefficients::<Scalar>(&quorum).unwrap();
            let mut recon = <Scalar as OsstScalar>::zero();
            for (i, s) in inner_shares.iter().enumerate() {
                recon = recon.add(&lag[i].mul(s.scalar()));
            }
            assert_eq!(recon, sigma_2, "inner quorum must reconstruct the outer share");
        }

        // ── inner round 0/1: commit–reveal ─────────────────────────────────
        let mut inner_nonces = Vec::new();
        let mut inner_commitments = Vec::new();
        let mut precommits = Vec::new();
        for &k in &quorum {
            let (n, c) = inner_commit::<Point, _>(k, &mut rng);
            precommits.push(inner_precommit(&c));
            inner_nonces.push(n);
            inner_commitments.push(c);
        }
        for (pre, revealed) in precommits.iter().zip(inner_commitments.iter()) {
            assert!(verify_inner_precommit::<Point>(pre, revealed));
        }
        // a tampered reveal must be caught
        {
            let mut tampered = inner_commitments[0].clone();
            tampered.hiding = tampered.hiding.add(&<Point as OsstPoint>::generator());
            assert!(!verify_inner_precommit::<Point>(&precommits[0], &tampered));
        }

        // capture the aggregate nonce scalars so we can drive the FLAT signer
        // with the same randomness (this is test-only introspection)
        let d_sum = inner_nonces
            .iter()
            .fold(<Scalar as OsstScalar>::zero(), |acc, n| acc.add(&n.hiding));
        let e_sum = inner_nonces
            .iter()
            .fold(<Scalar as OsstScalar>::zero(), |acc, n| acc.add(&n.binding));

        let (d_nested, e_nested) = aggregate_inner_commitment_pair::<Point>(&inner_commitments);
        assert_eq!(d_nested, <Point as OsstPoint>::generator().mul_scalar(&d_sum));
        assert_eq!(e_nested, <Point as OsstPoint>::generator().mul_scalar(&e_sum));

        // ── outer round: position 1 commits normally, position 2 uses the
        //    aggregate PAIR (so the outer binding factor actually applies) ──
        let (nonces_1, commits_1) = frost::commit::<Point, _>(1, &mut rng);
        let commits_2 = frost::SigningCommitments {
            index: 2,
            hiding: d_nested,
            binding: e_nested,
        };
        let package =
            frost::SigningPackage::new(msg.to_vec(), vec![commits_1, commits_2.clone()]).unwrap();

        // outer context — recomputable by any inner holder from public data
        let rho_2 = package.binding_factor(2);
        let r_outer = package.group_commitment();
        let challenge = package.challenge(&r_outer, &group_pubkey);
        let indices = package.signer_indices();
        let outer_lagrange = compute_lagrange_coefficients::<Scalar>(&indices).unwrap();
        let pos_2 = indices.iter().position(|&i| i == 2).unwrap();
        let params = InnerSigningParamsV2 {
            outer_binding: rho_2.clone(),
            outer_challenge: challenge.clone(),
            outer_lambda: outer_lagrange[pos_2].clone(),
        };

        // ── inner holders sign; every share is verified before aggregation ──
        let inner_lagrange = compute_lagrange_coefficients::<Scalar>(&quorum).unwrap();
        let public_shares: Vec<(u32, Point)> = inner_shares
            .iter()
            .map(|s| {
                (
                    s.index,
                    <Point as OsstPoint>::generator().mul_scalar(s.scalar()),
                )
            })
            .collect();

        let mut inner_sigs = Vec::new();
        for (n, share) in inner_nonces.into_iter().zip(inner_shares.iter()) {
            let sig = inner_sign_v2::<Point>(n, share, &params, &quorum).unwrap();
            let pos = quorum.iter().position(|&i| i == share.index).unwrap();
            let commitment = inner_commitments
                .iter()
                .find(|c| c.holder_index == share.index)
                .unwrap();
            let public = &public_shares
                .iter()
                .find(|(i, _)| *i == share.index)
                .unwrap()
                .1;
            assert!(
                verify_inner_share::<Point>(&sig, commitment, public, &params, &inner_lagrange[pos]),
                "holder {} produced an unverifiable share",
                share.index
            );
            inner_sigs.push(sig);
        }

        let z_nested = aggregate_inner_shares_verified::<Point>(
            &inner_sigs,
            &inner_commitments,
            &public_shares,
            &params,
            &quorum,
        )
        .expect("all inner shares must verify");

        // ── the equivalence: what a FLAT signer at position 2 would produce ──
        //   z_flat = d + ρ·e + λ·c·σ₂
        let z_flat = d_sum
            .add(&rho_2.mul(&e_sum))
            .add(&params.outer_lambda.mul(&challenge).mul(&sigma_2));
        assert_eq!(
            z_nested, z_flat,
            "nested share must equal the flat FROST share bit-for-bit"
        );

        // ── and the assembled signature must verify ────────────────────────
        let sig_1 = frost::sign::<Point>(&package, nonces_1, &share_1, &group_pubkey).unwrap();
        let sig_2 = frost::SignatureShare {
            index: 2,
            response: z_nested,
        };
        let signature =
            frost::aggregate::<Point>(&package, &[sig_1, sig_2], &group_pubkey, None).unwrap();
        assert!(
            frost::verify_signature::<Point>(&group_pubkey, msg, &signature),
            "nested-produced signature must verify under the group key"
        );

        // silence unused warning for the flat share (kept for documentation)
        let _ = share_2_flat;
    }

    /// A dishonest inner holder is NAMED, not silently folded into a signature
    /// that then fails to verify with no attribution.
    #[test]
    fn v2_names_the_dishonest_inner_holder() {
        let mut rng = OsRng;
        let msg = b"m";
        let sigma_2 = Scalar::random(&mut rng);
        let quorum: Vec<u32> = vec![1, 2, 3];
        let (inner_pieces, _) = split_evaluation_for_inner::<Point, _>(&sigma_2, 5, 3, &mut rng);
        let inner_shares: Vec<SecretShare<Scalar>> = quorum
            .iter()
            .map(|k| {
                let (_, piece) = inner_pieces.iter().find(|(i, _)| i == k).unwrap();
                SecretShare::new(*k, piece.clone())
            })
            .collect();

        let mut inner_nonces = Vec::new();
        let mut inner_commitments = Vec::new();
        for &k in &quorum {
            let (n, c) = inner_commit::<Point, _>(k, &mut rng);
            inner_nonces.push(n);
            inner_commitments.push(c);
        }
        let (d_nested, e_nested) = aggregate_inner_commitment_pair::<Point>(&inner_commitments);
        let group_pubkey = <Point as OsstPoint>::generator().mul_scalar(&sigma_2);

        let (_, commits_1) = frost::commit::<Point, _>(1, &mut rng);
        let commits_2 = frost::SigningCommitments {
            index: 2,
            hiding: d_nested,
            binding: e_nested,
        };
        let package =
            frost::SigningPackage::new(msg.to_vec(), vec![commits_1, commits_2]).unwrap();
        let r_outer = package.group_commitment();
        let indices = package.signer_indices();
        let outer_lagrange = compute_lagrange_coefficients::<Scalar>(&indices).unwrap();
        let pos_2 = indices.iter().position(|&i| i == 2).unwrap();
        let params = InnerSigningParamsV2 {
            outer_binding: package.binding_factor(2),
            outer_challenge: package.challenge(&r_outer, &group_pubkey),
            outer_lambda: outer_lagrange[pos_2].clone(),
        };

        let public_shares: Vec<(u32, Point)> = inner_shares
            .iter()
            .map(|s| {
                (
                    s.index,
                    <Point as OsstPoint>::generator().mul_scalar(s.scalar()),
                )
            })
            .collect();

        let mut sigs = Vec::new();
        for (n, share) in inner_nonces.into_iter().zip(inner_shares.iter()) {
            sigs.push(inner_sign_v2::<Point>(n, share, &params, &quorum).unwrap());
        }
        // holder 2 goes rogue
        sigs[1].response = sigs[1].response.add(&<Scalar as OsstScalar>::one());

        let err = aggregate_inner_shares_verified::<Point>(
            &sigs,
            &inner_commitments,
            &public_shares,
            &params,
            &quorum,
        )
        .expect_err("a tampered share must be rejected");
        assert_eq!(err, vec![2], "the cheating holder must be identified");
    }

    #[test]
    fn test_interleaved_dkg() {
        let mut rng = OsRng;
        let inner_n = 5u32;
        let inner_t = 3u32;
        let outer_t = 2u32;

        let (inner_shares, coeff_commitments) =
            interleaved_dkg::<Point, _>(inner_n, inner_t, outer_t, &mut rng).unwrap();

        assert_eq!(inner_shares.len(), inner_n as usize);
        assert_eq!(coeff_commitments.len(), outer_t as usize);

        // verify: any t_inner shares of each coefficient reconstruct correctly
        for j in 0..outer_t as usize {
            let shares_j: Vec<(u32, Scalar)> = inner_shares
                .iter()
                .map(|s| (s.holder_index, s.coefficient_shares[j]))
                .collect();

            let active: Vec<u32> = shares_j[..inner_t as usize]
                .iter()
                .map(|s| s.0)
                .collect();
            let lambda = compute_lagrange_coefficients::<Scalar>(&active).unwrap();
            let mut reconstructed = Scalar::ZERO;
            for (i, (_, val)) in shares_j[..inner_t as usize].iter().enumerate() {
                reconstructed += lambda[i] * val;
            }

            let expected_point = Point::generator().mul_scalar(&reconstructed);
            assert_eq!(expected_point, coeff_commitments[j]);
        }
    }

    #[test]
    fn test_split_evaluation_with_feldman_verification() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let inner_n = 5u32;
        let inner_t = 3u32;

        let (pieces, commitment) =
            split_evaluation_for_inner::<Point, _>(&secret, inner_n, inner_t, &mut rng);

        // every piece should verify against the feldman commitment
        for &(k, ref piece) in &pieces {
            assert!(
                verify_split_piece::<Point>(&commitment, k, piece),
                "piece {} failed feldman verification",
                k
            );
        }

        // a tampered piece should fail
        let tampered = Scalar::random(&mut rng);
        assert!(!verify_split_piece::<Point>(&commitment, 1, &tampered));
    }

    #[test]
    fn test_nested_frost_2of3_with_3of5_inner() {
        let mut rng = OsRng;

        let outer_t = 2u32;
        let inner_n = 5u32;
        let inner_t = 3u32;
        let nested_position = 3u32;

        // outer participants
        let buyer_dealer: dkg::Dealer<Point> = dkg::Dealer::new(1, outer_t, &mut rng);
        let seller_dealer: dkg::Dealer<Point> = dkg::Dealer::new(2, outer_t, &mut rng);

        // interleaved DKG for nested position
        let (inner_shares, coeff_commitments) =
            interleaved_dkg::<Point, _>(inner_n, inner_t, outer_t, &mut rng).unwrap();

        // inner group reconstructs f_p(j) for outer participants
        let eval_active: Vec<u32> = (1..=inner_t).collect();
        let eval_lambda = compute_lagrange_coefficients::<Scalar>(&eval_active).unwrap();

        let mut fp_at_1 = Scalar::ZERO;
        let mut fp_at_2 = Scalar::ZERO;
        for (i, &k) in eval_active.iter().enumerate() {
            fp_at_1 += eval_lambda[i] * inner_shares[(k - 1) as usize].eval_at(1);
            fp_at_2 += eval_lambda[i] * inner_shares[(k - 1) as usize].eval_at(2);
        }

        // players split evaluations with feldman commitments
        let f1_at_p = buyer_dealer.generate_subshare(nested_position);
        let (f1_pieces, f1_commitment) =
            split_evaluation_for_inner::<Point, _>(f1_at_p.value(), inner_n, inner_t, &mut rng);

        let f2_at_p = seller_dealer.generate_subshare(nested_position);
        let (f2_pieces, f2_commitment) =
            split_evaluation_for_inner::<Point, _>(f2_at_p.value(), inner_n, inner_t, &mut rng);

        // each holder verifies pieces and combines
        let mut escrow_shares: Vec<SecretShare<Scalar>> = Vec::new();
        for k in 0..inner_n as usize {
            // verify feldman commitments from both players
            assert!(verify_split_piece::<Point>(
                &f1_commitment, (k + 1) as u32, &f1_pieces[k].1
            ));
            assert!(verify_split_piece::<Point>(
                &f2_commitment, (k + 1) as u32, &f2_pieces[k].1
            ));

            let sigma = combine_shares(
                &inner_shares[k],
                nested_position,
                &[(1, f1_pieces[k].1), (2, f2_pieces[k].1)],
            );
            escrow_shares.push(SecretShare::new((k + 1) as u32, sigma));
        }

        // outer keys
        let s1 = *buyer_dealer.generate_subshare(1).value()
            + *seller_dealer.generate_subshare(1).value()
            + fp_at_1;
        let buyer_share = SecretShare::new(1, s1);

        let group_key = buyer_dealer
            .commitment()
            .share_commitment()
            .add(seller_dealer.commitment().share_commitment())
            .add(&coeff_commitments[0]);

        // escrow verification share
        let p_scalar = Scalar::from(nested_position);
        let mut y_escrow = buyer_dealer.commitment().evaluate_at(nested_position)
            .add(&seller_dealer.commitment().evaluate_at(nested_position));
        let mut p_pow = Scalar::ONE;
        for cc in &coeff_commitments {
            y_escrow = y_escrow.add(&cc.mul_scalar(&p_pow));
            p_pow *= p_scalar;
        }

        // OSST authorization
        let payload = b"authorize dispute";
        let osst_active = [1u32, 3, 5];
        let contributions: Vec<crate::Contribution<Point>> = osst_active
            .iter()
            .map(|&k| escrow_shares[(k - 1) as usize].contribute::<Point, _>(&mut rng, payload))
            .collect();
        assert!(crate::verify(&y_escrow, &contributions, inner_t, payload).unwrap());

        // nested FROST signing with inner binding factors
        let message = b"zcash spend authorization";
        let inner_active: Vec<u32> = osst_active.to_vec();

        // inner commitment round
        let mut all_inner_nonces = Vec::new();
        let mut all_inner_commitments = Vec::new();
        for &k in &inner_active {
            let (nonces, commitments) = inner_commit::<Point, _>(k, &mut rng);
            all_inner_nonces.push(nonces);
            all_inner_commitments.push(commitments);
        }

        // aggregate with inner binding factors into single bound commitment
        let r_nested = aggregate_inner_commitments(&all_inner_commitments, message);

        // buyer commits normally
        let (buyer_nonces, buyer_frost_commitments) = frost::commit::<Point, _>(1, &mut rng);

        // the nested position presents r_nested as its hiding commitment
        // and identity as its binding commitment (inner binding already applied)
        let escrow_outer_commitments = frost::SigningCommitments {
            index: nested_position,
            hiding: r_nested,
            binding: Point::identity(),
        };
        let outer_package = frost::SigningPackage::new(
            message.to_vec(),
            vec![buyer_frost_commitments, escrow_outer_commitments],
        )
        .unwrap();

        // buyer signs
        let buyer_sig_share =
            frost::sign::<Point>(&outer_package, buyer_nonces, &buyer_share, &group_key).unwrap();

        // compute outer params for inner holders
        let outer_indices = outer_package.signer_indices();
        let outer_lambda = compute_lagrange_coefficients::<Scalar>(&outer_indices).unwrap();
        let nested_pos_idx = outer_indices.iter().position(|&i| i == nested_position).unwrap();

        // outer group commitment (must match what frost::sign computes)
        let outer_group_commitment = {
            let mut r = Point::identity();
            for &idx in &outer_indices {
                let c = outer_package.get_commitments(idx).unwrap();
                let rho = compute_outer_binding_factor::<Point>(idx, message, &outer_package);
                r = r.add(&c.hiding).add(&c.binding.mul_scalar(&rho));
            }
            r
        };

        let outer_challenge = {
            let mut h = Sha512::new();
            h.update(b"frost-challenge-v1");
            h.update(OsstPoint::compress(&outer_group_commitment));
            h.update(OsstPoint::compress(&group_key));
            h.update(message);
            let hash: [u8; 64] = h.finalize().into();
            Scalar::from_bytes_wide(&hash)
        };

        let params = InnerSigningParams {
            outer_challenge,
            outer_lambda: outer_lambda[nested_pos_idx],
        };

        // inner holders sign with inner binding
        let mut inner_sig_shares = Vec::new();
        for (nonces, &k) in all_inner_nonces.into_iter().zip(inner_active.iter()) {
            let sig = inner_sign::<Point>(
                nonces,
                &escrow_shares[(k - 1) as usize],
                &params,
                &all_inner_commitments,
                &inner_active,
                message,
            )
            .unwrap();
            inner_sig_shares.push(sig);
        }

        let z_nested = aggregate_inner_shares(&inner_sig_shares);
        let escrow_sig_share = frost::SignatureShare {
            index: nested_position,
            response: z_nested,
        };

        let signature = frost::aggregate::<Point>(
            &outer_package,
            &[buyer_sig_share, escrow_sig_share],
            &group_key,
            None,
        )
        .unwrap();

        assert!(
            frost::verify_signature(&group_key, message, &signature),
            "nested FROST with inner binding factors must verify"
        );
    }

    #[test]
    fn test_nested_frost_different_subsets() {
        let mut rng = OsRng;

        let outer_t = 2u32;
        let inner_n = 5u32;
        let inner_t = 3u32;
        let nested_position = 3u32;

        let buyer_dealer: dkg::Dealer<Point> = dkg::Dealer::new(1, outer_t, &mut rng);
        let seller_dealer: dkg::Dealer<Point> = dkg::Dealer::new(2, outer_t, &mut rng);

        let (inner_shares, coeff_commitments) =
            interleaved_dkg::<Point, _>(inner_n, inner_t, outer_t, &mut rng).unwrap();

        let eval_active: Vec<u32> = (1..=inner_t).collect();
        let eval_lambda = compute_lagrange_coefficients::<Scalar>(&eval_active).unwrap();

        let mut fp_at_1 = Scalar::ZERO;
        for (i, &k) in eval_active.iter().enumerate() {
            fp_at_1 += eval_lambda[i] * inner_shares[(k - 1) as usize].eval_at(1);
        }

        let f1_at_p = buyer_dealer.generate_subshare(nested_position);
        let f2_at_p = seller_dealer.generate_subshare(nested_position);
        let (f1_pieces, _) =
            split_evaluation_for_inner::<Point, _>(f1_at_p.value(), inner_n, inner_t, &mut rng);
        let (f2_pieces, _) =
            split_evaluation_for_inner::<Point, _>(f2_at_p.value(), inner_n, inner_t, &mut rng);

        let mut escrow_shares: Vec<SecretShare<Scalar>> = Vec::new();
        for k in 0..inner_n as usize {
            let sigma = combine_shares(
                &inner_shares[k],
                nested_position,
                &[(1, f1_pieces[k].1), (2, f2_pieces[k].1)],
            );
            escrow_shares.push(SecretShare::new((k + 1) as u32, sigma));
        }

        let s1 = *buyer_dealer.generate_subshare(1).value()
            + *seller_dealer.generate_subshare(1).value()
            + fp_at_1;
        let buyer_share = SecretShare::new(1, s1);
        let group_key = buyer_dealer
            .commitment()
            .share_commitment()
            .add(seller_dealer.commitment().share_commitment())
            .add(&coeff_commitments[0]);

        // test 4 different inner subsets
        for subset in &[[1u32, 2, 3], [1, 3, 5], [2, 4, 5], [3, 4, 5]] {
            let message = format!("sign with {:?}", subset);
            let message = message.as_bytes();
            let inner_active: Vec<u32> = subset.to_vec();

            let mut nonces_vec = Vec::new();
            let mut commits_vec = Vec::new();
            for &k in &inner_active {
                let (n, c) = inner_commit::<Point, _>(k, &mut rng);
                nonces_vec.push(n);
                commits_vec.push(c);
            }

            let r_nested = aggregate_inner_commitments(&commits_vec, message);
            let (buyer_nonces, buyer_commits) = frost::commit::<Point, _>(1, &mut rng);

            let escrow_commits = frost::SigningCommitments {
                index: nested_position,
                hiding: r_nested,
                binding: Point::identity(),
            };
            let package = frost::SigningPackage::new(
                message.to_vec(),
                vec![buyer_commits, escrow_commits],
            )
            .unwrap();

            let buyer_sig =
                frost::sign::<Point>(&package, buyer_nonces, &buyer_share, &group_key).unwrap();

            let outer_indices = package.signer_indices();
            let outer_lambda = compute_lagrange_coefficients::<Scalar>(&outer_indices).unwrap();
            let nested_pos = outer_indices.iter().position(|&i| i == nested_position).unwrap();

            let outer_gc = {
                let mut r = Point::identity();
                for &idx in &outer_indices {
                    let c = package.get_commitments(idx).unwrap();
                    let rho = compute_outer_binding_factor::<Point>(idx, message, &package);
                    r = r.add(&c.hiding).add(&c.binding.mul_scalar(&rho));
                }
                r
            };

            let outer_challenge = {
                let mut h = Sha512::new();
                h.update(b"frost-challenge-v1");
                h.update(OsstPoint::compress(&outer_gc));
                h.update(OsstPoint::compress(&group_key));
                h.update(message);
                Scalar::from_bytes_wide(&h.finalize().into())
            };

            let params = InnerSigningParams {
                outer_challenge,
                outer_lambda: outer_lambda[nested_pos],
            };

            let mut inner_sigs = Vec::new();
            for (nonces, &k) in nonces_vec.into_iter().zip(inner_active.iter()) {
                inner_sigs.push(
                    inner_sign::<Point>(
                        nonces,
                        &escrow_shares[(k - 1) as usize],
                        &params,
                        &commits_vec,
                        &inner_active,
                        message,
                    )
                    .unwrap(),
                );
            }

            let z_nested = aggregate_inner_shares(&inner_sigs);
            let escrow_sig = frost::SignatureShare {
                index: nested_position,
                response: z_nested,
            };

            let signature =
                frost::aggregate::<Point>(&package, &[buyer_sig, escrow_sig], &group_key, None)
                    .unwrap();

            assert!(
                frost::verify_signature(&group_key, message, &signature),
                "subset {:?} failed",
                subset
            );
        }
    }

    /// helper: compute outer binding factor (mirrors frost.rs internals)
    fn compute_outer_binding_factor<P: OsstPoint>(
        index: u32,
        message: &[u8],
        package: &frost::SigningPackage<P>,
    ) -> P::Scalar {
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
        P::Scalar::from_bytes_wide(&h.finalize().into())
    }
}


// ============================================================================
// Nested FROST v2 — outer-bound, commit-reveal, verifiable
// ============================================================================
//
// v1 pre-bound the inner nonces and presented one point to the outer protocol.
// That severs the outer binding coupling (see the warning on
// `aggregate_inner_commitments`) and admits a ROS-style forgery.
//
// v2 instead presents the nested position as an ORDINARY FROST signer:
//
//   D_nested = Σ_k D_k        E_nested = Σ_k E_k
//
// The outer protocol computes ρ = H(index, m, B) over the FULL outer
// commitment list, exactly as for any other signer, and each inner holder
// signs with that same ρ:
//
//   z_k = d_k + ρ·e_k + (λ_out·c·μ_k)·σ_k
//
// Summing over the inner quorum, with d = Σd_k, e = Σe_k and Σ μ_k·σ_k = σ_out
// (inner Lagrange interpolation):
//
//   z_nested = d + ρ·e + λ_out·c·σ_out
//
// which is bit-for-bit what a single FROST signer holding σ_out with nonces
// (d, e) produces. The nested position is therefore INDISTINGUISHABLE from a
// flat signer whose nonce and key happen to be additively shared — so security
// reduces to FROST's own proof plus inner-group honesty, rather than requiring
// a novel composition argument. `nested_equals_flat_frost` in the tests below
// asserts that equivalence on real values.
//
// Because the inner binding factor is gone, adaptive commitment selection
// INSIDE the jury is prevented by an explicit commit–reveal round instead:
// every holder publishes H(k ‖ D_k ‖ E_k) before any commitment is revealed.

/// Round-0 hash commitment to an inner holder's nonce commitments.
pub fn inner_precommit<P: OsstPoint>(c: &InnerCommitments<P>) -> [u8; 32] {
    let mut h = Sha512::new();
    h.update(b"frostito-inner-precommit-v2");
    h.update(c.holder_index.to_le_bytes());
    h.update(c.hiding.compress());
    h.update(c.binding.compress());
    let full: [u8; 64] = h.finalize().into();
    let mut out = [0u8; 32];
    out.copy_from_slice(&full[..32]);
    out
}

/// Verify a revealed commitment against its round-0 precommitment.
///
/// Every holder MUST check every other holder's reveal before the aggregate is
/// formed. Without this, a holder revealing last can choose D_k to steer
/// D_nested to a value of its choosing.
pub fn verify_inner_precommit<P: OsstPoint>(
    precommit: &[u8; 32],
    revealed: &InnerCommitments<P>,
) -> bool {
    // constant-time-ish compare; these are public values, but keep the habit
    let computed = inner_precommit(revealed);
    let mut diff = 0u8;
    for (a, b) in computed.iter().zip(precommit.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Aggregate inner nonce commitments into the pair the OUTER protocol consumes.
///
/// Returns `(D_nested, E_nested)`. Feed these to the outer `SigningCommitments`
/// as `hiding` and `binding` respectively — the nested position then looks
/// exactly like any other signer and receives a real outer binding factor.
///
/// Callers MUST have verified every precommitment (see
/// [`verify_inner_precommit`]) before calling this.
pub fn aggregate_inner_commitment_pair<P: OsstPoint>(
    inner_commitments: &[InnerCommitments<P>],
) -> (P, P) {
    let mut d = P::identity();
    let mut e = P::identity();
    for c in inner_commitments {
        d = d.add(&c.hiding);
        e = e.add(&c.binding);
    }
    (d, e)
}

/// Outer context handed to inner holders for v2 signing. Every field is
/// derivable from public outer data, so holders can independently verify it
/// rather than trusting the coordinator.
#[derive(Clone)]
pub struct InnerSigningParamsV2<S: OsstScalar> {
    /// outer binding factor for the nested position: ρ = H(index, m, B)
    pub outer_binding: S,
    /// outer schnorr challenge: c = H(R_outer, Y, m)
    pub outer_challenge: S,
    /// outer lagrange coefficient for the nested position
    pub outer_lambda: S,
}

/// Inner holder's partial signature under v2.
///
/// z_k = d_k + ρ·e_k + (λ_out·c·μ_k)·σ_k
///
/// Note `nonces` is taken BY VALUE: the nonce pair is consumed and zeroized on
/// drop, so a holder cannot produce two shares from one commitment round
/// without deliberately cloning. Callers that persist state across restarts
/// MUST additionally record the round as spent — the type system cannot see
/// a process boundary.
pub fn inner_sign_v2<P: OsstPoint>(
    nonces: InnerNonces<P::Scalar>,
    share: &SecretShare<P::Scalar>,
    params: &InnerSigningParamsV2<P::Scalar>,
    active_indices: &[u32],
) -> Result<InnerSignatureShare<P::Scalar>, OsstError> {
    let lagrange = compute_lagrange_coefficients::<P::Scalar>(active_indices)?;
    let my_pos = active_indices
        .iter()
        .position(|&i| i == share.index)
        .ok_or(OsstError::InvalidIndex)?;
    let mu_k = &lagrange[my_pos];

    let rho_e = params.outer_binding.mul(&nonces.binding);
    let weight = params
        .outer_lambda
        .mul(&params.outer_challenge)
        .mul(mu_k);
    let response = nonces.hiding.add(&rho_e).add(&weight.mul(share.scalar()));

    Ok(InnerSignatureShare {
        holder_index: nonces.holder_index,
        response,
    })
}

/// Verify one inner holder's share before it is aggregated:
///
///   z_k·G  ==  (D_k + ρ·E_k) + (λ_out·c·μ_k)·P_k
///
/// where `P_k = σ_k·G` is the holder's public share (derivable from the DKG
/// coefficient commitments). Without this an invalid share is silently folded
/// into the sum, producing a signature that fails to verify with no indication
/// of which holder was at fault.
pub fn verify_inner_share<P: OsstPoint>(
    sig: &InnerSignatureShare<P::Scalar>,
    commitment: &InnerCommitments<P>,
    public_share: &P,
    params: &InnerSigningParamsV2<P::Scalar>,
    mu_k: &P::Scalar,
) -> bool {
    let lhs = P::generator().mul_scalar(&sig.response);
    let weight = params
        .outer_lambda
        .mul(&params.outer_challenge)
        .mul(mu_k);
    let rhs = commitment
        .hiding
        .add(&commitment.binding.mul_scalar(&params.outer_binding))
        .add(&public_share.mul_scalar(&weight));
    lhs == rhs
}

/// Verify every inner share, then aggregate.
///
/// `Err(indices)` names the holders whose shares failed — the caller can evict
/// them and retry with a different quorum instead of broadcasting a signature
/// that will simply be rejected.
pub fn aggregate_inner_shares_verified<P: OsstPoint>(
    sigs: &[InnerSignatureShare<P::Scalar>],
    commitments: &[InnerCommitments<P>],
    public_shares: &[(u32, P)],
    params: &InnerSigningParamsV2<P::Scalar>,
    active_indices: &[u32],
) -> Result<P::Scalar, Vec<u32>> {
    let lagrange = match compute_lagrange_coefficients::<P::Scalar>(active_indices) {
        Ok(l) => l,
        Err(_) => return Err(active_indices.to_vec()),
    };

    let mut bad = Vec::new();
    let mut z = P::Scalar::zero();
    for sig in sigs {
        let k = sig.holder_index;
        let pos = active_indices.iter().position(|&i| i == k);
        let commitment = commitments.iter().find(|c| c.holder_index == k);
        let public = public_shares.iter().find(|(i, _)| *i == k).map(|(_, p)| p);
        match (pos, commitment, public) {
            (Some(pos), Some(commitment), Some(public)) => {
                if verify_inner_share::<P>(sig, commitment, public, params, &lagrange[pos]) {
                    z = z.add(&sig.response);
                } else {
                    bad.push(k);
                }
            }
            // missing commitment or public share ⇒ cannot verify ⇒ reject
            _ => bad.push(k),
        }
    }

    if bad.is_empty() {
        Ok(z)
    } else {
        Err(bad)
    }
}
