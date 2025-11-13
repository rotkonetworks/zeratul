# Data Recovery in Zeratul

**Last Updated**: 2025-11-13

## Overview

This document explains what data can be recovered from different proof types in Zeratul, and how archive nodes can provide historical transaction data for block explorers, debuggers, and auditors.

## TL;DR - What's Recoverable?

| Storage Type | Size/TX | Original Witness Recoverable? | How? |
|--------------|---------|------------------------------|------|
| **ZODA Shards** | ~1-5 MB | ✅ **YES** | `Zoda::recover()` |
| **Ligerito Full Proof** | ~1-5 MB | ❌ **NO** | Only sampled rows, not full matrix |
| **Ligerito Succinct Proof** | ~255 KB | ❌ **NO** | Only commitments and evaluations |
| **Witness + Succinct Proof** | ~255 KB | ✅ **YES** | Stored separately |

**Recommended for Archive Nodes**: Store witness + succinct proof separately (smallest storage, full recovery)

## Understanding the Problem

When a user creates a transaction proof, they start with:

```rust
pub struct TransferInstance {
    // Sender's old state
    pub sender_old: AccountState,      // id, balance, nonce, salt

    // Receiver's old state
    pub receiver_old: AccountState,    // id, balance, nonce, salt

    // Transfer details
    pub amount: u64,
    pub sender_salt_new: [u8; 32],
    pub receiver_salt_new: [u8; 32],

    // Commitments (hashes of states)
    pub sender_commitment_old: [u8; 32],
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}
```

**Total**: ~232 bytes of witness data

**Question**: After generating a proof, can we recover this original witness data?

**Answer**: It depends on what proof type was stored!

## Proof Types and Recovery Capabilities

### Option 1: ZODA Shards (AccidentalComputer Pattern)

#### What's Stored

```rust
pub struct AccidentalComputerProof {
    pub zoda_commitment: Vec<u8>,        // Merkle root
    pub shard_indices: Vec<u16>,         // Which shards
    pub shards: Vec<Vec<u8>>,            // Reed-Solomon encoded shards
    pub sender_commitment_old: [u8; 32], // Public inputs
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}
```

**Storage Size**: ~1-5 MB per transaction

#### Recovery Process

```rust
// Step 1: Fetch ZODA proof from archive
let proof: AccidentalComputerProof = archive.get_zoda_proof(tx_hash)?;

// Step 2: Configure ZODA recovery
let coding_config = CodingConfig {
    minimum_shards: 3,
    extra_shards: 2,
};

// Step 3: Recover original bytes using Reed-Solomon decoding
let recovered_bytes = Zoda::<Sha256>::recover(
    &coding_config,
    &proof.zoda_commitment,
    &proof.shards
)?;

// Step 4: Deserialize back to witness
let witness = deserialize_transfer_instance(&recovered_bytes)?;

// ✅ SUCCESS: Full witness recovered!
assert_eq!(witness.amount, expected_amount);
assert_eq!(witness.sender_old.balance, expected_balance);
// ... all fields available!
```

#### Why It Works

**Reed-Solomon erasure coding is lossless:**

```
Original Data (232 bytes)
    ↓ ZODA encoding (Reed-Solomon)
Shards (1-5 MB total, with redundancy)
    ↓ ZODA recovery (Reed-Solomon decoding)
Original Data (232 bytes) ✅ EXACT MATCH
```

The ZODA encoding adds redundancy but preserves **all original information**. Given enough shards (minimum_shards = 3 in our case), we can perfectly reconstruct the original data.

#### Pros and Cons

**Pros:**
- ✅ Complete witness recovery
- ✅ Can reconstruct from partial shards (erasure coding property)
- ✅ Cryptographically verifiable via commitments

**Cons:**
- ❌ Large storage requirement (~1-5 MB per transaction)
- ❌ Bandwidth intensive
- ❌ Recovery computation required (Reed-Solomon decoding)

### Option 2: Ligerito Full Proof

#### What's Stored

```rust
pub struct FinalizedLigeritoProof<T, U> {
    pub initial_ligero_cm: RecursiveLigeroCommitment,  // Merkle root
    pub initial_ligero_proof: RecursiveLigeroProof<T>, // Sampled rows
    pub recursive_commitments: Vec<RecursiveLigeroCommitment>,
    pub recursive_proofs: Vec<RecursiveLigeroProof<U>>,
    pub final_ligero_proof: FinalLigeroProof<U>,
    pub sumcheck_transcript: SumcheckTranscript<U>,
}

// What's in the proofs?
pub struct RecursiveLigeroProof<T> {
    pub opened_rows: Vec<Vec<T>>,     // ⚠️ ONLY sampled rows!
    pub merkle_proof: BatchedMerkleProof,
}
```

**Storage Size**: ~1-5 MB per transaction

