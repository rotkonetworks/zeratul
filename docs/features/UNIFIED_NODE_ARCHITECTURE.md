# Zeratul Unified Node Architecture

**Last Updated**: 2025-11-13

## Overview

Zeratul uses a **single binary** with multiple operational modes controlled by CLI flags. This design simplifies deployment while supporting different use cases from resource-constrained light clients to archive nodes serving RPC queries.

## Node Modes

```rust
pub enum NodeMode {
    Validator,   // Participate in consensus (deterministic PolkaVM)
    Full,        // Full verification (fast native verification)
    Light,       // Minimal state (succinct proofs only)
    Archive,     // Full history + RPC (stores everything)
}
```

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│ User Client (Off-Chain)                                     │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  1. Build witness (transfer, state transition, etc.)       │
│  2. Generate Ligerito proof                                 │
│     - Uses Reed-Solomon (AccidentalComputer pattern!)      │
│     - Full proof ~1-5 MB                                    │
│  3. Extract succinct proof (~255 KB)                        │
│  4. Broadcast succinct proof to network                     │
│  5. Keep full proof available for archive nodes             │
│                                                             │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ↓ Succinct proof (~255 KB)
┌─────────────────────────────────────────────────────────────┐
│ Network (Gossip Layer)                                      │
├─────────────────────────────────────────────────────────────┤
│  - Propagate succinct proofs only                           │
│  - Efficient bandwidth usage                                │
│  - All nodes receive same proof data                        │
└─────────────────────┬───────────────────────────────────────┘
                      │
        ┌─────────────┼─────────────┬──────────────┐
        ↓             ↓             ↓              ↓
┌──────────────┐ ┌──────────┐ ┌───────────┐ ┌────────────┐
│ Validator    │ │ Full     │ │ Light     │ │ Archive    │
│ Mode         │ │ Mode     │ │ Mode      │ │ Mode       │
└──────────────┘ └──────────┘ └───────────┘ └────────────┘
```

## Mode Details

### 1. Validator Mode

**Purpose**: Participate in consensus, propose blocks, sign with FROST

**Verification Method**: PolkaVM (REQUIRED for determinism)

**Why PolkaVM**: All validators MUST agree on proof validity. PolkaVM guarantees deterministic execution across different hardware/OS.

**Usage**:
```bash
zeratul-node --mode=validator \
  --frost-key-share=/path/to/keyshare \
  --polkavm-verifier=/path/to/verifier.polkavm
```

**Characteristics**:
- ✅ Deterministic consensus
- ✅ FROST threshold signing
- ⚠️ Slower verification (~20-30ms via PolkaVM)
- ❌ Must maintain full state
- ❌ Higher resource requirements

### 2. Full Node Mode

**Purpose**: Verify all transactions, maintain full state, don't participate in consensus

**Verification Method**: Native Ligerito verification (FAST)

**Why Native**: Don't need determinism, just correctness. Native code is 2-4x faster than PolkaVM.

**Usage**:
```bash
zeratul-node --mode=full \
  --rpc-port=9933
```

**Characteristics**:
- ✅ Fast verification (~5-10ms native)
- ✅ Full state for queries
- ✅ Can serve as RPC endpoint
- ❌ Doesn't participate in consensus
- ❌ Still requires significant resources

### 3. Light Client Mode

**Purpose**: Minimal verification, bandwidth-constrained environments

**Verification Method**: PolkaVM or native (configurable)

**Why**: Minimal resource usage, can run on phones/IoT devices

**Usage**:
```bash
zeratul-node --mode=light \
  --polkavm-verifier=/path/to/verifier.polkavm
