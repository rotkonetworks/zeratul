//! DoS Prevention & Rate Limiting
//!
//! HARDENING: Protects against spam attacks and network flooding.
//!
//! ## Attack Vectors Mitigated
//!
//! 1. **Transaction Spam**: Attacker floods with invalid transactions
//! 2. **Proof Spam**: Attacker submits many invalid ZK proofs
//! 3. **Liquidation Spam**: Attacker spams liquidation proposals
//! 4. **Resource Exhaustion**: Validator CPU/memory exhaustion
//!
//! ## Defense Layers
//!
//! 1. **Transaction Fees**: Make spam expensive
//! 2. **Rate Limiting**: Limit transactions per address per block
//! 3. **Proof-of-Work**: Small PoW per transaction (optional)
//! 4. **Priority Queue**: Higher fees = faster execution

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use std::collections::HashMap;
use anyhow::{bail, Result};

/// DoS prevention configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoSPreventionConfig {
    /// Minimum transaction fee (in base currency)
    /// HARDENING: Makes spam expensive
    /// Default: 1000 (0.001 tokens if decimals=6)
    pub min_transaction_fee: u64,

    /// Maximum transactions per address per block
    /// HARDENING: Prevents address-based flooding
    /// Default: 10 (can submit up to 10 orders per block)
    pub max_transactions_per_address_per_block: u32,

    /// Maximum total transactions per block
    /// HARDENING: Prevents network-wide flooding
    /// Default: 1000
    pub max_transactions_per_block: u32,

    /// Maximum pending transactions in mempool per address
    /// HARDENING: Prevents mempool spam
    /// Default: 50
    pub max_pending_per_address: u32,

    /// Fee multiplier for priority ordering
    /// HARDENING: Higher fees get priority
    /// Default: 1.5 (50% more fee = higher priority)
    pub priority_fee_multiplier: f64,

    /// Whether to require proof-of-work for transactions
    /// HARDENING: Additional spam protection (optional)
    /// Default: false
    pub require_proof_of_work: bool,

    /// PoW difficulty (number of leading zero bits)
    /// Default: 10 (easy: ~1ms on modern CPU)
    pub pow_difficulty: u8,

    /// Penalty for invalid transactions (blocks)
    /// HARDENING: Ban address temporarily if invalid txs
    /// Default: 100 blocks (~3 minutes)
    pub invalid_tx_penalty_blocks: u64,

    /// Max invalid transactions before penalty
    /// Default: 3 (3 strikes rule)
    pub max_invalid_before_penalty: u32,
}

impl Default for DoSPreventionConfig {
    fn default() -> Self {
        Self {
            min_transaction_fee: 1000,                          // 0.001 tokens
            max_transactions_per_address_per_block: 10,         // 10 per address
            max_transactions_per_block: 1000,                   // 1000 total
            max_pending_per_address: 50,                        // 50 pending
            priority_fee_multiplier: 1.5,                       // 50% more = priority
            require_proof_of_work: false,                       // PoW disabled by default
            pow_difficulty: 10,                                 // 10 bits (~1ms)
            invalid_tx_penalty_blocks: 100,                     // 100 blocks (~3 min)
            max_invalid_before_penalty: 3,                      // 3 strikes
        }
    }
}

/// Transaction with fee and priority
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction sender (public key or commitment)
    pub sender: [u8; 32],

    /// Transaction data (proof, order, etc.)
    pub data: Vec<u8>,

    /// Fee paid for this transaction
    /// HARDENING: Higher fees get priority
    pub fee: u64,

    /// Nonce (prevents replay attacks)
    pub nonce: u64,

    /// Optional proof-of-work
    pub proof_of_work: Option<ProofOfWork>,

    /// Signature
    #[serde(with = "BigArray")]
    pub signature: [u8; 64],
}

impl Transaction {
    /// Calculate priority score for ordering
    ///
    /// HARDENING: Higher fees = higher priority
    pub fn priority_score(&self, base_fee: u64) -> u64 {
        if self.fee <= base_fee {
            0 // Below minimum, no priority
        } else {
            // Priority increases with fee
            self.fee - base_fee
        }
    }

    /// Verify proof-of-work (if required)
    pub fn verify_pow(&self, difficulty: u8) -> bool {
        if let Some(pow) = &self.proof_of_work {
            pow.verify(&self.data, difficulty)
        } else {
            false
        }
    }
}

/// Proof-of-work for transaction
///
/// HARDENING: Small PoW makes spam expensive (CPU-wise)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOfWork {
    /// Nonce used to find valid PoW
    pub nonce: u64,

    /// Hash of (data || nonce)
    pub hash: [u8; 32],
}

