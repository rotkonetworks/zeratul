# Data Availability with 2D Encoding and Treasury Incentives

**Last Updated**: 2025-11-13

## Overview

Zeratul uses **2D Reed-Solomon encoding** (tensor/matrix encoding) combined with **treasury-based incentives** to provide cost-effective data availability with long-term storage guarantees.

## Why 2D Encoding?

### 1D vs 2D Comparison

#### 1D Reed-Solomon (Traditional)
```
Data: [d₁, d₂, d₃, d₄, ..., dₙ]
    ↓ Encode with rate 1/2
Coded: [d₁, d₂, d₃, d₄, ..., dₙ, p₁, p₂, p₃, ..., pₙ]
```

**Problem**: Need to download entire encoded vector to verify

#### 2D Reed-Solomon (What We Use!)
```
Data arranged as matrix:
[d₁  d₂  d₃  d₄ ]
[d₅  d₆  d₇  d₈ ]
[d₉  d₁₀ d₁₁ d₁₂]
[d₁₃ d₁₄ d₁₅ d₁₆]

    ↓ Encode rows (horizontal)
    ↓ Encode columns (vertical)

Tensor encoding: Z = GX̃G'ᵀ

Can verify by sampling random rows OR columns!
```

**Benefit**: **Sublinear verification** - sample √N rows instead of N elements!

### This Is Exactly What We Already Have!

Looking at our code:

```rust
// From accidental_computer.rs
// We're ALREADY using 2D encoding!

// Step 2: ZODA encode the data (Reed-Solomon)
let commitment = Zoda::commit(&data, &codec_config)?;

// ZODA uses tensor encoding: Z = GX̃G'ᵀ
// This is 2D Reed-Solomon!
```

**We're already doing the right thing!** ZODA = 2D Reed-Solomon = What Celestia/Polkadot use

## Architecture with Treasury

```
┌─────────────────────────────────────────────────────────┐
│ Users Submit Transactions                               │
│ - Pay base transaction fee                              │
│ - Small portion goes to treasury (e.g., 10%)           │
└────────────┬────────────────────────────────────────────┘
             │
             ↓ Transaction fees flow in
┌─────────────────────────────────────────────────────────┐
│ Treasury / Community Pool                               │
│ - Accumulates from transaction fees                     │
│ - Governed by validators/token holders                  │
│ - Allocates to DA nodes and Archive nodes               │
└────────────┬────────────────────────────────────────────┘
             │
             ├──────────────────┬──────────────────┐
             ↓                  ↓                  ↓
┌──────────────────┐  ┌──────────────────┐  ┌─────────────┐
│ DA Nodes (30d)   │  │ Archive Nodes    │  │ Public Goods│
│ - Store witness  │  │ - Permanent      │  │ - Explorers │
│ - Earn from pool │  │ - Earn from pool │  │ - Analytics │
└──────────────────┘  └──────────────────┘  └─────────────┘
```

## Complete Flow

### Step 1: Transaction Submission

```rust
pub struct Transaction {
    witness: TransferInstance,
    proof: LigeritoSuccinctProof,
    fee: Amount,  // Base transaction fee
}

impl User {
    pub fn submit_transaction(&self, tx: Transaction) -> Result<()> {
        // User pays single fee
        // No separate DA fee needed!

        self.network.broadcast(tx)?;

        Ok(())
    }
}
```

### Step 2: Fee Distribution

```rust
pub struct FeeDistribution {
    validator_rewards: Amount,     // 70% - consensus participants
    treasury: Amount,               // 20% - DA + Archive funding
    burn: Amount,                   // 10% - deflationary
}

impl Protocol {
    pub fn distribute_fees(&mut self, total_fees: Amount) -> Result<()> {
        let distribution = FeeDistribution {
            validator_rewards: total_fees * 70 / 100,
            treasury: total_fees * 20 / 100,
            burn: total_fees * 10 / 100,
        };

        // Pay validators immediately
        self.distribute_to_validators(distribution.validator_rewards)?;

        // Add to treasury for DA/Archive
        self.treasury.deposit(distribution.treasury)?;

        // Burn for deflation
        self.burn(distribution.burn)?;

        Ok(())
    }
}
```

