//! Zeratul Validator Node
//!
//! MVP network implementation using litep2p uniformly for all networking.
//!
//! ## Architecture
//!
//! - **Networking**: litep2p with QUIC transport (JAM-style)
//! - **DKG**: Golden DKG crypto primitives with litep2p messaging
//! - **Consensus**: Safrole (ticket-based) + GRANDPA-style finality
//! - **State**: NOMT (Nearly-Optimal Merkle Trie)
//!
//! ## Startup Flow
//!
//! 1. Load validator keys (BLS + Ed25519)
//! 2. Connect to bootstrap peers via litep2p
//! 3. Run DKG to generate threshold keys for current epoch
//! 4. Start consensus and block production
//! 5. Sync state from peers if behind

use anyhow::Result;
use clap::Parser;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::{info, warn, Level};
use tracing_subscriber;

use zeratul_blockchain::dkg::frost_provider::FrostProvider;
use zeratul_blockchain::dkg::DKGProvider;
use zeratul_blockchain::privacy::HybridPrivacy;
use zeratul_blockchain::network::quic::{NetworkConfig, NetworkService};
use zeratul_blockchain::network::crypto_compat::Ed25519PrivateKey;
use zeratul_blockchain::network::protocols::{BlockAnnounce, ConsensusMessage, Vote, FinalityCertificate};
use zeratul_blockchain::block::Block;
use commonware_cryptography::sha256::Digest;
use std::collections::HashMap;

/// Validator CLI configuration
#[derive(Parser, Debug)]
#[command(name = "zeratul-validator")]
#[command(about = "Zeratul blockchain validator node", long_about = None)]
struct Cli {
    /// Config file path
    #[arg(long, short)]
    config: Option<PathBuf>,

    /// Validator index (0-indexed, overrides config)
    #[arg(long)]
    validator_index: Option<u32>,

    /// Listen address for P2P networking (overrides config)
    #[arg(long)]
    listen_addr: Option<SocketAddr>,

    /// Log level (error, warn, info, debug, trace)
    #[arg(long, default_value = "info")]
    log_level: Level,
}

/// Validator configuration (from YAML file)
#[derive(Debug, Deserialize)]
struct Config {
    validator: ValidatorConfig,
    network: NetworkYamlConfig,
    dkg: DKGConfig,
    storage: StorageConfig,
    #[serde(default)]
    logging: LoggingConfig,
}

#[derive(Debug, Deserialize)]
struct ValidatorConfig {
    index: u32,
    name: String,
}