impl ProofOfWork {
    /// Generate proof-of-work for data
    ///
    /// Finds nonce such that hash has required leading zero bits
    pub fn generate(data: &[u8], difficulty: u8) -> Self {
        use sha2::{Sha256, Digest};

        let target_zeros = difficulty;
        let mut nonce = 0u64;

        loop {
            let mut hasher = Sha256::new();
            hasher.update(data);
            hasher.update(&nonce.to_le_bytes());
            let hash: [u8; 32] = hasher.finalize().into();

            // Check leading zero bits
            if Self::count_leading_zero_bits(&hash) >= target_zeros {
                return Self { nonce, hash };
            }

            nonce += 1;
        }
    }

    /// Verify proof-of-work
    pub fn verify(&self, data: &[u8], difficulty: u8) -> bool {
        use sha2::{Sha256, Digest};

        // Recompute hash
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.update(&self.nonce.to_le_bytes());
        let computed_hash: [u8; 32] = hasher.finalize().into();

        // Verify hash matches
        if computed_hash != self.hash {
            return false;
        }

        // Verify difficulty
        Self::count_leading_zero_bits(&self.hash) >= difficulty
    }

    /// Count leading zero bits in hash
    fn count_leading_zero_bits(hash: &[u8; 32]) -> u8 {
        let mut count = 0;
        for byte in hash {
            if *byte == 0 {
                count += 8;
            } else {
                count += byte.leading_zeros() as u8;
                break;
            }
        }
        count
    }
}

/// Rate limiter per address
///
/// HARDENING: Tracks transaction counts per address per block
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// Configuration
    config: DoSPreventionConfig,

    /// Transaction count per address in current block
    /// Map: address -> count
    current_block_counts: HashMap<[u8; 32], u32>,

    /// Current block height
    current_block: u64,

    /// Invalid transaction tracker
    /// Map: address -> (count, last_penalty_block)
    invalid_tx_tracker: HashMap<[u8; 32], (u32, u64)>,

    /// Banned addresses (until block height)
    /// Map: address -> unban_at_block
    banned_addresses: HashMap<[u8; 32], u64>,
}

impl RateLimiter {
    pub fn new(config: DoSPreventionConfig) -> Self {
        Self {
            config,
            current_block_counts: HashMap::new(),
            current_block: 0,
            invalid_tx_tracker: HashMap::new(),
            banned_addresses: HashMap::new(),
        }
    }

    /// Start new block (reset counters)
    pub fn new_block(&mut self, block_height: u64) {
        self.current_block = block_height;
        self.current_block_counts.clear();

        // Clean up expired bans
        self.banned_addresses.retain(|_, unban_at| *unban_at > block_height);
    }

    /// Check if transaction is allowed
    ///
    /// HARDENING: Enforces rate limits and bans
    pub fn can_accept_transaction(&self, sender: &[u8; 32]) -> Result<()> {
        // Check if banned
        if let Some(unban_at) = self.banned_addresses.get(sender) {
            if *unban_at > self.current_block {
                bail!(
                    "Address banned until block {} (currently {})",
                    unban_at,
                    self.current_block
                );
            }
        }

        // Check per-address rate limit
        let count = self.current_block_counts.get(sender).unwrap_or(&0);
        if *count >= self.config.max_transactions_per_address_per_block {
            bail!(
                "Rate limit exceeded: {} transactions in block (max {})",
                count,
                self.config.max_transactions_per_address_per_block
            );
        }

        Ok(())
    }

    /// Record transaction from address
    ///
    /// HARDENING: Increments counter
    pub fn record_transaction(&mut self, sender: &[u8; 32]) {
        let count = self.current_block_counts.entry(*sender).or_insert(0);
        *count += 1;
    }

    /// Record invalid transaction
    ///
    /// HARDENING: Track invalid txs and ban repeat offenders
    pub fn record_invalid_transaction(&mut self, sender: &[u8; 32]) {
        let entry = self.invalid_tx_tracker.entry(*sender).or_insert((0, 0));
        entry.0 += 1;

        // Check if should ban
        if entry.0 >= self.config.max_invalid_before_penalty {
            let unban_at = self.current_block + self.config.invalid_tx_penalty_blocks;
            self.banned_addresses.insert(*sender, unban_at);

            // Reset counter
            entry.0 = 0;
            entry.1 = self.current_block;

            eprintln!(
                "Address {:?} banned until block {} for {} invalid transactions",
                sender, unban_at, self.config.max_invalid_before_penalty
            );
        }
    }

    /// Get current transaction count for address
    pub fn get_count(&self, sender: &[u8; 32]) -> u32 {
        *self.current_block_counts.get(sender).unwrap_or(&0)
    }

    /// Check if address is banned
    pub fn is_banned(&self, sender: &[u8; 32]) -> bool {
        if let Some(unban_at) = self.banned_addresses.get(sender) {
            *unban_at > self.current_block
        } else {
            false
        }
    }
}

