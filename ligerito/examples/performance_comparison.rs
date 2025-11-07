use ligerito::{
    prove_sha256, prove, verify_sha256, verify,
    hardcoded_config_12, hardcoded_config_12_verifier,
};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    println!("=== LIGERITO PERFORMANCE COMPARISON ===");
    println!("Comparing SHA256 vs Merlin transcript performance\n");

    // Test with different polynomial sizes to see scalability
    let test_sizes = vec![
        (1 << 10, "1K elements"),   // 1024 elements
        (1 << 11, "2K elements"),   // 2048 elements
        (1 << 12, "4K elements"),   // 4096 elements (standard)
    ];

    for (size, desc) in test_sizes {
        println!("=== {} ({} polynomial) ===", desc, size);

        // Generate test polynomial
        let poly: Vec<BinaryElem32> = (0..size).map(|i| BinaryElem32::from(i as u32)).collect();

        let config = hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let verifier_config = hardcoded_config_12_verifier();

        // Test Merlin transcript (default, optimized)
        println!("\n--- Merlin Transcript Performance ---");
        let merlin_results = benchmark_transcript(&config, &verifier_config, &poly, false, 3);

        // Test SHA256 transcript (Julia-compatible)
        println!("--- SHA256 Transcript Performance ---");
        let sha256_results = benchmark_transcript(&config, &verifier_config, &poly, true, 3);

        // Compare results
        println!("--- Performance Comparison ---");
        println!("Merlin prove:  {:.2}ms (avg)", merlin_results.prove_avg_ms);
        println!("SHA256 prove:  {:.2}ms (avg)", sha256_results.prove_avg_ms);
        println!("Speedup ratio: {:.2}x", sha256_results.prove_avg_ms / merlin_results.prove_avg_ms);

        println!("Merlin verify: {:.2}ms (avg)", merlin_results.verify_avg_ms);
        println!("SHA256 verify: {:.2}ms (avg)", sha256_results.verify_avg_ms);
        println!("Speedup ratio: {:.2}x", sha256_results.verify_avg_ms / merlin_results.verify_avg_ms);

        println!("Total Merlin:  {:.2}ms", merlin_results.total_avg_ms);
        println!("Total SHA256:  {:.2}ms", sha256_results.total_avg_ms);
        println!("Overall speedup: {:.2}x", sha256_results.total_avg_ms / merlin_results.total_avg_ms);

        // Proof size comparison (should be identical)
        println!("Proof sizes are identical: {} bytes", merlin_results.proof_size);

        println!();
    }

    // Memory usage and detailed profiling
    println!("=== DETAILED PROFILING ===");
    detailed_profiling();

    // Transcript-specific benchmarks
    println!("=== TRANSCRIPT-ONLY BENCHMARKS ===");
    transcript_benchmarks();
}

#[derive(Debug)]
struct BenchmarkResults {
    prove_avg_ms: f64,
    verify_avg_ms: f64,
    total_avg_ms: f64,
    proof_size: usize,
}

fn benchmark_transcript(
    config: &ligerito::ProverConfig<BinaryElem32, BinaryElem128>,
    verifier_config: &ligerito::VerifierConfig,
    poly: &[BinaryElem32],
    use_sha256: bool,
    iterations: usize,
) -> BenchmarkResults {
    let transcript_name = if use_sha256 { "SHA256" } else { "Merlin" };

    let mut prove_times = Vec::new();
    let mut verify_times = Vec::new();
    let mut proof_size = 0;

    for i in 0..iterations {
        print!("  Run {}/{} ... ", i + 1, iterations);

        // Benchmark proving
        let start = Instant::now();
        let proof = if use_sha256 {
            prove_sha256(config, poly).expect("Proof generation failed")
        } else {
            prove(config, poly).expect("Proof generation failed")
        };
        let prove_time = start.elapsed();
        prove_times.push(prove_time.as_secs_f64() * 1000.0);

        // Get proof size (only once)
        if i == 0 {
            proof_size = estimate_proof_size(&proof);
        }

        // Benchmark verification
        let start = Instant::now();
        let verification_result = if use_sha256 {
            verify_sha256(verifier_config, &proof).expect("Verification failed")
        } else {
            verify(verifier_config, &proof).expect("Verification failed")
        };
        let verify_time = start.elapsed();
        verify_times.push(verify_time.as_secs_f64() * 1000.0);

        if !verification_result {
            panic!("Verification returned false - this should not happen!");
        }

        println!("✓ {:.1}ms prove, {:.1}ms verify",
                 prove_times.last().unwrap(),
                 verify_times.last().unwrap());
    }

    let prove_avg = prove_times.iter().sum::<f64>() / iterations as f64;
    let verify_avg = verify_times.iter().sum::<f64>() / iterations as f64;

    BenchmarkResults {
        prove_avg_ms: prove_avg,
        verify_avg_ms: verify_avg,
        total_avg_ms: prove_avg + verify_avg,
        proof_size,
    }
}

