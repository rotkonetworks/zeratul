# Zeratul Architecture

## Crate Structure

### Core Principle
**Copy Penumbra's battle-proven code, make it faster with PolkaVM**

```
zeratul/
├── crates/
│   ├── zeratul-blockchain/          # ← All blockchain logic
│   │   └── src/
│   │       ├── penumbra/           # ← Copy Penumbra code here!
│   │       │   ├── dex/
│   │       │   │   ├── batch_swap.rs          # Batch auction (MEV-proof!)
│   │       │   │   ├── swap_execution.rs      # Pro-rata distribution
│   │       │   │   ├── liquidity_position.rs  # Concentrated liquidity
│   │       │   │   ├── trading_pair.rs
│   │       │   │   └── swap_claim.rs
│   │       │   ├── stake/
│   │       │   │   ├── delegation.rs          # delZT(v) tokens
│   │       │   │   ├── validator.rs
│   │       │   │   ├── undelegation.rs
│   │       │   │   ├── slashing.rs            # Penumbra slashing
│   │       │   │   └── rewards.rs
│   │       │   ├── governance/
│   │       │   │   ├── proposal.rs
│   │       │   │   ├── vote.rs
│   │       │   │   └── tally.rs
│   │       │   ├── shielded_pool/
│   │       │   │   ├── note.rs
│   │       │   │   ├── nullifier.rs
│   │       │   │   └── action.rs
│   │       │   └── ibc/                       # Already exists!
│   │       ├── consensus/
│   │       │   ├── bft.rs             # Stake-weighted BFT
│   │       │   ├── block.rs
│   │       │   └── finality.rs
│   │       ├── execution/
│   │       │   ├── pvm_runtime.rs     # PolkaVM execution (our improvement!)
│   │       │   ├── proof_generation.rs
│   │       │   └── verification.rs
│   │       ├── economics/
│   │       │   ├── target_staking.rs  # Our improvement: target ratio
│   │       │   ├── inflation.rs
│   │       │   └── fee_pool.rs
│   │       └── lib.rs
│   │
│   ├── zeratul-p2p/                 # ← ONLY networking!
│   │   └── src/
│   │       ├── transport/
│   │       │   ├── quic.rs          # QUIC transport
│   │       │   ├── connection.rs
│   │       │   └── stream.rs
│   │       ├── gossip/
│   │       │   ├── pubsub.rs        # Gossipsub protocol
│   │       │   ├── topic.rs
│   │       │   └── message.rs
│   │       ├── discovery/
│   │       │   ├── peer_discovery.rs
│   │       │   ├── dht.rs
│   │       │   └── bootstrap.rs
│   │       ├── sync/
│   │       │   ├── block_sync.rs
│   │       │   └── state_sync.rs
│   │       └── lib.rs
│   │
│   ├── ligerito/                    # ZK proof system
│   ├── polkavm-pcvm/               # PolkaVM integration
│   └── ...
```

## What Goes Where

### `zeratul-blockchain` (Business Logic)
- ✅ DEX (batch swaps, liquidity, trading)
- ✅ Staking (delegation, validators, rewards)
- ✅ Governance (proposals, voting)
- ✅ Shielded pool (notes, commitments, nullifiers)
- ✅ Consensus rules (BFT, finality)
- ✅ Economics (inflation, fees)
- ✅ Execution (PolkaVM runtime, proofs)

### `zeratul-p2p` (Networking Only)
- ✅ QUIC transport layer
- ✅ Gossipsub message broadcasting
- ✅ Peer discovery & DHT
- ✅ Block/state synchronization
- ✅ Connection management
- ❌ NO business logic!
- ❌ NO DEX/staking/consensus code!

## Copy Strategy from Penumbra

### Step 1: Direct Copy
Copy these from Penumbra as-is:

```bash
# DEX components
cp -r penumbra/crates/core/component/dex/src/* \
      zeratul/crates/zeratul-blockchain/src/penumbra/dex/

# Staking components
cp -r penumbra/crates/core/component/stake/src/* \
      zeratul/crates/zeratul-blockchain/src/penumbra/stake/

# Shielded pool
cp -r penumbra/crates/core/component/shielded-pool/src/* \
      zeratul/crates/zeratul-blockchain/src/penumbra/shielded_pool/

# Governance
cp -r penumbra/crates/core/component/governance/src/* \
      zeratul/crates/zeratul-blockchain/src/penumbra/governance/
```

### Step 2: Replace Execution Layer
Keep Penumbra's logic, swap execution backend:

```rust
// Penumbra uses CosmWasm
impl SwapExecution {
    fn execute_batch(delta_1, delta_2) -> (lambda_1, lambda_2) {
        // route_and_fill() logic stays same!
        // Just execute in PolkaVM instead of CosmWasm
        polkavm::execute(batch_swap_program, inputs)
    }
}
```

