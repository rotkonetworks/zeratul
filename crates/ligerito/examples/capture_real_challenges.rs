use ligerito::{prove_sha256, verify_sha256, hardcoded_config_12, hardcoded_config_12_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

fn main() {
    println!("=== capturing real challenges from working verifier ===");

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // use different polynomial to avoid degenerate case
    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("generating proof...");
    let proof = prove_sha256(&config, &poly).unwrap();

    println!("running verifier to capture challenges...");
    let verifier_config = hardcoded_config_12_verifier();

    // the verifier calls verify_ligero with the real challenges
    // we should see the debug output from our modified verify_ligero
    let result = verify_sha256(&verifier_config, &proof).unwrap();

    println!("verification result: {}", result);

    if !result {
        println!("verification failed - this is the bug we need to fix");
    } else {
        println!("verification passed - our mathematical relationship works!");
    }
}