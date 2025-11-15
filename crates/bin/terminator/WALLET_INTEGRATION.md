# Wallet Integration - Using pcli's Wallet & Database

## Overview

Terminator now uses **the exact same wallet and database as pcli**. This means:

✅ **Same seed phrase** - No separate wallet needed
✅ **Same view database** - Shares `pcli-view.sqlite`
✅ **Same balances** - Sees all your assets
✅ **Same transaction history** - All your past swaps and transfers
✅ **Zero configuration** - Just run `pcli init` once

## How It Works

### 1. Home Directory

Both pcli and Terminator use:
- **Linux**: `~/.local/share/pcli/`
- **macOS**: `~/Library/Application Support/zone.penumbra.pcli/`
- **Windows**: `%APPDATA%\penumbra\pcli\`

### 2. Files Shared

```
~/.local/share/pcli/
├── config.toml          # Contains FVK and gRPC endpoint
├── pcli-view.sqlite     # Wallet database (shared!)
└── registry.json        # Asset metadata (optional)
```

### 3. View Database

The `pcli-view.sqlite` file contains:
- **Scanned notes** - All your spendable notes
- **Transaction history** - Past swaps, sends, stakes
- **Balances** - Current holdings per asset
- **Sync state** - Last scanned block height

## Architecture

### wallet/mod.rs

```rust
pub struct Wallet {
    pub config: PcliConfig,           // FVK + gRPC URL
    pub view_client: ViewServiceClient,  // Queries wallet DB
    pub home: Utf8PathBuf,            // ~/.local/share/pcli/
}

impl Wallet {
    /// Load wallet from pcli's home directory
    pub async fn load() -> Result<Self> {
        let home = pcli_home();  // Same as pcli!
        let config = PcliConfig::load(home.join("config.toml"))?;

        // Load ViewServer from pcli's sqlite database
        let view_server = ViewServer::load_or_initialize(
            Some(home.join("pcli-view.sqlite")),
            /* ... */
        ).await?;

        // Wrap in gRPC client for easy querying
        let view_client = ViewServiceClient::new(view_server);

        Ok(Self { config, view_client, home })
    }

    /// Query account balances
    pub async fn query_balances(&mut self) -> Result<Vec<Value>> {
        // Queries the sqlite database via ViewServiceClient
    }
}
```

### Startup Flow

```rust
// main.rs
#[tokio::main]
async fn main() -> Result<()> {
    let mut app = AppState::new();

    // Load wallet from pcli
    match wallet::Wallet::load().await {
        Ok(wallet) => {
            eprintln!("✓ Wallet loaded from {}", wallet.home);
            eprintln!("  FVK: {}", wallet.fvk());
            eprintln!("  gRPC: {}", wallet.grpc_url());

            // Use wallet's gRPC endpoint
            app.connect_penumbra_with_wallet(wallet).await?;

            // Fetch balances
            app.update_balances().await;
            eprintln!("✓ {} assets found", app.balances.len());
        }
        Err(e) => {
            eprintln!("Warning: Failed to load pcli wallet: {}", e);
            eprintln!("Tip: Run 'pcli init' to set up your wallet");
        }
    }

    run_app(&mut terminal, &mut app).await?;
}
```

### Data Flow

```
Terminator Startup
    ↓
wallet::Wallet::load()
    ↓
Read ~/.local/share/pcli/config.toml
    ↓
Load ~/.local/share/pcli/pcli-view.sqlite
    ↓
ViewServer::load_or_initialize()
    ↓
Wrap in ViewServiceClient
    ↓
AppState::connect_penumbra_with_wallet()
    ↓
Query balances via ViewServiceClient
    ↓
Display in UI
```

## Benefits

### 1. **No Duplicate Wallets**
```bash
# Before (hypothetical):
pcli init          # Create wallet
terminator init    # Create separate wallet (bad!)

# Now:
pcli init          # Create wallet
terminator         # Uses same wallet (good!)
```

### 2. **Shared Transaction History**
```bash
# Send 100 penumbra via pcli
pcli tx send 100penumbra --to penumbra1abc...

# Check balance in Terminator
terminator  # Shows updated balance!
```

### 3. **Same gRPC Endpoint**

If you configured pcli to use a custom node:
```bash
pcli init --grpc-url https://my-custom-node.com
```

Terminator automatically uses the same endpoint!

### 4. **Wallet Sync is Shared**

When pcli syncs:
```bash
pcli view sync
```

Terminator sees the updated state immediately (same database!)

## Configuration

### Setting Up (First Time)

```bash
# 1. Initialize pcli wallet
pcli init --grpc-url https://penumbra.rotko.net

