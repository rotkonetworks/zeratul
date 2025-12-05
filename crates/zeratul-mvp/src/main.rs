//! Zeratul MVP - Workload-Agnostic Verification Layer
//!
//! A JAM-inspired blockchain where browser clients run arbitrary workloads
//! and Zeratul just verifies proofs and accumulates results.

use clap::Parser;
use zeratul_mvp::{
    node::{Node, NodeConfig, run_block_production_loop},
    WorkPackage,
    Validator,
    BLOCK_TIME_MS,
};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser)]
#[command(name = "zeratul-mvp")]
#[command(about = "Zeratul MVP - Workload-agnostic verification layer with 1s finality")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser)]
enum Command {
    /// Run a validator node (local, no networking)
    Run {
        /// Validator ID (0, 1, or 2 for default 3-validator set)
        #[arg(short, long, default_value = "0")]
        validator: u16,

        /// Number of blocks to produce (0 = infinite)
        #[arg(short, long, default_value = "10")]
        blocks: usize,
    },

    /// Run a testnet validator node with P2P networking
    #[cfg(feature = "networking")]
    Testnet {
        /// Validator ID
        #[arg(short, long)]
        validator: u16,

        /// Listen port for P2P
        #[arg(short, long, default_value = "30333")]
        port: u16,

        /// Bootstrap peer address (e.g., /ip4/127.0.0.1/tcp/30333/p2p/PEER_ID)
        #[arg(short = 'b', long)]
        bootstrap: Option<String>,
    },

    /// Benchmark block production with work packages
    Bench {
        /// Number of blocks to produce
        #[arg(short, long, default_value = "5")]
        blocks: usize,

        /// Work packages per block
        #[arg(short, long, default_value = "10")]
        work: usize,
    },

    /// Show chain info
    Info,
}

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive("zeratul_mvp=info".parse().unwrap()))
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Run { validator, blocks } => {
            run_validator(validator, blocks).await;
        }
        #[cfg(feature = "networking")]
        Command::Testnet { validator, port, bootstrap } => {
            run_testnet_validator(validator, port, bootstrap).await;
        }
        Command::Bench { blocks, work } => {
            run_benchmark(blocks, work).await;
        }
        Command::Info => {
            show_info();
        }
    }
}

async fn run_validator(validator_id: u16, blocks: usize) {
    tracing::info!("Starting Zeratul MVP validator {}", validator_id);
    tracing::info!("Block time: {}ms", BLOCK_TIME_MS);

    let config = NodeConfig {
        validator_id: Some(validator_id),
        signing_key: Some([validator_id as u8; 32]),
        ..Default::default()
    };

    let node = Node::new(config);

    tracing::info!("Genesis state initialized");
    tracing::info!("Validators: {}", node.state().active_validator_count());

    let results = run_block_production_loop(node, blocks).await;

    // Print summary
    println!("\n=== Block Production Summary ===");
    println!("Blocks produced: {}", results.len());

    if !results.is_empty() {
        let total_time: u64 = results.iter().map(|r| r.total_time_ms).sum();
        let avg_time = total_time / results.len() as u64;

        let total_prove: u64 = results.iter().map(|r| r.prove_time_ms).sum();
        let avg_prove = total_prove / results.len() as u64;

        let total_size: usize = results.iter().map(|r| r.proof_size).sum();
        let avg_size = total_size / results.len();

        let within_budget = results.iter().filter(|r| r.within_budget()).count();

        println!("Average block time: {}ms", avg_time);
        println!("Average prove time: {}ms", avg_prove);
        println!("Average proof size: {} bytes", avg_size);
        println!("Blocks within 1s budget: {}/{}", within_budget, results.len());
    }
}

async fn run_benchmark(blocks: usize, work_per_block: usize) {
    tracing::info!("Running Zeratul MVP benchmark");
    tracing::info!("Blocks: {}, Work packages per block: {}", blocks, work_per_block);

    // Single-validator setup for benchmark
    let config = NodeConfig {
        validator_id: Some(0),
        signing_key: Some([0u8; 32]),
        validators: vec![
            Validator { pubkey: [1u8; 32], stake: 1000, active: true },
        ],
    };

    let mut node = Node::new(config);
    let mut results = Vec::new();

    for block_num in 0..blocks {
        // Submit work packages
        for i in 0..work_per_block {
            let package = WorkPackage {
                service: 0, // Null service for testing
                payload: vec![(block_num * work_per_block + i) as u8; 32],
                gas_limit: 10000,
                proof: vec![], // Empty proof - MVP accepts for testing
                output_hash: [(block_num * work_per_block + i) as u8; 32],
                output: None,
                signature: [0u8; 64],
                submitter: [0u8; 32],
            };
            node.submit_work(package);
        }

        // Produce block
        if let Some(result) = node.produce_block() {
            println!(
                "Block {}: {}ms total (verify: {}ms, prove: {}ms), {} bytes, {} results",
                result.block.header.height,
                result.total_time_ms,
                result.verify_time_ms,
                result.prove_time_ms,
                result.proof_size,
                result.result_count,
            );

            // Apply block to update state for next round
            node.apply_block(result.block.clone()).unwrap();
            results.push(result);
        }
    }

    // Summary
    println!("\n=== Benchmark Summary ===");

    if !results.is_empty() {
        let total_time: u64 = results.iter().map(|r| r.total_time_ms).sum();
        let avg_time = total_time / results.len() as u64;
        let min_time = results.iter().map(|r| r.total_time_ms).min().unwrap();
        let max_time = results.iter().map(|r| r.total_time_ms).max().unwrap();

        let total_prove: u64 = results.iter().map(|r| r.prove_time_ms).sum();
        let avg_prove = total_prove / results.len() as u64;

        let total_verify: u64 = results.iter().map(|r| r.verify_time_ms).sum();
        let avg_verify = total_verify / results.len() as u64;

        let total_size: usize = results.iter().map(|r| r.proof_size).sum();
        let avg_size = total_size / results.len();

        let total_results: usize = results.iter().map(|r| r.result_count).sum();
        let rps = if total_time > 0 {
            (total_results as f64 / total_time as f64) * 1000.0
        } else {
            0.0
        };

        let within_budget = results.iter().filter(|r| r.within_budget()).count();

        println!("Total blocks: {}", results.len());
        println!("Total work results: {}", total_results);
        println!("Throughput: {:.1} results/sec", rps);
        println!();
        println!("Block time: avg {}ms, min {}ms, max {}ms", avg_time, min_time, max_time);
        println!("Verification time: avg {}ms", avg_verify);
        println!("Proving time: avg {}ms", avg_prove);
        println!("Proof size: avg {} bytes", avg_size);
        println!();
        println!("Blocks within 1s budget: {}/{} ({:.1}%)",
            within_budget, results.len(),
            (within_budget as f64 / results.len() as f64) * 100.0
        );
    }
}

