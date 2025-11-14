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
    prove, verify,
    hardcoded_config_12, hardcoded_config_12_verifier,
    hardcoded_config_16, hardcoded_config_16_verifier,
    hardcoded_config_20, hardcoded_config_20_verifier,
    hardcoded_config_24, hardcoded_config_24_verifier,
    hardcoded_config_28, hardcoded_config_28_verifier,
    hardcoded_config_30, hardcoded_config_30_verifier,
    VerifierConfig, FinalizedLigeritoProof,
};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::fs::File;

#[derive(Parser)]
#[command(name = "ligerito")]
#[command(about = "Ligerito polynomial commitment scheme CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a proof for a polynomial (read from stdin, write to stdout)
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

        /// Transcript backend: sha256, merlin, or blake3
        /// WARNING: Currently defaults to SHA256 regardless of this flag
        /// Prover and verifier MUST use the same backend!
        /// TODO: Implement runtime transcript selection
        #[arg(short, long, default_value = "sha256")]
        transcript: String,
    },

    /// Verify a proof (read from stdin, exit with code 0 if valid)
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

        /// Transcript backend: sha256, merlin, or blake3
        /// WARNING: Currently defaults to SHA256 regardless of this flag
        /// MUST match the transcript used for proving!
        /// TODO: Implement runtime transcript selection
        #[arg(short, long, default_value = "sha256")]
        transcript: String,

        /// Verbose output
        #[arg(short, long)]
        verbose: bool,
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
        Commands::Prove { size, config, format, transcript } => {
            if transcript != "sha256" && transcript != "merlin" && transcript != "blake3" {
                eprintln!("Warning: Unknown transcript '{}', using sha256", transcript);
            }
            prove_command(size, config, &format)?;
        }
        Commands::Verify { size, config, format, transcript, verbose } => {
            if transcript != "sha256" && transcript != "merlin" && transcript != "blake3" {
                eprintln!("Warning: Unknown transcript '{}', using sha256", transcript);
            }
            verify_command(size, config, &format, verbose)?;
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

fn prove_command(size: Option<usize>, config_path: Option<String>, format: &str) -> Result<()> {
    // TODO: Implement custom config loading for BYOC
    if config_path.is_some() {
        anyhow::bail!("Custom config loading not yet implemented. Use --size for now.");
    }

    let size = size.context("Must specify --size")?;

    // Read polynomial from stdin
    let mut buffer = Vec::new();
    io::stdin()
        .read_to_end(&mut buffer)
        .context("Failed to read polynomial from stdin")?;

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

    eprintln!("Read polynomial of size 2^{} ({} elements)", size, poly.len());

    // Get prover config
    let proof = match size {
        12 => {
            let config = hardcoded_config_12(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove(&config, &poly).context("Proving failed")?
        }
        16 => {
            let config = hardcoded_config_16(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove(&config, &poly).context("Proving failed")?
        }
        20 => {
            let config = hardcoded_config_20(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove(&config, &poly).context("Proving failed")?
        }
        24 => {
            let config = hardcoded_config_24(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove(&config, &poly).context("Proving failed")?
        }
        28 => {
            let config = hardcoded_config_28(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove(&config, &poly).context("Proving failed")?
        }
        30 => {
            let config = hardcoded_config_30(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove(&config, &poly).context("Proving failed")?
        }
        _ => anyhow::bail!("Unsupported size: {}. Must be 12, 16, 20, 24, 28, or 30", size),
    };

    eprintln!("Proof generated successfully");

    // Serialize and output
    match format {
        "bincode" => {
            let encoded = bincode::serialize(&proof)
                .context("Failed to serialize proof")?;
            io::stdout()
                .write_all(&encoded)
                .context("Failed to write proof to stdout")?;
        }
        "hex" => {
            let encoded = bincode::serialize(&proof)
                .context("Failed to serialize proof")?;
            let hex_str = hex::encode(&encoded);
            println!("{}", hex_str);
        }
        _ => anyhow::bail!("Unknown format: {}. Use 'bincode' or 'hex'", format),
    }

    Ok(())
}

fn verify_command(size: Option<usize>, config_path: Option<String>, format: &str, verbose: bool) -> Result<()> {
    // TODO: Implement custom config loading for BYOC
    if config_path.is_some() {
        anyhow::bail!("Custom config loading not yet implemented. Use --size for now.");
    }

    let size = size.context("Must specify --size")?;

    // Read proof from stdin
    let mut buffer = Vec::new();
    io::stdin()
        .read_to_end(&mut buffer)
        .context("Failed to read proof from stdin")?;

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

    if verbose {
        eprintln!("Proof size: {} bytes", proof_bytes.len());
        eprintln!("Proof structure size: {} bytes", proof.size_of());
    }

    // Get verifier config and verify
    let valid = match size {
        12 => {
            let config = hardcoded_config_12_verifier();
            verify(&config, &proof).context("Verification failed")?
        }
        16 => {
            let config = hardcoded_config_16_verifier();
            verify(&config, &proof).context("Verification failed")?
        }
        20 => {
            let config = hardcoded_config_20_verifier();
            verify(&config, &proof).context("Verification failed")?
        }
        24 => {
            let config = hardcoded_config_24_verifier();
            verify(&config, &proof).context("Verification failed")?
        }
        28 => {
            let config = hardcoded_config_28_verifier();
            verify(&config, &proof).context("Verification failed")?
        }
        30 => {
            let config = hardcoded_config_30_verifier();
            verify(&config, &proof).context("Verification failed")?
        }
        _ => anyhow::bail!("Unsupported size: {}. Must be 12, 16, 20, 24, 28, or 30", size),
    };

    if valid {
        println!("VALID");
        Ok(())
    } else {
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