#### Why Recovery FAILS

```
Original Polynomial (2^24 coefficients = witness data)
    ↓ Reed-Solomon encoding
Full Matrix (2^24 rows × 2^k columns)
    ↓ Ligerito prover samples ~148 random rows
Proof contains ONLY 148 rows ❌

Cannot reconstruct 2^24 coefficients from 148 rows!
```

**The problem**: Ligerito proofs only contain the **randomly sampled rows** used for verification, not the full encoded matrix. This is by design - it's what makes the proofs succinct!

**Example with numbers:**
- Original polynomial: 2^24 = 16,777,216 coefficients
- Ligerito samples: ~148 rows
- **Cannot** reconstruct 16M values from 148 samples!

#### Attempted Recovery

```rust
// ❌ This CANNOT work!
let proof: FinalizedLigeritoProof = archive.get_ligerito_proof(tx_hash)?;

// We have access to:
// - proof.initial_ligero_proof.opened_rows (~148 rows)
// - proof.recursive_proofs (more sampled rows)
// - Merkle roots (commitments)

// But we CANNOT reconstruct the original polynomial!
// We would need the FULL encoded matrix, not just sampled rows

return Err("Cannot recover witness from Ligerito proof - insufficient data");
```

#### Pros and Cons

**Pros:**
- ✅ Can verify proof correctness
- ✅ Provides cryptographic guarantees

**Cons:**
- ❌ **CANNOT recover witness data**
- ❌ Still large storage (~1-5 MB)
- ❌ Only contains sampled rows, not full data

### Option 3: Ligerito Succinct Proof

#### What's Stored

```rust
pub struct LigeritoSuccinctProof {
    pub proof_bytes: Vec<u8>,  // Serialized FinalizedLigeritoProof
    pub config_size: u32,
}
```

**Storage Size**: ~255 KB per transaction

#### Why Recovery FAILS (Even Worse!)

The succinct proof is even smaller than the full Ligerito proof - it contains:
- Commitments (Merkle roots)
- Sampled rows (even fewer than full proof!)
- Sumcheck polynomials

```rust
// ❌ This DEFINITELY cannot work!
let proof: LigeritoSuccinctProof = archive.get_succinct_proof(tx_hash)?;

// Even less data than the full Ligerito proof
// Absolutely cannot reconstruct witness

return Err("Succinct proofs only prove validity, not data availability");
```

#### Pros and Cons

**Pros:**
- ✅ Small storage (~255 KB)
- ✅ Fast verification
- ✅ Efficient network propagation

**Cons:**
- ❌ **CANNOT recover witness data**
- ❌ No data availability guarantees
- ❌ Only proves validity, not content

### Option 4: Witness + Succinct Proof (Recommended)

#### What's Stored

```rust
pub struct ArchiveEntry {
    pub witness: TransferInstance,        // Original data (~232 bytes)
    pub proof: LigeritoSuccinctProof,     // Verification proof (~255 KB)
    pub metadata: TransactionMetadata,    // Block height, timestamp, etc.
}
```

**Storage Size**: ~232 bytes + ~255 KB ≈ **255 KB per transaction**

**This is SMALLER than ZODA shards (~1-5 MB) and provides the same recovery capability!**

#### Storage and Recovery

```rust
impl ArchiveNode {
    /// Store a transaction with its witness and proof
    pub fn store_transaction(
        &self,
        witness: TransferInstance,
        proof: LigeritoSuccinctProof,
        metadata: TransactionMetadata,
    ) -> Result<()> {
        let tx_hash = self.compute_tx_hash(&witness, &proof)?;

        let entry = ArchiveEntry {
            witness,
            proof,
            metadata,
        };

        // Store in database (indexed by tx_hash)
        self.db.put(&tx_hash, &entry)?;

        Ok(())
    }

    /// Recover witness from archive (instant!)
    pub fn get_witness(&self, tx_hash: &Hash) -> Result<TransferInstance> {
        let entry = self.db.get(tx_hash)?;

        // ✅ Direct access to witness!
        Ok(entry.witness)
    }

    /// Verify that stored proof matches witness
    pub fn verify_stored_transaction(&self, tx_hash: &Hash) -> Result<bool> {
        let entry = self.db.get(tx_hash)?;

        // Can verify proof was generated from this witness
        let polynomial = witness_to_polynomial(&entry.witness)?;
        verify_ligerito_proof(&entry.proof, &polynomial)
    }

    /// RPC: Get transaction details (what block explorers need)
    pub fn get_transaction_details(&self, tx_hash: &Hash) -> Result<TransactionDetails> {
        let entry = self.db.get(tx_hash)?;

        Ok(TransactionDetails {
            from: entry.witness.sender_old.id,
            to: entry.witness.receiver_old.id,
            amount: entry.witness.amount,
            sender_balance_before: entry.witness.sender_old.balance,
            sender_balance_after: entry.witness.sender_old.balance - entry.witness.amount,
            receiver_balance_before: entry.witness.receiver_old.balance,
            receiver_balance_after: entry.witness.receiver_old.balance + entry.witness.amount,
            block_height: entry.metadata.block_height,
            timestamp: entry.metadata.timestamp,
            // ... all data available!
        })
    }

    /// RPC: Get account history
    pub fn get_account_history(&self, account_id: u64) -> Result<Vec<TransactionSummary>> {
        let mut history = Vec::new();

        // Scan all transactions
        for (tx_hash, entry) in self.db.iter() {
            // Check if this transaction involves the account
            if entry.witness.sender_old.id == account_id
                || entry.witness.receiver_old.id == account_id
            {
                history.push(TransactionSummary {
                    tx_hash,
                    amount: entry.witness.amount,
                    direction: if entry.witness.sender_old.id == account_id {
                        Direction::Outgoing
                    } else {
                        Direction::Incoming
                    },
                    timestamp: entry.metadata.timestamp,
                });
            }
        }

        Ok(history)
    }
}
```