#[cfg(feature = "networking")]
async fn run_testnet_validator(validator_id: u16, port: u16, bootstrap: Option<String>) {
    use zeratul_mvp::{
        NetworkService, NetworkConfig, NetworkEvent, NetworkMessage,
        node::{Node, NodeConfig},
    };

    tracing::info!("Starting Zeratul testnet validator {}", validator_id);
    tracing::info!("P2P port: {}", port);

    // Initialize network
    let net_config = NetworkConfig {
        port,
        bootstrap_peers: bootstrap.clone().map(|b| vec![b]).unwrap_or_default(),
        validator_id: Some(validator_id),
    };

    let mut network = match NetworkService::new(net_config).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("Failed to initialize network: {}", e);
            return;
        }
    };

    tracing::info!("Local peer ID: {}", network.local_peer_id());

    // Connect to bootstrap peer if provided
    if let Some(ref bootstrap_addr) = bootstrap {
        tracing::info!("Connecting to bootstrap peer: {}", bootstrap_addr);
        if let Err(e) = network.connect(bootstrap_addr).await {
            tracing::warn!("Failed to connect to bootstrap: {}", e);
        }
    }

    // Initialize node
    let node_config = NodeConfig {
        validator_id: Some(validator_id),
        signing_key: Some([validator_id as u8; 32]),
        ..Default::default()
    };

    let mut node = Node::new(node_config);
    tracing::info!("Genesis state initialized");
    tracing::info!("Validators: {}", node.state().active_validator_count());

    // Main event loop
    let mut block_timer = tokio::time::interval(std::time::Duration::from_millis(BLOCK_TIME_MS));

    loop {
        tokio::select! {
            // Block production timer
            _ = block_timer.tick() => {
                // Try to produce a block
                if let Some(result) = node.produce_block() {
                    tracing::info!(
                        "Produced block {} ({} results, {}ms)",
                        result.block.header.height,
                        result.result_count,
                        result.total_time_ms
                    );

                    // Apply block locally
                    if let Err(e) = node.apply_block(result.block.clone()) {
                        tracing::error!("Failed to apply block: {:?}", e);
                    }

                    // Broadcast block to peers
                    // Note: In production, would also collect votes
                }
            }

            // Network events
            event = network.poll() => {
                if let Some(event) = event {
                    match event {
                        NetworkEvent::PeerConnected(peer) => {
                            tracing::info!("Peer connected: {}", peer);
                        }
                        NetworkEvent::PeerDisconnected(peer) => {
                            tracing::info!("Peer disconnected: {}", peer);
                        }
                        NetworkEvent::Message(peer, msg) => {
                            tracing::debug!("Message from {}: {:?}", peer, msg);
                            match msg {
                                NetworkMessage::WorkPackage(pkg) => {
                                    node.submit_work(pkg);
                                }
                                NetworkMessage::Block(block) => {
                                    if let Err(e) = node.apply_block(block) {
                                        tracing::warn!("Invalid block from {}: {:?}", peer, e);
                                    }
                                }
                                NetworkMessage::Vote(vote) => {
                                    tracing::debug!("Vote from {}: {:?}", peer, vote);
                                    // TODO: Collect votes for finality
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
}

fn show_info() {
    println!("Zeratul MVP - Workload-Agnostic Verification Layer");
    println!();
    println!("Architecture:");
    println!("  Block time:          {}ms", BLOCK_TIME_MS);
    println!("  Proof budget:        {}ms", zeratul_mvp::PROOF_BUDGET_MS);
    println!("  Verify budget:       {}ms", zeratul_mvp::VERIFY_BUDGET_MS);
    println!("  Network budget:      {}ms", zeratul_mvp::NETWORK_BUDGET_MS);
    println!("  Max results/block:   {}", zeratul_mvp::MAX_RESULTS_PER_BLOCK);
    println!();
    println!("Consensus:");
    println!("  Type:                Leaderless BFT");
    println!("  Finality:            Immediate (with 2/3+1 votes)");
    println!("  Block proposal:      Any validator (proof-based)");
    println!();
    println!("Model:");
    println!("  Computation:         Browser clients (WASM)");
    println!("  Verification:        Zeratul chain");
    println!("  Data Availability:   2D ZODA");
    println!();
    println!("Proving:");
    println!("  System:              Ligerito (polynomial commitments)");
    println!("  Field:               Binary extension fields (F2^32/F2^128)");
    println!("  Security:            100-bit soundness");
}
