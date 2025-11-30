use anyhow::Result;
use clap::Parser;
use std::net::SocketAddr;
use tracing::{info, error, warn};
use tonic::transport::Server;

mod zebrad;
mod header_chain;
mod prover;
mod grpc_service;
mod storage;
mod compact;
mod error;
mod epoch;
mod constants;
mod witness;

// trustless v2 modules
mod checkpoint;
mod state_transition;
mod p2p;
mod tct;

// zanchor parachain integration
mod zanchor;

use crate::{grpc_service::ZidecarService, epoch::EpochManager};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "zidecar")]
#[command(about = "ligerito-powered zcash light server", long_about = None)]
struct Args {
    /// zebrad RPC endpoint
    #[arg(long, default_value = "http://127.0.0.1:8232")]
    zebrad_rpc: String,

    /// gRPC listen address
    #[arg(long, default_value = "0.0.0.0:50051")]
    listen: SocketAddr,

    /// RocksDB database path
    #[arg(long, default_value = "./zidecar.db")]
    db_path: String,

    /// Start height for header chain proofs
    #[arg(long, default_value_t = zync_core::ORCHARD_ACTIVATION_HEIGHT)]
    start_height: u32,

    /// Enable testnet mode
    #[arg(long)]
    testnet: bool,

    /// Zanchor parachain RPC endpoint (for Polkadot-secured checkpoints)
    #[arg(long)]
    zanchor_rpc: Option<String>,

    /// Enable relayer mode (submit attestations to zanchor)
    #[arg(long)]
    relayer: bool,

    /// Relayer seed phrase (required if --relayer is set)
    #[arg(long, env = "ZIDECAR_RELAYER_SEED")]
    relayer_seed: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zidecar=info,tower_http=debug".into()),
        )
        .init();

    let args = Args::parse();

    info!("starting zidecar");
    info!("zebrad RPC: {}", args.zebrad_rpc);
    info!("gRPC listen: {}", args.listen);
    info!("database: {}", args.db_path);
    info!("start height: {}", args.start_height);
    info!("testnet: {}", args.testnet);
    if let Some(ref rpc) = args.zanchor_rpc {
        info!("zanchor RPC: {}", rpc);
    }
    info!("relayer mode: {}", args.relayer);

    // initialize storage
    let storage = storage::Storage::open(&args.db_path)?;
    info!("opened database");

    // initialize zebrad client
    let zebrad = zebrad::ZebradClient::new(&args.zebrad_rpc);

    // verify connection
    match zebrad.get_blockchain_info().await {
        Ok(info) => {
            info!("connected to zebrad");
            info!("  chain: {}", info.chain);
            info!("  blocks: {}", info.blocks);
            info!("  bestblockhash: {}", info.bestblockhash);
        }
        Err(e) => {
            error!("failed to connect to zebrad: {}", e);
            return Err(e.into());
        }
    }

    // initialize prover configs
    info!("initialized ligerito prover configs");
    info!("  tip proof: 2^24 (~1.3s, max 1024 blocks)");
    info!("  gigaproof: 2^28 (~25s, multi-epoch)");

    // initialize epoch manager
    let storage_arc = Arc::new(storage);
    let epoch_manager = Arc::new(EpochManager::new(
        zebrad.clone(),
        storage_arc.clone(),
        zync_core::gigaproof_prover_config(),
        zync_core::tip_prover_config(),
        args.start_height,
    ));

    // generate initial gigaproof synchronously before starting background tasks
    // this ensures we have a proof ready before accepting gRPC requests
    info!("checking for existing gigaproof...");
    match epoch_manager.generate_gigaproof().await {
        Ok(_) => info!("gigaproof ready"),
        Err(e) => warn!("initial gigaproof generation failed: {}", e),
    }

    // start background gigaproof generator (regenerates hourly when epochs complete)
    let epoch_manager_bg = epoch_manager.clone();
    tokio::spawn(async move {
        epoch_manager_bg.run_background_prover().await;
    });

    // start background state root tracker (for trustless proofs)
    let epoch_manager_state = epoch_manager.clone();
    tokio::spawn(async move {
        epoch_manager_state.run_background_state_tracker().await;
    });

    // initialize zanchor client (if configured)
    let zanchor_client = if args.zanchor_rpc.is_some() || args.relayer {
        let mut client = zanchor::ZanchorClient::new(args.zanchor_rpc.as_deref());

        if args.relayer {
            if let Some(seed) = args.relayer_seed {
                client = client.with_relayer(seed);
                info!("relayer mode enabled");
            } else {
                warn!("--relayer requires --relayer-seed or ZIDECAR_RELAYER_SEED env var");
            }
        }

        // Try to connect (non-blocking, will retry in background)
        match client.connect().await {
            Ok(_) => {
                if let Ok(Some(height)) = client.get_latest_finalized_height().await {
                    info!("zanchor latest finalized zcash height: {}", height);
                }
                Some(Arc::new(tokio::sync::RwLock::new(client)))
            }
            Err(e) => {
                warn!("zanchor connection failed (will retry): {}", e);
                Some(Arc::new(tokio::sync::RwLock::new(client)))
            }
        }
    } else {
        info!("zanchor integration disabled (use --zanchor-rpc to enable)");
        None
    };

    // start background zanchor sync (if enabled)
    if let Some(ref zanchor) = zanchor_client {
        let zanchor_bg = zanchor.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

                let mut client = zanchor_bg.write().await;
                if !client.is_connected() {
                    if let Err(e) = client.connect().await {
                        warn!("zanchor reconnect failed: {}", e);
                        continue;
                    }
                }

                match client.get_latest_finalized_height().await {
                    Ok(Some(height)) => {
                        info!("zanchor finalized height: {}", height);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!("failed to fetch zanchor state: {}", e);
                    }
                }
            }
        });
    }

    // create gRPC service
    // TODO: pass zanchor_client to service for hybrid checkpoint verification
    let _zanchor_client = zanchor_client;

    let service = ZidecarService::new(
        zebrad,
        storage_arc,
        epoch_manager,
        args.start_height,
    );

    info!("starting gRPC server on {}", args.listen);
    info!("gRPC-web enabled for browser clients");

    // build gRPC service
    let grpc_service = zidecar::zidecar_server::ZidecarServer::new(service);

    // wrap with gRPC-web + CORS support for browser clients
    // tonic_web::enable() handles CORS and protocol translation
    let grpc_web_service = tonic_web::enable(grpc_service);

    Server::builder()
        .accept_http1(true) // required for gRPC-web
        .add_service(grpc_web_service)
        .serve(args.listen)
        .await?;

    Ok(())
}

// generated proto module
pub mod zidecar {
    tonic::include_proto!("zidecar.v1");
}