#### Pros and Cons

**Pros:**
- ✅ **Full witness recovery** (stored directly)
- ✅ **Smallest storage** (~255 KB vs ~1-5 MB for ZODA)
- ✅ **Instant recovery** (no Reed-Solomon decoding needed)
- ✅ Can verify proof matches witness
- ✅ Easy to query and index

**Cons:**
- ⚠️ Requires prover to send witness to archive nodes
- ⚠️ Archive nodes must be available when transaction is submitted

## Comparison Table

| Feature | ZODA Shards | Ligerito Full | Succinct Only | Witness + Succinct |
|---------|-------------|---------------|---------------|-------------------|
| **Storage per TX** | ~1-5 MB | ~1-5 MB | ~255 KB | ~255 KB |
| **Witness Recovery** | ✅ Yes | ❌ No | ❌ No | ✅ Yes |
| **Recovery Method** | RS decode | Impossible | Impossible | Direct read |
| **Recovery Speed** | Slow | N/A | N/A | Instant |
| **Bandwidth** | High | High | Low | Low |
| **Partial Reconstruction** | ✅ Yes | ❌ No | ❌ No | ❌ No |
| **Best For** | DA layers | Not recommended | Light clients | Archive nodes |

## Recommended Architecture

### Network Topology

```
┌──────────────────────────────────────────────────────────┐
│ User/Prover                                              │
│ - Generates witness                                      │
│ - Generates Ligerito proof                               │
│ - Extracts succinct proof (~255 KB)                      │
└────────┬─────────────────────────────────────────────────┘
         │
         ├─────────────────┬─────────────────┬──────────────┐
         ↓                 ↓                 ↓              ↓
┌─────────────────┐ ┌─────────────┐ ┌──────────────┐ ┌─────────────┐
│ Validator Node  │ │ Full Node   │ │ Light Client │ │ Archive Node│
│ - Receive:      │ │ - Receive:  │ │ - Receive:   │ │ - Receive:  │
│   Succinct proof│ │   Succinct  │ │   Succinct   │ │   Succinct  │
│ - Verify:       │ │ - Verify:   │ │ - Verify:    │ │   + Witness │
│   PolkaVM       │ │   Native    │ │   PolkaVM    │ │ - Verify:   │
│ - Store:        │ │ - Store:    │ │ - Store:     │ │   Native    │
│   Nothing       │ │   Nothing   │ │   Nothing    │ │ - Store:    │
│                 │ │             │ │              │ │   Both      │
│ ~0 bytes/tx     │ │ ~0 bytes/tx │ │ ~0 bytes/tx  │ │ ~255 KB/tx  │
└─────────────────┘ └─────────────┘ └──────────────┘ └─────────────┘
```

### Data Flow

```rust
// 1. User generates proof
let witness = build_transfer_witness(from, to, amount);
let proof = generate_ligerito_proof(&witness)?;
let succinct = extract_succinct_proof(&proof)?;

// 2. Broadcast succinct proof to ALL nodes
network.broadcast_to_all(&succinct)?;

// 3. Send witness to ARCHIVE nodes only
for archive in network.get_archive_nodes() {
    archive.store_witness(tx_hash, &witness)?;
}

// 4. All nodes verify succinct proof
// Validators use PolkaVM (deterministic)
// Full nodes use native (faster)
// Light clients use PolkaVM (minimal state)

// 5. Only archive nodes can serve historical queries
// Block explorers query archive nodes
// Debuggers query archive nodes
// Auditors query archive nodes
```

## Implementation Guide

### For Archive Nodes

