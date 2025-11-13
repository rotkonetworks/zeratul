# Data Availability Tiers and Incentives in Zeratul

**Last Updated**: 2025-11-13

## Overview

Zeratul uses a **tiered data availability** model with different retention guarantees and incentive mechanisms. This document explains the tiers, their guarantees, and how nodes are incentivized to provide each level of service.

## The Problem

Different use cases need different data availability guarantees:

- **Fraud proofs**: Need ~7 days (challenge period)
- **Dispute resolution**: Need ~30 days (legal/governance processes)
- **Block explorers**: Want permanent history
- **Auditors**: Need long-term access (years)
- **Regular users**: Just need current state

**Question**: How do we provide guarantees without requiring everyone to store everything forever?

**Answer**: Tiered data availability with different incentives!

## The Three Tiers

```
┌────────────────────────────────────────────────────────┐
│ Tier 1: Consensus Layer (Required)                    │
│ - Retention: Current epoch only (~1 day)              │
│ - Who: All validators + full nodes                    │
│ - Stores: Succinct proofs only (~255 KB/tx)          │
│ - Incentive: Block rewards + transaction fees         │
│ - Guarantee: Consensus finality                       │
└────────────────────────────────────────────────────────┘
            ↓ Proofs flow to DA layer
┌────────────────────────────────────────────────────────┐
│ Tier 2: Data Availability Layer (Incentivized)        │
│ - Retention: 30 days (configurable per epoch)         │
│ - Who: DA nodes (opt-in, paid)                        │
│ - Stores: Witness + Succinct proof (~255 KB/tx)      │
│ - Incentive: DA fees (paid by provers)                │
│ - Guarantee: Cryptographic proof of storage           │
└────────────────────────────────────────────────────────┘
            ↓ Historical data migrates
┌────────────────────────────────────────────────────────┐
│ Tier 3: Archive Layer (Voluntary or Paid)             │
│ - Retention: Permanent (years+)                        │
│ - Who: Archive nodes (opt-in)                         │
│ - Stores: Witness + Succinct proof (~255 KB/tx)      │
│ - Incentive: RPC fees, block explorer revenue, grants │
│ - Guarantee: Best-effort (no protocol-level SLA)      │
└────────────────────────────────────────────────────────┘
```

## Tier 1: Consensus Layer

### What It Provides

**Retention**: Current epoch only (~1 day, configurable)

**Purpose**: Enable consensus and immediate verification

**Stored Data**:
```rust
pub struct ConsensusData {
    succinct_proofs: HashMap<Hash, LigeritoSuccinctProof>,  // ~255 KB each
    current_state_root: Hash,
    epoch_number: u64,
}
```

**Total Storage**: ~255 KB × transactions_per_epoch

### Who Participates

