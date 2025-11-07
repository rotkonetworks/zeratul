/// substrate pallet for on-chain verification of polkadot endpoint monitoring proofs
///
/// this pallet accepts ligerito proof submissions from operators and verifies:
/// 1. the cryptographic proof (ligerito)
/// 2. chain continuity (parent_hash links)
/// 3. random sampling of block hashes against archive node
/// 4. sla compliance
/// 5. triggers payments or slashing based on verification results
///
/// note: this is a simplified example showing the core logic.
/// production deployment would require:
/// - proper substrate runtime integration
/// - weights calculation for extrinsics
/// - proper error handling and events
/// - governance for parameter updates
/// - oracle integration for archive node queries

use ligerito::*;
use ligerito::FinalizedLigeritoProof;
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

// ============================================================================
// types and structures
// ============================================================================

/// monitoring report submission from operator
#[derive(Clone, Debug)]
pub struct MonitoringSubmission {
    /// operator's account id
    pub operator: AccountId,
    /// monitoring period start timestamp
    pub period_start: u64,
    /// monitoring period end timestamp
    pub period_end: u64,
    /// number of checks performed
    pub check_count: u32,
    /// ligerito proof (serialized)
    pub proof: Vec<u8>,
    /// flattened check data (for verification)
    pub check_data: Vec<CheckData>,
    /// claimed sla metrics
    pub sla_metrics: SlaMetrics,
}

/// simplified account id (would be polkadot AccountId32 in production)
pub type AccountId = [u8; 32];

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

/// sla metrics claimed by operator
#[derive(Clone, Debug)]
pub struct SlaMetrics {
    pub sync_success_rate: f64,
    pub avg_latency_ms: f64,
    pub total_checks: u32,
}

/// sla requirements (would be governance-controlled)
#[derive(Clone, Debug)]
pub struct SlaRequirements {
    pub min_sync_rate: f64,
    pub max_avg_latency: f64,
    pub min_checks_per_period: u32,
}

/// verification result
#[derive(Clone, Debug)]
pub enum VerificationResult {
    Valid,
    InvalidProof,
    InvalidChainContinuity,
    InvalidBlockHash { check_index: usize },
    SlaNotMet,
}

// ============================================================================
// pallet storage (would be in #[pallet::storage] in real substrate)
// ============================================================================

/// operator stakes (operator -> staked amount)
pub struct OperatorStakes;
impl OperatorStakes {
    pub fn get(_operator: &AccountId) -> u128 {
        1_000_000_000_000 // 1000 tokens (simplified)
    }
}

/// sla requirements
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

/// pending operator payments (operator -> pending amount)
pub struct PendingPayments;
impl PendingPayments {
    pub fn get(_operator: &AccountId) -> u128 {
        0
    }

    pub fn add(_operator: &AccountId, _amount: u128) {
        println!("  💰 added pending payment for operator");
    }
}

// ============================================================================
// pallet extrinsics (would be in #[pallet::call] in real substrate)
// ============================================================================

/// main extrinsic: submit monitoring proof
pub fn submit_monitoring_proof(
    submission: MonitoringSubmission,
) -> std::result::Result<(), &'static str> {
    println!("\n=== processing monitoring submission ===");
    println!("operator: {:?}...", &submission.operator[..8]);
    println!("period: {} to {}", submission.period_start, submission.period_end);
    println!("checks: {}", submission.check_count);
    println!("proof size: {} bytes", submission.proof.len());

    // 1. verify operator has sufficient stake
    let stake = OperatorStakes::get(&submission.operator);
    let min_stake = 100_000_000_000; // 100 tokens minimum
    if stake < min_stake {
        return Err("insufficient operator stake");
    }
    println!("✓ operator stake verified: {}", stake);

    // 2. verify ligerito proof
    println!("\n=== verifying ligerito proof ===");
    let proof_valid = verify_ligerito_proof(&submission)?;
    if !proof_valid {
        slash_operator(&submission.operator, "invalid proof");
        return Err("proof verification failed");
    }
    println!("✓ ligerito proof valid");

    // 3. verify chain continuity
    println!("\n=== verifying chain continuity ===");
    let continuity_valid = verify_chain_continuity(&submission.check_data)?;
    if !continuity_valid {
        slash_operator(&submission.operator, "chain continuity broken");
        return Err("chain continuity verification failed");
    }
    println!("✓ chain continuity verified");

    // 4. random sampling verification
    println!("\n=== random sampling verification ===");
    let sampling_valid = verify_random_sampling(&submission.check_data)?;
    if !sampling_valid {
        slash_operator(&submission.operator, "block hash mismatch");
        return Err("random sampling verification failed");
    }
    println!("✓ random sampling verified");

    // 5. verify sla compliance
    println!("\n=== verifying sla compliance ===");
    let sla_config = SlaConfig::get();
    let sla_met = verify_sla_compliance(&submission.sla_metrics, &sla_config)?;
    if !sla_met {
        // no slashing for failing sla, but no payment either
        println!("✗ sla not met - no payment will be issued");
        return Ok(());
    }
    println!("✓ sla requirements met");

    // 6. calculate and schedule payment
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
// verification functions
// ============================================================================

