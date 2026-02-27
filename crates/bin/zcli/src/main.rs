mod address;
mod cli;
mod client;
mod error;
mod key;
mod ops;
mod tx;
mod wallet;
mod witness;

use clap::Parser;

use cli::{Cli, Command};
use error::Error;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match run(&cli).await {
        Ok(()) => 0,
        Err(e) => {
            let code = e.exit_code();
            if cli.script {
                let msg = serde_json::json!({ "error": e.to_string() });
                eprintln!("{}", msg);
            } else {
                eprintln!("error: {}", e);
            }
            code
        }
    };
    std::process::exit(code);
}

async fn run(cli: &Cli) -> Result<(), Error> {
    let mainnet = cli.is_mainnet();

    match &cli.command {
        Command::Address { orchard, transparent } => {
            cmd_address(cli, mainnet, *orchard, *transparent)
        }
        Command::Receive => {
            cmd_receive(cli, mainnet)
        }
        Command::Balance => {
            cmd_balance(cli, mainnet).await
        }
        Command::Sync { from, position } => {
            cmd_sync(cli, mainnet, *from, *position).await
        }
        Command::Shield { fee } => {
            let seed = load_seed(cli)?;
            ops::shield::shield(&seed, &cli.endpoint, *fee, mainnet, cli.script).await
        }
        Command::Send { amount, recipient, memo } => {
            let seed = load_seed(cli)?;
            ops::send::send(
                &seed, amount, recipient, memo.as_deref(),
                &cli.endpoint, mainnet, cli.script,
            ).await
        }
        Command::TreeInfo { height } => {
            cmd_tree_info(cli, *height).await
        }
        Command::Export => {
            let seed = load_seed(cli)?;
            ops::export::export(&seed, mainnet, cli.script)
        }
    }
}

fn cmd_address(
    cli: &Cli,
    mainnet: bool,
    show_orchard: bool,
    show_transparent: bool,
) -> Result<(), Error> {
    let seed = load_seed(cli)?;

    let (show_o, show_t) = if !show_orchard && !show_transparent {
        (true, true)
    } else {
        (show_orchard, show_transparent)
    };

    let mut result = serde_json::Map::new();

    if show_t {
        let taddr = address::transparent_address(&seed, mainnet)?;
        if cli.script {
            result.insert("transparent".into(), serde_json::Value::String(taddr));
        } else {
            println!("{}", taddr);
        }
    }

    if show_o {
        let uaddr = address::orchard_address(&seed, mainnet)?;
        if cli.script {
            result.insert("orchard".into(), serde_json::Value::String(uaddr));
        } else {
            println!("{}", uaddr);
        }
    }

    if cli.script {
        println!("{}", serde_json::Value::Object(result));
    }

    Ok(())
}

fn cmd_receive(cli: &Cli, mainnet: bool) -> Result<(), Error> {
    let seed = load_seed(cli)?;
    let uaddr = address::orchard_address(&seed, mainnet)?;
    let taddr = address::transparent_address(&seed, mainnet)?;

    if cli.script {
        println!("{}", serde_json::json!({
            "orchard": uaddr,
            "transparent": taddr,
        }));
        return Ok(());
    }

    // render unified address as terminal QR using unicode half-blocks
    use qrcode::QrCode;
    let code = QrCode::new(uaddr.as_bytes())
        .map_err(|e| Error::Other(format!("qr encode: {}", e)))?;
    let width = code.width();
    let modules = code.into_colors();

    let dark = |r: usize, c: usize| -> bool {
        if r < width && c < width {
            modules[r * width + c] == qrcode::Color::Dark
        } else {
            false
        }
    };

    // quiet zone + half-block rendering (2 rows per line using ▀▄█ )
    let quiet = 1;
    let total = width + quiet * 2;

    for row in (0..total).step_by(2) {
        for col in 0..total {
            let r0 = row.wrapping_sub(quiet);
            let c0 = col.wrapping_sub(quiet);
            let r1 = r0.wrapping_add(1);
            let top = dark(r0, c0);
            let bot = dark(r1, c0);
            match (top, bot) {
                (true, true) => print!("\u{2588}"),   // █
                (true, false) => print!("\u{2580}"),  // ▀
                (false, true) => print!("\u{2584}"),  // ▄
                (false, false) => print!(" "),
            }
        }
        println!();
    }

    println!();
    println!("unified:     {}", uaddr);
    println!("transparent: {}", taddr);

    Ok(())
}

async fn cmd_balance(cli: &Cli, mainnet: bool) -> Result<(), Error> {
    let seed = load_seed(cli)?;
    let bal = ops::balance::get_balance(&seed, &cli.endpoint, mainnet).await?;

    if cli.script {
        println!("{}", serde_json::json!({
            "transparent": bal.transparent,
            "shielded": bal.shielded,
            "total": bal.total,
            "transparent_zec": format!("{:.8}", bal.transparent as f64 / 1e8),
            "shielded_zec": format!("{:.8}", bal.shielded as f64 / 1e8),
            "total_zec": format!("{:.8}", bal.total as f64 / 1e8),
        }));
    } else {
        let t = bal.transparent as f64 / 1e8;
        let s = bal.shielded as f64 / 1e8;
        let total = bal.total as f64 / 1e8;
        println!("transparent: {:.8} ZEC", t);
        println!("shielded:    {:.8} ZEC", s);
        println!("total:       {:.8} ZEC", total);
    }

    Ok(())
}

async fn cmd_sync(cli: &Cli, mainnet: bool, from: Option<u32>, position: Option<u64>) -> Result<(), Error> {
    let seed = load_seed(cli)?;
    let found = ops::sync::sync(&seed, &cli.endpoint, mainnet, cli.script, from, position).await?;

    if cli.script {
        println!("{}", serde_json::json!({ "notes_found": found }));
    }

    Ok(())
}

async fn cmd_tree_info(cli: &Cli, height: u32) -> Result<(), Error> {
    let client = client::ZidecarClient::connect(&cli.endpoint).await?;
    let (tree_hex, actual_height) = client.get_tree_state(height).await?;

    // parse frontier to get tree size
    // lightwalletd orchard tree format: hex-encoded binary frontier
    let tree_bytes = hex::decode(&tree_hex)
        .map_err(|e| Error::Other(format!("invalid tree hex: {}", e)))?;
    // frontier encoding: depth-first serialization of the frontier
    // the number of leaves = tree size, derivable from the frontier structure
    // quick parse: count the 01-prefixed nodes in the frontier
    let tree_size = parse_frontier_size(&tree_bytes)?;

    if cli.script {
        println!("{}", serde_json::json!({
            "height": actual_height,
            "orchard_tree_size": tree_size,
            "tree_hex_len": tree_hex.len(),
        }));
    } else {
        eprintln!("height: {}", actual_height);
        eprintln!("orchard tree size (leaves): {}", tree_size);
        eprintln!("tree hex length: {} chars", tree_hex.len());
    }
    Ok(())
}

/// parse the size (number of leaves) from a zcashd frontier encoding
fn parse_frontier_size(data: &[u8]) -> Result<u64, Error> {
    witness::frontier_tree_size(data)
}

fn load_seed(cli: &Cli) -> Result<key::WalletSeed, Error> {
    if let Some(ref mnemonic) = cli.mnemonic {
        key::load_mnemonic_seed(mnemonic)
    } else {
        key::load_ssh_seed(&cli.identity_path())
    }
}
