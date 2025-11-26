//! IBP Probe CLI
//!
//! Runs provable monitoring checks against IBP endpoints.
//! Produces execution traces suitable for ligerito proof generation.
//!
//! ## Usage
//!
//! ```bash
//! # Run check against a single endpoint
//! ibp-probe check --endpoint wss://rpc.rotko.net --network polkadot
//!
//! # Batch check from config
//! ibp-probe batch --config endpoints.json
//!
//! # Generate trace (no network I/O, dummy data)
//! ibp-probe trace --program probe.polkavm --output trace.json
//! ```

use clap::{Parser, Subcommand};
use anyhow::Result;
use tracing::info;

use ibp_probe_host::{
    IbpHost, CheckConfig, CheckTarget, CheckResult, CheckParams, ServiceType,
    IbpMonitorResult,
};

#[derive(Parser)]
#[command(name = "ibp-probe")]
#[command(about = "IBP monitoring probe with provable execution")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Verbosity level
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a single endpoint check
    Check {
        /// Endpoint URL (e.g., wss://rpc.rotko.net/polkadot)
        #[arg(short, long)]
        endpoint: String,

        /// Network name (polkadot, kusama, etc.)
        #[arg(short, long, default_value = "polkadot")]
        network: String,

        /// Check IPv6 endpoint
        #[arg(long)]
        ipv6: bool,

        /// Maximum latency threshold (ms)
        #[arg(long, default_value = "1000")]
        max_latency: u32,

        /// Verify data against relay chain
        #[arg(long)]
        verify_relay: bool,

        /// Relay chain RPC endpoint
        #[arg(long, default_value = "wss://rpc.polkadot.io")]
        relay_rpc: String,

        /// Output format (json, text)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Run batch checks from config file
    Batch {
        /// Config file with endpoint list
        #[arg(short, long)]
        config: String,

        /// Relay chain RPC endpoint
        #[arg(long, default_value = "wss://rpc.polkadot.io")]
        relay_rpc: String,

        /// Output file
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Generate execution trace (for testing/proof generation)
    Trace {
        /// PolkaVM program blob
        #[arg(short, long)]
        program: String,

        /// Input data file
        #[arg(short, long)]
        input: Option<String>,

        /// Output trace file
        #[arg(short, long, default_value = "trace.json")]
        output: String,

        /// Use dummy host (no real network I/O)
        #[arg(long)]
        dummy: bool,
    },

    /// Check site-level health (hostname only)
    Site {
        /// Hostname (e.g., rpc.rotko.net)
        #[arg(short = 'H', long)]
        hostname: String,

        /// Check IPv6
        #[arg(long)]
        ipv6: bool,

        /// Output format
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Check domain-level health (all networks for a domain)
    Domain {
        /// Domain (e.g., rotko.net)
        #[arg(short, long)]
        domain: String,

        /// Network name
        #[arg(short, long)]
        network: String,

        /// Check IPv6
        #[arg(long)]
        ipv6: bool,

        /// Output format
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Prepare submission data for pallet-sla-monitor
    PrepareSubmit {
        /// Endpoint to check
        #[arg(short, long)]
        endpoint: String,

        /// Network name
        #[arg(short, long, default_value = "polkadot")]
        network: String,

        /// Relay chain RPC endpoint
        #[arg(long, default_value = "wss://rpc.polkadot.io")]
        relay_rpc: String,

        /// Include extended data (larger payload, stored on IPFS)
        #[arg(long)]
        include_extended: bool,

        /// Output format: json, hex, or call (encoded extrinsic)
        #[arg(short, long, default_value = "json")]
        output: String,
    },

    /// Verify RPC endpoint against P2P network using smoldot light client
    #[cfg(feature = "smoldot")]
    VerifyP2p {
        /// Endpoint URL to verify
        #[arg(short, long)]
        endpoint: String,

        /// Network name (polkadot, kusama, westend)
        #[arg(short, long, default_value = "polkadot")]
        network: String,

        /// Path to chain spec file (fetched automatically if not provided)
        #[arg(long)]
        chain_spec: Option<String>,

        /// Timeout for P2P sync (seconds)
        #[arg(long, default_value = "30")]
        timeout: u64,

        /// Output format (json, text)
        #[arg(short, long, default_value = "json")]
        output: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup tracing
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(format!("ibp_probe={}", level))
        .init();

    match cli.command {
        Commands::Check {
            endpoint,
            network,
            ipv6,
            max_latency,
            verify_relay,
            relay_rpc,
            output,
        } => {
            run_check(endpoint, network, ipv6, max_latency, verify_relay, relay_rpc, output).await
        }
        Commands::Batch {
            config,
            relay_rpc,
            output,
        } => run_batch(config, relay_rpc, output).await,
        Commands::Trace {
            program,
            input,
            output,
            dummy,
        } => run_trace(program, input, output, dummy).await,
        Commands::Site {
            hostname,
            ipv6,
            output,
        } => run_site_check(hostname, ipv6, output).await,
        Commands::Domain {
            domain,
            network,
            ipv6,
            output,
        } => run_domain_check(domain, network, ipv6, output).await,
        Commands::PrepareSubmit {
            endpoint,
            network,
            relay_rpc,
            include_extended,
            output,
        } => run_prepare_submit(endpoint, network, relay_rpc, include_extended, output).await,
        #[cfg(feature = "smoldot")]
        Commands::VerifyP2p {
            endpoint,
            network,
            chain_spec,
            timeout,
            output,
        } => run_verify_p2p(endpoint, network, chain_spec, timeout, output).await,
    }
}

async fn run_check(
    endpoint: String,
    network: String,
    ipv6: bool,
    max_latency: u32,
    verify_relay: bool,
    relay_rpc: String,
    output_format: String,
) -> Result<()> {
    info!("Checking endpoint: {} ({})", endpoint, network);

    let host = IbpHost::new(&relay_rpc);

    // Determine service type from URL
    let service_type = if endpoint.starts_with("wss://") || endpoint.starts_with("ws://") {
        ServiceType::WssRpc
    } else {
        ServiceType::Rpc
    };

    let config = CheckConfig {
        target: CheckTarget::Endpoint {
            url: endpoint.clone(),
            service_type,
            ipv6,
        },
        params: CheckParams {
            max_latency_ms: Some(max_latency),
            verify_finalized: verify_relay,
            check_sync: true,
            min_peers: Some(3),
            verify_archive: false,
            historical_block: None,
            custom_rpc: vec![],
        },
        timeout_ms: 30000,
    };

    let result = run_endpoint_check(&host, &config).await;

    if output_format == "json" {
        let ibp_result: IbpMonitorResult = result.clone().into();
        println!("{}", serde_json::to_string_pretty(&ibp_result)?);
    } else {
        println!("Endpoint: {}", endpoint);
        println!("Healthy: {}", result.healthy);
        for check in &result.checks {
            println!(
                "  {}: {} ({}ms)",
                check.name,
                if check.passed { "PASS" } else { "FAIL" },
                check.latency_ms
            );
        }
        if let Some(err) = &result.error {
            println!("Error: {}", err);
        }
    }

    // Return exit code based on health
    if !result.healthy {
        std::process::exit(1);
    }

    Ok(())
}

async fn run_endpoint_check(host: &IbpHost, config: &CheckConfig) -> CheckResult {
    let mut result = CheckResult::new(config.target.clone());

    // Extract hostname/URL based on target
    let url = match &config.target {
        CheckTarget::Endpoint { url, .. } => url.clone(),
        CheckTarget::Site { hostname, .. } => format!("wss://{}", hostname),
        CheckTarget::Domain { domain, network, .. } => format!("wss://rpc.{}/{}", domain, network),
        CheckTarget::BootNode { multiaddr, .. } => multiaddr.clone(),
    };

    // Determine if this is HTTP or WebSocket endpoint
    let is_http = url.starts_with("http://") || url.starts_with("https://");
    let is_wss = url.starts_with("wss://") || url.starts_with("ws://");

    // Extract hostname for TCP ping
    let hostname = url
        .replace("wss://", "")
        .replace("ws://", "")
        .replace("https://", "")
        .replace("http://", "")
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();

    // TCP ping check
    let ping_latency = host.tcp_ping(&hostname, 443, 5000).await.unwrap_or(u32::MAX);
    let ping_passed = ping_latency < config.params.max_latency_ms.unwrap_or(1000);
    result.add_check(ibp_probe_host::IndividualCheck {
        name: "tcp_ping".to_string(),
        passed: ping_passed,
        latency_ms: ping_latency,
        details: serde_json::json!({ "host": hostname, "port": 443 }),
        error: if ping_passed { None } else { Some("Ping timeout or too slow".to_string()) },
    });

    // WebSocket connect check (only for wss:// endpoints)
    if is_wss {
        let (wss_handle, wss_latency) = host.wss_connect(&url, 10000).await.unwrap_or((0, u32::MAX));
        let wss_passed = wss_handle != 0 && wss_latency < config.params.max_latency_ms.unwrap_or(1000);
        result.add_check(ibp_probe_host::IndividualCheck {
            name: "wss_connect".to_string(),
            passed: wss_passed,
            latency_ms: wss_latency,
            details: serde_json::json!({ "handle": wss_handle }),
            error: if wss_passed { None } else { Some("WebSocket connection failed".to_string()) },
        });
    } else if is_http {
        // For HTTP endpoints, mark WSS as skipped/passed
        result.add_check(ibp_probe_host::IndividualCheck {
            name: "wss_connect".to_string(),
            passed: true,
            latency_ms: 0,
            details: serde_json::json!({ "skipped": true, "reason": "HTTP endpoint" }),
            error: None,
        });
    }

    // RPC call check - get finalized head
    // Convert WSS to HTTPS for HTTP-based RPC calls
    let rpc_url = url
        .replace("wss://", "https://")
        .replace("ws://", "http://");
    let rpc_result = host.rpc_call(&rpc_url, "chain_getFinalizedHead", &[]).await;
    match rpc_result {
        Ok(hash) => {
            result.add_check(ibp_probe_host::IndividualCheck {
                name: "rpc_finalized".to_string(),
                passed: true,
                latency_ms: 0, // Would track from trace
                details: serde_json::json!({ "hash": hash }),
                error: None,
            });

            // Get sync state
            let sync_result = host.rpc_call(&rpc_url, "system_syncState", &[]).await;
            if let Ok(sync_state) = sync_result {
                // Check if node is synced: currentBlock should be close to highestBlock
                let current_block = sync_state.get("currentBlock")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let highest_block = sync_state.get("highestBlock")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                // Node is synced if current is within 2 blocks of highest
                let is_synced = highest_block > 0 && current_block + 2 >= highest_block;
                let sync_passed = is_synced || !config.params.check_sync;

                result.add_check(ibp_probe_host::IndividualCheck {
                    name: "sync_state".to_string(),
                    passed: sync_passed,
                    latency_ms: 0,
                    details: serde_json::json!({
                        "currentBlock": current_block,
                        "highestBlock": highest_block,
                        "is_synced": is_synced,
                    }),
                    error: if !sync_passed {
                        Some(format!("Node is syncing: {} / {}", current_block, highest_block))
                    } else {
                        None
                    },
                });
            }

            // Verify against relay if requested
            if config.params.verify_finalized {
                let relay_result = host.relay_finalized_block().await;
                if let Ok((relay_block, relay_hash)) = relay_result {
                    // Get endpoint's block header to compare
                    let header_result = host.rpc_call(&rpc_url, "chain_getHeader", &[hash.clone()]).await;
                    if let Ok(header) = header_result {
                        let endpoint_block = header.get("number")
                            .and_then(|n| n.as_str())
                            .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                            .unwrap_or(0);

                        // Allow some lag (6 seconds / 1 block)
                        let block_diff = relay_block.saturating_sub(endpoint_block);
                        let data_valid = block_diff <= 2;

                        result.add_check(ibp_probe_host::IndividualCheck {
                            name: "relay_verification".to_string(),
                            passed: data_valid,
                            latency_ms: 0,
                            details: serde_json::json!({
                                "endpoint_block": endpoint_block,
                                "relay_block": relay_block,
                                "block_diff": block_diff,
                                "relay_hash": hex::encode(relay_hash),
                            }),
                            error: if data_valid { None } else { Some("Endpoint behind relay".to_string()) },
                        });
                    }
                }
            }
        }
        Err(e) => {
            result.add_check(ibp_probe_host::IndividualCheck {
                name: "rpc_finalized".to_string(),
                passed: false,
                latency_ms: u32::MAX,
                details: serde_json::Value::Null,
                error: Some(e.to_string()),
            });
        }
    }

    result.finalize();
    result
}

async fn run_batch(config_path: String, relay_rpc: String, output: Option<String>) -> Result<()> {
    info!("Running batch checks from: {}", config_path);

    let config_data = std::fs::read_to_string(&config_path)?;
    let configs: Vec<CheckConfig> = serde_json::from_str(&config_data)?;

    let host = IbpHost::new(&relay_rpc);
    let mut results = Vec::new();

    for config in configs {
        info!("Checking: {:?}", config.target);
        let result = run_endpoint_check(&host, &config).await;
        results.push(IbpMonitorResult::from(result));
    }

    let output_json = serde_json::to_string_pretty(&results)?;

    if let Some(output_path) = output {
        std::fs::write(&output_path, &output_json)?;
        info!("Results written to: {}", output_path);
    } else {
        println!("{}", output_json);
    }

    Ok(())
}

async fn run_trace(
    program_path: String,
    input_path: Option<String>,
    output_path: String,
    dummy: bool,
) -> Result<()> {
    info!("Generating trace for: {}", program_path);

    #[cfg(feature = "trace")]
    {
        // Read program
        let program_blob = std::fs::read(&program_path)?;

        // Read input if provided
        let _input_data = if let Some(path) = input_path {
            std::fs::read(&path)?
        } else {
            Vec::new()
        };

        if dummy {
            // Use dummy host handler (no real network I/O)
            use polkavm_pcvm::host_calls::DummyHostHandler;

            let handler = DummyHostHandler::new();
            let trace_result = polkavm_pcvm::polkavm_tracer::extract_polkavm_trace_with_host(
                &program_blob,
                100_000,
                &handler,
            );

            match trace_result {
                Ok(extended_trace) => {
                    let trace_json = serde_json::json!({
                        "execution_steps": extended_trace.execution_trace.steps.len(),
                        "host_calls": extended_trace.host_trace.calls.len(),
                        "program_hash": hex::encode(extended_trace.execution_trace.program_hash.to_le_bytes()),
                        "trace_commitment": hex::encode(extended_trace.host_trace.commitment()),
                    });
                    std::fs::write(&output_path, serde_json::to_string_pretty(&trace_json)?)?;
                    info!("Trace written to: {}", output_path);
                }
                Err(e) => {
                    anyhow::bail!("Trace extraction failed: {:?}", e);
                }
            }
        } else {
            // Would use real IBP host - for now just error
            anyhow::bail!("Real host mode not yet implemented. Use --dummy for testing.");
        }
    }

    #[cfg(not(feature = "trace"))]
    {
        let _ = (program_path, input_path, output_path, dummy);
        anyhow::bail!("Trace generation requires the 'trace' feature. Build with: cargo build --features trace");
    }

    #[allow(unreachable_code)]
    Ok(())
}

async fn run_site_check(hostname: String, ipv6: bool, output_format: String) -> Result<()> {
    info!("Checking site: {}", hostname);

    let host = IbpHost::new("wss://rpc.polkadot.io");

    let config = CheckConfig {
        target: CheckTarget::Site {
            hostname: hostname.clone(),
            ipv6,
        },
        params: CheckParams::default(),
        timeout_ms: 30000,
    };

    let result = run_endpoint_check(&host, &config).await;

    if output_format == "json" {
        let ibp_result: IbpMonitorResult = result.into();
        println!("{}", serde_json::to_string_pretty(&ibp_result)?);
    } else {
        println!("Site: {}", hostname);
        println!("Healthy: {}", result.healthy);
    }

    Ok(())
}

async fn run_domain_check(
    domain: String,
    network: String,
    ipv6: bool,
    output_format: String,
) -> Result<()> {
    info!("Checking domain: {} for network: {}", domain, network);

    let host = IbpHost::new("wss://rpc.polkadot.io");

    let config = CheckConfig {
        target: CheckTarget::Domain {
            domain: domain.clone(),
            network: network.clone(),
            ipv6,
        },
        params: CheckParams {
            check_sync: true,
            verify_finalized: true,
            ..Default::default()
        },
        timeout_ms: 30000,
    };

    let result = run_endpoint_check(&host, &config).await;

    if output_format == "json" {
        let ibp_result: IbpMonitorResult = result.into();
        println!("{}", serde_json::to_string_pretty(&ibp_result)?);
    } else {
        println!("Domain: {} Network: {}", domain, network);
        println!("Healthy: {}", result.healthy);
    }

    Ok(())
}

async fn run_prepare_submit(
    endpoint: String,
    network: String,
    relay_rpc: String,
    include_extended: bool,
    output_format: String,
) -> Result<()> {
    use ibp_probe_host::substrate_types::{
        self, ProbeReport, ExtendedReport, node_id_from_endpoint, hash_extended_report,
    };
    use parity_scale_codec::Encode;

    info!("Running check and preparing submission for: {}", endpoint);

    let host = IbpHost::new(&relay_rpc);

    // Determine service type from URL
    let service_type = if endpoint.starts_with("wss://") || endpoint.starts_with("ws://") {
        ServiceType::WssRpc
    } else {
        ServiceType::Rpc
    };

    let config = CheckConfig {
        target: CheckTarget::Endpoint {
            url: endpoint.clone(),
            service_type,
            ipv6: false,
        },
        params: CheckParams {
            check_sync: true,
            verify_finalized: true,
            ..Default::default()
        },
        timeout_ms: 30000,
    };

    // Run the check
    let result = run_endpoint_check(&host, &config).await;

    // Convert to pallet types
    let node_id = node_id_from_endpoint(&endpoint);
    let probe_report = ProbeReport::from(&result);
    let extended_report = ExtendedReport::from(&result);
    let extended_hash = hash_extended_report(&extended_report);

    // Prepare submission data
    let submission = serde_json::json!({
        "node_id": hex::encode(node_id),
        "result": format!("{:?}", probe_report.result),
        "latency_ms": probe_report.latency_ms,
        "extended_hash": hex::encode(extended_hash),
        "extended_data_hex": if include_extended {
            Some(hex::encode(extended_report.encode()))
        } else {
            None
        },
        // For manual submission via polkadot.js or subxt
        "call_params": {
            "pallet": "SlaMonitor",
            "call": "submit_ibp_report",
            "args": {
                "node_id": format!("0x{}", hex::encode(node_id)),
                "result": probe_report.result as u8,
                "latency_ms": probe_report.latency_ms,
                "extended_hash": format!("0x{}", hex::encode(extended_hash)),
                "extended_data": if include_extended {
                    Some(format!("0x{}", hex::encode(extended_report.encode())))
                } else {
                    None::<String>
                },
                "proof": None::<String>,
            }
        },
        "check_result": {
            "healthy": result.healthy,
            "checks": result.checks.iter().map(|c| serde_json::json!({
                "name": c.name,
                "passed": c.passed,
                "latency_ms": c.latency_ms,
            })).collect::<Vec<_>>(),
        }
    });

    match output_format.as_str() {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&submission)?);
        }
        "hex" => {
            // Output just the encoded call data
            // Format: node_id (32) + result (1) + latency (4) + hash (32) + extended_data (optional)
            let mut call_data = Vec::new();
            call_data.extend_from_slice(&node_id);
            call_data.push(probe_report.result as u8);
            call_data.extend_from_slice(&probe_report.latency_ms.to_le_bytes());
            call_data.extend_from_slice(&extended_hash);

            if include_extended {
                let extended_encoded = extended_report.encode();
                // Length prefix for Option<Vec>
                call_data.push(1); // Some variant
                call_data.extend_from_slice(&(extended_encoded.len() as u32).to_le_bytes());
                call_data.extend_from_slice(&extended_encoded);
            } else {
                call_data.push(0); // None variant
            }

            // No proof
            call_data.push(0); // None variant

            println!("0x{}", hex::encode(call_data));
        }
        _ => {
            println!("{}", serde_json::to_string_pretty(&submission)?);
        }
    }

    Ok(())
}

#[cfg(feature = "smoldot")]
async fn run_verify_p2p(
    endpoint: String,
    network: String,
    chain_spec_path: Option<String>,
    timeout_secs: u64,
    output_format: String,
) -> Result<()> {
    use ibp_probe_host::{SmoldotVerifier, IbpHost};
    use std::time::Instant;

    info!("Verifying {} endpoint against P2P network ({})", endpoint, network);

    // Create smoldot verifier
    let verifier = if let Some(spec_path) = chain_spec_path {
        let spec_content = std::fs::read_to_string(&spec_path)?;
        SmoldotVerifier::new(spec_content, &network, timeout_secs)
    } else {
        info!("Fetching chain spec for {}...", network);
        SmoldotVerifier::from_network(&network, timeout_secs)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
    };

    // Get RPC result first (fast)
    let rpc_start = Instant::now();
    let rpc_url = endpoint
        .replace("wss://", "https://")
        .replace("ws://", "http://");

    let host = IbpHost::new(&rpc_url);
    let rpc_result = host.rpc_call(&rpc_url, "chain_getFinalizedHead", &[]).await;

    let (rpc_block, rpc_hash, rpc_latency) = match rpc_result {
        Ok(hash_val) => {
            let hash_hex = hash_val.as_str().unwrap_or("");
            let header = host.rpc_call(&rpc_url, "chain_getHeader", &[hash_val.clone()]).await;

            let block_num = header.ok()
                .and_then(|h| h.get("number").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);

            let mut hash_bytes = [0u8; 32];
            if let Ok(bytes) = hex::decode(hash_hex.trim_start_matches("0x")) {
                if bytes.len() >= 32 {
                    hash_bytes.copy_from_slice(&bytes[..32]);
                }
            }

            (block_num, hash_bytes, rpc_start.elapsed().as_millis() as u32)
        }
        Err(e) => {
            anyhow::bail!("RPC call failed: {}", e);
        }
    };

    info!("RPC returned block #{} in {}ms", rpc_block, rpc_latency);
    info!("Starting P2P sync via smoldot (timeout: {}s)...", timeout_secs);

    // Get P2P result (slow - needs sync)
    let p2p_result = verifier.get_finalized_block().await;

    if !p2p_result.success {
        anyhow::bail!("P2P sync failed: {:?}", p2p_result.error);
    }

    // Compare results
    let comparison = verifier.compare_with_rpc(
        &p2p_result,
        rpc_block,
        &rpc_hash,
        rpc_latency,
    );

    // Build output
    let output = serde_json::json!({
        "endpoint": endpoint,
        "network": network,
        "verification": {
            "rpc_valid": comparison.rpc_valid,
            "hashes_match": comparison.hashes_match,
            "block_diff": comparison.block_diff,
        },
        "rpc": {
            "finalized_block": rpc_block,
            "finalized_hash": hex::encode(rpc_hash),
            "latency_ms": rpc_latency,
        },
        "p2p": {
            "finalized_block": p2p_result.p2p_finalized_block,
            "finalized_hash": hex::encode(p2p_result.p2p_finalized_hash),
            "sync_time_ms": p2p_result.p2p_sync_time_ms,
            "peers_discovered": p2p_result.peers_discovered,
            "is_syncing": p2p_result.is_syncing,
        },
        "comparison": {
            "rpc_speedup": format!("{:.1}x", comparison.rpc_speedup),
            "rpc_faster_by_ms": p2p_result.p2p_sync_time_ms.saturating_sub(rpc_latency as u64),
        }
    });

    if output_format == "json" {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("=== P2P Verification Results ===");
        println!("Endpoint: {}", endpoint);
        println!("Network: {}", network);
        println!();
        println!("RPC Result:");
        println!("  Block: #{}", rpc_block);
        println!("  Hash: 0x{}", hex::encode(rpc_hash));
        println!("  Latency: {}ms", rpc_latency);
        println!();
        println!("P2P Result:");
        println!("  Block: #{}", p2p_result.p2p_finalized_block);
        println!("  Hash: 0x{}", hex::encode(p2p_result.p2p_finalized_hash));
        println!("  Sync Time: {}ms", p2p_result.p2p_sync_time_ms);
        println!("  Peers: {}", p2p_result.peers_discovered);
        println!();
        println!("Verification:");
        println!("  RPC Valid: {}", if comparison.rpc_valid { "✓ YES" } else { "✗ NO" });
        println!("  Hashes Match: {}", if comparison.hashes_match { "✓ YES" } else { "✗ NO" });
        println!("  Block Diff: {}", comparison.block_diff);
        println!("  RPC Speedup: {:.1}x faster than P2P", comparison.rpc_speedup);
    }

    if !comparison.rpc_valid {
        std::process::exit(1);
    }

    Ok(())
}
