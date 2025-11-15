# pcli vs pclientd - Architecture Comparison

## TL;DR

**pcli** = Command-line wallet (interactive, one-off commands)
**pclientd** = Daemon/server (long-running, serves gRPC API)
**Terminator** = TUI wallet (like pcli but with fancy terminal UI)

## pcli - Command Line Interface

### Purpose
Interactive wallet for humans to manage their Penumbra account.

### Architecture
```
User → pcli command → Execute → Print result → Exit
```

### Key Features
- **Interactive**: Run commands, get results, exit
- **Direct database access**: Opens `pcli-view.sqlite` directly
- **Local custody**: Keys stored locally (SoftKMS, Ledger, etc.)
- **Rich commands**: Query, transactions, staking, governance

### Example Usage
```bash
# Check balance
pcli view balance

# Send transaction
pcli tx send 100penumbra --to penumbra1abc...

# Query DEX
pcli query dex lp list

# Stake
pcli tx delegate 1000penumbra --to penumbravaloper1xyz...
```

### Storage Location
```
~/.local/share/pcli/
├── config.toml           # FVK, gRPC URL, custody config
├── pcli-view.sqlite      # Wallet state database
└── registry.json         # Asset metadata (optional)
```

### Code Structure
```rust
// main.rs
#[tokio::main]
async fn main() {
    let opt = Opt::parse();  // Parse command line

    // Load config & database
    let config = PcliConfig::load(opt.home)?;
    let view = ViewServer::load_or_initialize(...)?;
    let custody = SoftKms::new(...)?;

    // Execute command
    match opt.cmd {
        Command::View(view_cmd) => view_cmd.exec(&view).await?,
        Command::Transaction(tx_cmd) => tx_cmd.exec(&view, &custody).await?,
        Command::Query(query_cmd) => query_cmd.exec(&grpc_client).await?,
    }
}
```

## pclientd - Client Daemon

### Purpose
Long-running gRPC server that other applications can connect to.

### Architecture
```
pclientd (daemon)
    ├─ ViewService (gRPC server)
    ├─ CustodyService (gRPC server)  [optional, custody mode]
    └─ Query Proxies (gRPC → pd)

External apps → gRPC → pclientd → pd (full node)
```

### Key Features
- **Long-running**: Starts once, runs forever
- **gRPC API**: Other apps connect via gRPC
- **Two modes**:
  - **View mode**: Read-only (no spending)
  - **Custody mode**: Can sign transactions
- **Proxy**: Forwards queries to `pd` (full node)

### Example Usage
```bash
# Start in view mode (read-only)
pclientd init --view --grpc-url https://penumbra.rotko.net
pclientd start
# Now listening on 127.0.0.1:8081

# Other app connects:
let client = ViewServiceClient::connect("http://127.0.0.1:8081")?;
let balances = client.balances(request).await?;
```

### Storage Location
```
~/.local/share/pclientd/
├── config.toml           # FVK, bind addr, custody config
└── pclientd-view.sqlite  # Wallet state database (separate from pcli!)
```

### Code Structure
```rust
// lib.rs
pub async fn exec(opt: Opt) -> Result<()> {
    match opt.cmd {
        Command::Init { .. } => {
            // Create config
            let config = PclientdConfig { fvk, grpc_url, bind_addr, .. };
            config.save(opt.home.join("config.toml"))?;
        }
        Command::Start => {
            // Load config
            let config = PclientdConfig::load(opt.home)?;

            // Create ViewServer
            let view = ViewServer::load_or_initialize(...)?;
            let view_service = ViewServiceServer::new(view);

            // Create CustodyServer (if custody mode)
            let custody = if let Some(kms_config) = config.kms_config {
                let kms = SoftKms::new(kms_config)?;
                Some(CustodyServiceServer::new(kms))
            } else {
                None
            };

            // Start gRPC server
            Server::builder()
                .add_service(view_service)
                .add_optional_service(custody)
                .serve(config.bind_addr)
                .await?;
        }
    }
}
```

### Proxy Services

pclientd exposes proxy services that forward to the full node:

```rust
// proxy.rs
impl DexQueryProxy {
    // Forwards to pd's DEX query service
    async fn liquidity_positions(&self, req) -> Result {
        self.pd_client.liquidity_positions(req).await
    }
}

// Similar proxies for:
// - AppQueryProxy
// - StakeQueryProxy
// - GovernanceQueryProxy
// - ShieldedPoolQueryProxy
// - etc.
```

## Comparison Table

| Feature | pcli | pclientd | Terminator (our app!) |
|---------|------|----------|----------------------|
| **Type** | CLI tool | Daemon/Server | TUI application |
| **Lifetime** | One command, then exit | Long-running | Long-running (while UI open) |
| **Interface** | Terminal commands | gRPC API | Terminal UI (mouse/keyboard) |
| **Database** | pcli-view.sqlite | pclientd-view.sqlite | pcli-view.sqlite (shared!) |
| **Custody** | Local (various KMS) | Optional (custody mode) | Local (via ViewService) |
| **Use case** | Interactive wallet | Backend for other apps | Trading terminal |
| **Target user** | Humans | Developers/Apps | Traders |

## Why Two Separate Apps?

### pcli
- **For**: Direct human interaction
- **When**: Quick commands, manual operations
- **Example**: "I want to check my balance real quick"

### pclientd
- **For**: Programmatic access
- **When**: Building applications on top
- **Example**: Web wallet, mobile app, trading bot

### Analogy
```
pcli     = mysql client (command line)
pclientd = mysqld server (daemon that serves queries)
```

