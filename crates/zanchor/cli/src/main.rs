//! Zanchor CLI - Parachain and Coretime Management Tool
//!
//! Commands:
//! - balance: Check account balances
//! - transfer: Transfer funds between accounts
//! - reserve: Reserve a ParaId
//! - register: Register parachain genesis
//! - coretime: Manage coretime (purchase, assign)

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use sp_core::{sr25519, Pair};
use std::path::PathBuf;
use subxt::{OnlineClient, PolkadotConfig};
use tracing::{info, warn};

/// Paseo relay chain RPC
const PASEO_RPC: &str = "wss://paseo.rpc.amforc.com:443";
/// Paseo coretime chain RPC
const PASEO_CORETIME_RPC: &str = "wss://paseo-coretime-rpc.polkadot.io";

#[derive(Parser)]
#[command(name = "zanchor")]
#[command(about = "Zanchor parachain management CLI", long_about = None)]
struct Cli {
    /// RPC endpoint (default: Paseo)
    #[arg(long, default_value = PASEO_RPC)]
    rpc: String,

    /// Secret seed or mnemonic for signing
    #[arg(long, env = "ZANCHOR_SEED")]
    seed: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check account balance
    Balance {
        /// Account address (SS58)
        #[arg(short, long)]
        account: Option<String>,
    },

    /// Transfer funds
    Transfer {
        /// Destination address
        #[arg(short, long)]
        to: String,
        /// Amount in planck
        #[arg(short, long)]
        amount: u128,
    },

    /// Reserve a ParaId
    Reserve,

    /// Register parachain
    Register {
        /// ParaId to register
        #[arg(short, long)]
        para_id: u32,
        /// Path to genesis head hex file
        #[arg(long)]
        genesis_head: PathBuf,
        /// Path to validation code (wasm) hex file
        #[arg(long)]
        validation_code: PathBuf,
    },

    /// Coretime operations
    Coretime {
        #[command(subcommand)]
        action: CoretimeAction,
    },

