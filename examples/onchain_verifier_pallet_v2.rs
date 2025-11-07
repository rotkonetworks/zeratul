/// substrate pallet for on-chain verification of polkadot endpoint monitoring proofs
///
/// design goals:
/// - clean separation between data availability layer and verification logic
/// - easy migration path from on-chain storage to ZODA
/// - future-proof architecture for scaling to mainnet
///
/// v2: adds abstraction layer for data availability
///   - current: stores check data on-chain (simple, works for new parachain)
///   - future: can migrate to ZODA for off-chain DA with minimal code changes

use ligerito::*;
use ligerito::FinalizedLigeritoProof;
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

// ============================================================================
// data availability abstraction
// ============================================================================

/// commitment to monitoring data
///
/// v1 (current): simple hash of serialized check data
/// v2 (future): ZODA commitment (32 bytes) + metadata
#[derive(Clone, Debug, PartialEq)]
pub enum DataCommitment {
    /// on-chain: full data stored in substrate storage
    OnChain {
        /// hash of the check data for integrity
        data_hash: [u8; 32],
        /// size in bytes
        data_size: usize,
    },
    /// off-chain: ZODA commitment for future scalability
    #[allow(dead_code)]
    Zoda {
        /// ZODA commitment (from commonware)
        commitment: [u8; 32],
        /// size in bytes
        data_size: usize,
        /// number of shards
        shard_count: u16,
        /// minimum shards needed for reconstruction
        min_shards: u16,
    },
}

/// individual check data point
#[derive(Clone, Debug)]
pub struct CheckData {
    pub timestamp: u64,
    pub block_number: u32,
    pub block_hash: [u8; 32],
    pub parent_hash: [u8; 32],
    pub latency_ms: u32,
    pub sync_working: bool,
}

/// trait for data availability backends
///
/// this abstraction allows swapping between:
/// - on-chain storage (current)
/// - ZODA off-chain DA (future)
/// - other DA solutions
pub trait DataAvailability {
    /// store check data and return commitment
    fn store(&mut self, checks: Vec<CheckData>) -> DataCommitment;

    /// retrieve specific checks by indices (for sampling)
    fn get_checks(&self, commitment: &DataCommitment, indices: &[usize]) -> std::result::Result<Vec<CheckData>, &'static str>;

    /// retrieve all checks (for full reconstruction)
    fn get_all_checks(&self, commitment: &DataCommitment) -> std::result::Result<Vec<CheckData>, &'static str>;

    /// verify data is available and reconstructible
    fn verify_availability(&self, commitment: &DataCommitment) -> bool;
}

// ============================================================================
// implementation: on-chain storage (v1)
// ============================================================================

/// on-chain data availability (current implementation)
///
/// stores full check data in substrate storage
/// simple, works well for new parachains with limited operators
///
/// migration note: when moving to ZODA, just swap this with ZodaDataAvailability
pub struct OnChainDA {
    /// map from data_hash -> full check data
    storage: std::collections::HashMap<[u8; 32], Vec<CheckData>>,
}

impl OnChainDA {
    pub fn new() -> Self {
        Self {
            storage: std::collections::HashMap::new(),
        }
    }

    fn hash_checks(checks: &[CheckData]) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();

        for check in checks {
            hasher.update(check.timestamp.to_le_bytes());
            hasher.update(check.block_number.to_le_bytes());
            hasher.update(check.block_hash);
            hasher.update(check.parent_hash);
            hasher.update(check.latency_ms.to_le_bytes());
            hasher.update([check.sync_working as u8]);
        }

        hasher.finalize().into()
    }
}

impl DataAvailability for OnChainDA {
    fn store(&mut self, checks: Vec<CheckData>) -> DataCommitment {
        let data_hash = Self::hash_checks(&checks);
        let data_size = checks.len() * std::mem::size_of::<CheckData>();
        self.storage.insert(data_hash, checks);

        DataCommitment::OnChain {
            data_hash,
            data_size,
        }
    }

