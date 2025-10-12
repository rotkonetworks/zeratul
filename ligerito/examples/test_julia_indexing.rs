use ligerito::{prove_sha256, hardcoded_config_12};
use ligerito::utils::{evaluate_lagrange_basis, eval_sk_at_vks, evaluate_scaled_basis_inplace};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement, BinaryPolynomial};
use std::marker::PhantomData;

fn main() {
    println!("=== testing julia-style indexing conversion ===");

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    let proof = prove_sha256(&config, &poly).unwrap();

    // use exact values that match our verify_ligero output
    let challenges = vec![
        BinaryElem128::from_poly(<BinaryElem128 as BinaryFieldElement>::Poly::from_value(15409286712743540563)),
        BinaryElem128::from_poly(<BinaryElem128 as BinaryFieldElement>::Poly::from_value(7758652803507829072)),
    ];

    println!("testing with real challenges: {:?}", challenges);

    let gr = evaluate_lagrange_basis(&challenges);
    let n = proof.final_ligero_proof.yr.len().trailing_zeros() as usize;

    // try julia's approach: sks_vks as U type
    let sks_vks_t: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);
    let sks_vks_u: Vec<BinaryElem128> = sks_vks_t.iter().map(|&x| BinaryElem128::from(x)).collect();

    println!("testing query 0 with both approaches:");

    let query = 0;
    let row = &proof.final_ligero_proof.opened_rows[query];

    // compute dot product (same in both)
    let dot = row.iter()
        .zip(gr.iter())
        .fold(BinaryElem128::zero(), |acc, (&r, &g)| {
            let r_u = BinaryElem128::from(r);
            acc.add(&r_u.mul(&g))
        });

    println!("dot = {:?}", dot);

    // approach 1: our current rust approach
    println!("\n--- rust approach ---");
    let qf_rust = BinaryElem32::from_poly(
        <BinaryElem32 as BinaryFieldElement>::Poly::from_value(query as u64)
    );

    let mut local_sks_x = vec![BinaryElem32::zero(); sks_vks_t.len()];
    let mut local_basis = vec![BinaryElem128::zero(); 1 << n];
    let scale_rust = BinaryElem128::from(BinaryElem32::one());

    evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, &sks_vks_t, qf_rust, scale_rust);

    let e_rust = proof.final_ligero_proof.yr.iter()
        .zip(local_basis.iter())
        .fold(BinaryElem128::zero(), |acc, (&y, &b)| {
            let y_u = BinaryElem128::from(y);
            acc.add(&y_u.mul(&b))
        });

    println!("qf_rust = {:?}", qf_rust);
    println!("scale_rust = {:?}", scale_rust);
    println!("e_rust = {:?}", e_rust);
    println!("rust: e == dot? {}", e_rust == dot);

    // approach 2: try julia-style with different combinations
    println!("\n--- julia-style variations ---");

    // try with T scale but U sks_vks
    let mut local_basis2 = vec![BinaryElem128::zero(); 1 << n];
    let scale_julia = BinaryElem128::from(BinaryElem32::one()); // T(1) -> U

    // but we need to pass T type sks_vks to our function, so convert back
    evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis2, &sks_vks_t, qf_rust, scale_julia);

    let e_julia = proof.final_ligero_proof.yr.iter()
        .zip(local_basis2.iter())
        .fold(BinaryElem128::zero(), |acc, (&y, &b)| {
            let y_u = BinaryElem128::from(y);
            acc.add(&y_u.mul(&b))
        });

    println!("e_julia = {:?}", e_julia);
    println!("julia-style: e == dot? {}", e_julia == dot);

    if e_rust == e_julia {
        println!("both approaches give same result - type conversion not the issue");
    } else {
        println!("different results! type conversion matters");
    }
}