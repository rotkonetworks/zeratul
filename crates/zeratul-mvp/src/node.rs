//! Zeratul node - verification layer node
//!
//! Orchestrates proof verification, accumulation, and consensus.
//!
//! Key insight: With verifiable proofs, leadership becomes optional.
//! Anyone can propose a valid block, validators just verify and vote.

use crate::{
    types::*,
    state::State,
    accumulator::Accumulator,
    prover::BlockProver,
    consensus::InstantBFT,
    BLOCK_TIME_MS, MAX_RESULTS_PER_BLOCK,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio::time::interval;

/// Node configuration
#[derive(Clone)]
pub struct NodeConfig {
    /// Validator ID (None for non-validators)
    pub validator_id: Option<ValidatorId>,
    /// Signing key for validators
    pub signing_key: Option<[u8; 32]>,
    /// Initial validators
    pub validators: Vec<Validator>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            validator_id: None,
            signing_key: None,
            validators: vec![
                Validator { pubkey: [1u8; 32], stake: 1000, active: true },
                Validator { pubkey: [2u8; 32], stake: 1000, active: true },
                Validator { pubkey: [3u8; 32], stake: 1000, active: true },
            ],
        }
    }
}

/// Zeratul node
pub struct Node {
    /// Current blockchain state
    state: State,
    /// Work package accumulator
    accumulator: Accumulator,
    /// Block prover
    prover: BlockProver,
    /// Consensus engine
    consensus: InstantBFT,
    /// Pending work packages (mempool)
    pending_work: VecDeque<WorkPackage>,
    /// Block chain (simplified - just a vec)
    chain: Vec<Block>,
    /// Node configuration
    config: NodeConfig,
}

impl Node {
    /// Create new node with config
    pub fn new(config: NodeConfig) -> Self {
        let state = State::genesis(config.validators.clone());

        let consensus = match (config.validator_id, config.signing_key) {
            (Some(id), Some(key)) => InstantBFT::new_validator(id, key),
            _ => InstantBFT::new(),
        };

        Self {
            state,
            accumulator: Accumulator::new(),
            prover: BlockProver::new(),
            consensus,
            pending_work: VecDeque::new(),
            chain: Vec::new(),
            config,
        }
    }

    /// Get current state
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Get current height
    pub fn height(&self) -> Height {
        self.state.height()
    }

    /// Get chain
    pub fn chain(&self) -> &[Block] {
        &self.chain
    }

    /// Submit work package for accumulation
    pub fn submit_work(&mut self, package: WorkPackage) {
        self.pending_work.push_back(package);
    }

    /// Produce a new block
    ///
    /// In Zeratul, any validator can produce a block - no leader needed.
    /// The block is valid if all proofs verify.
    pub fn produce_block(&mut self) -> Option<BlockProductionResult> {
        // Must be a validator to produce blocks
        let validator_id = self.config.validator_id?;

        let start = Instant::now();
        let next_height = self.state.height() + 1;

        // Collect work packages from mempool
        let packages: Vec<WorkPackage> = self.pending_work
            .drain(..)
            .take(MAX_RESULTS_PER_BLOCK)
            .collect();

        let timestamp = chrono::Utc::now().timestamp_millis() as u64;

        // Verify and process work packages
        let verify_start = Instant::now();
        let results = self.accumulator.process_work_packages(&self.state, packages);
        let verify_time = verify_start.elapsed();

        // Build accumulation result
        let accum_result = self.accumulator.build_block(&self.state, results, timestamp);
        let result_count = accum_result.result_count();

        // Generate proof of valid accumulation
        let proof_start = Instant::now();
        let proof_result = self.prover.prove_accumulation(&accum_result.trace);
        let proof_time = proof_start.elapsed();

        // Build block
        let parent = self.chain.last()
            .map(|b| b.hash())
            .unwrap_or(GENESIS_PARENT);

        let block = accum_result.into_block(
            parent,
            next_height,
            validator_id,
            proof_result.proof.clone(),
            [0u8; 64], // MVP: dummy signature
        );

        let block_hash = block.hash();
        let total_time = start.elapsed();

        Some(BlockProductionResult {
            block,
            verify_time_ms: verify_time.as_millis() as u64,
            prove_time_ms: proof_time.as_millis() as u64,
            total_time_ms: total_time.as_millis() as u64,
            proof_size: proof_result.proof_size(),
            result_count,
            block_hash,
        })
    }

    /// Apply a block to our state
    pub fn apply_block(&mut self, block: Block) -> Result<(), String> {
        // Verify block proof
        let proof_valid = crate::prover::verify_proof(
            &block.proof,
            &block.header.proof_hash,
        );

        if !proof_valid {
            return Err("Invalid block proof".to_string());
        }

        // Apply to state
        self.state.apply_block(&block)
            .map_err(|e| e.to_string())?;

        self.chain.push(block);
        Ok(())
    }