### Step 3: Data Encoding (2D Reed-Solomon)

```rust
pub fn encode_block_data(transactions: Vec<Transaction>) -> Result<EncodedBlock> {
    // Step 1: Arrange transaction witnesses as matrix
    let witness_matrix = arrange_as_matrix(&transactions)?;

    // Step 2: ZODA encoding (this is 2D Reed-Solomon!)
    // Z = GX̃G'ᵀ where:
    // - G is Reed-Solomon encoder for rows
    // - G' is Reed-Solomon encoder for columns
    // - X̃ is the witness matrix

    let coding_config = CodingConfig {
        minimum_shards: 3,
        extra_shards: 2,
    };

    let codec_config = CodecConfig {
        maximum_shard_size: 1024 * 1024,  // 1 MB
    };

    let commitment = Zoda::commit(
        &witness_matrix,
        &codec_config
    )?;

    // Generate shards (both row and column samples)
    let shards = Zoda::generate_shards(
        &witness_matrix,
        &coding_config,
        &commitment
    )?;

    Ok(EncodedBlock {
        commitment,      // Merkle root of encoded data
        shards,          // 2D encoded shards
        row_roots: commitment.row_roots,
        col_roots: commitment.col_roots,
    })
}
```

### Step 4: Treasury Payout to DA Nodes

```rust
pub struct DANodeRegistration {
    node_id: PublicKey,
    storage_commitment: StorageCommitment,
    stake: Amount,
}

pub struct StorageCommitment {
    capacity_kb: u64,          // How much they can store
    retention_days: u32,        // How long they commit (30+)
    uptime_guarantee: f64,      // 99.9%
}

impl Treasury {
    /// Pay DA nodes proportionally to their storage
    pub fn pay_da_nodes(&mut self, epoch: u64) -> Result<()> {
        let total_budget = self.calculate_da_budget(epoch)?;
        let da_nodes = self.get_active_da_nodes(epoch)?;

        // Total storage provided by all DA nodes
        let total_storage: u64 = da_nodes
            .iter()
            .map(|n| n.storage_commitment.capacity_kb)
            .sum();

        // Pay proportionally to storage provided
        for node in da_nodes {
            let proportion = node.storage_commitment.capacity_kb as f64
                           / total_storage as f64;

            let payment = (total_budget as f64 * proportion) as Amount;

            // Verify they actually stored the data
            if self.verify_storage_proof(&node, epoch)? {
                self.transfer(node.node_id, payment)?;
            } else {
                // Slash their stake
                self.slash_node(&node.node_id)?;
            }
        }

        Ok(())
    }

    /// Calculate DA budget for epoch based on treasury balance
    fn calculate_da_budget(&self, epoch: u64) -> Result<Amount> {
        // Allocate 40% of treasury to DA (60% to archives/grants)
        let total = self.balance()?;
        Ok(total * 40 / 100)
    }
}
```

### Step 5: Verification with 2D Sampling

```rust
pub fn verify_data_availability(
    commitment: &ZodaCommitment,
    samples: u32,  // Number of random samples
) -> Result<bool> {
    // Sample random rows
    let row_samples = sample_random_rows(commitment, samples / 2);

    // Sample random columns
    let col_samples = sample_random_columns(commitment, samples / 2);

    // Verify each sample
    for sample in row_samples {
        let (index, row_data) = download_row(sample.index)?;

        // Verify this row is correctly encoded
        if !verify_row_encoding(row_data, commitment.row_roots[index])? {
            return Ok(false);  // Data not available!
        }
    }

    for sample in col_samples {
        let (index, col_data) = download_column(sample.index)?;

        // Verify this column is correctly encoded
        if !verify_col_encoding(col_data, commitment.col_roots[index])? {
            return Ok(false);
        }
    }

    // If all samples valid, data is available with high probability
    Ok(true)
}
```

**Key advantage**: Only need to download √N samples instead of N!

For 2^24 coefficients:
- 1D: Need ~2^24 = 16M samples
- 2D: Need ~2^12 = 4096 row samples + 4096 col samples = **8192 total**

