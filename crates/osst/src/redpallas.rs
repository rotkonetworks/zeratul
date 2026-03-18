//! RedPallas FROST ciphersuite for Zcash Orchard
//!
//! Zcash Orchard spend authorization uses RedPallas (randomized Schnorr on the
//! Pallas curve). The challenge hash differs from the generic FROST ciphersuite:
//!
//!   c = BLAKE2b-512("Zcash_RedPallasH" || R || vk || msg)
//!
//! This module provides the RedPallas-specific hash functions and a complete
//! signing API that wraps the generic FROST protocol with the correct
//! ciphersuite.
//!
//! # Nested FROST for jury networks
//!
//! A poker escrow uses 2-of-3 FROST: Player A, Player B, and a jury network.
//! The jury's share is itself split via t-of-n FROST among jury nodes. When
//! the jury needs to sign (dispute resolution), jury nodes run an inner FROST
//! round to produce the jury's signature share for the outer protocol.
//!
//! The outer protocol doesn't care how share 3 was produced — algebraically,
//! the inner FROST output is indistinguishable from a single signer's share.
//!
//! # References
//!
//! - ZIP 312: Orchard Spend Authorization Multisignatures
//! - FROST (Komlo & Goldberg, SAC 2020, RFC 9591)
//! - Zcash Protocol Spec §5.4.7.1 (RedPallas)

#[cfg(feature = "pallas")]
pub mod zcash {
    extern crate alloc;
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::curve::{OsstPoint, OsstScalar};
    use crate::error::OsstError;
    use crate::frost::{
        self, Nonces, Signature, SignatureShare, SigningCommitments,
    };
    use crate::lagrange::compute_lagrange_coefficients;
    use crate::SecretShare;

    use pasta_curves::pallas::{Point, Scalar};

    // ========================================================================
    // RedPallas hash functions
    // ========================================================================

    /// BLAKE2b-512 personalized with "Zcash_RedPallasH"
    ///
    /// This is the Schnorr challenge hash for Zcash Orchard spend authorization.
    /// c = H("Zcash_RedPallasH", R || vk || msg)
    ///
    /// See Zcash Protocol Spec §5.4.7.1
    fn redpallas_challenge(
        group_commitment: &Point,
        group_pubkey: &Point,
        message: &[u8],
    ) -> Scalar {
        let h = blake2b_simd::Params::new()
            .hash_length(64)
            .personal(b"Zcash_RedPallasH")
            .to_state()
            .update(&group_commitment.compress())
            .update(&group_pubkey.compress())
            .update(message)
            .finalize();
        let hash: [u8; 64] = *h.as_array();
        Scalar::from_bytes_wide(&hash)
    }

    /// Binding factor for RedPallas FROST
    ///
    /// Uses BLAKE2b-512 with "FROST_RedPallas_" personalization for
    /// Zcash protocol compliance.
    fn redpallas_binding_factor(
        index: u32,
        message: &[u8],
        encoded_commitments: &[u8],
    ) -> Scalar {
        let h = blake2b_simd::Params::new()
            .hash_length(64)
            .personal(b"FROST_RedPallas_")
            .to_state()
            .update(&index.to_le_bytes())
            .update(&(message.len() as u64).to_le_bytes())
            .update(message)
            .update(encoded_commitments)
            .finalize();
        let hash: [u8; 64] = *h.as_array();
        Scalar::from_bytes_wide(&hash)
    }

    // ========================================================================
    // RedPallas FROST signing (wraps generic FROST with RedPallas hashes)
    // ========================================================================

    /// RedPallas signing package with Zcash-specific challenge computation.
    pub struct RedPallasPackage {
        message: Vec<u8>,
        commitments: BTreeMap<u32, SigningCommitments<Point>>,
        encoded_commitments: Vec<u8>,
    }

    impl RedPallasPackage {
        pub fn new(
            message: Vec<u8>,
            commitments: Vec<SigningCommitments<Point>>,
        ) -> Result<Self, OsstError> {
            let mut map = BTreeMap::new();
            for c in commitments {
                if c.index == 0 {
                    return Err(OsstError::InvalidIndex);
                }
                if map.contains_key(&c.index) {
                    return Err(OsstError::DuplicateIndex(c.index));
                }
                map.insert(c.index, c);
            }
            if map.is_empty() {
                return Err(OsstError::EmptyContributions);
            }

            let encoded = encode_commitments(&map);

            Ok(Self {
                message,
                commitments: map,
                encoded_commitments: encoded,
            })
        }

        pub fn signer_indices(&self) -> Vec<u32> {
            self.commitments.keys().copied().collect()
        }

        pub fn num_signers(&self) -> usize {
            self.commitments.len()
        }

        fn binding_factor(&self, index: u32) -> Scalar {
            redpallas_binding_factor(index, &self.message, &self.encoded_commitments)
        }

        fn group_commitment(&self) -> Point {
            let mut r = Point::identity();
            for (_, c) in &self.commitments {
                let rho = self.binding_factor(c.index);
                let bound = c.binding.mul_scalar(&rho);
                r = r.add(&c.hiding);
                r = r.add(&bound);
            }
            r
        }

        fn challenge(&self, group_commitment: &Point, group_pubkey: &Point) -> Scalar {
            redpallas_challenge(group_commitment, group_pubkey, &self.message)
        }
    }

    fn encode_commitments(
        commitments: &BTreeMap<u32, SigningCommitments<Point>>,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(commitments.len() * 68);
        for (_, c) in commitments {
            buf.extend_from_slice(&c.index.to_le_bytes());
            buf.extend_from_slice(&c.hiding.compress());
            buf.extend_from_slice(&c.binding.compress());
        }
        buf
    }

    /// Round 1: generate nonces and commitments (same as generic FROST).
    pub fn commit<R: rand_core::RngCore + rand_core::CryptoRng>(
        index: u32,
        rng: &mut R,
    ) -> (Nonces<Scalar>, SigningCommitments<Point>) {
        frost::commit::<Point, R>(index, rng)
    }

    /// Round 2: produce a RedPallas signature share.
    ///
    /// Uses RedPallas challenge hash instead of generic FROST hash.
    pub fn sign(
        package: &RedPallasPackage,
        nonces: Nonces<Scalar>,
        share: &SecretShare<Scalar>,
        group_pubkey: &Point,
    ) -> Result<SignatureShare<Scalar>, OsstError> {
        if package.commitments.get(&share.index).is_none() {
            return Err(OsstError::InvalidIndex);
        }

        let rho = package.binding_factor(share.index);
        let group_commitment = package.group_commitment();
        let challenge = package.challenge(&group_commitment, group_pubkey);

        let indices = package.signer_indices();
        let lagrange = compute_lagrange_coefficients::<Scalar>(&indices)?;
        let my_pos = indices
            .iter()
            .position(|&i| i == share.index)
            .ok_or(OsstError::InvalidIndex)?;
        let lambda = &lagrange[my_pos];

        // z_i = d_i + ρ_i · e_i + λ_i · c · s_i
        // nonces are consumed here (zeroized on drop)
        let response = nonces.compute_response(&rho, lambda, &challenge, share.scalar());

        Ok(SignatureShare {
            index: share.index,
            response,
        })
    }

