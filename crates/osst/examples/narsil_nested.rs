//! narsil nested: true interleaved DKG where s₃ never exists
//!
//! The escrow share is born distributed via two inner DKGs
//! (one for each coefficient of the outer polynomial). Nobody
//! ever holds s₃ as a scalar. The inner holders produce the
//! outer FROST partial signature collectively.
//!
//! Flow:
//!   1. Inner DKG × 2: produce Shamir shares of f₃'s coefficients
//!   2. Outer DKG interleaved: buyer/seller Shamir-split their
//!      evaluations among inner holders
//!   3. OSST authorization: async proof of escrow control
//!   4. Inner holders produce outer FROST nonces collectively
//!   5. Inner holders produce outer FROST partial signature z₃
//!   6. Outer aggregation: standard Schnorr signature
//!
//! Run: cargo run --example narsil_nested --features ristretto255

use std::collections::BTreeMap;

use osst::curve::{OsstPoint, OsstScalar};
use osst::dkg;
use osst::frost;
use osst::reshare::DealerCommitment;
use osst::SecretShare;

use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use rand::rngs::OsRng;

type Point = RistrettoPoint;

/// Shamir-split a scalar into n shares with threshold t.
/// Returns (shares, feldman_commitment_point) for verification.
fn shamir_split_scalar(
    secret: &Scalar,
    n: u32,
    t: u32,
    rng: &mut (impl rand_core::CryptoRng + rand_core::RngCore),
) -> Vec<(u32, Scalar)> {
    let mut coeffs = vec![*secret];
    for _ in 1..t {
        coeffs.push(Scalar::random(rng));
    }
    (1..=n)
        .map(|i| {
            let x = Scalar::from(i);
            let mut y = Scalar::ZERO;
            let mut x_pow = Scalar::ONE;
            for coeff in &coeffs {
                y += coeff * x_pow;
                x_pow *= x;
            }
            (i, y)
        })
        .collect()
}

