//! Ligerito CLI - Prove and verify polynomial commitments via stdin/stdout
//!
//! # Examples
//!
//! ```bash
//! # Prove a polynomial
//! cat polynomial.bin | ligerito prove --size 20 > proof.bin
//!
//! # Verify a proof
//! cat proof.bin | ligerito verify --size 20
//! # Output: "VALID" or "INVALID" with exit code 0/1
//!
//! # Roundtrip
//! cat data.bin | ligerito prove --size 24 | ligerito verify --size 24
//! ```

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ligerito::{
    prove_with_transcript, verify_with_transcript,
    prover_config_for_log_size, verifier_config_for_log_size,
    config_info_for_log_size, MIN_LOG_SIZE, MAX_LOG_SIZE,
    VerifierConfig, FinalizedLigeritoProof,
};
use ligerito::transcript::{MerlinTranscript, FiatShamir};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::io::{self, Read, Write};
use std::fs::File;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "ligerito")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Ligerito polynomial commitment scheme CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a proof for a polynomial (read from stdin or file, write to stdout)
    ///
    /// If --size is not specified, automatically selects config based on input size.
    Prove {
        /// Log2 of polynomial size (20-30). If omitted, auto-detects from input.
        #[arg(short, long, conflicts_with = "config")]
        size: Option<u32>,

        /// Path to custom prover config JSON file (BYOC - Bring Your Own Config)
        #[arg(short, long, conflicts_with = "size")]
        config: Option<String>,

        /// Output format: bincode (default) or hex
        #[arg(short, long, default_value = "bincode")]
        format: String,

        /// Transcript backend: sha256 (default) or merlin
        /// Prover and verifier MUST use the same backend!
        #[arg(short, long, default_value = "sha256")]
        transcript: String,

        /// Input file (reads from stdin if not provided)
        #[arg(value_name = "FILE")]
        input: Option<String>,
    },

    /// Verify a proof (read from stdin or file, exit with code 0 if valid)
    ///
    /// If --size is not specified, automatically selects config based on proof metadata.
    Verify {
        /// Log2 of polynomial size (20-30). If omitted, auto-detects from proof.
        #[arg(short, long, conflicts_with = "config")]
        size: Option<u32>,

        /// Path to custom verifier config JSON file (BYOC - Bring Your Own Config)
        #[arg(short, long, conflicts_with = "size")]
        config: Option<String>,

        /// Input format: bincode (default) or hex
        #[arg(short, long, default_value = "bincode")]
        format: String,

        /// Transcript backend: sha256 (default) or merlin
        /// MUST match the transcript used for proving!
        #[arg(short, long, default_value = "sha256")]
        transcript: String,

        /// Verbose output
        #[arg(short, long)]
        verbose: bool,

        /// Input file (reads from stdin if not provided)
        #[arg(value_name = "FILE")]
        input: Option<String>,
    },

    /// Show configuration info for all sizes or a specific size
    Config {
        /// Log2 of polynomial size (shows all if omitted)
        #[arg(short, long)]
        size: Option<u32>,

        /// Generate a template config file for custom use (BYOC)
        #[arg(long)]
        generate: bool,

        /// Output format for generated config: json (default) or toml
        #[arg(long, default_value = "json")]
        output_format: String,
    },

    /// Generate random polynomial data for testing
    Generate {
        /// Log2 of polynomial size (default: 20)
        #[arg(short, long, default_value = "20")]
        size: u32,

        /// Pattern: random (default), zeros, ones, sequential
        #[arg(short, long, default_value = "random")]
        pattern: String,

        /// Output file (stdout if not specified)
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Benchmark proving performance (no I/O overhead)
    Bench {
        /// Log2 of polynomial size (default: 20)
        #[arg(short, long, default_value = "20")]
        size: u32,

        /// Number of iterations (default: 3)
        #[arg(short, long, default_value = "3")]
        iterations: usize,

        /// Also benchmark verification
        #[arg(short, long)]
        verify: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Prove { size, config, format, transcript, input } => {
            prove_command(size, config, &format, &transcript, input)?;
        }
        Commands::Verify { size, config, format, transcript, verbose, input } => {
            verify_command(size, config, &format, &transcript, verbose, input)?;
        }
        Commands::Config { size, generate, output_format } => {
            config_command(size, generate, &output_format)?;
        }
        Commands::Generate { size, pattern, output } => {
            generate_command(size, &pattern, output)?;
        }
        Commands::Bench { size, iterations, verify } => {
            bench_command(size, iterations, verify)?;
        }
    }

    Ok(())
}