/// Priority queue for transactions
///
/// HARDENING: Orders by fee for optimal execution
#[derive(Debug, Clone)]
pub struct TransactionQueue {
    /// Configuration
    config: DoSPreventionConfig,

    /// Pending transactions ordered by priority
    queue: Vec<Transaction>,

    /// Maximum queue size
    max_size: usize,
}

impl TransactionQueue {
    pub fn new(config: DoSPreventionConfig, max_size: usize) -> Self {
        Self {
            config,
            queue: Vec::new(),
            max_size,
        }
    }

    /// Add transaction to queue
    ///
    /// HARDENING: Validates fee and PoW before accepting
    pub fn add(&mut self, tx: Transaction) -> Result<()> {
        // Validate minimum fee
        if tx.fee < self.config.min_transaction_fee {
            bail!(
                "Transaction fee too low: {} < {} minimum",
                tx.fee,
                self.config.min_transaction_fee
            );
        }

        // Validate PoW if required
        if self.config.require_proof_of_work {
            if !tx.verify_pow(self.config.pow_difficulty) {
                bail!("Invalid proof-of-work");
            }
        }

        // Check queue size
        if self.queue.len() >= self.max_size {
            // Remove lowest priority if new tx has higher priority
            let min_priority = self.queue
                .iter()
                .map(|t| t.priority_score(self.config.min_transaction_fee))
                .min()
                .unwrap_or(0);

            let tx_priority = tx.priority_score(self.config.min_transaction_fee);

            if tx_priority <= min_priority {
                bail!("Queue full and transaction priority too low");
            }

            // Remove lowest priority transaction
            if let Some(min_idx) = self.queue
                .iter()
                .enumerate()
                .min_by_key(|(_, t)| t.priority_score(self.config.min_transaction_fee))
                .map(|(i, _)| i)
            {
                self.queue.remove(min_idx);
            }
        }

        // Add and sort by priority
        self.queue.push(tx);
        self.queue.sort_by_key(|t| std::cmp::Reverse(t.priority_score(self.config.min_transaction_fee)));

        Ok(())
    }

    /// Take top N transactions (highest priority)
    pub fn take_top(&mut self, n: usize) -> Vec<Transaction> {
        let take_count = n.min(self.queue.len());
        self.queue.drain(..take_count).collect()
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proof_of_work() {
        let data = b"hello world";
        let difficulty = 8; // 8 leading zero bits

        let pow = ProofOfWork::generate(data, difficulty);
        assert!(pow.verify(data, difficulty));

        // Should fail with wrong data
        assert!(!pow.verify(b"wrong data", difficulty));

        // Should fail with higher difficulty
        assert!(!pow.verify(data, difficulty + 1));
    }

    #[test]
    fn test_rate_limiter() {
        let config = DoSPreventionConfig::default();
        let mut limiter = RateLimiter::new(config);

        let sender = [1u8; 32];

        limiter.new_block(100);

        // Should allow up to max transactions
        for _ in 0..10 {
            assert!(limiter.can_accept_transaction(&sender).is_ok());
            limiter.record_transaction(&sender);
        }

        // Should reject 11th transaction
        assert!(limiter.can_accept_transaction(&sender).is_err());

        // Should reset on new block
        limiter.new_block(101);
        assert!(limiter.can_accept_transaction(&sender).is_ok());
    }

    #[test]
    fn test_invalid_tx_banning() {
        let config = DoSPreventionConfig::default();
        let mut limiter = RateLimiter::new(config);

        let sender = [1u8; 32];
        limiter.new_block(100);

        // Record 3 invalid transactions (triggers ban)
        for _ in 0..3 {
            limiter.record_invalid_transaction(&sender);
        }

        // Should be banned
        assert!(limiter.is_banned(&sender));
        assert!(limiter.can_accept_transaction(&sender).is_err());

        // Should remain banned for penalty period
        limiter.new_block(150);
        assert!(limiter.is_banned(&sender));

        // Should be unbanned after penalty period
        limiter.new_block(201);
        assert!(!limiter.is_banned(&sender));
        assert!(limiter.can_accept_transaction(&sender).is_ok());
    }

    #[test]
    fn test_priority_queue() {
        let config = DoSPreventionConfig::default();
        let mut queue = TransactionQueue::new(config.clone(), 10);

        // Add transactions with different fees
        for i in 0..5 {
            let tx = Transaction {
                sender: [i; 32],
                data: vec![i],
                fee: 1000 + (i as u64 * 100),
                nonce: 0,
                proof_of_work: None,
                signature: [0; 64],
            };
            queue.add(tx).unwrap();
        }

        // Take top 3 (highest fees)
        let top = queue.take_top(3);
        assert_eq!(top.len(), 3);

        // Should be ordered by fee (highest first)
        assert_eq!(top[0].fee, 1400); // i=4
        assert_eq!(top[1].fee, 1300); // i=3
        assert_eq!(top[2].fee, 1200); // i=2
    }
}
