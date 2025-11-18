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
    hardcoded_config_12, hardcoded_config_12_verifier,
    hardcoded_config_16, hardcoded_config_16_verifier,
    hardcoded_config_20, hardcoded_config_20_verifier,
    hardcoded_config_24, hardcoded_config_24_verifier,
    hardcoded_config_28, hardcoded_config_28_verifier,
    hardcoded_config_30, hardcoded_config_30_verifier,
    VerifierConfig, FinalizedLigeritoProof,
};
use ligerito::transcript::{Sha256Transcript, MerlinTranscript};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::io::{self, Read, Write};
use std::marker::PhantomData;
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
    Prove {
        /// Log2 of polynomial size (12, 16, 20, 24, 28, or 30) - mutually exclusive with --config
        #[arg(short, long, conflicts_with = "config")]
        size: Option<usize>,

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
    Verify {
        /// Log2 of polynomial size (12, 16, 20, 24, 28, or 30) - mutually exclusive with --config
        #[arg(short, long, conflicts_with = "config")]
        size: Option<usize>,

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

    /// Show or generate configuration
    Config {
        /// Log2 of polynomial size (for hardcoded configs)
        #[arg(short, long)]
        size: Option<usize>,

        /// Generate a template config file for custom use (BYOC)
        #[arg(long)]
        generate: bool,

        /// Output format for generated config: json (default) or toml
        #[arg(long, default_value = "json")]
        output_format: String,
    },

    /// Generate random polynomial data for testing
    Generate {
        /// Log2 of polynomial size
        #[arg(short, long)]
        size: usize,

        /// Pattern: random (default), zeros, ones, sequential
        #[arg(short, long, default_value = "random")]
        pattern: String,

        /// Output file (stdout if not specified)
        #[arg(short, long)]
        output: Option<String>,
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
    }

    Ok(())
}

fn prove_command(size: Option<usize>, config_path: Option<String>, format: &str, transcript_type: &str, input: Option<String>) -> Result<()> {
    // TODO: Implement custom config loading for BYOC
    if config_path.is_some() {
        anyhow::bail!("Custom config loading not yet implemented. Use --size for now.");
    }

    let size = size.context("Must specify --size")?;

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

    // Parse polynomial based on size
    let expected_len = 1 << size;
    let elem_size = std::mem::size_of::<BinaryElem32>();

    if buffer.len() != expected_len * elem_size {
        anyhow::bail!(
            "Expected {} bytes ({} elements of {} bytes), got {}",
            expected_len * elem_size,
            expected_len,
            elem_size,
            buffer.len()
        );
    }

    // Convert bytes to polynomial (assuming u32 representation for BinaryElem32)
    let poly: Vec<BinaryElem32> = buffer
        .chunks_exact(4)
        .map(|chunk| {
            let val = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            BinaryElem32::from(val)
        })
        .collect();

    // Print build info to help diagnose performance issues
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

    // Helper macro to prove with any transcript backend
    macro_rules! prove_with_backend {
        ($config:expr, $transcript_type:expr) => {
            match $transcript_type {
                "sha256" => {
                    let transcript = Sha256Transcript::new(0);
                    prove_with_transcript(&$config, &poly, transcript)
                }
                "merlin" => {
                    #[cfg(feature = "transcript-merlin")]
                    {
                        let transcript = MerlinTranscript::new(b"ligerito-v1");
                        prove_with_transcript(&$config, &poly, transcript)
                    }
                    #[cfg(not(feature = "transcript-merlin"))]
                    {
                        anyhow::bail!("Merlin transcript not available. Rebuild with --features transcript-merlin")
                    }
                }
                _ => anyhow::bail!("Unknown transcript backend: {}. Use sha256 or merlin", $transcript_type),
            }
        };
    }

    // Get prover config and prove
    let start = Instant::now();
    let proof = match size {
        12 => {
            let config = hardcoded_config_12(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prove_with_backend!(config, transcript_type).context("Proving failed")?
        }
        16 => {
            let config = hardcoded_config_16(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prove_with_backend!(config, transcript_type).context("Proving failed")?
        }
        20 => {
            let config = hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prove_with_backend!(config, transcript_type).context("Proving failed")?
        }
        24 => {
            let config = hardcoded_config_24(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prove_with_backend!(config, transcript_type).context("Proving failed")?
        }
        28 => {
            let config = hardcoded_config_28(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prove_with_backend!(config, transcript_type).context("Proving failed")?
        }
        30 => {
            let config = hardcoded_config_30(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prove_with_backend!(config, transcript_type).context("Proving failed")?
        }
        _ => anyhow::bail!("Unsupported size: {}. Must be 12, 16, 20, 24, 28, or 30", size),
    };
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

fn verify_command(size: Option<usize>, config_path: Option<String>, format: &str, transcript_type: &str, verbose: bool, input: Option<String>) -> Result<()> {
    // TODO: Implement custom config loading for BYOC
    if config_path.is_some() {
        anyhow::bail!("Custom config loading not yet implemented. Use --size for now.");
    }

    let size = size.context("Must specify --size")?;

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

    eprintln!("verify: 2^{} GF(2^32), {} bytes, {} transcript", size, proof_bytes.len(), transcript_type);
    if verbose {
        eprintln!("  proof structure: {} bytes", proof.size_of());
    }

    // Helper macro to verify with any transcript backend
    macro_rules! verify_with_backend {
        ($config:expr, $proof:expr, $transcript_type:expr) => {
            match $transcript_type {
                "sha256" => {
                    let transcript = Sha256Transcript::new(0);
                    verify_with_transcript(&$config, &$proof, transcript)
                }
                "merlin" => {
                    #[cfg(feature = "transcript-merlin")]
                    {
                        let transcript = MerlinTranscript::new(b"ligerito-v1");
                        verify_with_transcript(&$config, &$proof, transcript)
                    }
                    #[cfg(not(feature = "transcript-merlin"))]
                    {
                        anyhow::bail!("Merlin transcript not available. Rebuild with --features transcript-merlin")
                    }
                }
                _ => anyhow::bail!("Unknown transcript backend: {}. Use sha256 or merlin", $transcript_type),
            }
        };
    }

    // Get verifier config and verify
    let start = Instant::now();
    let valid = match size {
        12 => {
            let config = hardcoded_config_12_verifier();
            verify_with_backend!(config, proof, transcript_type).context("Verification failed")?
        }
        16 => {
            let config = hardcoded_config_16_verifier();
            verify_with_backend!(config, proof, transcript_type).context("Verification failed")?
        }
        20 => {
            let config = hardcoded_config_20_verifier();
            verify_with_backend!(config, proof, transcript_type).context("Verification failed")?
        }
        24 => {
            let config = hardcoded_config_24_verifier();
            verify_with_backend!(config, proof, transcript_type).context("Verification failed")?
        }
        28 => {
            let config = hardcoded_config_28_verifier();
            verify_with_backend!(config, proof, transcript_type).context("Verification failed")?
        }
        30 => {
            let config = hardcoded_config_30_verifier();
            verify_with_backend!(config, proof, transcript_type).context("Verification failed")?
        }
        _ => anyhow::bail!("Unsupported size: {}. Must be 12, 16, 20, 24, 28, or 30", size),
    };
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

fn config_command(size: Option<usize>, generate: bool, _output_format: &str) -> Result<()> {
    if generate {
        // TODO: Implement config generation for BYOC
        anyhow::bail!("Config generation not yet implemented. This will allow creating custom config files.");
    }

    let size = size.context("Must specify --size")?;

    println!("Ligerito Configuration for 2^{}", size);
    println!("====================================");

    match size {
        12 => print_config_info(&hardcoded_config_12_verifier()),
        16 => print_config_info(&hardcoded_config_16_verifier()),
        20 => print_config_info(&hardcoded_config_20_verifier()),
        24 => print_config_info(&hardcoded_config_24_verifier()),
        28 => print_config_info(&hardcoded_config_28_verifier()),
        30 => print_config_info(&hardcoded_config_30_verifier()),
        _ => anyhow::bail!("Unsupported size: {}. Must be 12, 16, 20, 24, 28, or 30", size),
    }

    Ok(())
}

fn print_config_info(config: &VerifierConfig) {
    println!("Polynomial elements: 2^{} = {}",
        config.initial_dim,
        1 << config.initial_dim
    );
    println!("Recursive steps: {}", config.recursive_steps);
    println!("Initial k: {}", config.initial_k);
    println!("Recursive ks: {:?}", config.ks);
    println!("Log dimensions: {:?}", config.log_dims);

    // Estimate sizes
    let poly_size_bytes = (1 << config.initial_dim) * 4; // 4 bytes per BinaryElem32
    println!("\nEstimated sizes:");
    println!("  Polynomial: {} bytes ({:.2} MB)",
        poly_size_bytes,
        poly_size_bytes as f64 / 1_048_576.0
    );
}

fn generate_command(size: usize, pattern: &str, output: Option<String>) -> Result<()> {
    let len = 1 << size;
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
