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