    fn get_checks(&self, commitment: &DataCommitment, indices: &[usize]) -> std::result::Result<Vec<CheckData>, &'static str> {
        match commitment {
            DataCommitment::OnChain { data_hash, .. } => {
                let all_checks = self.storage.get(data_hash)
                    .ok_or("data not found in storage")?;

                Ok(indices.iter()
                    .filter_map(|&i| all_checks.get(i).cloned())
                    .collect())
            }
            DataCommitment::Zoda { .. } => Err("ZODA not implemented yet"),
        }
    }

    fn get_all_checks(&self, commitment: &DataCommitment) -> std::result::Result<Vec<CheckData>, &'static str> {
        match commitment {
            DataCommitment::OnChain { data_hash, .. } => {
                self.storage.get(data_hash)
                    .cloned()
                    .ok_or("data not found in storage")
            }
            DataCommitment::Zoda { .. } => Err("ZODA not implemented yet"),
        }
    }

    fn verify_availability(&self, commitment: &DataCommitment) -> bool {
        match commitment {
            DataCommitment::OnChain { data_hash, .. } => {
                self.storage.contains_key(data_hash)
            }
            DataCommitment::Zoda { .. } => false, // not implemented
        }
    }
}

// ============================================================================
// placeholder: ZODA implementation (v2 - future)
// ============================================================================

/// ZODA data availability (future implementation)
///
/// this is a placeholder showing how ZODA would integrate
/// when ready to deploy at scale, implement this with commonware's ZODA
#[allow(dead_code)]
pub struct ZodaDA {
    /// ZODA encoder/decoder
    /// would use commonware_reed_solomon::zoda::Zoda<Sha256>
    _zoda: (),
    /// local shard storage (this validator's shards)
    _local_shards: std::collections::HashMap<[u8; 32], Vec<u8>>,
    /// peer discovery for requesting shards
    _peers: Vec<[u8; 32]>,
}

#[allow(dead_code)]
impl ZodaDA {
    pub fn new() -> Self {
        Self {
            _zoda: (),
            _local_shards: std::collections::HashMap::new(),
            _peers: Vec::new(),
        }
    }
}

// future implementation would look like:
// impl DataAvailability for ZodaDA {
//     fn store(&mut self, checks: Vec<CheckData>) -> DataCommitment {
//         // 1. serialize checks
//         // 2. encode with ZODA
//         // 3. distribute shards to validators via gossip
//         // 4. store local shard
//         // 5. return ZODA commitment
//     }
//
//     fn get_checks(&self, commitment: &DataCommitment, indices: &[usize]) -> std::result::Result<Vec<CheckData>, &'static str> {
//         // 1. request needed shards from peers
//         // 2. reconstruct full data (only if we have min_shards)
//         // 3. deserialize and return requested indices
//     }
// }

// ============================================================================
// pallet types
// ============================================================================

pub type AccountId = [u8; 32];

/// monitoring report submission from operator
#[derive(Clone, Debug)]
pub struct MonitoringSubmission {
    pub operator: AccountId,
    pub period_start: u64,
    pub period_end: u64,
    pub check_count: u32,
    /// ligerito proof (always ~150KB regardless of DA backend)
    pub proof: Vec<u8>,
    /// data commitment (on-chain or ZODA)
    pub data_commitment: DataCommitment,
    /// claimed sla metrics
    pub sla_metrics: SlaMetrics,
}

#[derive(Clone, Debug)]
pub struct SlaMetrics {
    pub sync_success_rate: f64,
    pub avg_latency_ms: f64,
    pub total_checks: u32,
}

#[derive(Clone, Debug)]
pub struct SlaRequirements {
    pub min_sync_rate: f64,
    pub max_avg_latency: f64,
    pub min_checks_per_period: u32,
}

// ============================================================================
// pallet storage
// ============================================================================

pub struct OperatorStakes;
impl OperatorStakes {
    pub fn get(_operator: &AccountId) -> u128 {
        1_000_000_000_000 // 1000 tokens
    }
}

pub struct SlaConfig;
impl SlaConfig {
    pub fn get() -> SlaRequirements {
        SlaRequirements {
            min_sync_rate: 99.0,
            max_avg_latency: 500.0,
            min_checks_per_period: 100,
        }
    }
}

pub struct PendingPayments;
impl PendingPayments {
    pub fn add(_operator: &AccountId, _amount: u128) {
        println!("  💰 added pending payment for operator");
    }
}

// ============================================================================
// pallet extrinsics
// ============================================================================