    /// Vote for a block
    pub fn vote(&self, block: &Block) -> Option<Vote> {
        self.consensus.vote_for_block(block)
    }

    /// Process incoming vote
    pub fn process_vote(&mut self, vote: Vote) -> Option<FinalityCertificate> {
        let validator_count = self.state.active_validator_count();
        self.consensus.add_vote(vote, validator_count)
    }

    /// Run single block production cycle
    pub async fn run_cycle(&mut self) -> Option<BlockProductionResult> {
        if let Some(result) = self.produce_block() {
            if let Err(e) = self.apply_block(result.block.clone()) {
                tracing::error!("Failed to apply own block: {}", e);
                return None;
            }

            if let Some(vote) = self.vote(&result.block) {
                self.process_vote(vote);
            }

            return Some(result);
        }

        None
    }
}

/// Result of block production
#[derive(Debug)]
pub struct BlockProductionResult {
    /// The produced block
    pub block: Block,
    /// Verification time in ms
    pub verify_time_ms: u64,
    /// Proving time in ms
    pub prove_time_ms: u64,
    /// Total time in ms
    pub total_time_ms: u64,
    /// Proof size in bytes
    pub proof_size: usize,
    /// Number of work results
    pub result_count: usize,
    /// Block hash
    pub block_hash: Hash,
}

impl BlockProductionResult {
    /// Check if block was produced within budget
    pub fn within_budget(&self) -> bool {
        self.total_time_ms <= BLOCK_TIME_MS
    }
}

/// Simple block production loop for testing
pub async fn run_block_production_loop(
    mut node: Node,
    blocks: usize,
) -> Vec<BlockProductionResult> {
    let mut results = Vec::new();
    let mut ticker = interval(Duration::from_millis(BLOCK_TIME_MS));

    for _ in 0..blocks {
        ticker.tick().await;

        if let Some(result) = node.run_cycle().await {
            tracing::info!(
                "Block {} produced in {}ms (verify: {}ms, prove: {}ms, size: {} bytes, results: {})",
                result.block.header.height,
                result.total_time_ms,
                result.verify_time_ms,
                result.prove_time_ms,
                result.proof_size,
                result.result_count,
            );
            results.push(result);
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> NodeConfig {
        NodeConfig {
            validator_id: Some(0),
            signing_key: Some([0u8; 32]),
            ..Default::default()
        }
    }

    #[test]
    fn test_node_creation() {
        let node = Node::new(test_config());
        assert_eq!(node.height(), 0);
        assert_eq!(node.state().active_validator_count(), 3);
    }

    #[test]
    fn test_submit_work() {
        let mut node = Node::new(test_config());

        let package = WorkPackage {
            service: 0,
            payload: vec![1, 2, 3],
            gas_limit: 10000,
            proof: vec![],
            output_hash: [0xAB; 32],
            output: None,
            signature: [0; 64],
            submitter: [0; 32],
        };

        node.submit_work(package);
        assert_eq!(node.pending_work.len(), 1);
    }

    #[test]
    fn test_produce_empty_block() {
        let mut node = Node::new(test_config());

        let result = node.produce_block();
        assert!(result.is_some());

        let result = result.unwrap();
        assert_eq!(result.block.header.height, 1);
        assert_eq!(result.result_count, 0);
    }

    #[test]
    fn test_produce_block_with_work() {
        let mut node = Node::new(test_config());

        let package = WorkPackage {
            service: 0,
            payload: vec![1, 2, 3],
            gas_limit: 10000,
            proof: vec![],
            output_hash: [0xCD; 32],
            output: None,
            signature: [0; 64],
            submitter: [0; 32],
        };

        node.submit_work(package);
        let result = node.produce_block().unwrap();

        assert_eq!(result.result_count, 1);
        println!("Block produced in {}ms", result.total_time_ms);
        println!("  Verify: {}ms, Prove: {}ms", result.verify_time_ms, result.prove_time_ms);
        println!("  Proof size: {} bytes", result.proof_size);
    }

    #[test]
    fn test_any_validator_can_produce() {
        // Validator 0 can produce
        let config0 = NodeConfig {
            validator_id: Some(0),
            signing_key: Some([0u8; 32]),
            ..Default::default()
        };
        let mut node0 = Node::new(config0);
        assert!(node0.produce_block().is_some());

        // Validator 1 can also produce (no leader restriction)
        let config1 = NodeConfig {
            validator_id: Some(1),
            signing_key: Some([1u8; 32]),
            ..Default::default()
        };
        let mut node1 = Node::new(config1);
        assert!(node1.produce_block().is_some());

        // Validator 2 can also produce
        let config2 = NodeConfig {
            validator_id: Some(2),
            signing_key: Some([2u8; 32]),
            ..Default::default()
        };
        let mut node2 = Node::new(config2);
        assert!(node2.produce_block().is_some());
    }
}