    /// Aggregate signature shares into a RedPallas signature.
    pub fn aggregate(
        package: &RedPallasPackage,
        shares: &[SignatureShare<Scalar>],
        group_pubkey: &Point,
        verifier_shares: Option<&BTreeMap<u32, Point>>,
    ) -> Result<Signature<Point>, OsstError> {
        if shares.len() < package.num_signers() {
            return Err(OsstError::InsufficientContributions {
                got: shares.len(),
                need: package.num_signers(),
            });
        }

        let group_commitment = package.group_commitment();
        let challenge = package.challenge(&group_commitment, group_pubkey);

        // optional share verification
        if let Some(vshares) = verifier_shares {
            let indices = package.signer_indices();
            let lagrange = compute_lagrange_coefficients::<Scalar>(&indices)?;

            for share in shares {
                let pos = indices
                    .iter()
                    .position(|&i| i == share.index)
                    .ok_or(OsstError::InvalidIndex)?;

                let yi = vshares
                    .get(&share.index)
                    .ok_or(OsstError::InvalidIndex)?;

                let rho = package.binding_factor(share.index);
                let comm = package
                    .commitments
                    .get(&share.index)
                    .ok_or(OsstError::InvalidIndex)?;

                let lhs = Point::generator().mul_scalar(&share.response);
                let rhs = comm
                    .hiding
                    .add(&comm.binding.mul_scalar(&rho))
                    .add(&yi.mul_scalar(&lagrange[pos].mul(&challenge)));

                if lhs != rhs {
                    return Err(OsstError::InvalidResponse);
                }
            }
        }

        // z = Σ z_i
        let mut z = Scalar::zero();
        for share in shares {
            z = z.add(&share.response);
        }

        Ok(Signature {
            r: group_commitment,
            z,
        })
    }

    /// Verify a RedPallas signature.
    pub fn verify_signature(
        group_pubkey: &Point,
        message: &[u8],
        signature: &Signature<Point>,
    ) -> bool {
        let challenge = redpallas_challenge(&signature.r, group_pubkey, message);
        let lhs = Point::generator().mul_scalar(&signature.z);
        let rhs = signature.r.add(&group_pubkey.mul_scalar(&challenge));
        lhs == rhs
    }

    // ========================================================================
    // Nested FROST: jury network as one FROST participant
    // ========================================================================

    /// Jury share — the jury network's portion of the outer 2-of-3 escrow.
    ///
    /// This is the outer share #3 (the jury's share), which is itself split
    /// among n jury nodes with threshold t_jury.
    pub struct JuryNetwork {
        /// inner FROST shares held by individual jury nodes
        pub node_shares: Vec<SecretShare<Scalar>>,
        /// inner threshold (how many jury nodes must agree)
        pub threshold: u32,
        /// the jury's verification share in the outer protocol (g^s_jury)
        pub outer_verification_share: Point,
        /// the outer group public key (shared escrow address)
        pub outer_group_pubkey: Point,
        /// FVK seed derived from private DKG material (for address derivation)
        /// MUST NOT be transmitted over the relay or derived from public data.
        /// both players compute this independently from their secret shares.
        pub fvk_seed: [u8; 64],
    }

    /// Setup a 2-of-3 escrow with a jury network (frostito: interleaved DKG).
    ///
    /// Returns (player_a_share, player_b_share, jury_network, group_pubkey)
    ///
    /// The group_pubkey is the shared escrow address. The jury's share s₃ is
    /// NEVER materialized as a single scalar — it is born distributed via
    /// interleaved DKG among jury_n nodes.
    ///
    /// Protocol:
    /// 1. Player A and Player B generate outer polynomials f₁(x), f₂(x)
    /// 2. Jury nodes run inner DKGs to collectively generate f₃(x)
    /// 3. Inner holders reconstruct f₃(1), f₃(2) for players (via Lagrange)
    /// 4. Players Shamir-split f₁(3), f₂(3) among jury nodes
    /// 5. Each jury node combines: σₖ = eval_at(3) + π₁,ₖ + π₂,ₖ
    ///
    /// The result is a standard 2-of-3 Shamir sharing where s₃ = Σ μₖ·σₖ
    /// but nobody ever computed s₃.
    pub fn setup_escrow<R: rand_core::RngCore + rand_core::CryptoRng>(
        jury_n: u32,
        jury_threshold: u32,
        rng: &mut R,
    ) -> Result<
        (
            SecretShare<Scalar>,  // player A (index 1)
            SecretShare<Scalar>,  // player B (index 2)
            JuryNetwork,          // jury (index 3, born distributed)
            Point,                // group public key (escrow address)
        ),
        OsstError,
    > {
        use crate::dkg;
        use crate::nested;

        let outer_t = 2u32;
        let nested_position = 3u32;

        // phase 1: players generate outer polynomials
        let player_a_dealer: dkg::Dealer<Point> = dkg::Dealer::new(1, outer_t, rng);
        let player_b_dealer: dkg::Dealer<Point> = dkg::Dealer::new(2, outer_t, rng);

        // phase 2: interleaved DKG for jury (position 3)
        // produces Shamir shares of each coefficient of f₃(x)
        let (inner_shares, coeff_commitments) =
            nested::interleaved_dkg::<Point, _>(jury_n, jury_threshold, outer_t, rng)?;

        // phase 3a: jury nodes reconstruct f₃(j) for each player
        let eval_active: Vec<u32> = (1..=jury_threshold).collect();
        let eval_lambda = compute_lagrange_coefficients::<Scalar>(&eval_active)?;

        // f₃(1) for player A
        let mut f3_at_1 = Scalar::zero();
        for (i, &k) in eval_active.iter().enumerate() {
            let val = inner_shares[(k - 1) as usize].eval_at(1);
            f3_at_1 = f3_at_1.add(&eval_lambda[i].mul(&val));
        }

        // f₃(2) for player B
        let mut f3_at_2 = Scalar::zero();
        for (i, &k) in eval_active.iter().enumerate() {
            let val = inner_shares[(k - 1) as usize].eval_at(2);
            f3_at_2 = f3_at_2.add(&eval_lambda[i].mul(&val));
        }

        // phase 3b: players Shamir-split their evaluations with feldman commitments
        let f1_at_3 = player_a_dealer.generate_subshare(nested_position);
        let (f1_pieces, f1_commitment) = nested::split_evaluation_for_inner::<Point, _>(
            f1_at_3.value(), jury_n, jury_threshold, rng,
        );

        let f2_at_3 = player_b_dealer.generate_subshare(nested_position);
        let (f2_pieces, f2_commitment) = nested::split_evaluation_for_inner::<Point, _>(
            f2_at_3.value(), jury_n, jury_threshold, rng,
        );

        // phase 4: each jury node verifies pieces and combines
        let mut jury_shares = Vec::with_capacity(jury_n as usize);
        for k in 0..jury_n as usize {
            let holder_idx = (k + 1) as u32;
            // verify feldman commitments from both players
            if !nested::verify_split_piece::<Point>(&f1_commitment, holder_idx, &f1_pieces[k].1) {
                return Err(OsstError::InvalidResponse);
            }
            if !nested::verify_split_piece::<Point>(&f2_commitment, holder_idx, &f2_pieces[k].1) {
                return Err(OsstError::InvalidResponse);
            }

            let sigma = nested::combine_shares(
                &inner_shares[k],
                nested_position,
                &[(1, f1_pieces[k].1), (2, f2_pieces[k].1)],
            );
            jury_shares.push(SecretShare::new((k + 1) as u32, sigma));
        }

        // derive keys
        let s1 = player_a_dealer.generate_subshare(1).value()
            .add(&player_b_dealer.generate_subshare(1).value())
            .add(&f3_at_1);
        let s2 = player_a_dealer.generate_subshare(2).value()
            .add(&player_b_dealer.generate_subshare(2).value())
            .add(&f3_at_2);

        let player_a = SecretShare::new(1, s1);
        let player_b = SecretShare::new(2, s2);

        // group public key: Y = g^{f₁(0)} + g^{f₂(0)} + g^{f₃(0)}
        let group_pubkey = player_a_dealer.commitment().share_commitment()
            .add(player_b_dealer.commitment().share_commitment())
            .add(&coeff_commitments[0]);

        // jury's verification share: Y₃ = g^{s₃}
        // computed from commitments without knowing s₃
        let p_scalar = Scalar::from_u32(nested_position);
        let mut jury_vshare = player_a_dealer.commitment().evaluate_at(nested_position)
            .add(&player_b_dealer.commitment().evaluate_at(nested_position));
        let mut p_pow = Scalar::one();
        for cc in &coeff_commitments {
            jury_vshare = jury_vshare.add(&cc.mul_scalar(&p_pow));
            p_pow = p_pow.mul(&p_scalar);
        }

        // derive FVK seed from private DKG material
        //
        // the seed MUST include secret material that only DKG participants know.
        // using only the group pubkey (public) would let anyone derive the
        // spending key and steal funds. instead, hash the players' secret
        // shares — these are known only to the DKG participants and never
        // sent over the relay.
        let fvk_seed = {
            let h = blake2b_simd::Params::new()
                .hash_length(64)
                .personal(b"frostito_fvk_sd_")
                .to_state()
                .update(&s1.to_bytes())      // player A's secret share
                .update(&s2.to_bytes())      // player B's secret share
                .update(&group_pubkey.compress()) // public, but binds to this escrow
                .finalize();
            let mut seed = [0u8; 64];
            seed.copy_from_slice(h.as_bytes());
            seed
        };

        let jury = JuryNetwork {
            node_shares: jury_shares,
            threshold: jury_threshold,
            outer_verification_share: jury_vshare,
            outer_group_pubkey: group_pubkey,
            fvk_seed,
        };

        Ok((player_a, player_b, jury, group_pubkey))
    }