fn prove_command(size: Option<u32>, config_path: Option<String>, format: &str, transcript_type: &str, input: Option<String>) -> Result<()> {
    // TODO: Implement custom config loading for BYOC
    if config_path.is_some() {
        anyhow::bail!("Custom config loading not yet implemented. Use --size for now.");
    }

    // Read polynomial from file or stdin
    let mut buffer = Vec::new();
    match input {
        Some(path) => {
            let mut file = File::open(&path)
                .context(format!("Failed to open input file: {}", path))?;
            file.read_to_end(&mut buffer)
                .context("Failed to read polynomial from file")?;
        }
        None => {
            io::stdin()
                .read_to_end(&mut buffer)
                .context("Failed to read polynomial from stdin")?;
        }
    }

    let elem_size = std::mem::size_of::<BinaryElem32>();
    let num_elements = buffer.len() / elem_size;

    // Determine size: use provided size or auto-detect from input
    let size = match size {
        Some(s) => {
            // Validate provided size
            if s < MIN_LOG_SIZE || s > MAX_LOG_SIZE {
                anyhow::bail!("Size must be between {} and {}, got {}", MIN_LOG_SIZE, MAX_LOG_SIZE, s);
            }
            let expected_len = 1usize << s;
            if num_elements != expected_len {
                anyhow::bail!(
                    "Expected {} bytes ({} elements), got {} bytes ({} elements)",
                    expected_len * elem_size,
                    expected_len,
                    buffer.len(),
                    num_elements
                );
            }
            s
        }
        None => {
            // Auto-detect size from input
            if num_elements == 0 || !num_elements.is_power_of_two() {
                anyhow::bail!(
                    "Input must be a power-of-2 number of elements. Got {} bytes ({} elements)",
                    buffer.len(),
                    num_elements
                );
            }
            let detected_size = num_elements.trailing_zeros();
            if detected_size < MIN_LOG_SIZE || detected_size > MAX_LOG_SIZE {
                anyhow::bail!(
                    "Auto-detected size 2^{} is out of range ({}-{})",
                    detected_size, MIN_LOG_SIZE, MAX_LOG_SIZE
                );
            }
            eprintln!("auto-detected size: 2^{}", detected_size);
            detected_size
        }
    };

    // Convert bytes to polynomial
    let poly: Vec<BinaryElem32> = buffer
        .chunks_exact(4)
        .map(|chunk| {
            let val = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            BinaryElem32::from(val)
        })
        .collect();

    // Print build info
    let mut build_info = vec![];

    #[cfg(debug_assertions)]
    build_info.push("DEBUG");

    #[cfg(not(debug_assertions))]
    build_info.push("release");

    #[cfg(all(target_arch = "x86_64", target_feature = "pclmulqdq"))]
    build_info.push("SIMD");

    #[cfg(not(all(target_arch = "x86_64", target_feature = "pclmulqdq")))]
    build_info.push("no-SIMD");

    eprintln!("prove: 2^{} GF(2^32), {} [{}]", size, transcript_type, build_info.join(" "));

    // Get config using autosizer
    let config = prover_config_for_log_size::<BinaryElem32, BinaryElem128>(size);

    // Prove with selected transcript backend
    let start = Instant::now();
    let proof = match transcript_type {
        "sha256" => {
            let transcript = FiatShamir::new_sha256(0);
            prove_with_transcript(&config, &poly, transcript)
        }
        "merlin" => {
            #[cfg(feature = "transcript-merlin")]
            {
                let transcript = MerlinTranscript::new(b"ligerito-v1");
                prove_with_transcript(&config, &poly, transcript)
            }
            #[cfg(not(feature = "transcript-merlin"))]
            {
                anyhow::bail!("Merlin transcript not available. Rebuild with --features transcript-merlin")
            }
        }
        _ => anyhow::bail!("Unknown transcript backend: {}. Use sha256 or merlin", transcript_type),
    }.context("Proving failed")?;
    let elapsed = start.elapsed();

    // Serialize for size calculation
    let encoded = bincode::serialize(&proof)
        .context("Failed to serialize proof")?;

    let prove_ms = elapsed.as_secs_f64() * 1000.0;
    let throughput = (poly.len() as f64) / elapsed.as_secs_f64();

    eprintln!("result: {:.2}ms, {:.2e} elem/s, {} bytes", prove_ms, throughput, encoded.len());

    // Output proof
    match format {
        "bincode" => {
            io::stdout()
                .write_all(&encoded)
                .context("Failed to write proof to stdout")?;
        }
        "hex" => {
            let hex_str = hex::encode(&encoded);
            println!("{}", hex_str);
        }
        _ => anyhow::bail!("Unknown format: {}. Use 'bincode' or 'hex'", format),
    }

    Ok(())
}