- ✅ **All validators** (mandatory)
- ✅ **All full nodes** (voluntary, but typical)
- ❌ Light clients (don't store proofs)

### Incentive Mechanism

```rust
// Validators earn from consensus participation
pub struct ValidatorRewards {
    block_rewards: Amount,           // Protocol inflation
    transaction_fees: Amount,         // Fees from transactions
    frost_signing_reward: Amount,     // For threshold signing
}

// No additional DA incentive needed - this is part of consensus
```

**Why it works**: Validators must participate in consensus to earn rewards. Storing current epoch data is a minimal requirement.

### Pruning Policy

```rust
impl ConsensusNode {
    pub fn prune_old_epochs(&mut self) -> Result<()> {
        let current_epoch = self.chain.current_epoch();

        // Keep only current epoch
        self.proofs.retain(|_, metadata| {
            metadata.epoch == current_epoch
        });

        // Older data is responsibility of DA layer
        Ok(())
    }
}
```

## Tier 2: Data Availability Layer (30 Days)

### What It Provides

**Retention**: 30 days (adjustable per epoch via governance)

**Purpose**:
- Enable fraud proofs and challenges
- Support dispute resolution
- Provide recent history for new nodes syncing

**Stored Data**:
```rust
pub struct DANodeStorage {
    // Full witness data for 30 days
    witnesses: HashMap<Hash, TransferInstance>,      // ~232 bytes each
    proofs: HashMap<Hash, LigeritoSuccinctProof>,   // ~255 KB each
    metadata: HashMap<Hash, DAMetadata>,

    // Retention tracking
    retention_period: Duration,                      // Default: 30 days
    oldest_stored_epoch: u64,
}

pub struct DAMetadata {
    epoch: u64,
    timestamp: u64,
    expiry: u64,              // When this can be pruned
    storage_commitment: Hash,  // Proof of storage
}
```

**Total Storage**: ~255 KB × transactions × 30 days

### Who Participates

**DA Nodes** (opt-in, economically motivated):
- Run DA node software
- Commit to storing data for 30 days
- Provide cryptographic proof of storage
- Earn DA fees

### Incentive Mechanism

#### Option A: Prover-Pays Model (Recommended)

```rust
pub struct TransactionSubmission {
    witness: TransferInstance,
    proof: LigeritoSuccinctProof,
    da_fee: Amount,              // Fee for 30-day storage
}

impl Prover {
    pub fn submit_transaction(
        &self,
        witness: TransferInstance,
        proof: LigeritoSuccinctProof,
    ) -> Result<()> {
        // Calculate DA fee based on storage cost
        let da_fee = calculate_da_fee(
            data_size: witness.size() + proof.size(),  // ~255 KB
            retention_period: 30 * 86400,               // 30 days in seconds
            current_storage_price: self.get_storage_price()?,
        );

        // Submit to network with DA fee
        self.network.submit(TransactionSubmission {
            witness,
            proof,
            da_fee,
        })?;

        Ok(())
    }
}

fn calculate_da_fee(
    data_size: usize,
    retention_period: u64,
    storage_price_per_kb_per_day: Amount
) -> Amount {
    let kb = data_size / 1024;
    let days = retention_period / 86400;
    kb * days * storage_price_per_kb_per_day
}
```

**Example costs** (assuming $0.01/GB/month storage):
- Transaction size: 255 KB
- Retention: 30 days
- Cost: ~$0.000008 USD per transaction

**DA nodes earn**:
```rust
pub struct DANodeEarnings {
    // Earn from storing transactions
    storage_fees: Amount,           // From prover DA fees

    // Bonus for uptime and availability
    availability_bonus: Amount,     // Protocol reward for high uptime

    // Slashing for unavailability
    slashed_stake: Amount,          // If fail to serve data
}
```

#### Option B: Protocol-Subsidized Model

```rust
// Protocol allocates budget for DA
pub struct ProtocolDABudget {
    epoch_allocation: Amount,        // From protocol treasury/inflation
    per_kb_per_day_rate: Amount,    // How much protocol pays DA nodes
}

impl DANode {
    pub fn claim_rewards(&self, epoch: u64) -> Result<Amount> {
        // Prove storage of data during epoch
        let proof = self.generate_storage_proof(epoch)?;

        // Submit proof to protocol
        let reward = self.protocol.claim_da_reward(proof)?;

        Ok(reward)
    }
}
```

### Storage Commitment Protocol

```rust
pub struct DAStorageCommitment {
    node_id: PublicKey,
    epoch: u64,
    merkle_root: Hash,          // Root of all stored transactions
    stake: Amount,              // Slashable stake
    expiry: u64,                // When data can be pruned
}

impl DANode {
    /// Generate proof that we're storing the data
    pub fn generate_storage_proof(&self, epoch: u64) -> Result<StorageProof> {
        // Create Merkle tree of all stored witnesses
        let witnesses: Vec<_> = self.get_epoch_witnesses(epoch)?;
        let tree = MerkleTree::from_leaves(&witnesses);

        Ok(StorageProof {
            epoch,
            root: tree.root(),
            total_size: witnesses.len() * 255 * 1024,  // ~255 KB each
            signature: self.sign(&tree.root()),
        })
    }

    /// Serve data when requested (with proof)
    pub fn serve_witness(&self, tx_hash: &Hash) -> Result<WitnessWithProof> {
        let witness = self.witnesses.get(tx_hash)?;
        let merkle_proof = self.tree.prove(tx_hash)?;

        Ok(WitnessWithProof {
            witness,
            merkle_proof,
        })
    }
}
```

### Slashing for Unavailability

```rust
pub struct DAChallenge {
    challenger: PublicKey,
    da_node: PublicKey,
    tx_hash: Hash,              // Which transaction is missing?
    epoch: u64,
}

impl Protocol {
    pub fn challenge_da_node(
        challenger: PublicKey,
        da_node: PublicKey,
        tx_hash: Hash,
    ) -> Result<()> {
        // DA node has 1 hour to respond with the data
        let deadline = current_time() + 3600;

        self.pending_challenges.insert(ChallengeId::new(), DAChallenge {
            challenger,
            da_node,
            tx_hash,
            epoch: current_epoch(),
        });

        Ok(())
    }

    pub fn resolve_challenge(challenge_id: ChallengeId) -> Result<ChallengeResult> {
        let challenge = self.pending_challenges.get(challenge_id)?;

        if let Some(response) = self.da_responses.get(challenge_id) {
            // DA node provided the data - challenge failed
            // Challenger loses small bond
            Ok(ChallengeResult::ChallengeFailed)
        } else {
            // DA node didn't respond - slash their stake
            self.slash_da_node(challenge.da_node, SlashAmount::Partial)?;

            // Reward challenger
            self.reward_challenger(challenge.challenger)?;

            Ok(ChallengeResult::ChallengeSucceeded)
        }
    }
}
```

### Pruning Policy

```rust
impl DANode {
    pub fn prune_expired_data(&mut self) -> Result<PruneStats> {
        let current_time = get_current_timestamp();
        let mut pruned = 0;

        self.witnesses.retain(|tx_hash, metadata| {
            if metadata.expiry < current_time {
                // Data is older than retention period
                pruned += 1;
                false  // Remove it
            } else {
                true   // Keep it
            }
        });

        Ok(PruneStats {
            pruned_count: pruned,
            freed_space: pruned * 255 * 1024,  // ~255 KB each
        })
    }
}
```

## Tier 3: Archive Layer (Permanent)

### What It Provides

**Retention**: Permanent (years+)

**Purpose**:
- Block explorers
- Historical queries
- Audit and compliance
- Research and analytics

**Stored Data**: Same as Tier 2, but forever

### Who Participates

**Archive Nodes** (various motivations):

1. **Commercial block explorers** (revenue from ads/premium features)
2. **Protocol foundation** (grants, public good)
3. **Exchanges** (compliance, customer support)
4. **Research institutions** (academic interest)

### Incentive Mechanism

#### Option 1: RPC Fee Market

```rust
pub struct ArchiveNode {
    rpc_config: RpcConfig,
}

pub struct RpcConfig {
    // Paid RPC endpoints
    price_per_query: Amount,
    price_per_mb_served: Amount,

    // Optional: Free tier
    free_queries_per_day: u32,
}

impl ArchiveNode {
    pub async fn handle_rpc_request(
        &self,
        request: RpcRequest,
        auth: AuthToken,
    ) -> Result<RpcResponse> {
        // Check if user has paid or within free tier
        if !self.check_authorization(&auth, &request)? {
            return Err("Payment required");
        }

        // Serve the data
        let response = match request {
            RpcRequest::GetWitness(tx_hash) => {
                let witness = self.db.get(&tx_hash)?;
                RpcResponse::Witness(witness)
            }
            RpcRequest::GetAccountHistory(account_id) => {
                let history = self.get_account_history(account_id)?;
                RpcResponse::AccountHistory(history)
            }
            _ => return Err("Unsupported request"),
        };

        // Charge user
        self.charge_for_request(&auth, &request)?;

        Ok(response)
    }
}
```

**Revenue model**:
- Free tier: 1,000 queries/day
- Paid tier: $0.0001 per query
- Block explorer tier: $100/month unlimited

#### Option 2: Protocol Grants

```rust
pub struct ArchiveGrant {
    recipient: PublicKey,
    amount: Amount,
    duration: Duration,
    requirements: GrantRequirements,
}

pub struct GrantRequirements {
    min_retention_period: Duration,    // Must store for X years
    min_uptime: f64,                   // 99.9% uptime required
    free_tier_queries: u32,            // Must provide N free queries/day
}

// Foundation/DAO provides grants to archive operators
impl Protocol {
    pub fn distribute_archive_grants(&self, epoch: u64) -> Result<()> {
        for grant in self.active_grants.values() {
            // Check if grantee meets requirements
            if self.verify_grant_compliance(grant, epoch)? {
                self.transfer(grant.recipient, grant.amount)?;
            } else {
                // Slash or revoke grant
                self.revoke_grant(grant.id)?;
            }
        }

        Ok(())
    }
}
```

#### Option 3: Hybrid Model (Recommended)

```rust
pub enum ArchiveIncentive {
    // Voluntary (public good, foundation funded)
    Grant { amount: Amount, requirements: GrantRequirements },

    // Commercial (RPC fees)
    RpcFees { pricing: RpcConfig },

    // Hybrid (grant + fees)
    Hybrid {
        base_grant: Amount,          // Cover base costs
        rpc_pricing: RpcConfig,      // Generate additional revenue
    },
}
```

### No Protocol-Level Slashing

**Important**: Archive nodes are NOT part of consensus and don't have protocol-level slashing.

**Why**:
- Permanent storage is expensive
- Should be market-driven
- Users can choose which archives to trust

**Instead**:
- Reputation systems (uptime tracking)
- Multiple archives for redundancy
- Users verify data against on-chain commitments

## Retention Period Governance

```rust
pub struct DAGovernanceProposal {
    proposal_id: u64,
    proposal_type: ProposalType,
}

pub enum ProposalType {
    // Change retention period
    ChangeRetentionPeriod {
        from_days: u32,
        to_days: u32,
    },

    // Change DA fee pricing
    ChangeDAFeeRate {
        from_rate: Amount,
        to_rate: Amount,
    },

    // Allocate archive grants
    ArchiveGrant {
        recipient: PublicKey,
        amount: Amount,
        duration: Duration,
    },
}

// Validators vote on proposals
impl Validator {
    pub fn vote_on_proposal(
        &self,
        proposal_id: u64,
        vote: bool,
    ) -> Result<()> {
        // FROST threshold signature for governance
        self.frost_vote(proposal_id, vote)
    }
}
```

## Complete Data Lifecycle

```
Day 0: Transaction Submitted
    ↓
[Tier 1: Consensus]
- Validators verify proof
- Store in mempool/current epoch
- Consensus finality achieved
    ↓
Day 1: Epoch Rotation
    ↓
[Tier 2: DA Layer]
- Prover pays DA fee
- DA nodes store witness + proof
- 30-day retention begins
- Validators prune from Tier 1
    ↓
Days 1-30: DA Period
- Challenges can be raised
- Disputes can be resolved
- DA nodes earn fees
- Data available via DA nodes
    ↓
Day 30: DA Expiry
    ↓
[Tier 3: Archive]
- DA nodes prune data
- Archive nodes keep forever
- Available via RPC (paid or free)
- No protocol guarantees
    ↓
Years+: Historical Access
- Block explorers query archives
- Auditors verify history
- Researchers analyze data
```

## Storage Cost Estimates

### Per Transaction

| Tier | Storage | Retention | Cost/TX (at $0.01/GB/month) |
|------|---------|-----------|---------------------------|
| Tier 1 | 255 KB | 1 day | ~$0.0000003 |
| Tier 2 | 255 KB | 30 days | ~$0.000008 |
| Tier 3 | 255 KB | Permanent | ~$0.003/year |

### Network Scale

| TPS | Storage/Day | Tier 2 (30d) | Tier 3 (1 year) |
|-----|-------------|--------------|-----------------|
| 10 | 221 MB | 6.6 GB | 80 GB |
| 100 | 2.2 GB | 66 GB | 800 GB |
| 1,000 | 22 GB | 660 GB | 8 TB |
| 10,000 | 221 GB | 6.6 TB | 80 TB |

## Recommendations

### For Validators (Tier 1)
- ✅ Store current epoch only
- ✅ Prune after epoch rotation
- ✅ Earn from consensus rewards

### For DA Nodes (Tier 2)
- ✅ Commit to 30-day storage
- ✅ Earn DA fees from provers
- ✅ Provide uptime guarantees
- ✅ Slash for unavailability
- ✅ Prune after 30 days

### For Archive Nodes (Tier 3)
- ✅ Store permanently
- ✅ Earn from RPC fees or grants
- ✅ No protocol-level requirements
- ✅ Compete on service quality

### For Protocol Design
- ✅ Use prover-pays model for Tier 2
- ✅ 30-day default retention (governance adjustable)
- ✅ Challenge/response for DA enforcement
- ✅ Grants or market fees for Tier 3
- ✅ No mandatory permanent storage

## Conclusion

**Tiered data availability solves the tension between:**
- Short-term needs (fraud proofs) → Tier 2 (30 days, incentivized)
- Long-term needs (history) → Tier 3 (permanent, market-driven)
- Consensus efficiency → Tier 1 (ephemeral, mandatory)

**Key insight**: Don't force validators to store everything forever. Use economic incentives to attract specialized DA and archive nodes for different time horizons!
