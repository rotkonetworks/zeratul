# Zeratul Architecture: AccidentalComputer + PolkaVM

**Date**: 2025-11-12

## TL;DR

We use **THREE separate technologies**, each for a different purpose:

1. **AccidentalComputer (ZODA)** - For full nodes to re-execute transactions with ZK proofs
2. **PolkaVM** - For light clients to verify Ligerito proofs without re-executing
3. **Polkadot SDK (Bandersnatch)** - For consensus (Safrole block production, Ring VRF)

## The Three-Tier Network

```
┌─────────────────────────────────────────────────────────────────┐
│  LIGHT CLIENTS (browser, mobile, embedded)                      │
│  - Only download block headers                                  │
│  - Verify Ligerito proofs using PolkaVM                         │
│  - Never execute transactions                                   │
│  Technology: PolkaVM runtime (RISC-V sandbox)                   │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│  FULL NODES (servers)                                           │
│  - Download blocks + witness data                               │
│  - Re-execute ALL transactions                                  │
│  - Verify using AccidentalComputer (ZODA encoding as ZK proof)  │
│  - Store full state                                             │
│  Technology: AccidentalComputer (ZODA reused as polynomial comm)│
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│  VALIDATORS (consensus nodes)                                   │
│  - Execute transactions                                         │
│  - Generate Ligerito proofs (polynomial commitment scheme)      │
│  - Produce blocks using Safrole consensus                       │
│  - Create Ring VRF tickets (anonymous block production)         │
│  Technology: Polkadot SDK (Bandersnatch, Safrole)              │
└─────────────────────────────────────────────────────────────────┘
```

## 1. AccidentalComputer (ZODA) - Full Node Verification

### What It Is

From the Ligerito paper (Section 5): **"Accidental Computer" pattern**

The key insight: ZODA's Reed-Solomon encoding (used for data availability) can be **reused as polynomial commitments** for zero-knowledge proofs.

### How It Works

#### Step 1: ZODA Encoding (Already Done for DA)
```
Data X̃ → Encode to matrix Y = GX̃G'ᵀ → Merkle tree commitment
```

#### Step 2: Accidental Computer (Reuse for ZK)
```
The ZODA encoding IS our polynomial commitment!
We can prove statements about X̃ using the same encoding.
No need to re-encode for ZK proofs!
```

### Why We Use It

**Benefits**:
- ✅ **Zero encoding overhead** - DA encoding doubles as ZK commitment
- ✅ **Smaller proofs** - Reuse DA commitments instead of separate PCS
- ✅ **Faster proving** - Skip the expensive encoding step
- ✅ **Full node efficiency** - Nodes that already have ZODA data can verify for free

**Use case**: Full nodes that re-execute transactions

### Where It Lives

**Circuit module**: `examples/state_transition_zkvm/circuit/`
- `src/accidental_computer.rs` - Implementation
- Used by full nodes to verify state transitions
- Reuses ZODA commitments from commonware

## 2. PolkaVM - Light Client Verification

### What It Is

**PolkaVM**: Polkadot's RISC-V virtual machine (deterministic sandbox)

Used to run **Ligerito verifier** in a sandboxed environment for light clients.

### Why We Use It

**Benefits**:
- ✅ **Light clients** - No need to re-execute transactions
- ✅ **Browser/mobile** - Runs anywhere (WASM, native, embedded)
- ✅ **Deterministic** - Same results on all platforms
- ✅ **Sandboxed** - Safe execution (can't escape VM)
- ✅ **Fast** - Verification ~20-30ms for 2^20 polynomial

**Use case**: Resource-constrained devices (browsers, mobile, IoT)

### Where It Lives

**PolkaVM integration**: `examples/polkavm_verifier/` + `examples/polkavm_service/`

## 3. Polkadot SDK (Bandersnatch) - Consensus

### What It Is

**Bandersnatch Ring VRF** from Polkadot SDK (`sp-core`)

Used for **Safrole consensus** (JAM-style block production):
- Anonymous ticket submission (Ring VRF)
- Outside-in sequencer
- Epoch-based validator rotation

### Why We Use It

**Benefits**:
- ✅ **Privacy** - Anonymous ticket submission (Ring VRF)
- ✅ **Fairness** - Outside-in sequencer prevents gaming
- ✅ **Security** - Based on discrete log hardness
- ✅ **Battle-tested** - Same crypto as Polkadot SASSAFRAS/BABE

**Use case**: Consensus layer (block production, validator selection)

### Where It Lives

**Consensus module**: `examples/state_transition_zkvm/blockchain/src/consensus/`
- `safrole.rs` - JAM-style block production
- `tickets.rs` - Ring VRF ticket system
- `entropy.rs` - VRF entropy accumulation

## Summary Table

| Technology | Purpose | Where | Who Uses It |
|-----------|---------|-------|-------------|
| **AccidentalComputer (ZODA)** | Full node verification | `circuit/accidental_computer.rs` | Full nodes (re-execute txs) |
| **PolkaVM** | Light client verification | `polkavm_verifier/` + `polkavm_service/` | Light clients (verify proofs) |
| **Polkadot SDK (Bandersnatch)** | Consensus (Ring VRF) | `blockchain/src/consensus/` | Validators (produce blocks) |

## Common Questions

### "Do we use AccidentalComputer OR PolkaVM?"

**Answer: We use BOTH, for different purposes:**

- **AccidentalComputer** = Full nodes verifying by re-executing (reuse ZODA encoding)
- **PolkaVM** = Light clients verifying without re-executing (run Ligerito verifier in VM)

They're **complementary**, not competing.

### "Why not use PolkaVM for everything?"

Because full nodes **already have ZODA data** from commonware's data availability layer. Using AccidentalComputer means they can verify "for free" by reusing that encoding as a polynomial commitment.

Light clients don't have ZODA data (too big), so they use PolkaVM to verify succinct Ligerito proofs.

### "Why not use AccidentalComputer for light clients?"

Because AccidentalComputer requires:
1. Full ZODA encoding (large, not succinct)
2. Re-executing transactions (compute-heavy)
3. Access to witness data

Light clients can't do any of these (resource-constrained). They need **succinct proofs**, which is what Ligerito provides, verified via PolkaVM.