```

**Characteristics**:
- ✅ Minimal bandwidth (succinct proofs only)
- ✅ Minimal state (header chain + recent proofs)
- ✅ Can run on constrained devices
- ✅ Still cryptographically secure
- ⚠️ Can't answer arbitrary queries
- ⚠️ Trusts consensus majority

### 4. Archive Mode

**Purpose**: Store complete history, serve RPC queries with full data

**Verification Method**: Native (fast verification + full storage)

**Why**: Enables block explorers, debuggers, auditors to access full proof data

**Usage**:
```bash
zeratul-node --mode=archive \
  --rpc-port=9933 \
  --archive-dir=/mnt/storage/zeratul-archive \
  --enable-full-proofs
```

**Characteristics**:
- ✅ Complete historical data
- ✅ Serves full proofs on demand
- ✅ Can reconstruct witness data
- ✅ Enables debugging/auditing
- ❌ Very high storage requirements (stores full ~MB proofs)
- ❌ Higher bandwidth (must fetch full proofs)

**RPC Endpoints**:
```rust
// Get succinct proof (available on all nodes)
rpc.getProof(txHash) -> LigeritoSuccinctProof (~255 KB)

// Get full proof (archive nodes only)
rpc.getFullProof(txHash) -> FinalizedLigeritoProof (~1-5 MB)

// Get original witness (archive nodes only)
rpc.getWitness(txHash) -> TransferWitness
```

## Implementation

### Main Binary Structure

```rust
// src/main.rs

use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long, value_enum)]
    mode: NodeMode,

    #[arg(long)]
    polkavm_verifier: Option<PathBuf>,

    #[arg(long)]
    frost_key_share: Option<PathBuf>,

    #[arg(long)]
    rpc_port: Option<u16>,

    #[arg(long)]
    archive_dir: Option<PathBuf>,
}

#[derive(Clone, ValueEnum)]
enum NodeMode {
    Validator,
    Full,
    Light,
    Archive,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let node = match args.mode {
        NodeMode::Validator => {
            require_arg(&args.polkavm_verifier, "--polkavm-verifier")?;
            require_arg(&args.frost_key_share, "--frost-key-share")?;

            Node::new_validator(
                args.polkavm_verifier.unwrap(),
                args.frost_key_share.unwrap(),
            )?
        }

        NodeMode::Full => {
            Node::new_full(args.rpc_port)?
        }

        NodeMode::Light => {
            Node::new_light(args.polkavm_verifier)?
        }

        NodeMode::Archive => {
            require_arg(&args.archive_dir, "--archive-dir")?;

            Node::new_archive(
                args.archive_dir.unwrap(),
                args.rpc_port,
            )?
        }
    };

    node.run()
}
```

### Verification Routing

```rust
// src/node.rs

pub struct Node {
    mode: NodeMode,
    verifier: Box<dyn ProofVerifier>,
    state: StateManager,
    network: NetworkManager,
    storage: Option<ArchiveStorage>,
}

impl Node {
    pub fn verify_proof(&self, proof: &LigeritoSuccinctProof) -> Result<bool> {
        match self.mode {
            NodeMode::Validator => {
                // MUST use PolkaVM for consensus determinism
                self.verifier.verify_via_polkavm(proof)
            }

            NodeMode::Full => {
                // Use native verification (faster)
                self.verifier.verify_native(proof)
            }

            NodeMode::Light => {
                // Can use either (configurable)
                if self.config.trust_native {
                    self.verifier.verify_native(proof)
                } else {
                    self.verifier.verify_via_polkavm(proof)
                }
            }

            NodeMode::Archive => {
                // Verify + store full proof
                let result = self.verifier.verify_native(proof)?;

                if result {
                    // Fetch and store full proof for RPC queries
                    if let Some(storage) = &self.storage {
                        let full_proof = self.fetch_full_proof(&proof.commitment)?;
                        storage.store_full_proof(&proof.commitment, &full_proof)?;
                    }
                }

                Ok(result)
            }
        }
    }
}