**2000x reduction in sampling cost!**

## Storage Proof (Proof of Storage)

```rust
pub struct StorageProof {
    epoch: u64,
    node_id: PublicKey,

    // Merkle proof of stored data
    merkle_root: Hash,
    merkle_proofs: Vec<MerkleProof>,

    // Challenges responded to
    challenges: Vec<ChallengeResponse>,
}

pub struct ChallengeResponse {
    challenge_id: Hash,
    requested_shard: ShardIndex,
    shard_data: Vec<u8>,
    merkle_proof: MerkleProof,
    timestamp: u64,
}

impl DANode {
    /// Respond to random challenge
    pub fn respond_to_challenge(
        &self,
        challenge: StorageChallenge
    ) -> Result<ChallengeResponse> {
        // Fetch the requested shard
        let shard_data = self.db.get_shard(
            challenge.epoch,
            challenge.shard_index
        )?;

        // Generate Merkle proof
        let merkle_proof = self.tree.prove(
            challenge.epoch,
            challenge.shard_index
        )?;

        Ok(ChallengeResponse {
            challenge_id: challenge.id,
            requested_shard: challenge.shard_index,
            shard_data,
            merkle_proof,
            timestamp: current_time(),
        })
    }
}

impl Protocol {
    /// Randomly challenge DA nodes each epoch
    pub fn challenge_da_nodes(&self, epoch: u64) -> Result<()> {
        let da_nodes = self.get_active_da_nodes(epoch)?;

        for node in da_nodes {
            // Pick random shard from random past epoch
            let random_epoch = sample_random_epoch(0, epoch)?;
            let random_shard = sample_random_shard()?;

            let challenge = StorageChallenge {
                id: Hash::random(),
                node_id: node.node_id,
                epoch: random_epoch,
                shard_index: random_shard,
                deadline: current_time() + 3600,  // 1 hour
            };

            self.pending_challenges.insert(challenge.id, challenge);
        }

        Ok(())
    }
}
```

## Treasury Governance

```rust
pub struct TreasuryGovernance {
    proposals: HashMap<ProposalId, Proposal>,
}

pub enum Proposal {
    // Adjust fee split
    ChangeFeeDistribution {
        validator_share: u8,    // %
        treasury_share: u8,     // %
        burn_share: u8,         // %
    },

    // Adjust DA budget allocation
    ChangeDAAllocation {
        da_nodes_share: u8,     // % of treasury
        archive_share: u8,      // % of treasury
    },

    // Grant to specific archive node
    ArchiveGrant {
        recipient: PublicKey,
        amount: Amount,
        duration_epochs: u64,
    },

    // Change retention requirements
    ChangeRetentionPeriod {
        from_days: u32,
        to_days: u32,
    },
}

impl Validator {
    /// Vote on treasury proposal using FROST
    pub fn vote_on_proposal(
        &self,
        proposal_id: ProposalId,
        approve: bool,
    ) -> Result<()> {
        // Threshold signature vote
        self.frost_sign(proposal_id, approve)
    }
}
```

## Complete Example

```rust
// Example: Epoch 100 processing

// 1. Collect transaction fees
let total_fees = 1000 tokens;  // From all transactions in epoch

// 2. Distribute fees
let fee_dist = FeeDistribution {
    validator_rewards: 700 tokens,    // 70%
    treasury: 200 tokens,              // 20%
    burn: 100 tokens,                  // 10%
};

treasury.deposit(200 tokens);
// Treasury now has 200 tokens for this epoch

// 3. Allocate from treasury
let da_budget = 200 * 0.4 = 80 tokens;       // 40% of treasury
let archive_budget = 200 * 0.4 = 80 tokens;  // 40% of treasury
let grants_budget = 200 * 0.2 = 40 tokens;   // 20% of treasury

// 4. Pay DA nodes (who stored epoch 70-99, last 30 days)
// 5 DA nodes, each storing 20% of data
for da_node in da_nodes {
    if verify_storage_proof(da_node, epoch) {
        pay(da_node, 80 tokens * 0.2 = 16 tokens);
    }
}

// 5. Pay archive nodes (who store everything forever)
// 3 archive nodes, each storing 100% of history
for archive_node in archive_nodes {
    if verify_uptime(archive_node) > 0.99 {
        pay(archive_node, 80 tokens / 3 = 26.67 tokens);
    }
}

// 6. Distribute grants (governance-approved)
for grant in active_grants {
    pay(grant.recipient, grant.amount_per_epoch);
}
```