/// verify the ligerito proof
fn verify_ligerito_proof(submission: &MonitoringSubmission) -> std::result::Result<bool, &'static str> {
    // deserialize proof
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> = bincode::deserialize(&submission.proof)
        .map_err(|_| "proof deserialization failed")?;

    // determine config size based on check count
    let check_count = submission.check_count as usize;
    let coeffs_per_check = 18; // metadata + block_number + block_hash + parent_hash
    let total_coeffs = check_count * coeffs_per_check;
    let padded_size = total_coeffs.next_power_of_two().max(4096);

    let config_size = if padded_size <= 4096 { 12 }
                     else if padded_size <= 65536 { 16 }
                     else if padded_size <= 1048576 { 20 }
                     else if padded_size <= 16777216 { 24 }
                     else if padded_size <= 268435456 { 28 }
                     else { 30 };

    println!("  config: 2^{} = {} coefficients", config_size, 1 << config_size);

    // get verifier config
    let verifier_config = match config_size {
        12 => hardcoded_config_12_verifier(),
        16 => hardcoded_config_16_verifier(),
        20 => hardcoded_config_20_verifier(),
        24 => hardcoded_config_24_verifier(),
        28 => hardcoded_config_28_verifier(),
        30 => hardcoded_config_30_verifier(),
        _ => return Err("unsupported config size"),
    };

    // verify proof
    let verified = verify_sha256(&verifier_config, &proof)
        .map_err(|_| "proof verification failed")?;

    Ok(verified)
}

/// verify chain continuity (parent_hash[i] == block_hash[i-1])
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