pub trait ProofVerifier: Send + Sync {
    fn verify_native(&self, proof: &LigeritoSuccinctProof) -> Result<bool>;
    fn verify_via_polkavm(&self, proof: &LigeritoSuccinctProof) -> Result<bool>;
}
```

## Proof Flow

### Proof Generation (User Client)

```rust
// User's wallet/client code
pub fn create_transfer_proof(
    from: &Account,
    to: &Account,
    amount: Amount,
) -> Result<(LigeritoSuccinctProof, FinalizedLigeritoProof)> {
    // 1. Build witness
    let witness = TransferWitness {
        sender_balance_old: from.balance,
        sender_balance_new: from.balance - amount,
        receiver_balance_old: to.balance,
        receiver_balance_new: to.balance + amount,
        amount,
    };

    // 2. Convert to polynomial
    let polynomial = witness.to_polynomial()?;

    // 3. Generate full Ligerito proof
    // (This uses Reed-Solomon internally - AccidentalComputer pattern!)
    let config = ligerito::hardcoded_config_24(...);
    let full_proof = ligerito::prover(&config, &polynomial)?;

    // 4. Extract succinct proof for network broadcast
    let succinct_proof = extract_succinct_proof(&full_proof)?;

    Ok((succinct_proof, full_proof))
}
```

### Network Broadcast

```rust
// Only broadcast succinct proof (~255 KB)
network.broadcast(succinct_proof)?;

// Keep full proof available for archive nodes
rpc_server.register_proof(full_proof.commitment, full_proof)?;
```

### Verification at Nodes

```rust
// All nodes receive succinct proof from network
network.on_proof(|succinct_proof| {
    node.verify_proof(&succinct_proof)?;

    if node.mode == NodeMode::Archive {
        // Fetch full proof from prover's RPC
        let full_proof = fetch_full_proof_from_prover(
            &succinct_proof.prover_rpc_url,
            &succinct_proof.commitment
        )?;

        // Store for future queries
        archive.store(full_proof)?;
    }
});
```

## Benefits of This Design

### 1. Single Binary
- ✅ Easier deployment (one artifact)
- ✅ Simpler testing (same code paths)
- ✅ Configuration via flags (not separate binaries)

### 2. Flexible Deployment
- ✅ Start as light client, upgrade to full node
- ✅ Run multiple modes on same machine (different data dirs)
- ✅ Easy switching between modes

### 3. Efficient Network
- ✅ Succinct proofs for consensus (~255 KB)
- ✅ Full proofs available on-demand (archive nodes)
- ✅ Bandwidth scales with security needs

### 4. Performance Optimization
- ✅ Validators use PolkaVM (deterministic)
- ✅ Full nodes use native (2-4x faster)
- ✅ Light clients minimal resources
- ✅ Archive nodes serve historical data

## Comparison Table

| Feature | Validator | Full | Light | Archive |
|---------|-----------|------|-------|---------|
| Verification | PolkaVM | Native | PolkaVM/Native | Native |
| Speed | ~20-30ms | ~5-10ms | ~20-30ms | ~5-10ms |
| State | Full | Full | Minimal | Full + History |
| Storage | ~GB | ~GB | ~MB | ~TB |
| Consensus | ✅ Yes | ❌ No | ❌ No | ❌ No |
| RPC | Basic | Full | Limited | Historical |
| Full Proofs | ❌ No | ❌ No | ❌ No | ✅ Yes |

## Migration Path

Users can easily migrate between modes:

```bash
# Start as light client (minimal resources)
zeratul-node --mode=light --data-dir=/data

# Upgrade to full node (download full state)
zeratul-node --mode=full --data-dir=/data --sync-from=light

# Become validator (add FROST key)
zeratul-node --mode=validator --data-dir=/data --frost-key-share=/key

# Add archive capability (start storing full proofs)
zeratul-node --mode=archive --data-dir=/data --archive-dir=/archive
```

## Next Steps

1. ✅ Design unified binary architecture
2. ⏳ Implement NodeMode enum and routing
3. ⏳ Build PolkaVM verifier binary
4. ⏳ Implement archive storage layer
5. ⏳ Add RPC endpoints for full proofs
6. ⏳ Test mode switching and migration