/// main extrinsic: submit monitoring proof
///
/// note: verification logic is independent of DA backend
/// works the same whether using on-chain or ZODA storage
pub fn submit_monitoring_proof<DA: DataAvailability>(
    da: &DA,
    submission: MonitoringSubmission,
) -> std::result::Result<(), &'static str> {
    println!("\n=== processing monitoring submission ===");
    println!("operator: {:?}...", &submission.operator[..8]);
    println!("period: {} to {}", submission.period_start, submission.period_end);
    println!("checks: {}", submission.check_count);
    println!("proof size: {} bytes", submission.proof.len());
    println!("da backend: {}", match submission.data_commitment {
        DataCommitment::OnChain { .. } => "on-chain storage",
        DataCommitment::Zoda { .. } => "ZODA (off-chain)",
    });

    // 1. verify operator has sufficient stake
    let stake = OperatorStakes::get(&submission.operator);
    let min_stake = 100_000_000_000;
    if stake < min_stake {
        return Err("insufficient operator stake");
    }
    println!("✓ operator stake verified: {}", stake);

    // 2. verify data availability
    println!("\n=== verifying data availability ===");
    if !da.verify_availability(&submission.data_commitment) {
        return Err("data not available");
    }
    println!("✓ data availability verified");

    // 3. verify ligerito proof
    println!("\n=== verifying ligerito proof ===");
    let proof_valid = verify_ligerito_proof(&submission)?;
    if !proof_valid {
        slash_operator(&submission.operator, "invalid proof");
        return Err("proof verification failed");
    }
    println!("✓ ligerito proof valid");

    // 4. verify chain continuity
    println!("\n=== verifying chain continuity ===");
    let all_checks = da.get_all_checks(&submission.data_commitment)?;
    let continuity_valid = verify_chain_continuity(&all_checks)?;
    if !continuity_valid {
        slash_operator(&submission.operator, "chain continuity broken");
        return Err("chain continuity verification failed");
    }
    println!("✓ chain continuity verified");

    // 5. random sampling verification
    println!("\n=== random sampling verification ===");
    let sampling_valid = verify_random_sampling(da, &submission.data_commitment)?;
    if !sampling_valid {
        slash_operator(&submission.operator, "block hash mismatch");
        return Err("random sampling verification failed");
    }
    println!("✓ random sampling verified");

    // 6. verify sla compliance
    println!("\n=== verifying sla compliance ===");
    let sla_config = SlaConfig::get();
    let sla_met = verify_sla_compliance(&submission.sla_metrics, &sla_config)?;
    if !sla_met {
        println!("✗ sla not met - no payment will be issued");
        return Ok(());
    }
    println!("✓ sla requirements met");

    // 7. calculate and schedule payment
    let payment_amount = calculate_payment(
        &submission.sla_metrics,
        &sla_config,
        submission.period_end - submission.period_start,
    );
    PendingPayments::add(&submission.operator, payment_amount);

    println!("\n=== submission accepted ===");
    println!("payment scheduled: {} tokens", payment_amount);

    Ok(())
}

// ============================================================================
// verification functions (DA-agnostic)
// ============================================================================

fn verify_ligerito_proof(submission: &MonitoringSubmission) -> std::result::Result<bool, &'static str> {
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> = bincode::deserialize(&submission.proof)
        .map_err(|_| "proof deserialization failed")?;

    let check_count = submission.check_count as usize;
    let coeffs_per_check = 18;
    let total_coeffs = check_count * coeffs_per_check;
    let padded_size = total_coeffs.next_power_of_two().max(4096);

    let config_size = if padded_size <= 4096 { 12 }
                     else if padded_size <= 65536 { 16 }
                     else if padded_size <= 1048576 { 20 }
                     else if padded_size <= 16777216 { 24 }
                     else if padded_size <= 268435456 { 28 }
                     else { 30 };

    println!("  config: 2^{} = {} coefficients", config_size, 1 << config_size);

    let verifier_config = match config_size {
        12 => hardcoded_config_12_verifier(),
        16 => hardcoded_config_16_verifier(),
        20 => hardcoded_config_20_verifier(),
        24 => hardcoded_config_24_verifier(),
        28 => hardcoded_config_28_verifier(),
        30 => hardcoded_config_30_verifier(),
        _ => return Err("unsupported config size"),
    };

    let verified = verify_sha256(&verifier_config, &proof)
        .map_err(|_| "proof verification failed")?;

    Ok(verified)
}

fn verify_chain_continuity(checks: &[CheckData]) -> std::result::Result<bool, &'static str> {
    for i in 1..checks.len() {
        if checks[i].parent_hash != checks[i-1].block_hash {
            println!("  ✗ chain broken at check {}", i);
            println!("    expected parent: {:?}...", &checks[i-1].block_hash[..8]);
            println!("    actual parent:   {:?}...", &checks[i].parent_hash[..8]);
            return Ok(false);
        }
    }

    println!("  checked {} chain links", checks.len() - 1);
    Ok(true)
}