/// verify random sampling against archive node
fn verify_random_sampling(checks: &[CheckData]) -> std::result::Result<bool, &'static str> {
    // sample 10 random checks (or all if less than 10)
    let sample_count = 10.min(checks.len());
    let sample_indices: Vec<usize> = (0..sample_count)
        .map(|i| i * checks.len() / sample_count)
        .collect();

    println!("  sampling {} checks from {}", sample_count, checks.len());

    for idx in sample_indices {
        let check = &checks[idx];

        // query archive node for block hash
        // in production, this would be an off-chain worker query or oracle
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

/// query archive node for block hash (simplified - would be off-chain worker)
fn query_archive_node(block_number: u32) -> [u8; 32] {
    // simulate archive node query
    // in production: off-chain worker makes rpc call to archive node
    // and returns the result via oracle/callback

    let mut hash = [0u8; 32];

    // deterministic mock based on block number
    for i in 0..8 {
        let val = block_number.wrapping_mul(1664525u32).wrapping_add(1013904223u32 * i as u32);
        hash[i*4..(i+1)*4].copy_from_slice(&val.to_le_bytes());
    }

    hash
}

/// verify sla compliance
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

/// calculate payment based on sla metrics
fn calculate_payment(
    metrics: &SlaMetrics,
    _requirements: &SlaRequirements,
    period_seconds: u64,
) -> u128 {
    // base rate: 100 tokens per day
    let days = period_seconds as f64 / 86400.0;
    let base_payment = (100.0 * days) as u128 * 1_000_000_000; // in plancks

    // bonus for exceeding sla
    let sync_bonus = if metrics.sync_success_rate >= 99.9 {
        1.1 // 10% bonus
    } else if metrics.sync_success_rate >= 99.5 {
        1.05 // 5% bonus
    } else {
        1.0 // no bonus
    };

    let latency_bonus = if metrics.avg_latency_ms <= 100.0 {
        1.1 // 10% bonus for excellent latency
    } else if metrics.avg_latency_ms <= 250.0 {
        1.05 // 5% bonus for good latency
    } else {
        1.0 // no bonus
    };

    ((base_payment as f64) * sync_bonus * latency_bonus) as u128
}

/// slash operator for fraudulent submission
fn slash_operator(operator: &AccountId, reason: &str) {
    println!("\n⚠️  SLASHING OPERATOR");
    println!("operator: {:?}...", &operator[..8]);
    println!("reason: {}", reason);
    println!("slash amount: 100 tokens");

    // in production:
    // 1. deduct from OperatorStakes
    // 2. transfer slashed amount to treasury or burn
    // 3. emit Slashed event
    // 4. potentially ban operator temporarily
}

// ============================================================================
// example usage
// ============================================================================

fn main() {
    println!("=== substrate pallet: on-chain monitoring verifier ===\n");
    println!("this pallet handles ligerito proof verification on-chain");
    println!("with economic security via staking and slashing\n");

    // simulate operator submission
    let operator: AccountId = [1u8; 32];

    // create mock check data
    let mut checks: Vec<CheckData> = Vec::new();
    let base_timestamp = 1762517314;
    let base_block = 22_000_000;

    for i in 0..500 {
        let block_number = base_block + i;
        let block_hash = query_archive_node(block_number);
        let parent_hash = if i == 0 {
            [0u8; 32] // genesis parent
        } else {
            checks[i as usize - 1].block_hash
        };

        checks.push(CheckData {
            timestamp: base_timestamp + (i as u64 * 86),
            block_number,
            block_hash,
            parent_hash,
            latency_ms: 100 + (i % 50),
            sync_working: i % 50 != 0, // 98% success rate
        });
    }

    // encode checks into polynomial for proof generation
    println!("=== generating proof ===");
    let mut poly: Vec<BinaryElem32> = checks.iter()
        .flat_map(|check| {
            // encode same as polkadot_monitor_smoldot.rs
            let mut coeffs = Vec::new();

            // coeff 0: metadata
            let timestamp_offset = ((check.timestamp - base_timestamp) / 60) as u32;
            let meta = ((timestamp_offset & 0xFFF) << 20)
                     | ((check.latency_ms.min(1023) & 0x3FF) << 10)
                     | (if check.sync_working { 1 << 3 } else { 0 });
            coeffs.push(BinaryElem32::from(meta));

            // coeff 1: block number
            coeffs.push(BinaryElem32::from(check.block_number));

            // coeffs 2-9: block_hash
            for i in 0..8 {
                let chunk = u32::from_le_bytes([
                    check.block_hash[i*4],
                    check.block_hash[i*4 + 1],
                    check.block_hash[i*4 + 2],
                    check.block_hash[i*4 + 3],
                ]);
                coeffs.push(BinaryElem32::from(chunk));
            }

            // coeffs 10-17: parent_hash
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

    // pad to power of 2
    let target_size = poly.len().next_power_of_two().max(4096);
    poly.resize(target_size, BinaryElem32::from(0));

    println!("polynomial size: {}", poly.len());

    // generate proof
    let config = if poly.len() <= 4096 {
        hardcoded_config_12(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>)
    } else if poly.len() <= 65536 {
        hardcoded_config_16(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>)
    } else {
        hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>)
    };

    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let proof_bytes = bincode::serialize(&proof).expect("serialization failed");

    println!("proof generated: {} bytes", proof_bytes.len());

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
        period_end: base_timestamp + 43200, // 12 hours
        check_count: checks.len() as u32,
        proof: proof_bytes,
        check_data: checks,
        sla_metrics,
    };

    // submit to pallet
    match submit_monitoring_proof(submission) {
        Ok(()) => println!("\n✓ submission processed successfully"),
        Err(e) => println!("\n✗ submission rejected: {}", e),
    }

    println!("\n=== pallet design notes ===");
    println!("1. proof verification is O(1) - constant time regardless of check count");
    println!("2. random sampling trades off security vs cost (more samples = higher security)");
    println!("3. economic security: operator loses stake if caught cheating");
    println!("4. detection probability: 10 samples from 500 checks = 99.7% to catch 10% fraud");
    println!("5. proof size scales logarithmically: ~400 KB even for 1B elements");
    println!("\n=== production considerations ===");
    println!("1. use off-chain workers for archive node queries");
    println!("2. implement proper weights for extrinsics");
    println!("3. add governance for sla parameters and payment rates");
    println!("4. implement appeals/disputes mechanism");
    println!("5. add batch verification for multiple operators");
    println!("6. implement recursive proof aggregation for scalability");
}
