use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "zcli", about = "zcash wallet CLI — ssh keys as wallet seed")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// path to ed25519 ssh private key
    #[arg(short = 'i', long = "identity", global = true, env = "ZCLI_IDENTITY",
          default_value = "~/.ssh/id_ed25519")]
    pub identity: String,

    /// use bip39 mnemonic instead of ssh key
    #[arg(long, global = true, env = "ZCLI_MNEMONIC")]
    pub mnemonic: Option<String>,

    /// zidecar gRPC endpoint
    #[arg(long, global = true, env = "ZCLI_ENDPOINT",
          default_value = "https://zcash.rotko.net")]
    pub endpoint: String,

    /// machine-readable json output, no prompts/progress/qr
    #[arg(long, visible_alias = "json", global = true, env = "ZCLI_SCRIPT")]
    pub script: bool,

    /// use mainnet (default)
    #[arg(long, global = true, default_value_t = true)]
    pub mainnet: bool,

    /// use testnet
    #[arg(long, global = true)]
    pub testnet: bool,
}

impl Cli {
    pub fn is_mainnet(&self) -> bool {
        !self.testnet
    }

    /// expand ~ in identity path
    pub fn identity_path(&self) -> String {
        if self.identity.starts_with("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                return format!("{}/{}", home.to_string_lossy(), &self.identity[2..]);
            }
        }
        self.identity.clone()
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// show wallet addresses
    Address {
        /// show orchard (shielded) address
        #[arg(long)]
        orchard: bool,

        /// show transparent address
        #[arg(long)]
        transparent: bool,
    },

    /// show wallet balance
    Balance,

    /// shield transparent funds (t→z)
    Shield {
        /// fee override in zatoshis (auto-computed if omitted)
        #[arg(long)]
        fee: Option<u64>,
    },

    /// send zcash
    Send {
        /// amount in ZEC (e.g. 0.001)
        amount: String,

        /// recipient: t1.../u1...
        recipient: String,

        /// memo text (shielded only)
        #[arg(long)]
        memo: Option<String>,
    },

    /// print receiving address
    Receive,

    /// scan chain for wallet notes
    Sync {
        /// start scanning from this block height
        #[arg(long)]
        from: Option<u32>,

        /// starting orchard position counter (use with --from to skip full scan)
        #[arg(long)]
        position: Option<u64>,
    },

    /// export wallet keys (requires confirmation)
    Export,

    /// show orchard tree info at a height (for --position)
    TreeInfo {
        /// block height
        height: u32,
    },

}