#[derive(Debug, Deserialize)]
struct NetworkYamlConfig {
    listen_addr: String,
    peers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DKGConfig {
    validator_count: u32,
    threshold: u32,
    epoch: u64,
}

#[derive(Debug, Deserialize)]
struct StorageConfig {
    path: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
struct LoggingConfig {
    #[serde(default = "default_log_level")]
    level: String,
    file: Option<PathBuf>,
}

fn default_log_level() -> String {
    "info".to_string()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load configuration
    let config = if let Some(config_path) = &cli.config {
        let config_str = std::fs::read_to_string(config_path)?;
        serde_yaml::from_str::<Config>(&config_str)?
    } else {
        anyhow::bail!("Config file required (use --config <path>)");
    };

    // Setup logging
    tracing_subscriber::fmt()
        .with_max_level(cli.log_level)
        .with_target(false)
        .compact()
        .init();

    let validator_index = cli.validator_index.unwrap_or(config.validator.index);

    info!(
        validator_index,
        validator_name = %config.validator.name,
        listen_addr = %config.network.listen_addr,
        data_dir = ?config.storage.path,
        "🚀 Starting Zeratul validator"
    );

    // Create data directory
    std::fs::create_dir_all(&config.storage.path)?;

    // Initialize DKG provider
    info!(
        validator_count = config.dkg.validator_count,
        threshold = config.dkg.threshold,
        epoch = config.dkg.epoch,
        "Initializing FROST DKG"
    );

    let mut dkg_provider = FrostProvider::new();

    // Initialize privacy layer
    info!("Initializing 3-tier privacy system");
    let _privacy = HybridPrivacy::new(
        validator_index,
        config.dkg.validator_count,
        config.dkg.threshold,
    );

    info!("✅ Validator components initialized");

    // Start DKG ceremony for epoch 0
    info!("Starting DKG ceremony for epoch {}...", config.dkg.epoch);

    match dkg_provider.start_ceremony(
        config.dkg.epoch,
        validator_index,
        config.dkg.validator_count,
        config.dkg.threshold,
    ) {
        Ok(_round1_msg) => {
            info!("✅ DKG Round 1 complete (commitments generated)");
            // Note: For MVP, we skip network coordination of DKG rounds 2/3
            // In production, we would broadcast round1_msg and wait for others
            // For now, each validator generates their own keys deterministically
        }
        Err(e) => {
            warn!("DKG ceremony initialization failed: {}", e);
        }
    }

    info!("📦 Validator ready for block production (DKG simplified for MVP)");

    // Initialize network
    info!("Initializing network layer...");

    // Parse listen address
    let listen_addr: SocketAddr = config.network.listen_addr.parse()
        .map_err(|e| anyhow::anyhow!("Invalid listen address: {}", e))?;

    // Pass bootstrap peer multiaddrs directly
    let bootstrap_peers: Vec<String> = config.network.peers.clone();

    // Generate keypair for this validator (deterministic from index)
    let mut secret_bytes = [0u8; 32];
    secret_bytes[0] = validator_index as u8 + 1;
    let private_key = Ed25519PrivateKey::from_bytes(&secret_bytes)?;
    let public_key = private_key.public_key();

    let network_config = NetworkConfig {
        keypair: (private_key.clone(), public_key),
        listen_addrs: vec![listen_addr],
        genesis_hash: [0u8; 32],
        bootstrap_peers,
    };

    // Create network service
    let (network_service, mut network_handles) = NetworkService::new(network_config).await?;

    info!(
        peer_id = ?network_service.local_peer_id(),
        "Network service initialized"
    );

    info!("📡 Validator ready - starting network...");

    // Run network in background
    let network_handle = tokio::spawn(async move {
        if let Err(e) = network_service.run().await {
            warn!("Network service error: {}", e);
        }
    });

    info!("Press Ctrl+C to shutdown");

    // Block production configuration
    let slot_duration = std::time::Duration::from_secs(1); // 1 second slots
    let genesis_time = std::time::SystemTime::now();
    let mut last_slot = 0u64;
    let mut block_height = 0u64;
    let mut parent_hash: Digest = Digest::from([0u8; 32]); // Genesis parent

    // Voting state for single-slot finality
    // Key: block hash (as bytes), Value: Vec of (validator_index, signature)
    let mut votes_by_block: HashMap<[u8; 32], Vec<(u32, Vec<u8>)>> = HashMap::new();
    let threshold = config.dkg.threshold as usize; // 2f+1 votes needed
    let mut finalized_height = 0u64;

    info!("⏱️  Starting block production ({}ms slots, threshold={})", slot_duration.as_millis(), threshold);

    // Block production loop
    let mut slot_timer = tokio::time::interval(slot_duration);
    loop {
        tokio::select! {
            // Slot timer tick
            _ = slot_timer.tick() => {
                let elapsed = genesis_time.elapsed().unwrap_or_default();
                let current_slot = elapsed.as_secs() / slot_duration.as_secs();

                if current_slot > last_slot {
                    last_slot = current_slot;

                    // Simple round-robin leader selection
                    let leader = (current_slot % config.dkg.validator_count as u64) as u32;

                    if leader == validator_index {
                        // We are the leader for this slot - produce block
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64;

                        block_height += 1;

                        // Create empty block (no proofs for MVP)
                        let block = Block::new(
                            parent_hash,
                            block_height,
                            current_slot,
                            timestamp,
                            [0u8; 32], // Empty state root for MVP
                            vec![],    // No proofs for MVP
                            vec![validator_index as u8; 48], // Dummy pubkey
                            vec![],    // No signature for MVP
                        );

                        parent_hash = block.digest();

                        info!(
                            slot = current_slot,
                            height = block_height,
                            "🔨 Produced block"
                        );

                        // Broadcast block to peers
                        let announce = BlockAnnounce::from_block(&block);
                        let msg = ConsensusMessage::BlockAnnounce(announce.clone());
                        if let Err(e) = network_handles.consensus_tx.send(msg) {
                            warn!("Failed to broadcast block: {}", e);
                        }

                        // Self-vote for our own block
                        let hash_slice = parent_hash.as_ref();
                        let signature = private_key.sign(hash_slice);

                        // Add our vote to the collection
                        let mut hash_bytes = [0u8; 32];
                        hash_bytes.copy_from_slice(hash_slice);
                        let votes = votes_by_block.entry(hash_bytes).or_insert_with(Vec::new);
                        votes.push((validator_index, signature));

                        info!(
                            height = block_height,
                            "🗳️ Self-voted for produced block"
                        );
                    } else {
                        // We're not the leader, wait for block from leader
                        // info!(slot = current_slot, leader, "Waiting for block from validator {}", leader);
                    }
                }
            }

            // Receive consensus messages from network
            Some(msg) = network_handles.consensus_rx.recv() => {
                match msg {
                    ConsensusMessage::BlockAnnounce(block_announce) => {
                        info!(
                            height = block_announce.height,
                            slot = block_announce.timeslot,
                            "📥 Received block from peer"
                        );

                        // Update chain state if this is a new best block
                        if block_announce.height > block_height {
                            block_height = block_announce.height;
                            parent_hash = block_announce.hash;
                            last_slot = block_announce.timeslot;

                            info!(
                                height = block_height,
                                slot = last_slot,
                                "✅ Imported block"
                            );

                            // Sign the block hash and broadcast vote
                            let hash_slice = block_announce.hash.as_ref();
                            let signature = private_key.sign(hash_slice);

                            let vote = Vote {
                                block_hash: block_announce.hash,
                                height: block_announce.height,
                                timeslot: block_announce.timeslot,
                                validator_index,
                                signature,
                            };

                            let vote_msg = ConsensusMessage::Vote(vote);
                            if let Err(e) = network_handles.consensus_tx.send(vote_msg) {
                                warn!("Failed to broadcast vote: {}", e);
                            } else {
                                info!(
                                    height = block_announce.height,
                                    "🗳️ Broadcast vote for block"
                                );
                            }
                        }
                    }
                    ConsensusMessage::Vote(vote) => {
                        info!(
                            height = vote.height,
                            slot = vote.timeslot,
                            from = vote.validator_index,
                            "🗳️ Received vote"
                        );

                        // Collect votes for this block
                        let hash_slice = vote.block_hash.as_ref();
                        let mut hash_bytes = [0u8; 32];
                        hash_bytes.copy_from_slice(hash_slice);
                        let votes = votes_by_block.entry(hash_bytes).or_insert_with(Vec::new);

                        // Check for duplicate votes from same validator
                        if !votes.iter().any(|(idx, _)| *idx == vote.validator_index) {
                            votes.push((vote.validator_index, vote.signature.clone()));

                            info!(
                                height = vote.height,
                                votes = votes.len(),
                                threshold,
                                "Vote collected"
                            );

                            // Check if we have enough votes for finality
                            if votes.len() >= threshold && vote.height > finalized_height {
                                finalized_height = vote.height;

                                // Create finality certificate
                                let signers: Vec<u32> = votes.iter().map(|(idx, _)| *idx).collect();

                                // For MVP, concatenate signatures (real impl would aggregate BLS)
                                let aggregate_signature: Vec<u8> = votes.iter()
                                    .flat_map(|(_, sig)| sig.clone())
                                    .collect();

                                let cert = FinalityCertificate {
                                    block_hash: vote.block_hash,
                                    height: vote.height,
                                    aggregate_signature,
                                    signers: signers.clone(),
                                };

                                let cert_msg = ConsensusMessage::Finality(cert);
                                if let Err(e) = network_handles.consensus_tx.send(cert_msg) {
                                    warn!("Failed to broadcast finality certificate: {}", e);
                                } else {
                                    info!(
                                        height = vote.height,
                                        signers = ?signers,
                                        "🏁 Broadcast finality certificate"
                                    );
                                }

                                // Clean up old votes (keep only recent blocks)
                                if votes_by_block.len() > 100 {
                                    votes_by_block.clear();
                                }
                            }
                        }
                    }
                    ConsensusMessage::Finality(cert) => {
                        info!(
                            height = cert.height,
                            signers = cert.signers.len(),
                            "🏁 Received finality certificate"
                        );

                        // Update finalized height if this is newer
                        if cert.height > finalized_height {
                            finalized_height = cert.height;
                            info!(
                                height = cert.height,
                                signers = ?cert.signers,
                                "✅ Block FINALIZED"
                            );
                        }
                    }
                }
            }

            // Shutdown signal
            _ = tokio::signal::ctrl_c() => {
                info!("🛑 Shutting down validator");
                break;
            }
        }
    }

    // Cancel network task
    network_handle.abort();

    // Check if DKG completed
    if dkg_provider.is_complete(config.dkg.epoch) {
        info!("DKG ceremony completed successfully");

        // Get group public key
        if let Ok(group_pubkey) = dkg_provider.get_group_pubkey(config.dkg.epoch) {
            info!("Group public key: {:?}", hex::encode(&group_pubkey));
        }
    } else {
        warn!("DKG ceremony not completed");
    }

    Ok(())
}
