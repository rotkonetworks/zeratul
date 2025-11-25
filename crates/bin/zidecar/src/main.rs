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

    // start background gigaproof generator
    let epoch_manager_bg = epoch_manager.clone();
    tokio::spawn(async move {
        epoch_manager_bg.run_background_prover().await;
    });

    // generate initial gigaproof if needed
    info!("checking for existing gigaproof...");
    match epoch_manager.generate_gigaproof().await {
        Ok(_) => info!("gigaproof ready"),
        Err(e) => warn!("initial gigaproof generation failed: {}", e),
    }

    // create gRPC service
    let service = ZidecarService::new(
        zebrad,
        storage_arc,
        epoch_manager,
        args.start_height,
    );

    info!("starting gRPC server on {}", args.listen);

    // build and start server
    let service = zidecar::zidecar_server::ZidecarServer::new(service);

    Server::builder()
        .add_service(service)
        .serve(args.listen)
        .await?;

    Ok(())
}

// generated proto module
pub mod zidecar {
    tonic::include_proto!("zidecar.v1");
}