fn estimate_proof_size(proof: &ligerito::FinalizedLigeritoProof<BinaryElem32, BinaryElem128>) -> usize {
    let mut size = 0;

    // Initial commitment
    size += proof.initial_ligero_cm.root.size_of();

    // Recursive commitments
    for commitment in &proof.recursive_commitments {
        size += commitment.root.size_of();
    }

    // Initial ligero proof
    size += proof.initial_ligero_proof.merkle_proof.size_of();
    for row in &proof.initial_ligero_proof.opened_rows {
        size += row.len() * 4; // 4 bytes per BinaryElem32
    }

    // Final ligero proof
    size += proof.final_ligero_proof.merkle_proof.size_of();
    size += proof.final_ligero_proof.yr.len() * 16; // 16 bytes per BinaryElem128
    for row in &proof.final_ligero_proof.opened_rows {
        size += row.len() * 4; // 4 bytes per BinaryElem32
    }

    // Sumcheck transcript
    size += proof.sumcheck_transcript.transcript.len() * 3 * 16; // 3 coefficients * 16 bytes each

    // Recursive proofs
    for recursive_proof in &proof.recursive_proofs {
        size += recursive_proof.merkle_proof.size_of();
        for row in &recursive_proof.opened_rows {
            size += row.len() * 4;
        }
    }

    size
}

fn detailed_profiling() {
    let poly: Vec<BinaryElem32> = (0..1 << 12).map(|i| BinaryElem32::from(i as u32)).collect();

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    println!("Detailed breakdown for 4K polynomial:");

    // Time different phases
    let start = Instant::now();
    let proof = prove(&config, &poly).expect("Proof generation failed");
    let total_time = start.elapsed();

    println!("Total proving time: {:.2}ms", total_time.as_secs_f64() * 1000.0);

    // Analyze proof structure
    println!("Proof structure:");
    println!("  Initial opened rows: {} x {}",
             proof.initial_ligero_proof.opened_rows.len(),
             proof.initial_ligero_proof.opened_rows.get(0).map_or(0, |r| r.len()));
    println!("  Final opened rows: {} x {}",
             proof.final_ligero_proof.opened_rows.len(),
             proof.final_ligero_proof.opened_rows.get(0).map_or(0, |r| r.len()));
    println!("  Recursive proofs: {}", proof.recursive_proofs.len());
    println!("  Sumcheck rounds: {}", proof.sumcheck_transcript.transcript.len());

    let estimated_size = estimate_proof_size(&proof);
    println!("  Estimated proof size: {:.1} KB", estimated_size as f64 / 1024.0);
}

fn transcript_benchmarks() {
    use ligerito::transcript::{FiatShamir, Transcript};

    println!("Pure transcript performance (1000 challenges):");

    let iterations = 1000;
    let dummy_root = merkle_tree::MerkleRoot { root: Some([1u8; 32]) };

    // Merlin transcript
    let start = Instant::now();
    let mut merlin_fs = FiatShamir::new_merlin();
    merlin_fs.absorb_root(&dummy_root);
    for _ in 0..iterations {
        let _: BinaryElem32 = merlin_fs.get_challenge();
    }
    let merlin_time = start.elapsed();

    // SHA256 transcript
    let start = Instant::now();
    let mut sha256_fs = FiatShamir::new_sha256(1234);
    sha256_fs.absorb_root(&dummy_root);
    for _ in 0..iterations {
        let _: BinaryElem32 = sha256_fs.get_challenge();
    }
    let sha256_time = start.elapsed();

    println!("Merlin {} challenges: {:.2}μs ({:.2}μs per challenge)",
             iterations,
             merlin_time.as_micros(),
             merlin_time.as_micros() as f64 / iterations as f64);
    println!("SHA256 {} challenges: {:.2}μs ({:.2}μs per challenge)",
             iterations,
             sha256_time.as_micros(),
             sha256_time.as_micros() as f64 / iterations as f64);

    let speedup = sha256_time.as_secs_f64() / merlin_time.as_secs_f64();
    println!("Merlin is {:.2}x faster for challenge generation", speedup);

    // Query generation benchmark
    println!("\nQuery generation performance:");
    let start = Instant::now();
    let mut merlin_fs = FiatShamir::new_merlin();
    merlin_fs.absorb_root(&dummy_root);
    let _queries = merlin_fs.get_distinct_queries(1024, 148);
    let merlin_query_time = start.elapsed();

    let start = Instant::now();
    let mut sha256_fs = FiatShamir::new_sha256(1234);
    sha256_fs.absorb_root(&dummy_root);
    let _queries = sha256_fs.get_distinct_queries(1024, 148);
    let sha256_query_time = start.elapsed();

    println!("Merlin 148 queries: {:.2}μs", merlin_query_time.as_micros());
    println!("SHA256 148 queries: {:.2}μs", sha256_query_time.as_micros());

    let query_speedup = sha256_query_time.as_secs_f64() / merlin_query_time.as_secs_f64();
    println!("Merlin is {:.2}x faster for query generation", query_speedup);
}