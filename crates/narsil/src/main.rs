//! narsil cli - syndicate management for penumbra
//!
//! commands:
//!   init     - create a new syndicate (generates spending key)
//!   address  - show syndicate receiving address
//!   balance  - show syndicate balance (requires view service)
//!   send     - propose a spend transaction

use clap::{Parser, Subcommand};
use penumbra_sdk_keys::keys::{SpendKey, SeedPhrase, Bip44Path, AddressIndex};
use std::path::PathBuf;

/// narsil - threshold syndicate for penumbra
#[derive(Parser)]
#[command(name = "narsil")]
#[command(about = "threshold syndicate management for penumbra")]
#[command(version)]
struct Cli {
    /// data directory (default: ~/.narsil)
    #[arg(short, long)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// initialize a new syndicate (single-member for testing)
    Init {
        /// syndicate name
        #[arg(short, long, default_value = "default")]
        name: String,
    },

    /// show syndicate receiving address
    Address {
        /// syndicate name
        #[arg(short, long, default_value = "default")]
        name: String,

        /// address index (default: 0)
        #[arg(short, long, default_value = "0")]
        index: u32,
    },

    /// show full viewing key (for sharing with members)
    Fvk {
        /// syndicate name
        #[arg(short, long, default_value = "default")]
        name: String,
    },

    /// show spending key (DANGER: reveals secret)
    #[command(hide = true)]
    ExportSpendKey {
        /// syndicate name
        #[arg(short, long, default_value = "default")]
        name: String,
    },
}

fn get_data_dir(custom: Option<PathBuf>) -> PathBuf {
    custom.unwrap_or_else(|| {
        directories::ProjectDirs::from("", "", "narsil")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".narsil"))
    })
}

fn syndicate_path(data_dir: &PathBuf, name: &str) -> PathBuf {
    data_dir.join(format!("{}.spend_key", name))
}

fn main() {
    let cli = Cli::parse();
    let data_dir = get_data_dir(cli.data_dir);

    match cli.command {
        Commands::Init { name } => {
            let path = syndicate_path(&data_dir, &name);
            if path.exists() {
                eprintln!("syndicate '{}' already exists at {:?}", name, path);
                std::process::exit(1);
            }

            // create data dir
            std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

            // generate seed phrase and spending key
            let seed_phrase = SeedPhrase::generate(rand_core::OsRng);
            let spend_key = SpendKey::from_seed_phrase_bip44(seed_phrase.clone(), &Bip44Path::new(0));
            let fvk = spend_key.full_viewing_key();
            let (address, _) = fvk.payment_address(AddressIndex::new(0));

            // save seed phrase (more portable than raw spend key bytes)
            let seed_path = data_dir.join(format!("{}.seed", name));
            std::fs::write(&seed_path, seed_phrase.to_string()).expect("failed to save seed");

            // also save spend key bytes for quick loading
            let spend_key_bytes = spend_key.to_bytes();
            std::fs::write(&path, spend_key_bytes.0).expect("failed to save spend key");

            println!("syndicate '{}' initialized", name);
            println!("address: {}", address);
            println!("");
            println!("seed phrase (BACK THIS UP!):");
            println!("  {}", seed_phrase);
            println!("");
            println!("send test funds with:");
            println!("  pcli tx send 1mpenumbra --to {}", address);
        }

        Commands::Address { name, index } => {
            let path = syndicate_path(&data_dir, &name);
            if !path.exists() {
                eprintln!("syndicate '{}' not found. run 'narsil init' first", name);
                std::process::exit(1);
            }

            let spend_key = load_spend_key(&path);
            let fvk = spend_key.full_viewing_key();
            let (address, _) = fvk.payment_address(AddressIndex::new(index));

            println!("{}", address);
        }

        Commands::Fvk { name } => {
            let path = syndicate_path(&data_dir, &name);
            if !path.exists() {
                eprintln!("syndicate '{}' not found", name);
                std::process::exit(1);
            }

            let spend_key = load_spend_key(&path);
            let fvk = spend_key.full_viewing_key();

            // encode FVK as bech32
            println!("{}", fvk);
        }

        Commands::ExportSpendKey { name } => {
            let path = syndicate_path(&data_dir, &name);
            if !path.exists() {
                eprintln!("syndicate '{}' not found", name);
                std::process::exit(1);
            }

            eprintln!("WARNING: this reveals the spending key!");
            let bytes = std::fs::read(&path).expect("failed to read spend key");
            println!("{}", hex::encode(&bytes));
        }
    }
}

fn load_spend_key(path: &PathBuf) -> SpendKey {
    let bytes = std::fs::read(path).expect("failed to read spend key");
    let bytes: [u8; 32] = bytes.try_into().expect("invalid spend key length");
    SpendKey::from(penumbra_sdk_keys::keys::SpendKeyBytes(bytes))
}