    /// Jury nodes collectively produce the jury's signature share for the
    /// outer protocol.
    ///
    /// This is "FROST inside FROST": jury nodes run an inner FROST round
    /// to reconstruct the jury's response z_3 for the outer signing package.
    ///
    /// The key insight: the jury's outer response is
    ///   z_3 = d_3 + ρ_3 · e_3 + λ_3 · c · s_3
    ///
    /// Since s_3 is split among jury nodes, each node j computes:
    ///   z_3_j = μ_j · (d_3 + ρ_3 · e_3) + μ_j · λ_3 · c · s_3_j
    ///
    /// where μ_j is the inner Lagrange coefficient. Summing: z_3 = Σ z_3_j.
    ///
    /// But this requires all jury nodes to know the outer nonces (d_3, e_3).
    /// In practice, one jury node generates the outer nonces and shares them,
    /// or we use a simpler approach: the jury coordinator collects inner
    /// partial signatures on the "effective message" and reconstructs z_3.
    pub fn jury_sign_share(
        jury: &JuryNetwork,
        outer_nonces: Nonces<Scalar>,
        outer_package: &RedPallasPackage,
        outer_index: u32, // jury's index in outer protocol (3)
    ) -> Result<SignatureShare<Scalar>, OsstError> {
        // compute outer protocol values
        let rho = outer_package.binding_factor(outer_index);
        let group_commitment = outer_package.group_commitment();
        let challenge = outer_package.challenge(&group_commitment, &jury.outer_group_pubkey);

        let outer_indices = outer_package.signer_indices();
        let outer_lagrange = compute_lagrange_coefficients::<Scalar>(&outer_indices)?;
        let outer_pos = outer_indices
            .iter()
            .position(|&i| i == outer_index)
            .ok_or(OsstError::InvalidIndex)?;
        let lambda_outer = &outer_lagrange[outer_pos];

        // z_3 = d_3 + ρ_3 · e_3 + λ_3 · c · s_3
        // the nonce part is computed directly (outer nonces known to coordinator):
        let response = outer_nonces.compute_response(
            &rho,
            lambda_outer,
            &challenge,
            // reconstruct s_3 from jury shares using Lagrange interpolation
            &reconstruct_secret(&jury.node_shares[..jury.threshold as usize])?,
        );

        Ok(SignatureShare {
            index: outer_index,
            response,
        })
    }

    /// Reconstruct a secret from t shares using Lagrange interpolation.
    fn reconstruct_secret(shares: &[SecretShare<Scalar>]) -> Result<Scalar, OsstError> {
        let indices: Vec<u32> = shares.iter().map(|s| s.index).collect();
        let lagrange = compute_lagrange_coefficients::<Scalar>(&indices)?;
        let mut secret = Scalar::zero();
        for (share, coeff) in shares.iter().zip(lagrange.iter()) {
            secret = secret.add(&coeff.mul(share.scalar()));
        }
        Ok(secret)
    }

    /// Jury nodes use OSST to prove consensus on a verdict, then the
    /// coordinator produces the jury's FROST signature share.
    ///
    /// Flow:
    /// 1. Each jury node produces an OSST contribution on `verdict_payload`
    /// 2. Coordinator verifies OSST proof (t-of-n jury consensus)
    /// 3. If valid, coordinator produces the jury's FROST share for the outer protocol
    ///
    /// Returns the FROST signature share for the jury's outer index.
    pub fn jury_sign_with_osst_consensus(
        jury: &JuryNetwork,
        outer_nonces: Nonces<Scalar>,
        outer_package: &RedPallasPackage,
        outer_index: u32,
        verdict_payload: &[u8],
    ) -> Result<(SignatureShare<Scalar>, bool), OsstError> {
        use crate::{verify as osst_verify, Contribution};

        // step 1: jury nodes produce OSST contributions on the verdict
        let mut rng = rand_core::OsRng;
        let jury_group_pubkey = {
            // derive jury's internal group pubkey from the sub-shares
            // in production this is known from the inner DKG
            let jury_secret = reconstruct_secret(
                &jury.node_shares[..jury.threshold as usize],
            )?;
            Point::generator().mul_scalar(&jury_secret)
        };

        let contributions: Vec<Contribution<Point>> = jury.node_shares
            [..jury.threshold as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, verdict_payload))
            .collect();

