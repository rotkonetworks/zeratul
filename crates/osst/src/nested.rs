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