fn verify_random_sampling<DA: DataAvailability>(
    da: &DA,
    commitment: &DataCommitment,
) -> std::result::Result<bool, &'static str> {
    let all_checks = da.get_all_checks(commitment)?;
    let sample_count = 10.min(all_checks.len());
    let sample_indices: Vec<usize> = (0..sample_count)
        .map(|i| i * all_checks.len() / sample_count)
        .collect();

    println!("  sampling {} checks from {}", sample_count, all_checks.len());

    // in production, this would use DA layer to fetch only sampled indices
    // let sampled_checks = da.get_checks(commitment, &sample_indices)?;

    for idx in sample_indices {
        let check = &all_checks[idx];
        let onchain_hash = query_archive_node(check.block_number);

        if check.block_hash != onchain_hash {
            println!("  ✗ block hash mismatch at check {}", idx);
            println!("    operator claimed: {:?}...", &check.block_hash[..8]);
            println!("    archive node has: {:?}...", &onchain_hash[..8]);
            return Ok(false);
        }
    }

    println!("  all sampled hashes match archive node");
    Ok(true)
}

fn query_archive_node(block_number: u32) -> [u8; 32] {
    let mut hash = [0u8; 32];
    for i in 0..8 {
        let val = block_number.wrapping_mul(1664525u32).wrapping_add(1013904223u32 * i as u32);
        hash[i*4..(i+1)*4].copy_from_slice(&val.to_le_bytes());
    }
    hash
}

fn verify_sla_compliance(
    metrics: &SlaMetrics,
    requirements: &SlaRequirements,
) -> std::result::Result<bool, &'static str> {
    println!("  sync rate: {:.2}% (min: {:.2}%)", metrics.sync_success_rate, requirements.min_sync_rate);
    println!("  avg latency: {:.2}ms (max: {:.2}ms)", metrics.avg_latency_ms, requirements.max_avg_latency);
    println!("  total checks: {} (min: {})", metrics.total_checks, requirements.min_checks_per_period);

    let meets_sync = metrics.sync_success_rate >= requirements.min_sync_rate;
    let meets_latency = metrics.avg_latency_ms <= requirements.max_avg_latency;
    let meets_count = metrics.total_checks >= requirements.min_checks_per_period;

    Ok(meets_sync && meets_latency && meets_count)
}

fn calculate_payment(
    metrics: &SlaMetrics,
    _requirements: &SlaRequirements,
    period_seconds: u64,
) -> u128 {
    let days = period_seconds as f64 / 86400.0;
    let base_payment = (100.0 * days) as u128 * 1_000_000_000;

    let sync_bonus = if metrics.sync_success_rate >= 99.9 { 1.1 }
                     else if metrics.sync_success_rate >= 99.5 { 1.05 }
                     else { 1.0 };

    let latency_bonus = if metrics.avg_latency_ms <= 100.0 { 1.1 }
                        else if metrics.avg_latency_ms <= 250.0 { 1.05 }
                        else { 1.0 };

    ((base_payment as f64) * sync_bonus * latency_bonus) as u128
}

fn slash_operator(operator: &AccountId, reason: &str) {
    println!("\n⚠️  SLASHING OPERATOR");
    println!("operator: {:?}...", &operator[..8]);
    println!("reason: {}", reason);
    println!("slash amount: 100 tokens");
}

// ============================================================================
// example usage
// ============================================================================