```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub witness: TransferInstance,
    pub proof: LigeritoSuccinctProof,
    pub metadata: TransactionMetadata,
}

#[derive(Serialize, Deserialize)]
pub struct TransactionMetadata {
    pub block_height: u64,
    pub timestamp: u64,
    pub tx_index: u32,
}

pub struct ArchiveNode {
    db: Database,
    indices: IndexManager,
}

impl ArchiveNode {
    pub fn new(db_path: &str) -> Result<Self> {
        Ok(Self {
            db: Database::open(db_path)?,
            indices: IndexManager::new(),
        })
    }

    pub fn store_transaction(
        &mut self,
        tx_hash: Hash,
        witness: TransferInstance,
        proof: LigeritoSuccinctProof,
        metadata: TransactionMetadata,
    ) -> Result<()> {
        let entry = ArchiveEntry { witness, proof, metadata };

        // Store main entry
        self.db.put(&tx_hash, &entry)?;

        // Update indices for fast queries
        self.indices.index_account(witness.sender_old.id, tx_hash)?;
        self.indices.index_account(witness.receiver_old.id, tx_hash)?;
        self.indices.index_block(metadata.block_height, tx_hash)?;

        Ok(())
    }

    // RPC endpoints
    pub fn get_witness(&self, tx_hash: &Hash) -> Result<TransferInstance> {
        let entry: ArchiveEntry = self.db.get(tx_hash)?;
        Ok(entry.witness)
    }

    pub fn get_account_transactions(
        &self,
        account_id: u64,
        limit: usize,
        offset: usize
    ) -> Result<Vec<TransactionSummary>> {
        let tx_hashes = self.indices.get_account_txs(account_id, limit, offset)?;

        tx_hashes.into_iter()
            .map(|hash| {
                let entry: ArchiveEntry = self.db.get(&hash)?;
                Ok(TransactionSummary::from_entry(&entry, &hash))
            })
            .collect()
    }

    pub fn get_block_transactions(&self, block_height: u64) -> Result<Vec<ArchiveEntry>> {
        let tx_hashes = self.indices.get_block_txs(block_height)?;

        tx_hashes.into_iter()
            .map(|hash| self.db.get(&hash))
            .collect()
    }
}
```

### For Block Explorers

```rust
// Connect to archive node RPC
let archive_client = ArchiveRpcClient::connect("https://archive.zeratul.network")?;

// Get transaction details
let tx_details = archive_client.get_transaction_details(tx_hash).await?;

println!("From: {}", tx_details.from);
println!("To: {}", tx_details.to);
println!("Amount: {}", tx_details.amount);
println!("Sender Balance: {} → {}",
    tx_details.sender_balance_before,
    tx_details.sender_balance_after
);

// Get account history
let history = archive_client.get_account_history(account_id).await?;

for tx in history {
    println!("{}: {} ({})",
        tx.timestamp,
        tx.amount,
        if tx.direction == Direction::Outgoing { "sent" } else { "received" }
    );
}
```

## Storage Estimates

### For Different Network Sizes

| Transactions/Day | Archive Storage/Day | Archive Storage/Year |
|------------------|--------------------|--------------------|
| 1,000 | 255 MB | 93 GB |
| 10,000 | 2.5 GB | 930 GB |
| 100,000 | 25 GB | 9.3 TB |
| 1,000,000 | 255 GB | 93 TB |

**Note**: These are upper bounds. With compression and deduplication, actual storage can be significantly lower.

### Pruning Strategies

Archive nodes can implement pruning to manage storage:

```rust
impl ArchiveNode {
    /// Prune transactions older than retention period
    pub fn prune_old_transactions(&mut self, retention_days: u64) -> Result<usize> {
        let cutoff_timestamp = current_timestamp() - (retention_days * 86400);
        let mut pruned = 0;

        for (tx_hash, entry) in self.db.iter() {
            if entry.metadata.timestamp < cutoff_timestamp {
                self.db.delete(&tx_hash)?;
                pruned += 1;
            }
        }

        Ok(pruned)
    }
}
```

## Conclusion

**For Zeratul archive nodes, the recommended approach is:**

✅ **Store witness + succinct proof separately**

**Reasons:**
1. Smallest storage requirement (~255 KB vs ~1-5 MB for ZODA)
2. Full witness recovery (instant, no decoding needed)
3. Can verify proof matches witness
4. Easy to query and index

**Not recommended:**
- ❌ Ligerito full proofs (large storage, no recovery)
- ❌ Succinct proofs only (no witness recovery)
- ⚠️ ZODA shards (works but 4-20x larger storage)

**Archive nodes provide:**
- Complete transaction history
- Account state at any point in time
- Block explorer support
- Audit and compliance capabilities
- Debugging and analysis tools

**The key insight:** Proofs prove validity, but don't preserve data. For historical queries, store the witness separately!
