use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use ligerito::{prove_sha256, verify_sha256, ProverConfig, VerifierConfig};
use ligerito_reed_solomon::reed_solomon;

fn create_tiny_config() -> ProverConfig<BinaryElem32, BinaryElem128> {
    // Even smaller: 2^8 = 256 elements
    let recursive_steps = 1;
    let inv_rate = 4;

    let initial_dims = (1 << 6, 1 << 2);  // (64, 4)
    let dims = vec![(1 << 4, 1 << 2)];    // (16, 4)

    let initial_k = 2;
    let ks = vec![2];

    let initial_reed_solomon = ligerito_reed_solomon::<BinaryElem32>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = vec![
        ligerito_reed_solomon::<BinaryElem128>(dims[0].0, dims[0].0 * inv_rate),
    ];

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

fn create_tiny_verifier_config() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 1,
        initial_dim: 6,
        log_dims: vec![4],
        initial_k: 2,
        ks: vec![2],
    }
}

fn main() {
    println!("=== TINY DEBUG TEST ===");
    println!("Testing with 2^8 = 256 elements\n");

    let config = create_tiny_config();
    let verifier_config = create_tiny_verifier_config();

    // Create a simple polynomial - all ones
    let poly = vec![BinaryElem32::one(); 1 << 8];

    println!("Generating proof...");
    let proof = prove_sha256(&config, &poly).expect("Proving failed");
    println!("Proof generated successfully");

    println!("\nVerifying proof...");

    // Use SHA256 verifier to match the prover
    match verify_sha256(&verifier_config, &proof) {
        Ok(true) => println!("✓ Verification PASSED"),
        Ok(false) => println!("✗ Verification FAILED"),
        Err(e) => println!("✗ Verification ERROR: {:?}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tiny_polynomial() {
        let config = create_tiny_config();
        let verifier_config = create_tiny_verifier_config();
        
        // Test with all zeros
        let poly = vec![BinaryElem32::zero(); 1 << 8];
        let proof = prove_sha256(&config, &poly).unwrap();
        assert!(verify_sha256(&verifier_config, &proof).unwrap());
        
        // Test with all ones
        let poly = vec![BinaryElem32::one(); 1 << 8];
        let proof = prove_sha256(&config, &poly).unwrap();
        assert!(verify_sha256(&verifier_config, &proof).unwrap());
    }
}