fn main() {
    println!("=== substrate pallet v2: data availability abstraction ===\n");
    println!("this version adds clean separation between:");
    println!("  - ligerito proof verification");
    println!("  - data availability layer (on-chain or ZODA)");
    println!("  - verification logic\n");

    println!("migration path to ZODA:");
    println!("  1. implement DataAvailability trait for ZodaDA");
    println!("  2. swap OnChainDA with ZodaDA");
    println!("  3. verification logic remains unchanged");
    println!("  4. savings: move ~50KB check data off-chain per submission\n");

    // ========================================================================
    // example 1: on-chain DA (current)
    // ========================================================================

    println!("\n=== EXAMPLE 1: ON-CHAIN DATA AVAILABILITY (current) ===\n");

    let mut on_chain_da = OnChainDA::new();
    let operator: AccountId = [1u8; 32];

    // create mock check data
    let mut checks: Vec<CheckData> = Vec::new();
    let base_timestamp = 1762517314;
    let base_block = 22_000_000;

    for i in 0..500 {
        let block_number = base_block + i;
        let block_hash = query_archive_node(block_number);
        let parent_hash = if i == 0 {
            [0u8; 32]
        } else {
            checks[i as usize - 1].block_hash
        };

        checks.push(CheckData {
            timestamp: base_timestamp + (i as u64 * 86),
            block_number,
            block_hash,
            parent_hash,
            latency_ms: 150 + (i % 50),
            sync_working: i % 50 != 0, // 98% success rate
        });
    }

    // store data and get commitment
    println!("storing check data on-chain...");
    let data_commitment = on_chain_da.store(checks.clone());
    println!("data commitment: {:?}", data_commitment);

    // generate proof (same for both DA backends)
    println!("\ngenerating ligerito proof...");
    let mut poly: Vec<BinaryElem32> = checks.iter()
        .flat_map(|check| {
            let mut coeffs = Vec::new();
            let timestamp_offset = ((check.timestamp - base_timestamp) / 60) as u32;
            let meta = ((timestamp_offset & 0xFFF) << 20)
                     | ((check.latency_ms.min(1023) & 0x3FF) << 10)
                     | (if check.sync_working { 1 << 3 } else { 0 });
            coeffs.push(BinaryElem32::from(meta));
            coeffs.push(BinaryElem32::from(check.block_number));
            for i in 0..8 {
                let chunk = u32::from_le_bytes([
                    check.block_hash[i*4],
                    check.block_hash[i*4 + 1],
                    check.block_hash[i*4 + 2],
                    check.block_hash[i*4 + 3],
                ]);
                coeffs.push(BinaryElem32::from(chunk));
            }
            for i in 0..8 {
                let chunk = u32::from_le_bytes([
                    check.parent_hash[i*4],
                    check.parent_hash[i*4 + 1],
                    check.parent_hash[i*4 + 2],
                    check.parent_hash[i*4 + 3],
                ]);
                coeffs.push(BinaryElem32::from(chunk));
            }
            coeffs
        })
        .collect();

    let target_size = poly.len().next_power_of_two().max(4096);
    poly.resize(target_size, BinaryElem32::from(0));

    let config = if poly.len() <= 4096 {
        hardcoded_config_12(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>)
    } else if poly.len() <= 65536 {
        hardcoded_config_16(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>)
    } else {
        hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>)
    };

    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let proof_bytes = bincode::serialize(&proof).expect("serialization failed");
    println!("proof size: {} bytes", proof_bytes.len());

    // calculate sla metrics
    let successful_syncs = checks.iter().filter(|c| c.sync_working).count();
    let total_latency: u32 = checks.iter()
        .filter(|c| c.sync_working)
        .map(|c| c.latency_ms)
        .sum();

    let sla_metrics = SlaMetrics {
        sync_success_rate: (successful_syncs as f64 / checks.len() as f64) * 100.0,
        avg_latency_ms: total_latency as f64 / successful_syncs as f64,
        total_checks: checks.len() as u32,
    };

    // create submission
    let submission = MonitoringSubmission {
        operator,
        period_start: base_timestamp,
        period_end: base_timestamp + 43200,
        check_count: checks.len() as u32,
        proof: proof_bytes,
        data_commitment,
        sla_metrics,
    };

    // submit to pallet
    match submit_monitoring_proof(&on_chain_da, submission) {
        Ok(()) => println!("\n✓ submission processed successfully"),
        Err(e) => println!("\n✗ submission rejected: {}", e),
    }

    // ========================================================================
    // cost analysis
    // ========================================================================

    println!("\n=== COST ANALYSIS ===");
    println!("\non-chain storage (current):");
    println!("  - proof: 150 KB");
    println!("  - check data: ~50 KB (500 checks × 100 bytes)");
    println!("  - commitment: 32 bytes");
    println!("  - TOTAL: ~200 KB per submission");
    println!("  - stored by: all validators");

    println!("\nwith ZODA (future):");
    println!("  - proof: 150 KB (on-chain)");
    println!("  - commitment: 32 bytes (on-chain)");
    println!("  - check data: off-chain, sharded across validators");
    println!("  - TOTAL on-chain: ~150 KB per submission");
    println!("  - savings: 50 KB × number of submissions");
    println!("  - each validator stores: their shard(s) only (~10-20 KB)");

    println!("\nfor 1000 submissions/day:");
    println!("  - on-chain savings: 50 MB/day");
    println!("  - yearly: 18 GB saved from chain state");

    println!("\n=== MIGRATION CHECKLIST ===");
    println!("□ implement ZodaDA struct with commonware_reed_solomon");
    println!("□ implement DataAvailability trait for ZodaDA");
    println!("□ add p2p layer for shard dissemination");
    println!("□ add shard request/response protocol");
    println!("□ update MonitoringSubmission to use ZODA commitment");
    println!("□ test with real validator network");
    println!("□ benchmark reconstruction latency");
    println!("□ deploy with feature flag for gradual rollout");
}
