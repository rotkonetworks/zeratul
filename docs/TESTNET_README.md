# Zeratul Local Testnet

Local 4-validator testnet for development and testing.

## Quick Start

```bash
# Start testnet
./scripts/testnet-local.sh start

# Check status
./scripts/testnet-local.sh status

# View logs
./scripts/testnet-local.sh logs 0

# Run tests
./scripts/testnet-local.sh test

# Stop testnet
./scripts/testnet-local.sh stop
```

## Architecture

### Validators

- **Count:** 4 validators
- **Threshold:** 3 (2f+1 Byzantine fault tolerance)
- **Ports:** 9000-9003 (localhost)
- **Data:** `./testnet-local/validator-{0-3}/`

### Components

**DKG (Distributed Key Generation):**
- FROST 3-round interactive DKG
- Generates threshold keys for epoch
- decaf377 curve (Penumbra-compatible)

**Privacy (3-Tier System):**
- Tier 1: MPC-ZODA (~10ms) - Simple operations
- Tier 2: PolkaVM-ZODA (~160ms) - Smart contracts
- Tier 3: Ligerito (~113ms) - Arbitrary proofs

**Consensus:** (TODO)
- Safrole ticket-based selection
- GRANDPA finality

**Network:** (TODO)
- litep2p with TCP transport
- P2P message broadcasting

## Commands

### Start Testnet

```bash
./scripts/testnet-local.sh start
```

Builds binaries, sets up configs, and starts 4 validators.

### Check Status

```bash
./scripts/testnet-local.sh status
```

Shows validator status:
```
Validators:
  ✓ Validator 0 (PID 12345, port 9000)
  ✓ Validator 1 (PID 12346, port 9001)
  ✓ Validator 2 (PID 12347, port 9002)
  ✓ Validator 3 (PID 12348, port 9003)
```

### View Logs

```bash
# Tail logs for validator 0
./scripts/testnet-local.sh logs 0

# Tail logs for validator 1
./scripts/testnet-local.sh logs 1
```

### Run Tests

```bash
./scripts/testnet-local.sh test
```

Runs test suite:
- Privacy tiers test
- MPC transfer test
- PolkaVM reconstruction test
- FROST DKG test

### Stop Testnet

```bash
./scripts/testnet-local.sh stop
```

Gracefully stops all validators.

### Restart Testnet

```bash
./scripts/testnet-local.sh restart
```

Stops and starts testnet (fresh state).

### Clean Up

```bash
./scripts/testnet-local.sh cleanup
```

Removes all testnet data and logs.

## Configuration

### Validator Config

Each validator has a `config.yaml`:

```yaml
validator:
  index: 0
  name: "validator-0"

network:
  listen_addr: "127.0.0.1:9000"
  peers:
    - "127.0.0.1:9001"
    - "127.0.0.1:9002"
    - "127.0.0.1:9003"

dkg:
  validator_count: 4
  threshold: 3
  epoch: 0

storage:
  path: "./testnet-local/validator-0/data"

logging:
  level: "info"
  file: "./testnet-local/validator-0/validator.log"
```

### Network Topology

```
┌─────────────┐     ┌─────────────┐
│ Validator 0 │────▶│ Validator 1 │
│   :9000     │◀────│   :9001     │
└─────────────┘     └─────────────┘
       │  ▲               │  ▲
       │  │               │  │
       ▼  │               ▼  │
┌─────────────┐     ┌─────────────┐
│ Validator 2 │────▶│ Validator 3 │
│   :9002     │◀────│   :9003     │
└─────────────┘     └─────────────┘
```

All validators connect to all others (full mesh).

## File Structure

```
testnet-local/
├── validator-0/
│   ├── config.yaml       # Validator configuration
│   ├── validator.log     # Runtime logs
│   ├── validator.pid     # Process ID
│   └── data/             # Blockchain state
├── validator-1/
│   └── ...
├── validator-2/
│   └── ...
└── validator-3/
    └── ...
```

## Development Workflow

### 1. Start Testnet

```bash
./scripts/testnet-local.sh start
```

### 2. Monitor Logs

In separate terminals:

```bash
# Terminal 1
./scripts/testnet-local.sh logs 0

# Terminal 2
./scripts/testnet-local.sh logs 1
```

### 3. Run Tests

```bash
./scripts/testnet-local.sh test
```

### 4. Make Changes

Edit code, then restart:

```bash
./scripts/testnet-local.sh restart
```

### 5. Stop When Done

```bash
./scripts/testnet-local.sh stop
```

## Current Status

### ✅ Implemented

- FROST DKG (3 rounds)
- MPC-ZODA privacy layer
- PolkaVM-ZODA verification
- Ligerito proof system
- Hybrid privacy router
- Validator binary with config loading

### 🔄 In Progress

- Network message routing
- DKG ceremony completion (rounds 2-3 over network)
- Peer discovery

### 📋 TODO

- Consensus (Safrole + GRANDPA)
- Block production
- Transaction pool
- State sync
- RPC API

## Troubleshooting

### Validators Won't Start

```bash
# Check if already running
./scripts/testnet-local.sh status

# Clean up and restart
./scripts/testnet-local.sh cleanup
./scripts/testnet-local.sh start
```

### Port Already in Use

Edit `scripts/testnet-local.sh`:

```bash
BASE_PORT=9000  # Change to different base port
```

### Build Errors

```bash
# Clean build
cargo clean
./scripts/testnet-local.sh start
```

### Missing Dependencies

```bash
# Install required deps
cargo fetch
```

## Performance Notes

### Build Flags

The script uses optimized build flags:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

This enables:
- AVX2 SIMD instructions
- Native CPU optimizations
- ~44x faster Ligerito proofs!

### Resource Usage

Per validator:
- Memory: ~100MB
- CPU: <5% idle
- Disk: ~10MB

Total testnet: ~400MB RAM, minimal CPU when idle.

## Next Steps

1. **Wire up network layer** - Complete litep2p integration
2. **Finish DKG ceremony** - Broadcast rounds 2-3
3. **Add consensus** - Safrole ticket generation
4. **Test privacy txs** - End-to-end privacy transactions

## Contributing

When testing changes:

1. Stop testnet
2. Make changes
3. Restart testnet
4. Check logs for errors
5. Run test suite

Keep logs clean with lowercase commit messages (no llm-slop!).

## References

- **FROST DKG:** `crates/zeratul-blockchain/src/dkg/frost_provider.rs`
- **Privacy:** `crates/zeratul-blockchain/src/privacy/`
- **Validator:** `crates/zeratul-blockchain/src/bin/validator.rs`
- **Tests:** `crates/zeratul-blockchain/examples/test_*.rs`