fn verify_command(size: Option<u32>, config_path: Option<String>, format: &str, transcript_type: &str, verbose: bool, input: Option<String>) -> Result<()> {
    // TODO: Implement custom config loading for BYOC
    if config_path.is_some() {
        anyhow::bail!("Custom config loading not yet implemented. Use --size for now.");
    }

    // Read proof from file or stdin
    let mut buffer = Vec::new();
    match input {
        Some(path) => {
            let mut file = File::open(&path)
                .context(format!("Failed to open input file: {}", path))?;
            file.read_to_end(&mut buffer)
                .context("Failed to read proof from file")?;
        }
        None => {
            io::stdin()
                .read_to_end(&mut buffer)
                .context("Failed to read proof from stdin")?;
        }
    }

    // Decode based on format
    let proof_bytes = match format {
        "bincode" => buffer,
        "hex" => {
            let hex_str = String::from_utf8(buffer)
                .context("Invalid UTF-8 in hex input")?;
            hex::decode(hex_str.trim())
                .context("Failed to decode hex")?
        }
        _ => anyhow::bail!("Unknown format: {}. Use 'bincode' or 'hex'", format),
    };

    // Deserialize proof
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        bincode::deserialize(&proof_bytes)
            .context("Failed to deserialize proof")?;

    // Determine size: use provided size or infer from proof structure
    let size = match size {
        Some(s) => {
            if s < MIN_LOG_SIZE || s > MAX_LOG_SIZE {
                anyhow::bail!("Size must be between {} and {}, got {}", MIN_LOG_SIZE, MAX_LOG_SIZE, s);
            }
            s
        }
        None => {
            // Infer size from proof structure (initial_k + initial_dim = log_size)
            // We can estimate from the yr length in final proof
            let yr_len = proof.final_ligero_proof.yr.len();
            if yr_len == 0 || !yr_len.is_power_of_two() {
                anyhow::bail!("Cannot auto-detect size from proof. Please specify --size");
            }
            // yr_len = 2^k for the final round, we need to work backwards
            // For simplicity, require explicit size for now
            anyhow::bail!("Auto-detect from proof not yet implemented. Please specify --size");
        }
    };

    eprintln!("verify: 2^{} GF(2^32), {} bytes, {} transcript", size, proof_bytes.len(), transcript_type);
    if verbose {
        eprintln!("  proof structure: {} bytes", proof.size_of());
    }

    // Get verifier config using autosizer
    let config = verifier_config_for_log_size(size);

    // Verify with selected transcript backend
    let start = Instant::now();
    let valid = match transcript_type {
        "sha256" => {
            let transcript = FiatShamir::new_sha256(0);
            verify_with_transcript(&config, &proof, transcript)
        }
        "merlin" => {
            #[cfg(feature = "transcript-merlin")]
            {
                let transcript = MerlinTranscript::new(b"ligerito-v1");
                verify_with_transcript(&config, &proof, transcript)
            }
            #[cfg(not(feature = "transcript-merlin"))]
            {
                anyhow::bail!("Merlin transcript not available. Rebuild with --features transcript-merlin")
            }
        }
        _ => anyhow::bail!("Unknown transcript backend: {}. Use sha256 or merlin", transcript_type),
    }.context("Verification failed")?;
    let elapsed = start.elapsed();
    let verify_ms = elapsed.as_secs_f64() * 1000.0;

    if valid {
        eprintln!("result: VALID {:.2}ms", verify_ms);
        println!("VALID");
        Ok(())
    } else {
        eprintln!("result: INVALID {:.2}ms", verify_ms);
        println!("INVALID");
        std::process::exit(1);
    }
}

fn config_command(size: Option<u32>, generate: bool, _output_format: &str) -> Result<()> {
    if generate {
        // TODO: Implement config generation for BYOC
        anyhow::bail!("Config generation not yet implemented. This will allow creating custom config files.");
    }

    match size {
        Some(s) => {
            if s < MIN_LOG_SIZE || s > MAX_LOG_SIZE {
                anyhow::bail!("Size must be between {} and {}", MIN_LOG_SIZE, MAX_LOG_SIZE);
            }
            println!("Ligerito Configuration for 2^{}", s);
            println!("====================================");
            let config = verifier_config_for_log_size(s);
            print_config_info(&config, s);
        }
        None => {
            // Show all configs
            ligerito::autosizer::print_config_summary();
        }
    }

    Ok(())
}

fn print_config_info(config: &VerifierConfig, log_size: u32) {
    let info = config_info_for_log_size(log_size);

    println!("Polynomial size: 2^{} = {} elements", log_size, info.poly_size);
    println!("Recursive steps: {}", config.recursive_steps);
    println!("Initial k: {}", config.initial_k);
    println!("Recursive ks: {:?}", config.ks);
    println!("Log dimensions: {:?}", config.log_dims);

    // Sizes
    let poly_size_bytes = info.poly_size * 4; // 4 bytes per BinaryElem32
    println!("\nEstimated sizes:");
    println!("  Polynomial: {} bytes ({:.2} MB)",
        poly_size_bytes,
        poly_size_bytes as f64 / 1_048_576.0
    );
    println!("  Proof: ~{} KB", info.estimated_proof_bytes / 1024);
}