## How Terminator Fits In

Terminator is **like pcli but with a TUI** (Terminal User Interface):

```
pcli       = Command-line interface (type commands)
Terminator = TUI interface (mouse + keyboard, panels)
```

### What Terminator Does

**Same as pcli:**
- ✅ Loads from `~/.local/share/pcli/`
- ✅ Uses same `pcli-view.sqlite`
- ✅ Same ViewService
- ✅ Same gRPC connections

**Different from pcli:**
- ❌ Not command-line (TUI with panels)
- ✅ Real-time updates (order book streaming)
- ✅ Mouse control (resize, drag panels)
- ✅ Visual charts (candlesticks, depth bars)
- ✅ Optimized for trading

### Why Not Use pclientd?

We could build Terminator to connect to pclientd:

```
Terminator → pclientd (gRPC) → pd
```

**Pros:**
- Separate concerns
- Could work with remote pclientd

**Cons:**
- Extra complexity
- Extra process to manage
- Latency (gRPC overhead)

**Our choice: Direct like pcli**
```
Terminator → ViewService (in-process) → pd (gRPC)
```

Benefits:
- Simpler
- Faster (no IPC overhead)
- Shares pcli's database directly
- One process to manage

## Code Reuse from pcli

### What We Copy

**1. Config loading**
```rust
// From pcli/src/lib.rs
pub fn default_home() -> Utf8PathBuf {
    ProjectDirs::from("zone", "penumbra", "pcli")
        .expect("...")
        .data_dir()
        .to_path_buf()
}

// We use this in terminator/src/wallet/mod.rs!
```

**2. ViewService initialization**
```rust
// From pcli
let view = ViewServer::load_or_initialize(
    Some(home.join("pcli-view.sqlite")),
    Some(home.join("registry.json")),
    &config.full_viewing_key,
    config.grpc_url,
).await?;

// Same in Terminator!
```

**3. Query patterns**
```rust
// From pcli/src/command/query/dex.rs
let request = LiquidityPositionsRequest { .. };
let mut stream = client.liquidity_positions(request).await?;
while let Some(response) = stream.next().await {
    // Process position
}

// We do this in terminator/src/network/penumbra/grpc_client.rs!
```

### What We Don't Need

**1. Command parsing** (we have TUI instead)
**2. Terminal output formatting** (we use ratatui)
**3. One-shot execution** (we run continuously)

## Architecture Diagrams

### pcli
```
┌─────────┐
│  User   │
└────┬────┘
     │ pcli tx send ...
     ▼
┌─────────────────┐
│      pcli       │
│  (one command)  │
├─────────────────┤
│ ViewService     │ ◄─── ~/.local/share/pcli/pcli-view.sqlite
│ CustodyService  │
└────┬────────────┘
     │ gRPC
     ▼
┌─────────────────┐
│   pd (node)     │
└─────────────────┘
```

### pclientd
```
┌──────────┐  ┌──────────┐  ┌──────────┐
│  Web App │  │ Mobile   │  │  Bot     │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     │ gRPC        │ gRPC        │ gRPC
     └─────────────┴─────────────┘
                   │
                   ▼
          ┌────────────────┐
          │   pclientd     │
          │   (daemon)     │
          ├────────────────┤
          │ ViewService    │ ◄─── ~/.local/share/pclientd/pclientd-view.sqlite
          │ CustodyService │
          │ Query Proxies  │
          └────┬───────────┘
               │ gRPC
               ▼
          ┌────────────────┐
          │   pd (node)    │
          └────────────────┘
```

### Terminator
```
┌─────────────────┐
│   Terminal UI   │
│  (ratatui TUI)  │
├─────────────────┤
│   Order Book    │
│   Chart         │
│   Positions     │
└────┬────────────┘
     │ in-process
     ▼
┌─────────────────┐
│  ViewService    │ ◄─── ~/.local/share/pcli/pcli-view.sqlite
│  (in-process)   │      (SHARED with pcli!)
└────┬────────────┘
     │ gRPC
     ▼
┌─────────────────┐
│  pd / Penumbra  │
│     Network     │
└─────────────────┘
```

## Key Insight: Database Sharing

**Critical point**: Terminator shares pcli's database!

```bash
# pcli writes to database
pcli tx send 100penumbra --to penumbra1abc...
# Updates: ~/.local/share/pcli/pcli-view.sqlite

# Terminator sees it immediately!
# Both read from same file!
```

This is **much simpler** than:
- Running separate pclientd daemon
- Maintaining separate database
- Syncing state between processes

## When to Use Each

### Use pcli when:
- Quick balance check
- Manual transaction
- Administrative tasks
- Learning/exploring

### Use pclientd when:
- Building web app
- Building mobile app
- Need remote access
- Multiple clients

### Use Terminator when:
- Active trading
- Monitoring markets
- Real-time order book
- LP management

## Summary

**pcli** = Interactive CLI wallet (run command → see result → exit)
**pclientd** = gRPC daemon (serve API for other apps)
**Terminator** = TUI trading terminal (like pcli but with real-time UI)

All three use the same underlying components:
- ViewService (wallet state)
- CustodyService (key management)
- gRPC clients (talk to pd/network)

Terminator's approach:
- ✅ Share pcli's config & database
- ✅ Use ViewService directly (in-process)
- ✅ Build TUI on top (ratatui)
- ✅ Add real-time streaming
- ✅ Optimize for trading

We're building the **trading-optimized frontend** using Penumbra's battle-tested backend! 🎯
