//! State Transition Client
//!
//! This client:
//! 1. Prepares transactions
//! 2. Fetches current state root and witnesses from server
//! 3. Runs STF in PolkaVM zkVM to generate proof
//! 4. Submits proof + state transition to server

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use tracing::info;

type StateRoot = [u8; 32];

#[derive(Parser)]
#[command(name = "state-transition-client")]
#[command(about = "Client for zkVM-based state transitions")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Server URL
    #[arg(long, default_value = "http://localhost:3000")]
    server: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Transfer tokens between accounts
    Transfer {
        /// Source account
        #[arg(long)]
        from: String,

        /// Destination account
        #[arg(long)]
        to: String,

        /// Amount to transfer
        #[arg(long)]
        amount: u64,
    },

    /// Check current state root
    StateRoot,
}

#[derive(Debug, Serialize, Deserialize)]
struct Transaction {
    from: String,
    to: String,
    amount: u64,
    #[serde(with = "serde_bytes")]
    signature: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct TransitionRequest {
    old_state_root: StateRoot,
    transaction: Transaction,
    new_state_root: StateRoot,
    state_diffs: Vec<(Vec<u8>, Vec<u8>)>,
    #[serde(with = "serde_bytes")]
    proof: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct StateRootResponse {
    root: StateRoot,
}

#[derive(Debug, Serialize)]
struct WitnessRequest {
    keys: Vec<Vec<u8>>,
}

#[derive(Debug, Deserialize)]
struct WitnessResponse {
    #[serde(with = "serde_bytes")]
    witnesses: Vec<u8>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "state_transition_client=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Transfer { from, to, amount } => {
            handle_transfer(&cli.server, from, to, amount).await?;
        }
        Commands::StateRoot => {
            handle_state_root(&cli.server).await?;
        }
    }

    Ok(())
}

async fn handle_transfer(
    server: &str,
    from: String,
    to: String,
    amount: u64,
) -> anyhow::Result<()> {
    info!("Preparing transfer: {} -> {} ({} units)", from, to, amount);

    // 1. Fetch current state root
    let client = reqwest::Client::new();
    let state_root_resp: StateRootResponse = client
        .get(format!("{}/state_root", server))
        .send()
        .await?
        .json()
        .await?;

    info!("Current state root: {:?}", state_root_resp.root);

    // 2. Fetch NOMT witnesses for accounts we'll access
    let witness_req = WitnessRequest {
        keys: vec![
            format!("account:{}", from).into_bytes(),
            format!("account:{}", to).into_bytes(),
        ],
    };

    let witnesses_resp: WitnessResponse = client
        .post(format!("{}/get_witnesses", server))
        .json(&witness_req)
        .send()
        .await?
        .json()
        .await?;

    info!("Received {} bytes of witness data", witnesses_resp.witnesses.len());

    // 3. Prepare transaction (with placeholder signature)
    let tx = Transaction {
        from: from.clone(),
        to: to.clone(),
        amount,
        signature: vec![0u8; 64], // TODO: Real signature
    };

    // 4. Run STF in PolkaVM zkVM to generate proof
    // TODO: Replace with actual PolkaVM execution
    info!("Generating proof (TODO: actual PolkaVM integration)");

    // Placeholder: simulate state transition
    let (new_state_root, state_diffs, proof) =
        simulate_state_transition(&state_root_resp.root, &tx);

    info!("Proof generated, new state root: {:?}", new_state_root);

    // 5. Submit to server
    let transition_req = TransitionRequest {
        old_state_root: state_root_resp.root,
        transaction: tx,
        new_state_root,
        state_diffs,
        proof,
    };

    let response = client
        .post(format!("{}/submit_transition", server))
        .json(&transition_req)
        .send()
        .await?;

    if response.status().is_success() {
        info!("✓ Transaction accepted by server");
    } else {
        let error_text = response.text().await?;
        anyhow::bail!("Transaction rejected: {}", error_text);
    }

    Ok(())
}

async fn handle_state_root(server: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let resp: StateRootResponse = client
        .get(format!("{}/state_root", server))
        .send()
        .await?
        .json()
        .await?;

    println!("Current state root: {:02x?}", resp.root);

    Ok(())
}

/// Placeholder for actual PolkaVM zkVM execution
fn simulate_state_transition(
    old_root: &StateRoot,
    tx: &Transaction,
) -> (StateRoot, Vec<(Vec<u8>, Vec<u8>)>, Vec<u8>) {
    // In reality, this would:
    // 1. Load STF bytecode
    // 2. Execute in PolkaVM with proving enabled
    // 3. Extract new state root, diffs, and proof

    let mut new_root = *old_root;
    new_root[0] = new_root[0].wrapping_add(1); // Simulate state change

    let state_diffs = vec![
        (format!("account:{}", tx.from).into_bytes(), vec![]), // Placeholder
        (format!("account:{}", tx.to).into_bytes(), vec![]),
    ];

    let proof = vec![0u8; 1024]; // Placeholder proof

    (new_root, state_diffs, proof)
}

mod serde_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<u8>::deserialize(deserializer)
    }
}