    /// Generate chainspec with para_id
    Chainspec {
        /// ParaId to use
        #[arg(short, long)]
        para_id: u32,
        /// Input chainspec template
        #[arg(short, long)]
        input: PathBuf,
        /// Output chainspec file
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Export genesis files from chainspec
    ExportGenesis {
        /// Input chainspec
        #[arg(short, long)]
        chainspec: PathBuf,
        /// Output directory
        #[arg(short, long)]
        output_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum CoretimeAction {
    /// Check coretime status
    Status,
    /// Purchase coretime
    Purchase {
        /// Price limit
        #[arg(long)]
        price_limit: u128,
    },
    /// Assign coretime region to task
    Assign {
        /// Region ID
        #[arg(long)]
        region_id: String,
        /// Task ID (ParaId)
        #[arg(long)]
        task_id: u32,
        /// Use final finality (permanent assignment)
        #[arg(long)]
        finality: Option<String>,
    },
}

/// Subxt signer wrapper
struct SubxtSigner {
    pair: sr25519::Pair,
    account_id: subxt::utils::AccountId32,
}

impl SubxtSigner {
    fn from_seed(seed: &str) -> Result<Self> {
        let pair = sr25519::Pair::from_string(seed, None)
            .map_err(|e| anyhow!("Invalid seed: {:?}", e))?;
        let account_id = subxt::utils::AccountId32::from(pair.public().0);
        Ok(Self { pair, account_id })
    }

    fn account_id(&self) -> &subxt::utils::AccountId32 {
        &self.account_id
    }
}

impl subxt::tx::Signer<PolkadotConfig> for SubxtSigner {
    fn account_id(&self) -> subxt::utils::AccountId32 {
        self.account_id.clone()
    }

    fn address(&self) -> <PolkadotConfig as subxt::Config>::Address {
        self.account_id.clone().into()
    }

    fn sign(&self, payload: &[u8]) -> <PolkadotConfig as subxt::Config>::Signature {
        let sig = self.pair.sign(payload);
        subxt::utils::MultiSignature::Sr25519(sig.0)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("zanchor=info".parse()?)
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Balance { account } => {
            cmd_balance(&cli.rpc, account, cli.seed.as_deref()).await?;
        }
        Commands::Transfer { to, amount } => {
            let seed = cli.seed.ok_or_else(|| anyhow!("--seed required for transfer"))?;
            cmd_transfer(&cli.rpc, &seed, &to, amount).await?;
        }
        Commands::Reserve => {
            let seed = cli.seed.ok_or_else(|| anyhow!("--seed required for reserve"))?;
            cmd_reserve(&cli.rpc, &seed).await?;
        }
        Commands::Register {
            para_id,
            genesis_head,
            validation_code,
        } => {
            let seed = cli.seed.ok_or_else(|| anyhow!("--seed required for register"))?;
            cmd_register(&cli.rpc, &seed, para_id, &genesis_head, &validation_code).await?;
        }
        Commands::Coretime { action } => {
            cmd_coretime(&cli.rpc, cli.seed.as_deref(), action).await?;
        }
        Commands::Chainspec { para_id, input, output } => {
            cmd_chainspec(para_id, &input, &output)?;
        }
        Commands::ExportGenesis { chainspec, output_dir } => {
            cmd_export_genesis(&chainspec, &output_dir)?;
        }
    }

    Ok(())
}

async fn cmd_balance(rpc: &str, account: Option<String>, seed: Option<&str>) -> Result<()> {
    let client = OnlineClient::<PolkadotConfig>::from_url(rpc).await?;

    let account_id = if let Some(addr) = account {
        parse_account(&addr)?
    } else if let Some(seed) = seed {
        let signer = SubxtSigner::from_seed(seed)?;
        signer.account_id().clone()
    } else {
        return Err(anyhow!("Provide --account or --seed"));
    };

    info!("Checking balance for: {}", account_id);

    // Query System.Account storage
    let storage_key = subxt::dynamic::storage("System", "Account", vec![
        subxt::dynamic::Value::from_bytes(account_id.0),
    ]);

    let result = client.storage().at_latest().await?.fetch(&storage_key).await?;

    if let Some(value) = result {
        let data = value.to_value()?;
        println!("Account: {}", account_id);
        println!("Data: {:#?}", data);
    } else {
        println!("Account: {} (no data - unfunded)", account_id);
    }

    Ok(())
}

async fn cmd_transfer(rpc: &str, seed: &str, to: &str, amount: u128) -> Result<()> {
    let client = OnlineClient::<PolkadotConfig>::from_url(rpc).await?;
    let signer = SubxtSigner::from_seed(seed)?;
    let dest = parse_account(to)?;

    info!("Transferring {} planck from {} to {}", amount, signer.account_id(), dest);

    let tx = subxt::dynamic::tx("Balances", "transfer_keep_alive", vec![
        subxt::dynamic::Value::unnamed_variant("Id", vec![
            subxt::dynamic::Value::from_bytes(dest.0),
        ]),
        subxt::dynamic::Value::u128(amount),
    ]);

    let result = client
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?
        .wait_for_finalized_success()
        .await?;

    println!("Transfer successful!");
    println!("Extrinsic hash: {:?}", result.extrinsic_hash());

    Ok(())
}

async fn cmd_reserve(rpc: &str, seed: &str) -> Result<()> {
    let client = OnlineClient::<PolkadotConfig>::from_url(rpc).await?;
    let signer = SubxtSigner::from_seed(seed)?;

    info!("Reserving ParaId from account: {}", signer.account_id());

    // First check next free ParaId
    let next_id_key = subxt::dynamic::storage("Registrar", "NextFreeParaId", vec![]);
    if let Some(value) = client.storage().at_latest().await?.fetch(&next_id_key).await? {
        let data = value.to_value()?;
        println!("Next free ParaId: {:?}", data);
    }

    let no_args: Vec<subxt::dynamic::Value> = vec![];
    let tx = subxt::dynamic::tx("Registrar", "reserve", no_args);

    let result = client
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?
        .wait_for_finalized_success()
        .await?;

    println!("ParaId reserved!");
    println!("Extrinsic hash: {:?}", result.extrinsic_hash());

    // Check events for the reserved ParaId
    for event in result.all_events_in_block().iter() {
        if let Ok(ev) = event {
            println!("Event: {}::{}", ev.pallet_name(), ev.variant_name());
        }
    }

    Ok(())
}

async fn cmd_register(
    rpc: &str,
    seed: &str,
    para_id: u32,
    genesis_head_path: &PathBuf,
    validation_code_path: &PathBuf,
) -> Result<()> {
    let client = OnlineClient::<PolkadotConfig>::from_url(rpc).await?;
    let signer = SubxtSigner::from_seed(seed)?;

    info!("Registering ParaId {} from account: {}", para_id, signer.account_id());

    // Read genesis files
    let genesis_head_hex = std::fs::read_to_string(genesis_head_path)?;
    let validation_code_hex = std::fs::read_to_string(validation_code_path)?;

    let genesis_head = hex::decode(genesis_head_hex.trim().trim_start_matches("0x"))?;
    let validation_code = hex::decode(validation_code_hex.trim().trim_start_matches("0x"))?;

    info!("Genesis head: {} bytes", genesis_head.len());
    info!("Validation code: {} bytes", validation_code.len());

    // Calculate deposit (Paseo constants)
    let para_deposit: u128 = 1_000_000_000_000; // 100 PAS (Paseo uses 10 decimals)
    let data_deposit_per_byte: u128 = 10_000_000; // 0.001 PAS per byte
    let data_bytes = genesis_head.len() as u128 + validation_code.len() as u128;
    let estimated_deposit = para_deposit + (data_bytes * data_deposit_per_byte);

    println!("Estimated deposit: {} PAS", estimated_deposit / 10_000_000_000);

    let tx = subxt::dynamic::tx("Registrar", "register", vec![
        subxt::dynamic::Value::u128(para_id as u128),
        subxt::dynamic::Value::from_bytes(genesis_head),
        subxt::dynamic::Value::from_bytes(validation_code),
    ]);

    println!("Submitting registration transaction...");

    let result = client
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await?
        .wait_for_finalized_success()
        .await?;

    println!("Parachain registered!");
    println!("Extrinsic hash: {:?}", result.extrinsic_hash());

    Ok(())
}

async fn cmd_coretime(rpc: &str, seed: Option<&str>, action: CoretimeAction) -> Result<()> {
    // For coretime, we need to connect to the coretime chain
    let coretime_rpc = PASEO_CORETIME_RPC;

    match action {
        CoretimeAction::Status => {
            let client = OnlineClient::<PolkadotConfig>::from_url(coretime_rpc).await?;

            // Query sale info
            let sale_key = subxt::dynamic::storage("Broker", "SaleInfo", vec![]);
            if let Some(value) = client.storage().at_latest().await?.fetch(&sale_key).await? {
                let data = value.to_value()?;
                println!("Sale Info: {:#?}", data);
            }

            // Query configuration
            let config_key = subxt::dynamic::storage("Broker", "Configuration", vec![]);
            if let Some(value) = client.storage().at_latest().await?.fetch(&config_key).await? {
                let data = value.to_value()?;
                println!("Configuration: {:#?}", data);
            }

            Ok(())
        }
        CoretimeAction::Purchase { price_limit } => {
            let seed = seed.ok_or_else(|| anyhow!("--seed required"))?;
            let client = OnlineClient::<PolkadotConfig>::from_url(coretime_rpc).await?;
            let signer = SubxtSigner::from_seed(seed)?;

            info!("Purchasing coretime with price limit: {}", price_limit);

            let tx = subxt::dynamic::tx("Broker", "purchase", vec![
                subxt::dynamic::Value::u128(price_limit),
            ]);

            let result = client
                .tx()
                .sign_and_submit_then_watch_default(&tx, &signer)
                .await?
                .wait_for_finalized_success()
                .await?;

            println!("Coretime purchased!");
            println!("Extrinsic hash: {:?}", result.extrinsic_hash());

            Ok(())
        }
        CoretimeAction::Assign { region_id, task_id, finality } => {
            let seed = seed.ok_or_else(|| anyhow!("--seed required"))?;
            let client = OnlineClient::<PolkadotConfig>::from_url(coretime_rpc).await?;
            let signer = SubxtSigner::from_seed(seed)?;

            info!("Assigning region {} to task {} with {:?} finality", region_id, task_id, finality);

            // Parse region_id - format: "begin,core,mask" or just use the raw values
            // For now, we'll use the assign extrinsic
            let finality_value = match finality.as_deref() {
                Some("final") | Some("Final") => "Final",
                _ => "Provisional",
            };

            // Note: Region ID is a tuple (begin: u32, core: u16, mask: [u8; 10])
            // This is simplified - real implementation needs proper region ID parsing
            warn!("Region ID parsing is simplified - use polkadot.js for complex regions");

            let region_values: Vec<subxt::dynamic::Value> = vec![
                subxt::dynamic::Value::u128(0), // begin
                subxt::dynamic::Value::u128(0), // core
                subxt::dynamic::Value::from_bytes(vec![0xff; 10]), // mask (all cores)
            ];
            let empty: Vec<subxt::dynamic::Value> = vec![];
            let tx = subxt::dynamic::tx("Broker", "assign", vec![
                // region_id - this needs to be properly structured
                subxt::dynamic::Value::unnamed_composite(region_values),
                subxt::dynamic::Value::u128(task_id as u128),
                subxt::dynamic::Value::unnamed_variant(finality_value, empty),
            ]);

            let result = client
                .tx()
                .sign_and_submit_then_watch_default(&tx, &signer)
                .await?
                .wait_for_finalized_success()
                .await?;

            println!("Coretime assigned!");
            println!("Extrinsic hash: {:?}", result.extrinsic_hash());

            Ok(())
        }
    }
}

fn cmd_chainspec(para_id: u32, input: &PathBuf, output: &PathBuf) -> Result<()> {
    info!("Updating chainspec with para_id: {}", para_id);

    let content = std::fs::read_to_string(input)?;
    let mut chainspec: serde_json::Value = serde_json::from_str(&content)?;

    // Update para_id in the chainspec
    chainspec["para_id"] = serde_json::Value::Number(para_id.into());

    // Also update in genesis config if present
    if let Some(genesis) = chainspec.get_mut("genesis") {
        if let Some(runtime) = genesis.get_mut("runtimeGenesis") {
            if let Some(config) = runtime.get_mut("config") {
                if let Some(parachain_info) = config.get_mut("parachainInfo") {
                    parachain_info["parachainId"] = serde_json::Value::Number(para_id.into());
                }
            }
        }
    }

    let output_content = serde_json::to_string_pretty(&chainspec)?;
    std::fs::write(output, &output_content)?;

    println!("Chainspec written to: {}", output.display());
    println!("Para ID: {}", para_id);

    Ok(())
}

fn cmd_export_genesis(chainspec: &PathBuf, output_dir: &PathBuf) -> Result<()> {
    info!("Exporting genesis from chainspec");

    std::fs::create_dir_all(output_dir)?;

    let content = std::fs::read_to_string(chainspec)?;
    let chainspec: serde_json::Value = serde_json::from_str(&content)?;

    // Extract runtime code (validation code)
    let code = chainspec
        .get("genesis")
        .and_then(|g| g.get("runtimeGenesis"))
        .and_then(|r| r.get("code"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("No genesis code found in chainspec"))?;

    // Write validation code
    let wasm_path = output_dir.join("zanchor-genesis-wasm.hex");
    std::fs::write(&wasm_path, code)?;
    println!("Wrote: {}", wasm_path.display());

    // For genesis head, we need to compute it from the genesis state
    // This is a simplified version - just use empty/default head
    // Real implementation would need to compute the genesis block header
    let genesis_head = "0x000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
    let head_path = output_dir.join("zanchor-genesis-head.hex");
    std::fs::write(&head_path, genesis_head)?;
    println!("Wrote: {}", head_path.display());

    println!("\nNote: Genesis head is placeholder. Use `chain-spec-builder` for accurate genesis.");

    Ok(())
}

fn parse_account(addr: &str) -> Result<subxt::utils::AccountId32> {
    use sp_core::crypto::Ss58Codec;
    let account = sp_core::sr25519::Public::from_ss58check(addr)
        .map_err(|e| anyhow!("Invalid SS58 address: {:?}", e))?;
    Ok(subxt::utils::AccountId32::from(account.0))
}