### Step 3: Add Our Improvements
- Target staking ratio (new!)
- Superlinear slashing (Polkadot formula)
- Faster proof generation (Ligerito)

## Dependency Flow

```
┌─────────────────────┐
│   zeratul-client    │  (User interface)
└──────────┬──────────┘
           │
┌──────────▼──────────┐
│ zeratul-blockchain  │  (Business logic - copied from Penumbra!)
│                     │
│ ├─ penumbra/dex/    │  ← Direct copy
│ ├─ penumbra/stake/  │  ← Direct copy
│ ├─ execution/pvm    │  ← Our improvement (PolkaVM)
│ └─ economics/target │  ← Our improvement
└──────────┬──────────┘
           │
┌──────────▼──────────┐
│   zeratul-p2p       │  (Networking only!)
│                     │
│ ├─ quic transport   │
│ ├─ gossipsub        │
│ └─ peer discovery   │
└──────────┬──────────┘
           │
           ▼
      Network Layer
```

## What to Copy First

### Priority 1: Core DEX (Week 1)
```
penumbra/crates/core/component/dex/src/
├── batch_swap_output_data.rs  ← Copy this!
├── swap_execution.rs           ← Copy this!
├── trading_pair.rs             ← Copy this!
├── component/
│   ├── flow.rs                 ← SwapFlow aggregation
│   ├── router/
│   │   ├── route_and_fill.rs   ← Battle-tested routing
│   │   ├── fill_route.rs
│   │   └── path_search.rs
│   └── position_manager/       ← Liquidity positions
```

### Priority 2: Staking (Week 2)
```
penumbra/crates/core/component/stake/src/
├── validator.rs                ← Validator management
├── delegation.rs               ← delZT tokens
├── undelegation.rs             ← Exchange rate logic
├── uptime.rs                   ← Slashing triggers
└── component/
    ├── validator_handler/
    └── delegation_manager/
```

### Priority 3: Shielded Pool (Week 3)
```
penumbra/crates/core/component/shielded-pool/src/
├── note.rs                     ← Note structure
├── spend.rs                    ← Spend proofs
├── output.rs                   ← Output proofs
└── nullifier.rs                ← Nullifier tracking
```

## Testing Strategy

### Use Penumbra's Tests!
```rust
// Copy their test suite
cp -r penumbra/crates/core/component/dex/src/component/tests/* \
      zeratul/crates/zeratul-blockchain/tests/penumbra_dex/

// Run to verify we copied correctly
cargo test --package zeratul-blockchain
```

### Add Performance Tests
```rust
#[test]
fn test_batch_swap_performance() {
    // Penumbra: ~2s proof generation
    // Zeratul: ~400ms (10x faster!)
    let batch = create_batch_with_1000_swaps();
    let start = Instant::now();
    let proof = execute_batch_pvm(batch);
    assert!(start.elapsed() < Duration::from_millis(500));
}
```

## Migration Plan

### Current State (Bad!)
```
zeratul-p2p/src/
├── zswap.rs         ← DEX logic (wrong crate!)
├── delegation.rs    ← Staking logic (wrong crate!)
├── bft.rs           ← Consensus (wrong crate!)
└── gossip.rs        ← Networking (correct!)
```

### Target State (Good!)
```
zeratul-blockchain/src/penumbra/
├── dex/             ← Copied from Penumbra
├── stake/           ← Copied from Penumbra
└── governance/      ← Copied from Penumbra

zeratul-p2p/src/
├── quic.rs          ← ONLY networking
├── gossip.rs        ← ONLY networking
└── discovery.rs     ← ONLY networking
```

## Next Steps

1. **Clean up `zeratul-p2p`**
   - Move all blockchain logic to `zeratul-blockchain`
   - Keep only networking code

2. **Copy Penumbra components**
   - Start with DEX (batch_swap_output_data, swap_execution)
   - Then staking (delegation tokens)
   - Then shielded pool

3. **Replace execution backend**
   - Keep Penumbra's routing/aggregation logic
   - Execute in PolkaVM instead of CosmWasm
   - Use Ligerito for proofs instead of Groth16

4. **Add our improvements**
   - Target staking ratio
   - Superlinear slashing
   - QUIC P2P instead of Tendermint

## Philosophy

**Don't reinvent the wheel!**

Penumbra has:
- 3+ years of development
- Battle-tested in production
- Audited by cryptography experts
- Proven MEV resistance
- Working privacy

We take that and make it **10-100x faster** with PolkaVM.

That's our value proposition:
- ✅ Same security (copied from Penumbra)
- ✅ Same MEV resistance (copied from Penumbra)
- ✅ Same privacy (copied from Penumbra)
- ⚡ 10-100x faster execution (our PolkaVM improvement)
- ⚡ Lower latency (our QUIC P2P improvement)
