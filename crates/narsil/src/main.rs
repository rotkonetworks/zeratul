//! narsil cli - syndicate management for penumbra
//!
//! commands:
//!   init     - create a new syndicate (generates spending key)
//!   address  - show syndicate receiving address
//!   balance  - scan and show syndicate balance
//!   send     - send funds from syndicate
//!   status   - show syndicate governance status

use clap::{Parser, Subcommand};
use penumbra_sdk_keys::keys::{SpendKey, SeedPhrase, Bip44Path, AddressIndex};
use std::path::PathBuf;
use std::process::Command;

/// narsil - threshold syndicate for penumbra
#[derive(Parser)]
#[command(name = "narsil")]
#[command(about = "threshold syndicate management for penumbra")]
#[command(version)]
struct Cli {
    /// data directory (default: ~/.narsil)
    #[arg(short, long)]
    data_dir: Option<PathBuf>,

    /// penumbra grpc endpoint
    #[arg(long, default_value = "https://penumbra.rotko.net")]
    grpc_url: String,

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

    /// scan chain and show balance
    Balance {
        /// syndicate name
        #[arg(short, long, default_value = "default")]
        name: String,
    },

    /// send funds from syndicate (single-member mode)
    Send {
        /// syndicate name
        #[arg(short, long, default_value = "default")]
        name: String,

        /// amount (e.g. "5mpenumbra", "100upenumbra")
        #[arg(short, long)]
        amount: String,

        /// destination address
        #[arg(short, long)]
        to: String,
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

/// get or create pcli home for a syndicate
fn pcli_home(data_dir: &PathBuf, name: &str) -> PathBuf {
    data_dir.join(format!("{}.pcli", name))
}

/// ensure pcli is initialized for this syndicate
fn ensure_pcli(data_dir: &PathBuf, name: &str, grpc_url: &str) -> PathBuf {
    let home = pcli_home(data_dir, name);
    if home.join("config.toml").exists() {
        return home;
    }

    let seed_path = data_dir.join(format!("{}.seed", name));
    let seed = std::fs::read_to_string(&seed_path).expect("failed to read seed phrase");

    std::fs::create_dir_all(&home).expect("failed to create pcli home");

    let status = Command::new("pcli")
        .args(["--home", home.to_str().unwrap()])
        .args(["init", "--grpc-url", grpc_url, "soft-kms", "import-phrase"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                writeln!(stdin, "{}", seed.trim())?;
            }
            child.wait()
        })
        .expect("failed to init pcli");

    if !status.success() {
        eprintln!("pcli init failed");
        std::process::exit(1);
    }

    home
}

/// run pcli command for a syndicate
fn run_pcli(home: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new("pcli")
        .args(["--home", home.to_str().unwrap()])
        .args(args)
        .output()
        .expect("failed to run pcli")
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

            std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

            let seed_phrase = SeedPhrase::generate(rand_core::OsRng);
            let spend_key = SpendKey::from_seed_phrase_bip44(seed_phrase.clone(), &Bip44Path::new(0));
            let fvk = spend_key.full_viewing_key();
            let (address, _) = fvk.payment_address(AddressIndex::new(0));

            let seed_path = data_dir.join(format!("{}.seed", name));
            std::fs::write(&seed_path, seed_phrase.to_string()).expect("failed to save seed");

            let spend_key_bytes = spend_key.to_bytes();
            std::fs::write(&path, spend_key_bytes.0).expect("failed to save spend key");

            println!("syndicate '{}' initialized", name);
            println!("address: {}", address);
            println!("");
            println!("seed phrase:");
            println!("  {}", seed_phrase);
            println!("");
            println!("send test funds:");
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
            println!("{}", fvk);
        }

        Commands::Balance { name } => {
            let path = syndicate_path(&data_dir, &name);
            if !path.exists() {
                eprintln!("syndicate '{}' not found", name);
                std::process::exit(1);
            }

            let home = ensure_pcli(&data_dir, &name, &cli.grpc_url);
            let output = run_pcli(&home, &["view", "balance"]);

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stderr.is_empty() {
                eprint!("{}", stderr);
            }
            print!("{}", stdout);
        }

        Commands::Send { name, amount, to } => {
            let path = syndicate_path(&data_dir, &name);
            if !path.exists() {
                eprintln!("syndicate '{}' not found", name);
                std::process::exit(1);
            }

            let home = ensure_pcli(&data_dir, &name, &cli.grpc_url);

            println!("sending {} to {}...", amount, &to[..20]);
            let output = run_pcli(&home, &["tx", "send", &amount, "--to", &to]);

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stderr.is_empty() {
                eprint!("{}", stderr);
            }
            if !stdout.is_empty() {
                print!("{}", stdout);
            }

            if output.status.success() {
                println!("send complete");
            } else {
                eprintln!("send failed");
                std::process::exit(1);
            }
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