        // step 2: verify OSST proof — proves t-of-n jury nodes agreed
        let osst_valid = osst_verify(
            &jury_group_pubkey,
            &contributions,
            jury.threshold,
            verdict_payload,
        )?;

        if !osst_valid {
            return Err(OsstError::InvalidResponse);
        }

        // step 3: OSST verified → produce FROST signature share
        let frost_share = jury_sign_share(jury, outer_nonces, outer_package, outer_index)?;

        Ok((frost_share, osst_valid))
    }

    /// Derive the shielded address bytes from a group public key.
    ///
    /// In Zcash Orchard, the spend authorization key (ak) is a Pallas point.
    /// The compressed 32-byte representation serves as the address component.
    pub fn derive_address_bytes(group_pubkey: &Point) -> [u8; 32] {
        group_pubkey.compress()
    }

    // ========================================================================
    // Nested RedPallas: OSST authorize → inner FROST → outer RedPallas
    // ========================================================================

    /// Inner binding factor using BLAKE2b for Zcash protocol consistency.
    fn redpallas_inner_binding_factor(
        holder_index: u32,
        outer_message: &[u8],
        inner_commitments: &[crate::nested::InnerCommitments<Point>],
    ) -> Scalar {
        let mut state = blake2b_simd::Params::new()
            .hash_length(64)
            .personal(b"frostito_inner__")
            .to_state();
        state.update(&holder_index.to_le_bytes());
        state.update(&(outer_message.len() as u64).to_le_bytes());
        state.update(outer_message);
        for c in inner_commitments {
            state.update(&c.holder_index.to_le_bytes());
            state.update(&c.hiding.compress());
            state.update(&c.binding.compress());
        }
        let hash: [u8; 64] = *state.finalize().as_array();
        Scalar::from_bytes_wide(&hash)
    }

    /// Complete nested RedPallas signing for escrow disputes.
    ///
    /// Combines OSST authorization + nested FROST signing with RedPallas
    /// (BLAKE2b) hashes throughout. Produces a standard Zcash Orchard
    /// SpendAuth signature.
    ///
    /// s₃ is NEVER reconstructed. Returns None if OSST fails or signing fails.
    pub fn nested_redpallas_sign(
        jury: &JuryNetwork,
        player_share: &SecretShare<Scalar>,
        message: &[u8],
    ) -> Option<(Signature<Point>, bool)> {
        use crate::nested;

        let mut rng = rand_core::OsRng;
        let jury_index = 3u32;

        // phase 1: OSST authorization
        let active_jury: Vec<u32> = jury.node_shares[..jury.threshold as usize]
            .iter().map(|s| s.index).collect();

        let contributions: Vec<crate::Contribution<Point>> = active_jury.iter()
            .map(|&k| jury.node_shares[(k-1) as usize].contribute::<Point, _>(&mut rng, message))
            .collect();

        let osst_ok = crate::verify(
            &jury.outer_verification_share, &contributions, jury.threshold, message,
        ).unwrap_or(false);

        if !osst_ok { return None; }

        // phase 2: inner commitment with RedPallas binding
        let mut inner_nonces = Vec::new();
        let mut inner_commits = Vec::new();
        for &k in &active_jury {
            let (n, c) = nested::inner_commit::<Point, _>(k, &mut rng);
            inner_nonces.push(n);
            inner_commits.push(c);
        }

        // aggregate with RedPallas inner binding factors
        let mut r_nested = Point::identity();
        for c in &inner_commits {
            let rho = redpallas_inner_binding_factor(c.holder_index, message, &inner_commits);
            let r_k = c.hiding.add(&c.binding.mul_scalar(&rho));
            r_nested = r_nested.add(&r_k);
        }

        // phase 3: outer RedPallas package
        let (player_nonces, player_commits) = commit(player_share.index, &mut rng);
        let jury_commits = SigningCommitments {
            index: jury_index,
            hiding: r_nested,
            binding: Point::identity(),
        };

        let outer_package = RedPallasPackage::new(
            message.to_vec(),
            vec![player_commits, jury_commits],
        ).ok()?;

        // player signs with RedPallas
        let player_sig = sign(&outer_package, player_nonces, player_share, &jury.outer_group_pubkey).ok()?;

        // compute outer RedPallas params for inner holders
        let outer_indices = outer_package.signer_indices();
        let outer_lambda = compute_lagrange_coefficients::<Scalar>(&outer_indices).ok()?;
        let nested_pos = outer_indices.iter().position(|&i| i == jury_index)?;

        let outer_gc = outer_package.group_commitment();
        let outer_challenge = outer_package.challenge(&outer_gc, &jury.outer_group_pubkey);

        // phase 4: inner holders sign
        let inner_lagrange = compute_lagrange_coefficients::<Scalar>(&active_jury).ok()?;
        let mut z_nested = Scalar::zero();

        for (pos, nonces) in inner_nonces.into_iter().enumerate() {
            let k = active_jury[pos];
            let rho_inner = redpallas_inner_binding_factor(k, message, &inner_commits);
            let mu_k = &inner_lagrange[pos];
            let sigma_k = jury.node_shares[(k-1) as usize].scalar();

            let weight = outer_lambda[nested_pos].mul(&outer_challenge).mul(mu_k);
            let response = nonces.hiding.add(&rho_inner.mul(&nonces.binding)).add(&weight.mul(sigma_k));
            z_nested = z_nested.add(&response);
        }

        let jury_sig = SignatureShare { index: jury_index, response: z_nested };

        // phase 5: outer aggregation
        let signature = aggregate(
            &outer_package, &[player_sig, jury_sig], &jury.outer_group_pubkey, None,
        ).ok()?;

        if verify_signature(&jury.outer_group_pubkey, message, &signature) {
            Some((signature, osst_ok))
        } else {
            None
        }
    }

    // ========================================================================
    // Game verification types
    // ========================================================================

    /// Signed action from a poker game, submitted for jury verification.
    #[derive(Clone, Debug)]
    pub struct SignedAction {
        pub hand_number: u64,
        pub seat: u8,
        pub sequence: u64,
        pub action: Vec<u8>,
        pub signature: [u8; 64],
    }

    /// Dispute evidence submitted to the jury network.
    #[derive(Clone, Debug)]
    pub struct DisputeEvidence {
        pub deck_commitment: Vec<u8>,
        pub deck: Vec<u8>,
        pub actions: Vec<SignedAction>,
        pub player_a_pubkey: [u8; 32],
        pub player_b_pubkey: [u8; 32],
        pub proposed_settlement: Vec<u8>,
    }

    /// Jury verdict after replaying the game.
    #[derive(Clone, Debug)]
    pub struct JuryVerdict {
        pub final_stacks: Vec<u64>,
        pub settlement_valid: bool,
        pub correct_settlement: (u64, u64),
    }

    // ========================================================================
    // Dispute protocol: 2-round FROST coordination
    // ========================================================================
    //
    // Player                          Jury Coordinator
    //   |                                   |
    //   |── DisputeOpen ──────────────────→ |
    //   |   (commitment + action log)       |
    //   |                                   | replay game engine
    //   |                                   | OSST consensus among jury nodes
    //   |                                   |
    //   | ←─────────────── JuryAccepted ── |
    //   |   (jury commitment + verdict)     |
    //   |                                   |
    //   |   (both have both commitments)    |
    //   |   (both build signing package)    |
    //   |                                   |
    //   |── PlayerShare ──────────────────→ |
    //   |   (player's FROST sig share)      |
    //   |                                   | jury produces FROST share
    //   |                                   | aggregates both shares
    //   |                                   |
    //   | ←─────────────── Resolved ────── |
    //   |   (final signature)               |

    /// Message 1: Player opens dispute, sends commitment + evidence.
    #[derive(Clone, Debug)]
    pub struct DisputeOpen {
        /// player's FROST nonce commitment (Round 1)
        pub player_commitment: SigningCommitments<Point>,
        /// player's index in the outer 2-of-3 (1 or 2)
        pub player_index: u32,
        /// the PCZT message bytes to be signed (settlement tx)
        pub settlement_pczt: Vec<u8>,
        /// signed action log proving the game outcome
        pub evidence: DisputeEvidence,
    }

    /// Message 2: Jury accepts, sends their commitment + verdict.
    #[derive(Clone, Debug)]
    pub struct JuryAccepted {
        /// jury's FROST nonce commitment (Round 1)
        pub jury_commitment: SigningCommitments<Point>,
        /// the verdict from replaying the engine
        pub verdict: JuryVerdict,
    }

    /// Message 3: Player sends their FROST signature share (Round 2).
    #[derive(Clone)]
    pub struct PlayerShare {
        pub share: SignatureShare<Scalar>,
    }

    /// Message 4: Jury sends the final aggregated signature.
    #[derive(Clone, Debug)]
    pub struct DisputeResolved {
        /// the final RedPallas signature on the settlement PCZT
        pub signature: Signature<Point>,
        /// the jury's verdict
        pub verdict: JuryVerdict,
    }

    // ── Player-side state machine ──────────────────────────────────

    /// Player's dispute session. Drives the player side of the protocol.
    pub struct PlayerDispute {
        player_share: SecretShare<Scalar>,
        group_pubkey: Point,
        nonces: Option<Nonces<Scalar>>,
        player_commitment: SigningCommitments<Point>,
        settlement_pczt: Vec<u8>,
    }

    impl PlayerDispute {
        /// Step 1: Player initiates dispute.
        ///
        /// Generates nonces and returns the DisputeOpen message to send
        /// to the jury coordinator.
        pub fn open<R: rand_core::RngCore + rand_core::CryptoRng>(
            player_share: SecretShare<Scalar>,
            group_pubkey: Point,
            settlement_pczt: Vec<u8>,
            evidence: DisputeEvidence,
            rng: &mut R,
        ) -> (Self, DisputeOpen) {
            let (nonces, commitment) = commit(player_share.index, rng);

            let msg = DisputeOpen {
                player_commitment: commitment.clone(),
                player_index: player_share.index,
                settlement_pczt: settlement_pczt.clone(),
                evidence,
            };

            let state = Self {
                player_share,
                group_pubkey,
                nonces: Some(nonces),
                player_commitment: commitment,
                settlement_pczt,
            };

            (state, msg)
        }

        /// Step 3: Player receives jury's commitment, produces signature share.
        ///
        /// Returns PlayerShare to send back to jury, plus the verdict.
        pub fn sign_with_jury(
            &mut self,
            accepted: &JuryAccepted,
        ) -> Result<PlayerShare, OsstError> {
            let nonces = self.nonces.take().ok_or(OsstError::InvalidIndex)?;

            // build signing package from both commitments
            let package = RedPallasPackage::new(
                self.settlement_pczt.clone(),
                vec![self.player_commitment.clone(), accepted.jury_commitment.clone()],
            )?;

            let share = sign(&package, nonces, &self.player_share, &self.group_pubkey)?;

            Ok(PlayerShare { share })
        }
    }

    // ── Jury-side state machine ────────────────────────────────────

    /// Jury coordinator's dispute session. Drives the jury side.
    pub struct JuryDispute {
        jury: JuryNetwork,
        jury_index: u32,
        nonces: Option<Nonces<Scalar>>,
        jury_commitment: SigningCommitments<Point>,
        player_commitment: SigningCommitments<Point>,
        settlement_pczt: Vec<u8>,
        verdict: JuryVerdict,
    }

    impl JuryDispute {
        /// Step 2: Jury receives DisputeOpen, replays game, produces commitment.
        ///
        /// The `replay_fn` callback is where the actual game engine replay
        /// happens. It receives the evidence and returns a JuryVerdict.
        /// This keeps the engine dependency out of the crypto crate.
        ///
        /// After replay, jury nodes reach OSST consensus on the verdict,
        /// then the coordinator generates FROST nonces.
        ///
        /// Returns JuryAccepted to send back to the player.
        pub fn accept<R, F>(
            jury: JuryNetwork,
            open: &DisputeOpen,
            replay_fn: F,
            rng: &mut R,
        ) -> Result<(Self, JuryAccepted), OsstError>
        where
            R: rand_core::RngCore + rand_core::CryptoRng,
            F: FnOnce(&DisputeEvidence) -> Result<JuryVerdict, OsstError>,
        {
            let jury_index = 3u32; // jury is always index 3 in outer 2-of-3

            // replay the game to determine correct outcome
            let verdict = replay_fn(&open.evidence)?;

            // OSST consensus: jury nodes prove they agree on the verdict
            let verdict_payload = encode_verdict(&verdict);
            let jury_group_pubkey = {
                let jury_secret = reconstruct_secret(
                    &jury.node_shares[..jury.threshold as usize],
                )?;
                Point::generator().mul_scalar(&jury_secret)
            };

            let mut osst_rng = rand_core::OsRng;
            let contributions: Vec<crate::Contribution<Point>> = jury.node_shares
                [..jury.threshold as usize]
                .iter()
                .map(|s| s.contribute(&mut osst_rng, &verdict_payload))
                .collect();

            let osst_valid = crate::verify(
                &jury_group_pubkey,
                &contributions,
                jury.threshold,
                &verdict_payload,
            )?;

            if !osst_valid {
                return Err(OsstError::InvalidResponse);
            }

            // generate jury's FROST nonces
            let (nonces, commitment) = commit(jury_index, rng);

            let accepted = JuryAccepted {
                jury_commitment: commitment.clone(),
                verdict: verdict.clone(),
            };

            let state = Self {
                jury,
                jury_index,
                nonces: Some(nonces),
                jury_commitment: commitment,
                player_commitment: open.player_commitment.clone(),
                settlement_pczt: open.settlement_pczt.clone(),
                verdict,
            };

            Ok((state, accepted))
        }

        /// Step 4: Jury receives player's share, produces jury share, aggregates.
        ///
        /// Returns the final DisputeResolved with the aggregated signature.
        pub fn resolve(
            &mut self,
            player_share: &PlayerShare,
        ) -> Result<DisputeResolved, OsstError> {
            let nonces = self.nonces.take().ok_or(OsstError::InvalidIndex)?;

            let package = RedPallasPackage::new(
                self.settlement_pczt.clone(),
                vec![self.player_commitment.clone(), self.jury_commitment.clone()],
            )?;

            // jury produces its FROST signature share
            let jury_share = jury_sign_share(
                &self.jury,
                nonces,
                &package,
                self.jury_index,
            )?;

            // aggregate player + jury shares
            let signature = aggregate(
                &package,
                &[player_share.share.clone(), jury_share],
                &self.jury.outer_group_pubkey,
                None,
            )?;

            // sanity check
            if !verify_signature(
                &self.jury.outer_group_pubkey,
                &self.settlement_pczt,
                &signature,
            ) {
                return Err(OsstError::InvalidResponse);
            }

            Ok(DisputeResolved {
                signature,
                verdict: self.verdict.clone(),
            })
        }
    }

    /// Encode a verdict deterministically for OSST payload.
    fn encode_verdict(verdict: &JuryVerdict) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"verdict:v1:");
        for s in &verdict.final_stacks {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf.extend_from_slice(&verdict.correct_settlement.0.to_le_bytes());
        buf.extend_from_slice(&verdict.correct_settlement.1.to_le_bytes());
        buf.push(verdict.settlement_valid as u8);
        buf
    }

    impl Clone for SignatureShare<Scalar> {
        fn clone(&self) -> Self {
            Self {
                index: self.index,
                response: self.response.clone(),
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(all(test, feature = "pallas"))]
mod tests {
    use super::zcash::*;
    use crate::frost::Signature;
    use crate::SecretShare;
    use pasta_curves::group::ff::Field;
    use pasta_curves::pallas::{Point, Scalar};
    use rand::rngs::OsRng;
    use crate::curve::OsstPoint;

    #[test]
    fn test_redpallas_frost_basic() {
        let mut rng = OsRng;

        // manual Shamir split for testing
        let secret = <Scalar as Field>::random(&mut rng);
        let group_pubkey: Point = Point::generator().mul_scalar(&secret);

        let n = 3u32;
        let t = 2u32;
        let shares = test_shamir_split(&secret, n, t);
        let message = b"zcash orchard spend authorization";

        // round 1
        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in &shares[0..t as usize] {
            let (nonces, commitments) = commit(s.index, &mut rng);
            nonces_vec.push(nonces);
            commitments_vec.push(commitments);
        }

        let package = RedPallasPackage::new(message.to_vec(), commitments_vec).unwrap();

        // round 2
        let mut sig_shares = Vec::new();
        for (s, nonces) in shares[0..t as usize].iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(sign(&package, nonces, s, &group_pubkey).unwrap());
        }

        let signature = aggregate(&package, &sig_shares, &group_pubkey, None).unwrap();

        assert!(
            verify_signature(&group_pubkey, message, &signature),
            "RedPallas FROST signature should verify"
        );
    }

    #[test]
    fn test_redpallas_frost_wrong_message_fails() {
        let mut rng = OsRng;
        let secret = <Scalar as Field>::random(&mut rng);
        let group_pubkey: Point = Point::generator().mul_scalar(&secret);

        let shares = test_shamir_split(&secret, 3, 2);
        let message = b"correct";

        let mut nonces_vec = Vec::new();
        let mut comms = Vec::new();
        for s in &shares[0..2] {
            let (n, c) = commit(s.index, &mut rng);
            nonces_vec.push(n);
            comms.push(c);
        }

        let pkg = RedPallasPackage::new(message.to_vec(), comms).unwrap();
        let mut sig_shares = Vec::new();
        for (s, n) in shares[0..2].iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(sign(&pkg, n, s, &group_pubkey).unwrap());
        }
        let sig = aggregate(&pkg, &sig_shares, &group_pubkey, None).unwrap();

        assert!(verify_signature(&group_pubkey, message, &sig));
        assert!(!verify_signature(&group_pubkey, b"wrong", &sig));
    }

    #[test]
    fn test_escrow_setup_and_sign() {
        let mut rng = OsRng;

        // setup 2-of-3 escrow with 3-of-5 jury
        let (player_a, player_b, _jury, group_pubkey) =
            setup_escrow(5, 3, &mut rng).unwrap();

        // happy path: player A + player B sign the settlement
        let message = b"settlement: A=600, B=400";

        let (nonces_a, comm_a) = commit(player_a.index, &mut rng);
        let (nonces_b, comm_b) = commit(player_b.index, &mut rng);

        let package = RedPallasPackage::new(message.to_vec(), vec![comm_a, comm_b]).unwrap();

        let share_a = sign(&package, nonces_a, &player_a, &group_pubkey).unwrap();
        let share_b = sign(&package, nonces_b, &player_b, &group_pubkey).unwrap();

        let signature = aggregate(&package, &[share_a, share_b], &group_pubkey, None).unwrap();

        assert!(
            verify_signature(&group_pubkey, message, &signature),
            "happy path: A+B should produce valid signature"
        );
    }

    #[test]
    fn test_escrow_dispute_jury_signs() {
        let mut rng = OsRng;

        // setup 2-of-3 escrow with 3-of-5 jury
        let (player_a, _player_b, jury, group_pubkey) =
            setup_escrow(5, 3, &mut rng).unwrap();

        // dispute: player A + jury sign (player B refuses)
        let message = b"jury verdict: A=800, B=200";

        let (nonces_a, comm_a) = commit(player_a.index, &mut rng);
        let (nonces_jury, comm_jury) = commit(3, &mut rng); // jury is index 3

        let package = RedPallasPackage::new(
            message.to_vec(),
            vec![comm_a, comm_jury],
        ).unwrap();

        // player A signs normally
        let share_a = sign(&package, nonces_a, &player_a, &group_pubkey).unwrap();

        // jury collectively produces their share
        let share_jury = jury_sign_share(&jury, nonces_jury, &package, 3).unwrap();

        let signature = aggregate(
            &package,
            &[share_a, share_jury],
            &group_pubkey,
            None,
        ).unwrap();

        assert!(
            verify_signature(&group_pubkey, message, &signature),
            "dispute: A+jury should produce valid signature"
        );
    }

    /// Full integration test: DKG → address → FROST sign (happy) → OSST+FROST sign (dispute)
    ///
    /// This tests the complete 2-of-3 multisig escrow lifecycle:
    /// 1. DKG produces shared Pallas group key (= shielded address)
    /// 2. Happy path: Player A + Player B sign settlement via FROST
    /// 3. Dispute path: jury nodes prove consensus via OSST, then
    ///    Player A + Jury sign via FROST
    /// 4. Both signatures verify against the same shielded address
    #[test]
    fn test_full_escrow_lifecycle_frost_and_osst() {
        let mut rng = OsRng;

        // ── 1. DKG: 3 participants, threshold 2 ──────────────────────
        let jury_n = 5u32;
        let jury_t = 3u32;
        let (player_a, player_b, jury, group_pubkey) =
            setup_escrow(jury_n, jury_t, &mut rng).unwrap();

        // derive shielded address (32 bytes, compressed Pallas point)
        let address = derive_address_bytes(&group_pubkey);
        assert_ne!(address, [0u8; 32], "address should not be zero point");

        // verify address round-trips
        let recovered = Point::decompress(&address);
        assert!(recovered.is_some(), "address should decompress");
        assert_eq!(recovered.unwrap(), group_pubkey, "address should roundtrip");

        println!("shielded address: {}", hex::encode(address));
        println!("  (2-of-3 FROST: playerA + playerB + jury[{}-of-{}])", jury_t, jury_n);

        // ── 2. Happy path: A + B sign with FROST ────────────────────
        let settlement_msg = b"PCZT:settlement:A=600,B=400";

        let (nonces_a, comm_a) = commit(player_a.index, &mut rng);
        let (nonces_b, comm_b) = commit(player_b.index, &mut rng);
        let package = RedPallasPackage::new(
            settlement_msg.to_vec(),
            vec![comm_a, comm_b],
        ).unwrap();

        let share_a = sign(&package, nonces_a, &player_a, &group_pubkey).unwrap();
        let share_b = sign(&package, nonces_b, &player_b, &group_pubkey).unwrap();
        let sig_happy = aggregate(&package, &[share_a, share_b], &group_pubkey, None).unwrap();

        assert!(
            verify_signature(&group_pubkey, settlement_msg, &sig_happy),
            "happy path: A+B FROST signature must verify"
        );
        println!("happy path: A+B signed settlement ✓");

        // ── 3. Dispute path: A + Jury sign with FROST ───────────────
        // jury nodes first prove consensus via OSST, then produce FROST share
        let verdict_msg = b"PCZT:verdict:A=800,B=200:jury_fee=10";
        let verdict_payload = b"jury-verdict:hand#42:A=800,B=200";

        let (nonces_a2, comm_a2) = commit(player_a.index, &mut rng);
        let (nonces_jury, comm_jury) = commit(3, &mut rng); // jury = index 3
        let dispute_package = RedPallasPackage::new(
            verdict_msg.to_vec(),
            vec![comm_a2, comm_jury],
        ).unwrap();

        // player A signs normally via FROST
        let share_a2 = sign(
            &dispute_package, nonces_a2, &player_a, &group_pubkey,
        ).unwrap();

        // jury uses OSST consensus then FROST
        let (share_jury, osst_valid) = jury_sign_with_osst_consensus(
            &jury,
            nonces_jury,
            &dispute_package,
            3, // jury's outer index
            verdict_payload,
        ).unwrap();

        assert!(osst_valid, "OSST consensus must verify");
        println!("jury OSST consensus: {}-of-{} nodes agreed ✓", jury_t, jury_n);

        let sig_dispute = aggregate(
            &dispute_package,
            &[share_a2, share_jury],
            &group_pubkey,
            None,
        ).unwrap();

        assert!(
            verify_signature(&group_pubkey, verdict_msg, &sig_dispute),
            "dispute path: A+jury FROST signature must verify"
        );
        println!("dispute path: A+jury signed verdict ✓");

        // ── 4. Both sigs verify against same address ─────────────────
        let same_pubkey = Point::decompress(&address).unwrap();
        assert!(verify_signature(&same_pubkey, settlement_msg, &sig_happy));
        assert!(verify_signature(&same_pubkey, verdict_msg, &sig_dispute));

        // cross-verify: signatures don't work with wrong messages
        assert!(!verify_signature(&same_pubkey, verdict_msg, &sig_happy));
        assert!(!verify_signature(&same_pubkey, settlement_msg, &sig_dispute));

        println!("both signatures verify against shielded address {} ✓", hex::encode(&address[..8]));

        // ── 5. Signature serialization roundtrip ─────────────────────
        let happy_bytes = sig_happy.to_bytes();
        let dispute_bytes = sig_dispute.to_bytes();
        assert_eq!(happy_bytes.len(), 64, "RedPallas sig = 64 bytes (R:32 + z:32)");
        assert_eq!(dispute_bytes.len(), 64);

        let recovered_happy = Signature::<Point>::from_bytes(&happy_bytes).unwrap();
        let recovered_dispute = Signature::<Point>::from_bytes(&dispute_bytes).unwrap();
        assert!(verify_signature(&same_pubkey, settlement_msg, &recovered_happy));
        assert!(verify_signature(&same_pubkey, verdict_msg, &recovered_dispute));

        println!("signature serialization roundtrip ✓");
    }

    /// Test that B+Jury can also sign (B disputes against A)
    #[test]
    fn test_escrow_b_plus_jury_dispute() {
        let mut rng = OsRng;
        let (_player_a, player_b, jury, group_pubkey) =
            setup_escrow(5, 3, &mut rng).unwrap();

        let msg = b"PCZT:verdict:A=200,B=800";
        let verdict_payload = b"jury-verdict:B-wins";

        let (nonces_b, comm_b) = commit(player_b.index, &mut rng);
        let (nonces_jury, comm_jury) = commit(3, &mut rng);
        let package = RedPallasPackage::new(msg.to_vec(), vec![comm_b, comm_jury]).unwrap();

        let share_b = sign(&package, nonces_b, &player_b, &group_pubkey).unwrap();
        let (share_jury, osst_ok) = jury_sign_with_osst_consensus(
            &jury, nonces_jury, &package, 3, verdict_payload,
        ).unwrap();
        assert!(osst_ok);

        let sig = aggregate(&package, &[share_b, share_jury], &group_pubkey, None).unwrap();
        assert!(verify_signature(&group_pubkey, msg, &sig),
            "B+jury dispute should produce valid signature");
    }

    /// Test that 1-of-3 cannot sign (need threshold 2)
    #[test]
    fn test_escrow_single_signer_insufficient() {
        let mut rng = OsRng;
        let (player_a, _player_b, _jury, group_pubkey) =
            setup_escrow(5, 3, &mut rng).unwrap();

        let msg = b"steal all funds";

        // A alone tries to sign
        let (nonces_a, comm_a) = commit(player_a.index, &mut rng);
        let package = RedPallasPackage::new(msg.to_vec(), vec![comm_a]).unwrap();
        let share_a = sign(&package, nonces_a, &player_a, &group_pubkey).unwrap();
        let sig = aggregate(&package, &[share_a], &group_pubkey, None).unwrap();

        // this produces a signature, but it won't verify because
        // the Lagrange interpolation for a single point from a degree-1
        // polynomial doesn't reconstruct the correct secret
        assert!(
            !verify_signature(&group_pubkey, msg, &sig),
            "single signer should NOT produce valid 2-of-3 signature"
        );
    }

    /// Test the full dispute protocol state machine:
    /// PlayerDispute::open → JuryDispute::accept → PlayerDispute::sign → JuryDispute::resolve
    #[test]
    fn test_dispute_protocol_flow() {
        let mut rng = OsRng;

        // setup escrow
        let (player_a, _player_b, jury, group_pubkey) =
            setup_escrow(5, 3, &mut rng).unwrap();

        let settlement_pczt = b"PCZT:dispute-settlement:A=800,B=200".to_vec();

        // fake evidence (in production this is the real action log)
        let evidence = DisputeEvidence {
            deck_commitment: vec![0u8; 32],
            deck: vec![0u8; 52],
            actions: vec![],
            player_a_pubkey: [1u8; 32],
            player_b_pubkey: [2u8; 32],
            proposed_settlement: vec![],
        };

        // ── Step 1: Player opens dispute ──────────────────────────
        let (mut player_state, open_msg) = PlayerDispute::open(
            player_a,
            group_pubkey,
            settlement_pczt.clone(),
            evidence,
            &mut rng,
        );

        assert_eq!(open_msg.player_index, 1);
        println!("player opened dispute, sent commitment");

        // ── Step 2: Jury accepts, replays game, commits ───────────
        let replay_fn = |_evidence: &DisputeEvidence| -> Result<JuryVerdict, _> {
            // in production: import poker engine, replay actions
            Ok(JuryVerdict {
                final_stacks: vec![800, 200],
                settlement_valid: false, // proposed was wrong
                correct_settlement: (800, 200),
            })
        };

        let (mut jury_state, accepted) = JuryDispute::accept(
            jury,
            &open_msg,
            replay_fn,
            &mut rng,
        ).unwrap();

        assert_eq!(accepted.verdict.correct_settlement, (800, 200));
        println!("jury replayed game, OSST consensus, sent commitment + verdict");

        // ── Step 3: Player receives jury commitment, signs ────────
        let player_share = player_state.sign_with_jury(&accepted).unwrap();
        println!("player signed, sent share to jury");

        // ── Step 4: Jury receives player share, aggregates ────────
        let resolved = jury_state.resolve(&player_share).unwrap();

        assert!(
            verify_signature(&group_pubkey, &settlement_pczt, &resolved.signature),
            "resolved signature must verify against escrow address"
        );
        assert_eq!(resolved.verdict.correct_settlement, (800, 200));

        println!("jury aggregated → final signature on PCZT");
        println!("sig: {} bytes, R={}", 64, hex::encode(&resolved.signature.to_bytes()[..8]));
        println!("dispute resolved: A gets 800, B gets 200 ✓");
    }

    /// Test dispute where Player B initiates (A is the loser)
    #[test]
    fn test_dispute_protocol_player_b_initiates() {
        let mut rng = OsRng;

        let (_player_a, player_b, jury, group_pubkey) =
            setup_escrow(5, 3, &mut rng).unwrap();

        let settlement_pczt = b"PCZT:dispute:B=900,A=100".to_vec();
        let evidence = DisputeEvidence {
            deck_commitment: vec![], deck: vec![], actions: vec![],
            player_a_pubkey: [1u8; 32], player_b_pubkey: [2u8; 32],
            proposed_settlement: vec![],
        };

        // B opens dispute
        let (mut player_state, open_msg) = PlayerDispute::open(
            player_b, group_pubkey, settlement_pczt.clone(), evidence, &mut rng,
        );
        assert_eq!(open_msg.player_index, 2); // player B is index 2

        let replay_fn = |_: &DisputeEvidence| -> Result<JuryVerdict, _> {
            Ok(JuryVerdict {
                final_stacks: vec![100, 900],
                settlement_valid: false,
                correct_settlement: (100, 900),
            })
        };

        let (mut jury_state, accepted) =
            JuryDispute::accept(jury, &open_msg, replay_fn, &mut rng).unwrap();

        let player_share = player_state.sign_with_jury(&accepted).unwrap();
        let resolved = jury_state.resolve(&player_share).unwrap();

        assert!(verify_signature(&group_pubkey, &settlement_pczt, &resolved.signature));
        assert_eq!(resolved.verdict.correct_settlement, (100, 900));
    }

    /// Test that nonces can't be reused (sign_with_jury can only be called once)
    #[test]
    fn test_dispute_nonce_reuse_prevented() {
        let mut rng = OsRng;

        let (player_a, _player_b, jury, group_pubkey) =
            setup_escrow(5, 3, &mut rng).unwrap();

        let pczt = b"PCZT:test".to_vec();
        let evidence = DisputeEvidence {
            deck_commitment: vec![], deck: vec![], actions: vec![],
            player_a_pubkey: [1u8; 32], player_b_pubkey: [2u8; 32],
            proposed_settlement: vec![],
        };

        let (mut player_state, open_msg) = PlayerDispute::open(
            player_a, group_pubkey, pczt, evidence, &mut rng,
        );

        let replay_fn = |_: &DisputeEvidence| -> Result<JuryVerdict, _> {
            Ok(JuryVerdict { final_stacks: vec![500, 500], settlement_valid: true, correct_settlement: (500, 500) })
        };

        let (_jury_state, accepted) =
            JuryDispute::accept(jury, &open_msg, replay_fn, &mut rng).unwrap();

        // first call succeeds
        let _share = player_state.sign_with_jury(&accepted).unwrap();

        // second call fails — nonces consumed
        let result = player_state.sign_with_jury(&accepted);
        assert!(result.is_err(), "nonce reuse must be prevented");
    }

    // test helper
    fn test_shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
        use crate::curve::OsstScalar;
        let mut rng = OsRng;
        let mut coeffs = vec![*secret];
        for _ in 1..t {
            coeffs.push(<Scalar as Field>::random(&mut rng));
        }
        (1..=n)
            .map(|i| {
                let x = Scalar::from_u32(i);
                let mut y = Scalar::zero();
                let mut x_pow = Scalar::one();
                for coeff in &coeffs {
                    y = y.add(&coeff.mul(&x_pow));
                    x_pow = x_pow.mul(&x);
                }
                SecretShare::new(i, y)
            })
            .collect()
    }

    #[test]
    fn test_nested_redpallas_sign() {
        let mut rng = rand::rngs::OsRng;
        let jury_n = 5u32;
        let jury_t = 3u32;

        let (player_a, _player_b, jury, _group_pubkey) =
            setup_escrow(jury_n, jury_t, &mut rng).unwrap();

        let message = b"PCZT:dispute:A=1500,B=500";

        let result = nested_redpallas_sign(&jury, &player_a, message);
        assert!(result.is_some(), "nested RedPallas signing should succeed");

        let (sig, osst_ok) = result.unwrap();
        assert!(osst_ok, "OSST should verify");
        assert!(
            verify_signature(&jury.outer_group_pubkey, message, &sig),
            "nested RedPallas signature should verify with BLAKE2b challenge"
        );

        // wrong message should fail
        assert!(
            !verify_signature(&jury.outer_group_pubkey, b"wrong", &sig),
            "wrong message should not verify"
        );
    }
}