## Cost Comparison: Polkadot vs Zeratul

### Polkadot DA Costs
- Uses 2D Reed-Solomon ✅
- Very cheap (~$0.001 per blob)
- No long-term storage guarantees ❌
- Data availability windows are short

### Celestia DA Costs
- Uses 2D Reed-Solomon ✅
- Cheap (~$0.01 per blob)
- No long-term storage ❌
- Ephemeral by design

### Zeratul DA Model
- Uses 2D Reed-Solomon ✅ (via ZODA)
- Treasury-funded (users pay base tx fee) ✅
- **30-day guarantee via DA nodes** ✅
- **Permanent via archive nodes** ✅
- **All funded from treasury** ✅

## Storage Costs

### DA Nodes (30-day retention)

**Network doing 1000 TPS:**
- Data per day: 1000 tx/s × 86400 s × 255 KB = 22 GB/day
- 30-day storage: 660 GB
- Cost at $0.01/GB/month: **$6.60/month per DA node**

**Treasury needs to pay** (for 5 DA nodes):
- 5 nodes × $6.60 = **$33/month**

**Treasury income** (at 1000 TPS, $0.01/tx):
- 1000 tx/s × 86400 s × $0.01 = **$864/day** = **$25,920/month**
- 20% to treasury = **$5,184/month**

**Result**: Treasury has **157x more** than needed for DA! ✅

### Archive Nodes (Permanent)

**Storage grows over time:**
- Year 1: 22 GB/day × 365 = 8 TB
- Year 2: 16 TB total
- Year 3: 24 TB total

**Cost at $0.01/GB/month:**
- Year 1: 8 TB × $10 = **$80/year**
- Year 2: 16 TB × $10 = **$160/year**
- Year 3: 24 TB × $10 = **$240/year**

**For 3 archive nodes** (redundancy):
- Year 1: $240/year = **$20/month**
- Year 2: $480/year = **$40/month**
- Year 3: $720/year = **$60/month**

**Treasury can easily afford this!** Even in year 3, only using ~1% of treasury income for archives.

## Incentive Formula

```rust
pub fn calculate_da_node_reward(
    node: &DANode,
    epoch: u64,
    treasury_budget: Amount,
) -> Amount {
    // Base reward proportional to storage
    let total_storage = get_total_network_storage(epoch);
    let base_reward = treasury_budget
        * node.storage_commitment.capacity_kb
        / total_storage;

    // Uptime multiplier
    let uptime = measure_uptime(node, epoch);
    let uptime_multiplier = uptime / 0.999;  // 99.9% is 1.0x

    // Challenge response bonus
    let challenges_passed = count_successful_challenges(node, epoch);
    let challenge_bonus = challenges_passed * 100;  // 100 tokens per challenge

    base_reward * uptime_multiplier + challenge_bonus
}
```

## Summary

**Zeratul's DA model:**

1. **2D Reed-Solomon encoding** (already implemented via ZODA!) ✅
2. **Treasury-funded** (from 20% of tx fees) ✅
3. **30-day DA guarantee** (via incentivized DA nodes) ✅
4. **Permanent archives** (via treasury grants + RPC fees) ✅
5. **Sublinear verification** (sample √N instead of N) ✅
6. **Governable** (validators vote on treasury allocation) ✅

**Economics work out:**
- Treasury income: ~$5,000/month (at 1000 TPS)
- DA costs: ~$33/month (5 nodes, 30 days)
- Archive costs: ~$60/month (3 nodes, year 3)
- **Surplus: ~$4,900/month for grants, development, etc.**

**This is cheaper than Polkadot/Celestia AND provides long-term storage guarantees!** 🎯
