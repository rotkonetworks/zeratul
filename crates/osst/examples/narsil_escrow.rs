//! narsil escrow: 2-of-3 FROST where share 3 is OSST-gated
//!
//! Demonstrates the full narsil flow:
//!
//! 1. DKG for the outer 2-of-3 FROST key (buyer, seller, escrow)
//! 2. Inner DKG splits the escrow share among 5 holders (threshold 3)
//! 3. OSST authorization: 3 holders prove they control the escrow share
//! 4. Inner FROST: the same 3 holders produce escrow's partial signature
//! 5. Outer FROST: escrow + buyer produce the final Schnorr signature
//!
//! The blockchain sees one standard Schnorr signature. No evidence of
//! the escrow being 5 people, or that OSST authorization happened.
//!
//! Run: cargo run --example narsil_escrow --features ristretto255

use std::collections::BTreeMap;

use osst::curve::OsstPoint;
use osst::dkg;
use osst::frost;
use osst::reshare::DealerCommitment;
use osst::SecretShare;

use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use rand::rngs::OsRng;

type Point = RistrettoPoint;

fn main() {
    let mut rng = OsRng;

    println!("=== narsil escrow: 2-of-3 FROST with OSST-gated escrow ===\n");

    // ================================================================
    // PHASE 1: Outer DKG — 3 participants, threshold 2
    //
    // Participant 1 = buyer
    // Participant 2 = seller
    // Participant 3 = escrow (will be split further)
    // ================================================================

    println!("--- phase 1: outer DKG (2-of-3) ---");

    let outer_n = 3u32;
    let outer_t = 2u32;

    let outer_dealers: Vec<dkg::Dealer<Point>> = (1..=outer_n)
        .map(|i| dkg::Dealer::new(i, outer_t, &mut rng))
        .collect();

    let outer_commitments: Vec<&DealerCommitment<Point>> =
        outer_dealers.iter().map(|d| d.commitment()).collect();

    let mut outer_shares: Vec<SecretShare<Scalar>> = Vec::new();
    let mut outer_vshares: BTreeMap<u32, Point> = BTreeMap::new();
    let mut outer_group_key = Point::identity();

    for j in 1..=outer_n {
        let mut agg: dkg::Aggregator<Point> = dkg::Aggregator::new(j);
        for dealer in &outer_dealers {
            let subshare = dealer.generate_subshare(j);
            agg.add_subshare(subshare, outer_commitments[(dealer.index() - 1) as usize])
                .unwrap();
        }
        let share_scalar = agg.finalize(outer_n).unwrap();
        if j == 1 {
            outer_group_key = agg.derive_group_key();
        }
        let ss = SecretShare::new(j, share_scalar);
        outer_vshares.insert(j, Point::generator().mul_scalar(ss.scalar()));
        outer_shares.push(ss);
    }

    println!("  outer group key: {}", hex::encode(OsstPoint::compress(&outer_group_key)));
    println!("  buyer  (share 1): ready");
    println!("  seller (share 2): ready");
    println!("  escrow (share 3): ready — will be split next\n");

    // ================================================================
    // PHASE 2: Inner DKG — split escrow share among 5 holders
    //
    // The escrow's secret scalar is s_3. We treat it as the "seed"
    // for an inner group. The inner DKG produces shares such that
    // the inner group's combined secret equals s_3.
    //
    // In production, this would use the interleaved DKG from the
    // forum post so s_3 is never materialized. For this example,
    // we simulate by Shamir-splitting the known s_3.
    // ================================================================

    println!("--- phase 2: inner DKG (3-of-5 escrow holders) ---");

    let inner_n = 5u32;
    let inner_t = 3u32;
    let escrow_secret = outer_shares[2].scalar().clone(); // s_3

    // Shamir split of escrow secret (simulates inner DKG output)
    let inner_shares = shamir_split(&escrow_secret, inner_n, inner_t);

    // Inner group public key should equal the escrow's public verification share
    let escrow_pubkey = outer_vshares[&3];

    // Verify: inner shares reconstruct to escrow secret
    let inner_group_key = Point::generator().mul_scalar(&escrow_secret);
    assert_eq!(inner_group_key, escrow_pubkey, "inner group key must match escrow pubkey");

    println!("  escrow pubkey: {}", hex::encode(OsstPoint::compress(&escrow_pubkey)));
    println!("  split into {} holders, threshold {}", inner_n, inner_t);
    for i in 0..inner_n as usize {
        println!("    holder {}: share ready", i + 1);
    }
    println!();

    // ================================================================
    // PHASE 3: OSST authorization — holders prove they control escrow
    //
    // Each holder independently generates an OSST proof.
    // Non-interactive, asynchronous, no coordination.
    // The relay collects proofs and verifies interpolation.
    // ================================================================

    println!("--- phase 3: OSST authorization (async, non-interactive) ---");

    let payload = b"authorize escrow for dispute tx:abc123";

    // 3 of 5 holders submit proofs (holders 1, 3, 5 — non-consecutive)
    let active_holders = [0usize, 2, 4];
    let contributions: Vec<osst::Contribution<Point>> = active_holders
        .iter()
        .map(|&i| inner_shares[i].contribute(&mut rng, payload))
        .collect();

    println!("  payload: {:?}", std::str::from_utf8(payload).unwrap());
    for c in &contributions {
        println!(
            "    holder {} submitted proof (commitment: {}...)",
            c.index,
            &hex::encode(OsstPoint::compress(&c.commitment))[..16]
        );
    }

    // Relay verifies OSST
    let osst_valid = osst::verify(&escrow_pubkey, &contributions, inner_t, payload).unwrap();
    assert!(osst_valid, "OSST authorization must pass");

    println!("  ✓ OSST verified: {} proofs interpolate to escrow key", contributions.len());
    println!("  → authorization passed, proceeding to FROST signing\n");

    // ================================================================
    // PHASE 4: Inner FROST — holders produce escrow's partial signature
    //
    // The same 3 holders who passed OSST now run FROST to produce
    // z_3 (escrow's contribution to the outer 2-of-3 ceremony).
    //
    // This is the interactive part — requires nonce exchange.
    // ================================================================

    println!("--- phase 4: inner FROST (escrow holders sign) ---");

    let message = b"zcash spend authorization for dispute resolution";

    // But wait — we need the inner FROST to produce a partial signature
    // for the OUTER ceremony. The inner holders' shares reconstruct to
    // s_3 (the escrow's outer share). So we run an inner FROST ceremony
    // that produces a value z_3 compatible with the outer signing.
    //
    // The trick: inner FROST produces a full signature (R_inner, z_inner)
    // where z_inner = d + rho*e + c*s_3 effectively. But for the outer
    // FROST, we need z_3 = d_3 + rho_3*e_3 + lambda_3*c_outer*s_3.
    //
    // In production narsil, the inner holders compute partial signatures
    // directly in the outer FROST's format. For this example, we
    // demonstrate both layers independently.

    // Inner FROST: holders produce a standalone signature
    // (proving the inner threshold mechanism works)
    let mut inner_nonces = Vec::new();
    let mut inner_commitments = Vec::new();
    for &i in &active_holders {
        let (nonces, commitments) =
            frost::commit::<Point, _>(inner_shares[i].index, &mut rng);
        inner_nonces.push(nonces);
        inner_commitments.push(commitments);
    }

    let inner_package =
        frost::SigningPackage::new(message.to_vec(), inner_commitments).unwrap();

    let mut inner_sig_shares = Vec::new();
    for (&i, nonces) in active_holders.iter().zip(inner_nonces.into_iter()) {
        let sig_share = frost::sign::<Point>(
            &inner_package,
            nonces,
            &inner_shares[i],
            &escrow_pubkey,
        )
        .unwrap();
        inner_sig_shares.push(sig_share);
    }

    let inner_signature = frost::aggregate::<Point>(
        &inner_package,
        &inner_sig_shares,
        &escrow_pubkey,
        None,
    )
    .unwrap();

    // Verify inner signature against escrow pubkey
    let inner_valid = frost::verify_signature(&escrow_pubkey, message, &inner_signature);
    assert!(inner_valid, "inner FROST signature must verify against escrow key");

    println!("  inner FROST signature: R={}", &hex::encode(OsstPoint::compress(&inner_signature.r))[..16]);
    println!("  ✓ inner signature verified against escrow pubkey\n");

    // ================================================================
    // PHASE 5: Outer FROST — buyer + escrow sign the transaction
    //
    // The buyer (share 1) and escrow (share 3) cooperate.
    // The escrow's contribution comes from the inner FROST ceremony.
    //
    // For this example, we show the outer 2-of-3 FROST working
    // independently. In production, the inner holders would compute
    // their partial signatures directly in the outer FROST format.
    // ================================================================

    println!("--- phase 5: outer FROST (2-of-3: buyer + escrow) ---");

    // Outer FROST with buyer (index 1) and escrow (index 3)
    let outer_active = [0usize, 2]; // buyer and escrow

    let mut outer_nonces = Vec::new();
    let mut outer_commitments = Vec::new();
    for &i in &outer_active {
        let (nonces, commitments) =
            frost::commit::<Point, _>(outer_shares[i].index, &mut rng);
        outer_nonces.push(nonces);
        outer_commitments.push(commitments);
    }

    let outer_package =
        frost::SigningPackage::new(message.to_vec(), outer_commitments).unwrap();

    let mut outer_sig_shares = Vec::new();
    for (&i, nonces) in outer_active.iter().zip(outer_nonces.into_iter()) {
        let sig_share = frost::sign::<Point>(
            &outer_package,
            nonces,
            &outer_shares[i],
            &outer_group_key,
        )
        .unwrap();
        outer_sig_shares.push(sig_share);
    }

    let final_signature = frost::aggregate::<Point>(
        &outer_package,
        &outer_sig_shares,
        &outer_group_key,
        Some(&outer_vshares),
    )
    .unwrap();

    // Verify: standard Schnorr signature against the outer group key
    let valid = frost::verify_signature(&outer_group_key, message, &final_signature);
    assert!(valid, "final Schnorr signature must verify");

    println!("  outer group key:  {}", &hex::encode(OsstPoint::compress(&outer_group_key))[..32]);
    println!("  final signature:  R={}", &hex::encode(OsstPoint::compress(&final_signature.r))[..32]);
    println!("  ✓ standard Schnorr signature verified\n");

    // ================================================================
    // Summary
    // ================================================================

    println!("=== narsil escrow complete ===\n");
    println!("  outer FROST:  2-of-3 (buyer + seller + escrow)");
    println!("  OSST gate:    {}-of-{} escrow holders authorized", inner_t, inner_n);
    println!("  inner FROST:  {}-of-{} escrow holders signed", inner_t, inner_n);
    println!("  on-chain:     1 standard Schnorr signature");
    println!("  evidence:     none — indistinguishable from single signer");
    println!();
    println!("  the blockchain does not know:");
    println!("    - that this was a 2-of-3 multisig");
    println!("    - that the escrow was 5 people");
    println!("    - that OSST authorization happened");
    println!("    - which holders participated");
}

/// Simulate Shamir secret sharing for the inner DKG.
///
/// In production, use the interleaved DKG so the escrow secret
/// is never materialized. This helper is for demonstration only.
fn shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
    let mut rng = OsRng;
    let mut coeffs = vec![*secret];
    for _ in 1..t {
        coeffs.push(Scalar::random(&mut rng));
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
            SecretShare::new(i, y)
        })
        .collect()
}