fn main() {
    let mut rng = OsRng;

    println!("=== narsil nested: interleaved DKG, s₃ never exists ===\n");

    let outer_n = 3u32;
    let outer_t = 2u32;
    let inner_n = 5u32;
    let inner_t = 3u32;

    // ================================================================
    // PHASE 1: Buyer and Seller generate their outer DKG polynomials
    //
    // f₁(x) = a₁ + b₁·x   (buyer, position 1)
    // f₂(x) = a₂ + b₂·x   (seller, position 2)
    // ================================================================

    println!("--- phase 1: buyer and seller generate outer polynomials ---");

    let buyer_dealer: dkg::Dealer<Point> = dkg::Dealer::new(1, outer_t, &mut rng);
    let seller_dealer: dkg::Dealer<Point> = dkg::Dealer::new(2, outer_t, &mut rng);

    println!("  buyer  (position 1): polynomial ready");
    println!("  seller (position 2): polynomial ready\n");

    // ================================================================
    // PHASE 2: Inner DKGs — collectively generate f₃'s coefficients
    //
    // The inner group generates f₃(x) = a₃ + b₃·x without anyone
    // knowing a₃ or b₃. Two separate inner DKGs, one per coefficient.
    //
    // After this phase, each inner holder k has:
    //   αₖ = Shamir share of a₃  (from inner DKG #1)
    //   βₖ = Shamir share of b₃  (from inner DKG #2)
    // ================================================================

    println!("--- phase 2: inner DKGs for f₃ coefficients ---");

    // Inner DKG #1: shares of a₃ (constant term)
    let dealers_a: Vec<dkg::Dealer<Point>> = (1..=inner_n)
        .map(|k| dkg::Dealer::new(k, inner_t, &mut rng))
        .collect();
    let commitments_a: Vec<&DealerCommitment<Point>> =
        dealers_a.iter().map(|d| d.commitment()).collect();

    // Inner DKG #2: shares of b₃ (linear term)
    let dealers_b: Vec<dkg::Dealer<Point>> = (1..=inner_n)
        .map(|k| dkg::Dealer::new(k, inner_t, &mut rng))
        .collect();
    let commitments_b: Vec<&DealerCommitment<Point>> =
        dealers_b.iter().map(|d| d.commitment()).collect();

    // Each inner holder aggregates both DKGs
    let mut alpha: Vec<Scalar> = Vec::new(); // αₖ for k=1..5
    let mut beta: Vec<Scalar> = Vec::new();  // βₖ for k=1..5
    let mut a3_pubkey = Point::identity();
    let mut b3_pubkey = Point::identity();

    for k in 1..=inner_n {
        let mut agg_a: dkg::Aggregator<Point> = dkg::Aggregator::new(k);
        let mut agg_b: dkg::Aggregator<Point> = dkg::Aggregator::new(k);
        for dealer in &dealers_a {
            let sub = dealer.generate_subshare(k);
            agg_a.add_subshare(sub, commitments_a[(dealer.index() - 1) as usize]).unwrap();
        }
        for dealer in &dealers_b {
            let sub = dealer.generate_subshare(k);
            agg_b.add_subshare(sub, commitments_b[(dealer.index() - 1) as usize]).unwrap();
        }
        if k == 1 {
            a3_pubkey = agg_a.derive_group_key(); // g^{a₃}
            b3_pubkey = agg_b.derive_group_key(); // g^{b₃}
        }
        alpha.push(agg_a.finalize(inner_n).unwrap());
        beta.push(agg_b.finalize(inner_n).unwrap());
    }

    println!("  inner DKG #1 (a₃): {} holders, threshold {}", inner_n, inner_t);
    println!("  inner DKG #2 (b₃): {} holders, threshold {}", inner_n, inner_t);
    println!("  g^a₃ = {}", &hex::encode(OsstPoint::compress(&a3_pubkey))[..16]);
    println!("  nobody knows a₃ or b₃\n");

    // ================================================================
    // PHASE 3: Interleaved outer DKG
    //
    // 3a. Inner group sends f₃(1) to buyer and f₃(2) to seller.
    //     Each holder k computes αₖ + j·βₖ and sends to participant j.
    //     Participant j sums (using Lagrange) to get f₃(j).
    //
    // 3b. Buyer Shamir-splits f₁(3) among inner holders.
    //     Seller Shamir-splits f₂(3) among inner holders.
    //
    // 3c. Each inner holder k computes final share:
    //     σₖ = (αₖ + 3·βₖ) + π₁,ₖ + π₂,ₖ
    // ================================================================

    println!("--- phase 3: interleaved outer DKG ---");

    // 3a: compute f₃(j) for buyer (j=1) and seller (j=2)
    // Using inner holders 1,2,3 (any t=3 of 5)
    let active_for_eval = [0usize, 1, 2]; // holders 1,2,3
    let active_indices: Vec<u32> = active_for_eval.iter().map(|&i| (i + 1) as u32).collect();
    let lagrange = osst::compute_lagrange_coefficients::<Scalar>(&active_indices).unwrap();

    // f₃(1) = a₃ + 1·b₃, reconstructed via Lagrange from inner shares
    let mut f3_at_1 = Scalar::ZERO;
    for (pos, &i) in active_for_eval.iter().enumerate() {
        let val = alpha[i] + Scalar::from(1u64) * beta[i]; // αₖ + 1·βₖ
        f3_at_1 += lagrange[pos] * val;
    }

    // f₃(2) = a₃ + 2·b₃
    let mut f3_at_2 = Scalar::ZERO;
    for (pos, &i) in active_for_eval.iter().enumerate() {
        let val = alpha[i] + Scalar::from(2u64) * beta[i];
        f3_at_2 += lagrange[pos] * val;
    }

    println!("  f₃(1) sent to buyer  (reconstructed by 3 holders)");
    println!("  f₃(2) sent to seller (reconstructed by 3 holders)");

    // 3b: buyer computes f₁(3) and Shamir-splits among inner holders
    let f1_at_3 = buyer_dealer.generate_subshare(3);
    let f1_at_3_shares = shamir_split_scalar(f1_at_3.value(), inner_n, inner_t, &mut rng);

    // seller computes f₂(3) and Shamir-splits among inner holders
    let f2_at_3 = seller_dealer.generate_subshare(3);
    let f2_at_3_shares = shamir_split_scalar(f2_at_3.value(), inner_n, inner_t, &mut rng);

    println!("  buyer  Shamir-split f₁(3) into {} shares (threshold {})", inner_n, inner_t);
    println!("  seller Shamir-split f₂(3) into {} shares (threshold {})", inner_n, inner_t);

    // 3c: each inner holder computes their final share of s₃
    // σₖ = (αₖ + 3·βₖ) + π₁,ₖ + π₂,ₖ
    let three = Scalar::from(3u64);
    let mut escrow_shares: Vec<SecretShare<Scalar>> = Vec::new();

    for k in 0..inner_n as usize {
        let tau_k = alpha[k] + three * beta[k];  // Shamir share of f₃(3)
        let pi_1_k = f1_at_3_shares[k].1;         // Shamir share of f₁(3)
        let pi_2_k = f2_at_3_shares[k].1;         // Shamir share of f₂(3)
        let sigma_k = tau_k + pi_1_k + pi_2_k;    // Shamir share of s₃
        escrow_shares.push(SecretShare::new((k + 1) as u32, sigma_k));
    }

    println!("  each holder computed σₖ = (αₖ + 3·βₖ) + π₁,ₖ + π₂,ₖ");
    println!("  s₃ never existed as a scalar — born distributed\n");

    // ================================================================
    // Derive keys: buyer's share, seller's share, group key, escrow pubkey
    // ================================================================

    println!("--- key derivation ---");

    // Buyer's outer share: s₁ = f₁(1) + f₂(1) + f₃(1)
    let f1_at_1 = buyer_dealer.generate_subshare(1);
    let f2_at_1 = seller_dealer.generate_subshare(1);
    let s1 = *f1_at_1.value() + *f2_at_1.value() + f3_at_1;

    // Seller's outer share: s₂ = f₁(2) + f₂(2) + f₃(2)
    let f1_at_2 = buyer_dealer.generate_subshare(2);
    let f2_at_2 = seller_dealer.generate_subshare(2);
    let s2 = *f1_at_2.value() + *f2_at_2.value() + f3_at_2;

    let buyer_share = SecretShare::new(1, s1);
    let seller_share = SecretShare::new(2, s2);

    // Group public key: Y = g^{f₁(0)} · g^{f₂(0)} · g^{f₃(0)}
    // g^{f₁(0)} from buyer's commitment, g^{f₂(0)} from seller's, g^{f₃(0)} = g^{a₃}
    let y1 = buyer_dealer.commitment().share_commitment().clone();
    let y2 = seller_dealer.commitment().share_commitment().clone();
    let y3 = a3_pubkey; // g^{a₃} = g^{f₃(0)}
    let group_key = y1.add(&y2).add(&y3);

    // Escrow's public verification share: Y₃ = g^{s₃}
    // We can compute this from outer polynomial commitments evaluated at 3
    let y3_verify = buyer_dealer.commitment().evaluate_at(3)
        .add(&seller_dealer.commitment().evaluate_at(3))
        .add(&a3_pubkey.add(&b3_pubkey.mul_scalar(&three))); // g^{a₃+3·b₃}

    // Outer verification shares for FROST
    let mut outer_vshares: BTreeMap<u32, Point> = BTreeMap::new();
    outer_vshares.insert(1, Point::generator().mul_scalar(buyer_share.scalar()));
    outer_vshares.insert(2, Point::generator().mul_scalar(seller_share.scalar()));
    outer_vshares.insert(3, y3_verify);

    println!("  group key:     {}", &hex::encode(OsstPoint::compress(&group_key))[..32]);
    println!("  buyer  Y₁:     {}", &hex::encode(OsstPoint::compress(&outer_vshares[&1]))[..32]);
    println!("  seller Y₂:     {}", &hex::encode(OsstPoint::compress(&outer_vshares[&2]))[..32]);
    println!("  escrow Y₃:     {}", &hex::encode(OsstPoint::compress(&y3_verify))[..32]);
    println!("  s₃ status:     NEVER EXISTED\n");

    // ================================================================
    // PHASE 4: OSST authorization
    // ================================================================

    println!("--- phase 4: OSST authorization ---");

    let payload = b"authorize dispute tx:deadbeef";

    // Holders 1, 3, 5 authorize
    let osst_active = [0usize, 2, 4];
    let contributions: Vec<osst::Contribution<Point>> = osst_active
        .iter()
        .map(|&i| escrow_shares[i].contribute(&mut rng, payload))
        .collect();

    let osst_ok = osst::verify(&y3_verify, &contributions, inner_t, payload).unwrap();
    assert!(osst_ok, "OSST must verify");
    println!("  ✓ OSST: 3-of-5 holders authorized (async, non-interactive)\n");

    // ================================================================
    // PHASE 5: Nested FROST signing
    //
    // The same 3 holders produce the escrow's outer FROST contribution.
    //
    // Step 5a: inner holders generate nonces, relay sums commitments
    //          to form escrow's outer nonce (D₃, E₃)
    // Step 5b: outer FROST round 1 — buyer + escrow commitments
    // Step 5c: inner holders compute partial z₃,ₖ using outer challenge
    // Step 5d: relay sums to get z₃
    // Step 5e: outer aggregation
    // ================================================================

    println!("--- phase 5: nested FROST signing ---");

    let message = b"zcash spend authorization for dispute";

    // 5a: inner holders generate nonces
    let inner_active = osst_active; // same holders who passed OSST
    let inner_active_indices: Vec<u32> = inner_active.iter().map(|&i| (i + 1) as u32).collect();
    let inner_lagrange = osst::compute_lagrange_coefficients::<Scalar>(&inner_active_indices).unwrap();

    // Each holder generates nonce pair
    let mut inner_hiding_nonces: Vec<Scalar> = Vec::new();
    let mut inner_binding_nonces: Vec<Scalar> = Vec::new();
    let mut inner_hiding_commits: Vec<Point> = Vec::new();
    let mut inner_binding_commits: Vec<Point> = Vec::new();

    for _ in &inner_active {
        let d = Scalar::random(&mut rng);
        let e = Scalar::random(&mut rng);
        inner_hiding_commits.push(Point::generator().mul_scalar(&d));
        inner_binding_commits.push(Point::generator().mul_scalar(&e));
        inner_hiding_nonces.push(d);
        inner_binding_nonces.push(e);
    }

    // Relay sums nonce commitments to form escrow's outer commitments
    // D₃ = Σ D₃,ₖ    E₃ = Σ E₃,ₖ
    let mut d3_commit = Point::identity();
    let mut e3_commit = Point::identity();
    for i in 0..inner_active.len() {
        d3_commit = d3_commit.add(&inner_hiding_commits[i]);
        e3_commit = e3_commit.add(&inner_binding_commits[i]);
    }

    println!("  inner holders generated nonces");
    println!("  escrow D₃ = Σ D₃,ₖ  (relay summed {} commitments)", inner_active.len());

    // 5b: outer FROST round 1 — buyer commits normally
    let (buyer_nonces, buyer_commitments) = frost::commit::<Point, _>(1, &mut rng);

    // Build the outer signing package manually
    // We need escrow's commitments as a SigningCommitments struct
    let escrow_commitments = frost::SigningCommitments {
        index: 3,
        hiding: d3_commit,
        binding: e3_commit,
    };

    let outer_package = frost::SigningPackage::new(
        message.to_vec(),
        vec![buyer_commitments, escrow_commitments],
    ).unwrap();

    println!("  outer FROST package: buyer (1) + escrow (3)");

    // 5c: buyer signs normally
    let buyer_sig_share = frost::sign::<Point>(
        &outer_package, buyer_nonces, &buyer_share, &group_key,
    ).unwrap();

    // 5c: inner holders compute their pieces of z₃
    //
    // z₃,ₖ = d₃,ₖ + ρ₃·e₃,ₖ + λ₃·c·μₖ·σₖ
    //
    // where:
    //   ρ₃ = binding factor for position 3 in outer FROST
    //   c  = outer Schnorr challenge
    //   λ₃ = outer Lagrange coefficient for position 3
    //   μₖ = inner Lagrange coefficient for holder k

    // Compute outer FROST parameters that inner holders need
    // (In production, the relay distributes these)
    let group_commitment = {
        let rho_1 = compute_binding_factor(1, message, &outer_package);
        let rho_3 = compute_binding_factor(3, message, &outer_package);
        let buyer_c = outer_package.get_commitments(1).unwrap();
        let escrow_c = outer_package.get_commitments(3).unwrap();
        buyer_c.hiding.add(&buyer_c.binding.mul_scalar(&rho_1))
            .add(&escrow_c.hiding)
            .add(&escrow_c.binding.mul_scalar(&rho_3))
    };

    let rho_3 = compute_binding_factor(3, message, &outer_package);
    let challenge = compute_challenge(&group_commitment, &group_key, message);
    let outer_lagrange = osst::compute_lagrange_coefficients::<Scalar>(&[1, 3]).unwrap();
    let lambda_3 = outer_lagrange[1]; // λ₃ for index 3 in set {1, 3}

    // Each inner holder computes their piece
    let mut z3 = Scalar::ZERO;
    for (pos, &i) in inner_active.iter().enumerate() {
        let d_k = inner_hiding_nonces[pos];
        let e_k = inner_binding_nonces[pos];
        let mu_k = &inner_lagrange[pos];    // inner Lagrange for this holder
        let sigma_k = escrow_shares[i].scalar();

        // z₃,ₖ = d₃,ₖ + ρ₃·e₃,ₖ + (λ₃·c·μₖ)·σₖ
        let z3_k = d_k + rho_3 * e_k + lambda_3 * challenge * mu_k * sigma_k;
        z3 += z3_k;

        println!("    holder {} computed z₃,{}", i + 1, i + 1);
    }

    println!("  relay summed: z₃ = Σ z₃,ₖ");

    // 5d: construct escrow's signature share
    let escrow_sig_share = frost::SignatureShare {
        index: 3,
        response: z3,
    };

    // 5e: outer aggregation
    let signature = frost::aggregate::<Point>(
        &outer_package,
        &[buyer_sig_share, escrow_sig_share],
        &group_key,
        Some(&outer_vshares),
    ).unwrap();

    let valid = frost::verify_signature(&group_key, message, &signature);
    assert!(valid, "final signature must verify");

    println!("\n  ✓ standard Schnorr signature verified against group key\n");

    // ================================================================
    // Summary
    // ================================================================

    println!("=== narsil nested complete ===\n");
    println!("  outer FROST:    2-of-3 (buyer + seller + escrow)");
    println!("  escrow DKG:     interleaved — s₃ never existed");
    println!("  OSST gate:      {}-of-{} holders authorized (async)", inner_t, inner_n);
    println!("  inner signing:  {}-of-{} holders produced z₃", inner_active.len(), inner_n);
    println!("  on-chain:       1 standard Schnorr signature");
    println!();
    println!("  cryptographic guarantees:");
    println!("    - buyer alone cannot sign (needs 1 more share)");
    println!("    - seller alone cannot sign (needs 1 more share)");
    println!("    - escrow holders cannot sign alone (need buyer or seller)");
    println!("    - no single escrow holder has s₃ (born distributed)");
    println!("    - {} escrow holders colluding still cannot sign without buyer/seller", inner_t - 1);
}