# 2. Sync the wallet
pcli view sync

# 3. Check balance
pcli view balance

# 4. Run Terminator (uses same wallet!)
terminator
```

### Checking Wallet Status

```rust
// In Terminator
if Wallet::is_initialized() {
    let wallet = Wallet::load().await?;
    let balances = wallet.query_balances().await?;
    println!("Balances: {:?}", balances);
}
```

### Programmatic Access

```rust
// Load wallet
let mut wallet = Wallet::load().await?;

// Get FVK
let fvk = wallet.fvk();
println!("FVK: {}", fvk);

// Get gRPC endpoint
let endpoint = wallet.grpc_url();
println!("Connecting to: {}", endpoint);

// Query balances
let balances = wallet.query_balances().await?;
for balance in balances {
    println!("{}", balance.format(&asset::REGISTRY));
}

// Access view client directly for advanced queries
let notes = wallet.view_client.notes(request).await?;
```

## Implementation Details

### ViewServiceClient vs Direct DB Access

We use `ViewServiceClient` instead of raw SQL because:

1. **Abstraction** - Don't need to know DB schema
2. **Type safety** - Returns proper Penumbra types
3. **Compatibility** - If DB schema changes, we still work
4. **Reusability** - Same API as pcli uses

### In-Process gRPC

```rust
// We use in-process gRPC (no network overhead)
let view_server = ViewServer::load_or_initialize(...).await?;
let view_svc = ViewServiceServer::new(view_server);
let view_client = ViewServiceClient::new(box_grpc_svc::local(view_svc));

// Now we can call gRPC methods:
view_client.balances(request).await?;
view_client.notes(request).await?;
view_client.transaction_info_by_hash(request).await?;
```

This is **local**, not over the network!

### Future: Transaction Submission

To submit transactions, we'll need `CustodyService`:

```rust
// Load custody service (handles signing)
let custody = match &config.custody {
    CustodyConfig::SoftKms(config) => {
        let soft_kms = SoftKms::new(config.clone());
        CustodyServiceServer::new(soft_kms)
    }
    // ... other custody types
};

// Build transaction
let tx = TransactionBuilder::new()
    .swap(...)
    .build(&view_client, &custody)
    .await?;

// Broadcast
client.broadcast_tx_sync(tx).await?;
```

## Troubleshooting

### "pcli not initialized"

```
Error: pcli not initialized. Run 'pcli init' first.
Expected home directory: ~/.local/share/pcli/
```

**Solution:**
```bash
pcli init --grpc-url https://penumbra.rotko.net
```

### "Failed to load view database"

```
Error: Failed to load view database
```

**Possible causes:**
1. Database is locked by running pcli instance
2. Database file corrupted
3. Permissions issue

**Solution:**
```bash
# Close all pcli instances
pkill pcli

# Check permissions
ls -la ~/.local/share/pcli/pcli-view.sqlite

# If corrupted, reset (WARNING: loses local state):
pcli view reset
pcli view sync
```

### "No balances found"

```
✓ Wallet loaded from ~/.local/share/pcli/
✓ 0 assets found
```

**Cause:** Wallet hasn't synced yet.

**Solution:**
```bash
pcli view sync
```

## Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcli_home_path() {
        let home = pcli_home();
        assert!(home.as_str().contains("pcli"));
    }

    #[test]
    fn test_is_initialized() {
        let initialized = Wallet::is_initialized();
        // Will be true if pcli init has been run
    }

    #[tokio::test]
    async fn test_load_wallet() {
        // Only runs if wallet exists
        if Wallet::is_initialized() {
            let wallet = Wallet::load().await.unwrap();
            assert!(wallet.fvk().to_string().len() > 0);
        }
    }
}
```

## Security Notes

### Private Keys

Terminator **never** directly accesses private keys. The `config.toml` only contains:
- Full Viewing Key (FVK) - Can see balances/transactions
- gRPC URL - Public endpoint

Private keys are handled by:
- `SoftKms` - Encrypted on disk
- `Encrypted` - Password-protected
- `Threshold` - Multi-party signing
- `Ledger` - Hardware wallet

### Database Security

The `pcli-view.sqlite` file contains:
- ✅ Public: Asset IDs, amounts, transaction hashes
- ✅ Semi-private: Your addresses (derived from FVK)
- ❌ No private keys
- ❌ No seed phrase

File permissions should be `600` (user read/write only).

---

**Status:** Fully integrated with pcli! Same wallet, same database, zero duplication. 🎯