fn generate_command(size: u32, pattern: &str, output: Option<String>) -> Result<()> {
    if size < MIN_LOG_SIZE || size > MAX_LOG_SIZE {
        anyhow::bail!("Size must be between {} and {}", MIN_LOG_SIZE, MAX_LOG_SIZE);
    }
    let len = 1usize << size;
    eprintln!("Generating 2^{} = {} elements with pattern '{}'", size, len, pattern);

    let poly: Vec<BinaryElem32> = match pattern {
        "random" => {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            (0..len).map(|_| BinaryElem32::from(rng.gen::<u32>())).collect()
        }
        "zeros" => vec![BinaryElem32::from(0); len],
        "ones" => vec![BinaryElem32::from(1); len],
        "sequential" => (0..len).map(|i| BinaryElem32::from(i as u32)).collect(),
        _ => anyhow::bail!("Unknown pattern '{}'. Use: random, zeros, ones, sequential", pattern),
    };

    // Convert to bytes using bytemuck
    let bytes = bytemuck::cast_slice(&poly).to_vec();

    // Write output
    match output {
        Some(path) => {
            let mut file = File::create(&path)
                .context(format!("Failed to create output file: {}", path))?;
            file.write_all(&bytes)
                .context("Failed to write polynomial data")?;
            eprintln!("Wrote {} bytes to {}", bytes.len(), path);
        }
        None => {
            io::stdout().write_all(&bytes)
                .context("Failed to write polynomial data to stdout")?;
        }
    }

    Ok(())
}

fn bench_command(size: u32, iterations: usize, do_verify: bool) -> Result<()> {
    use ligerito::prove_sha256;

    if size < MIN_LOG_SIZE || size > MAX_LOG_SIZE {
        anyhow::bail!("Size must be between {} and {}", MIN_LOG_SIZE, MAX_LOG_SIZE);
    }

    eprintln!("Benchmarking 2^{} ({} elements)", size, 1usize << size);
    eprintln!("Iterations: {}", iterations);
    eprintln!("Threads: {}", rayon::current_num_threads());

    // Generate polynomial
    let poly: Vec<BinaryElem32> = (0..(1usize << size))
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    // Get configs using autosizer
    let prover_config = prover_config_for_log_size::<BinaryElem32, BinaryElem128>(size);
    let verifier_config = verifier_config_for_log_size(size);

    // Warmup
    eprintln!("Warming up...");
    let warmup_proof = prove_sha256(&prover_config, &poly).context("Warmup failed")?;

    // Benchmark proving
    eprintln!("Running prove benchmark...");
    let mut prove_times = Vec::new();
    let mut proof = warmup_proof;

    for i in 0..iterations {
        let start = Instant::now();
        proof = prove_sha256(&prover_config, &poly).context("Proving failed")?;
        let elapsed = start.elapsed();
        prove_times.push(elapsed);
        eprintln!("  Run {}: {:.2}ms", i + 1, elapsed.as_secs_f64() * 1000.0);
    }

    let avg_prove = prove_times.iter().map(|d| d.as_millis()).sum::<u128>() / prove_times.len() as u128;
    let min_prove = prove_times.iter().map(|d| d.as_millis()).min().unwrap();
    let max_prove = prove_times.iter().map(|d| d.as_millis()).max().unwrap();

    eprintln!("\nProve results:");
    eprintln!("  Average: {}ms", avg_prove);
    eprintln!("  Min: {}ms", min_prove);
    eprintln!("  Max: {}ms", max_prove);
    eprintln!("  Proof size: {} bytes", proof.size_of());

    // Benchmark verification if requested
    if do_verify {
        eprintln!("\nRunning verify benchmark...");
        let mut verify_times = Vec::new();

        for i in 0..iterations {
            let start = Instant::now();
            let valid = ligerito::verify_sha256(&verifier_config, &proof)
                .context("Verification failed")?;
            let elapsed = start.elapsed();
            verify_times.push(elapsed);
            eprintln!("  Run {}: {:.2}ms ({})", i + 1, elapsed.as_secs_f64() * 1000.0,
                if valid { "VALID" } else { "INVALID" });
        }

        let avg_verify = verify_times.iter().map(|d| d.as_millis()).sum::<u128>() / verify_times.len() as u128;
        let min_verify = verify_times.iter().map(|d| d.as_millis()).min().unwrap();

        eprintln!("\nVerify results:");
        eprintln!("  Average: {}ms", avg_verify);
        eprintln!("  Min: {}ms", min_verify);
    }

    Ok(())
}