// ============================================================================
// Helper: recompute FROST internals for the inner signing step
// (In production, the relay would provide these values)
// ============================================================================

fn compute_binding_factor(
    index: u32,
    message: &[u8],
    package: &frost::SigningPackage<Point>,
) -> Scalar {
    use sha2::{Digest, Sha512};
    let mut encoded = Vec::new();
    for idx in package.signer_indices() {
        let c = package.get_commitments(idx).unwrap();
        encoded.extend_from_slice(&c.index.to_le_bytes());
        encoded.extend_from_slice(&OsstPoint::compress(&c.hiding));
        encoded.extend_from_slice(&OsstPoint::compress(&c.binding));
    }
    let mut h = Sha512::new();
    h.update(b"frost-binding-v1");
    h.update(index.to_le_bytes());
    h.update((message.len() as u64).to_le_bytes());
    h.update(message);
    h.update(&encoded);
    let hash: [u8; 64] = h.finalize().into();
    <Scalar as OsstScalar>::from_bytes_wide(&hash)
}

fn compute_challenge(
    group_commitment: &Point,
    group_pubkey: &Point,
    message: &[u8],
) -> Scalar {
    use sha2::{Digest, Sha512};
    let mut h = Sha512::new();
    h.update(b"frost-challenge-v1");
    h.update(OsstPoint::compress(group_commitment));
    h.update(OsstPoint::compress(group_pubkey));
    h.update(message);
    let hash: [u8; 64] = h.finalize().into();
    <Scalar as OsstScalar>::from_bytes_wide(&hash)
}
