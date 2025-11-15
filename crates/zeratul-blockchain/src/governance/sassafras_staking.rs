//! SASSAFRAS-Based Anonymous Staking
//!
//! Uses anonymous tickets with delayed revelation to prevent privacy leaks.
//! Based on Polkadot's SASSAFRAS consensus.

use super::{AccountId, Balance, EraIndex};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Ticket identifier (VRF output)
pub type TicketId = u128;

/// Ephemeral public key (Ed25519)
pub type EphemeralPublic = [u8; 32];

/// Ephemeral signature (Ed25519)
pub type EphemeralSignature = [u8; 64];

/// Ring VRF signature (placeholder - will use sp-consensus-sassafras)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RingVrfSignature {
    /// VRF output
    pub output: [u8; 32],

    /// VRF proof
    pub proof: Vec<u8>,

    /// Ring proof (proves signer is in validator set)
    pub ring_proof: Vec<u8>,
}

/// Anonymous staking ticket
///
/// Submitted during era N, nobody knows who created it!
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingTicket {
    /// Ticket body
    pub body: StakingTicketBody,

    /// Ring VRF signature (anonymous)
    pub ring_signature: RingVrfSignature,
}

impl StakingTicket {
    /// Compute ticket ID from VRF output
    pub fn ticket_id(&self) -> TicketId {
        u128::from_le_bytes(self.ring_signature.output[0..16].try_into().unwrap())
    }
}

/// Staking ticket body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingTicketBody {
    /// Era when ticket is valid
    pub era: EraIndex,

    /// Attempt index (for anonymity - prevents linking)
    pub attempt_idx: u32,

    /// Encrypted stake amount
    pub encrypted_amount: EncryptedAmount,

    /// Validator commitment (hash of validator ID)
    pub validator_commitment: [u8; 32],

    /// Ephemeral public key (for revelation)
    pub ephemeral_public: EphemeralPublic,

    /// Erased public key (destroyed on claim)
    pub erased_public: EphemeralPublic,
}

/// Encrypted stake amount
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedAmount {
    /// Ciphertext
    pub ciphertext: Vec<u8>,

    /// Nonce
    pub nonce: [u8; 24],
}

impl EncryptedAmount {
    /// Create from plaintext (placeholder encryption)
    pub fn encrypt(amount: Balance, _key: &[u8; 32]) -> Self {
        // TODO: Actual encryption (ChaCha20 or AES-GCM)
        let ciphertext = amount.to_le_bytes().to_vec();

        Self {
            ciphertext,
            nonce: [0u8; 24],
        }
    }

    /// Decrypt (placeholder)
    pub fn decrypt(&self, _key: &[u8; 32]) -> Result<Balance> {
        // TODO: Actual decryption
        if self.ciphertext.len() >= 16 {
            let bytes: [u8; 16] = self.ciphertext[0..16].try_into()?;
            Ok(Balance::from_le_bytes(bytes))
        } else {
            bail!("Invalid ciphertext length");
        }
    }
}

/// Ticket claim (submitted at era transition)
///
/// Reveals validator identity to claim nomination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketClaim {
    /// Ticket ID being claimed
    pub ticket_id: TicketId,

    /// Validator claiming the ticket
    pub validator: AccountId,

    /// Signature proving ownership of erased key
    pub erased_signature: EphemeralSignature,

    /// Revealed stake amount (decrypted)
    pub revealed_amount: Balance,
}

impl TicketClaim {
    /// Verify claim matches ticket
    pub fn verify(&self, ticket: &StakingTicket) -> Result<bool> {
        // Check ticket ID matches
        if self.ticket_id != ticket.ticket_id() {
            return Ok(false);
        }

        // Check validator commitment
        let commitment = blake3::hash(self.validator.as_slice());
        if commitment.as_bytes() != &ticket.body.validator_commitment {
            return Ok(false);
        }

        // TODO: Verify erased_signature against ticket.body.erased_public
        // For now, just check signature is not empty
        if self.erased_signature == [0u8; 64] {
            return Ok(false);
        }

        Ok(true)
    }
}

/// Ticket pool for an era
///
/// Stores anonymous tickets and tracks claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketPool {
    /// Era number
    pub era: EraIndex,

    /// Submitted tickets (anonymous)
    pub tickets: BTreeMap<TicketId, StakingTicket>,

    /// Claimed tickets (revealed)
    pub claims: BTreeMap<TicketId, TicketClaim>,

    /// Ticket threshold (only accept tickets below this)
    pub threshold: TicketId,

    /// Era randomness (for VRF)
    pub randomness: [u8; 32],

    /// Maximum tickets per era
    pub max_tickets: usize,
}

impl TicketPool {
    /// Create new ticket pool
    pub fn new(era: EraIndex, randomness: [u8; 32]) -> Self {
        Self {
            era,
            tickets: BTreeMap::new(),
            claims: BTreeMap::new(),
            threshold: TicketId::MAX / 2, // Accept ~50% of tickets
            randomness,
            max_tickets: 10_000,
        }
    }

    /// Submit anonymous ticket
    pub fn submit_ticket(&mut self, ticket: StakingTicket) -> Result<()> {
        // Verify era matches
        if ticket.body.era != self.era {
            bail!("Ticket era mismatch: expected {}, got {}", self.era, ticket.body.era);
        }

        // Check max tickets
        if self.tickets.len() >= self.max_tickets {
            bail!("Maximum tickets reached");
        }

        // Verify Ring VRF signature
        self.verify_ring_vrf(&ticket.ring_signature)?;

        // Compute ticket ID
        let ticket_id = ticket.ticket_id();

        // Check threshold
        if ticket_id > self.threshold {
            bail!("Ticket ID {} exceeds threshold {}", ticket_id, self.threshold);
        }

        // Check not duplicate
        if self.tickets.contains_key(&ticket_id) {
            bail!("Duplicate ticket ID");
        }

        // Store ticket
        self.tickets.insert(ticket_id, ticket);

        tracing::debug!(
            "Submitted anonymous ticket {} for era {} ({} total)",
            ticket_id,
            self.era,
            self.tickets.len()
        );

        Ok(())
    }

    /// Claim ticket (reveal validator)
    pub fn claim_ticket(&mut self, claim: TicketClaim) -> Result<()> {
        // Get ticket
        let ticket = self
            .tickets
            .get(&claim.ticket_id)
            .ok_or_else(|| anyhow::anyhow!("Ticket {} not found", claim.ticket_id))?;

        // Verify claim
        if !claim.verify(ticket)? {
            bail!("Invalid ticket claim");
        }

        // Check not already claimed
        if self.claims.contains_key(&claim.ticket_id) {
            bail!("Ticket already claimed");
        }

        // Store claim
        self.claims.insert(claim.ticket_id, claim.clone());

        tracing::info!(
            "Validator {} claimed ticket {} (amount: {})",
            hex::encode(claim.validator),
            claim.ticket_id,
            claim.revealed_amount
        );

        Ok(())
    }

    /// Aggregate revealed backing per validator
    pub fn aggregate_backing(&self) -> BTreeMap<AccountId, Balance> {
        let mut backing: BTreeMap<AccountId, Balance> = BTreeMap::new();

        for claim in self.claims.values() {
            *backing.entry(claim.validator).or_insert(0) += claim.revealed_amount;
        }

        backing
    }

    /// Get total backing
    pub fn total_backing(&self) -> Balance {
        self.claims.values().map(|c| c.revealed_amount).sum()
    }

    /// Check if claim period is active
    pub fn is_claim_period(&self, current_era: EraIndex) -> bool {
        // Claims for era N can be submitted in era N+1
        current_era == self.era + 1
    }

    /// Verify Ring VRF signature (placeholder)
    fn verify_ring_vrf(&self, signature: &RingVrfSignature) -> Result<()> {
        // TODO: Actual Ring VRF verification using sp-consensus-sassafras
        // Should verify:
        // 1. VRF output matches proof
        // 2. Ring proof shows signer is in validator set
        // 3. No information about which validator

        if signature.proof.is_empty() {
            bail!("Empty VRF proof");
        }

        if signature.ring_proof.is_empty() {
            bail!("Empty ring proof");
        }

        Ok(())
    }

    /// Get ticket statistics
    pub fn stats(&self) -> TicketPoolStats {
        TicketPoolStats {
            era: self.era,
            tickets_submitted: self.tickets.len(),
            tickets_claimed: self.claims.len(),
            total_backing: self.total_backing(),
            unique_validators: self.claims.values().map(|c| c.validator).collect::<std::collections::BTreeSet<_>>().len(),
        }
    }
}

/// Ticket pool statistics
#[derive(Debug, Clone)]
pub struct TicketPoolStats {
    pub era: EraIndex,
    pub tickets_submitted: usize,
    pub tickets_claimed: usize,
    pub total_backing: Balance,
    pub unique_validators: usize,
}

/// Era transition with SASSAFRAS tickets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SassafrasEraTransition {
    /// From/to eras
    pub from_era: EraIndex,
    pub to_era: EraIndex,

    /// Ticket pool from era N
    pub ticket_pool: TicketPool,

    /// Aggregated backing per validator
    pub validator_backing: BTreeMap<AccountId, Balance>,

    /// Elected validators (top 15 by backing)
    pub elected_validators: Vec<AccountId>,

    /// FROST signature (11/15 validators)
    pub frost_signature: Option<[u8; 64]>,
}

impl SassafrasEraTransition {
    /// Create new era transition
    pub fn new(from_era: EraIndex, to_era: EraIndex, ticket_pool: TicketPool) -> Self {
        Self {
            from_era,
            to_era,
            ticket_pool,
            validator_backing: BTreeMap::new(),
            elected_validators: Vec::new(),
            frost_signature: None,
        }
    }

    /// Process ticket claims
    pub fn process_claims(&mut self, claims: Vec<TicketClaim>) -> Result<()> {
        for claim in claims {
            self.ticket_pool.claim_ticket(claim)?;
        }

        // Aggregate backing
        self.validator_backing = self.ticket_pool.aggregate_backing();

        tracing::info!(
            "Processed {} ticket claims for era transition {} → {}",
            self.ticket_pool.claims.len(),
            self.from_era,
            self.to_era
        );

        Ok(())
    }

    /// Run election (select top 15 validators by backing)
    pub fn run_election(&mut self, validator_count: usize) -> Result<()> {
        // Sort validators by backing (descending)
        let mut candidates: Vec<(AccountId, Balance)> =
            self.validator_backing.iter().map(|(v, b)| (*v, *b)).collect();

        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        // Select top N
        self.elected_validators = candidates
            .iter()
            .take(validator_count)
            .map(|(v, _)| *v)
            .collect();

        if self.elected_validators.len() < validator_count {
            bail!(
                "Insufficient validators: got {}, need {}",
                self.elected_validators.len(),
                validator_count
            );
        }

        tracing::info!(
            "Elected {} validators for era {} (total backing: {})",
            self.elected_validators.len(),
            self.to_era,
            self.validator_backing.values().sum::<Balance>()
        );

        Ok(())
    }

    /// Add FROST signature (11/15 validators)
    pub fn frost_sign(&mut self, signature: [u8; 64]) -> Result<()> {
        self.frost_signature = Some(signature);
        Ok(())
    }

    /// Verify transition is valid
    pub fn verify(&self) -> Result<bool> {
        // Check FROST signature present
        if self.frost_signature.is_none() {
            return Ok(false);
        }

        // Check era progression
        if self.to_era != self.from_era + 1 {
            return Ok(false);
        }

        // Check elected validators
        if self.elected_validators.is_empty() {
            return Ok(false);
        }

        // Check all elected validators have backing
        for validator in &self.elected_validators {
            if !self.validator_backing.contains_key(validator) {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_account(id: u8) -> AccountId {
        let mut account = [0u8; 32];
        account[0] = id;
        account
    }

    fn create_test_ticket(era: EraIndex, validator: AccountId, amount: Balance) -> StakingTicket {
        let validator_commitment = blake3::hash(validator.as_slice());

        StakingTicket {
            body: StakingTicketBody {
                era,
                attempt_idx: 0,
                encrypted_amount: EncryptedAmount::encrypt(amount, &[0u8; 32]),
                validator_commitment: *validator_commitment.as_bytes(),
                ephemeral_public: [1u8; 32],
                erased_public: [2u8; 32],
            },
            ring_signature: RingVrfSignature {
                output: {
                    let mut output = [0u8; 32];
                    output[0] = validator[0]; // Make unique per validator
                    output
                },
                proof: vec![1, 2, 3],
                ring_proof: vec![4, 5, 6],
            },
        }
    }

    #[test]
    fn test_ticket_submission() {
        let mut pool = TicketPool::new(1, [0u8; 32]);
        let validator = create_test_account(1);
        let ticket = create_test_ticket(1, validator, 1000 * 10u128.pow(18));

        pool.submit_ticket(ticket).unwrap();

        assert_eq!(pool.tickets.len(), 1);
        assert_eq!(pool.claims.len(), 0);
    }

    #[test]
    fn test_ticket_claim() {
        let mut pool = TicketPool::new(1, [0u8; 32]);
        let validator = create_test_account(1);
        let ticket = create_test_ticket(1, validator, 1000 * 10u128.pow(18));

        pool.submit_ticket(ticket.clone()).unwrap();

        let claim = TicketClaim {
            ticket_id: ticket.ticket_id(),
            validator,
            erased_signature: [1u8; 64],
            revealed_amount: 1000 * 10u128.pow(18),
        };

        pool.claim_ticket(claim).unwrap();

        assert_eq!(pool.claims.len(), 1);
    }

    #[test]
    fn test_aggregate_backing() {
        let mut pool = TicketPool::new(1, [0u8; 32]);

        // Three tickets for validator 1
        let v1 = create_test_account(1);
        for i in 0..3 {
            let mut ticket = create_test_ticket(1, v1, 100 * 10u128.pow(18));
            ticket.ring_signature.output[1] = i; // Make unique
            pool.submit_ticket(ticket.clone()).unwrap();

            let claim = TicketClaim {
                ticket_id: ticket.ticket_id(),
                validator: v1,
                erased_signature: [1u8; 64],
                revealed_amount: 100 * 10u128.pow(18),
            };
            pool.claim_ticket(claim).unwrap();
        }

        // Two tickets for validator 2
        let v2 = create_test_account(2);
        for i in 0..2 {
            let mut ticket = create_test_ticket(1, v2, 200 * 10u128.pow(18));
            ticket.ring_signature.output[1] = i + 10; // Make unique
            pool.submit_ticket(ticket.clone()).unwrap();

            let claim = TicketClaim {
                ticket_id: ticket.ticket_id(),
                validator: v2,
                erased_signature: [1u8; 64],
                revealed_amount: 200 * 10u128.pow(18),
            };
            pool.claim_ticket(claim).unwrap();
        }

        let backing = pool.aggregate_backing();

        assert_eq!(backing.len(), 2);
        assert_eq!(backing.get(&v1).unwrap(), &(300 * 10u128.pow(18)));
        assert_eq!(backing.get(&v2).unwrap(), &(400 * 10u128.pow(18)));
    }

    #[test]
    fn test_era_transition() {
        let mut pool = TicketPool::new(1, [0u8; 32]);

        // Submit tickets for 5 validators
        let mut claims = Vec::new();
        for i in 0..5 {
            let validator = create_test_account(i);
            let amount = (i as u128 + 1) * 1000 * 10u128.pow(18);

            let mut ticket = create_test_ticket(1, validator, amount);
            ticket.ring_signature.output[1] = i; // Make unique
            pool.submit_ticket(ticket.clone()).unwrap();

            claims.push(TicketClaim {
                ticket_id: ticket.ticket_id(),
                validator,
                erased_signature: [1u8; 64],
                revealed_amount: amount,
            });
        }

        let mut transition = SassafrasEraTransition::new(1, 2, pool);
        transition.process_claims(claims).unwrap();
        transition.run_election(3).unwrap();

        // Top 3 validators should be elected
        assert_eq!(transition.elected_validators.len(), 3);

        // Check they're sorted by backing (descending)
        let v5 = create_test_account(4);
        let v4 = create_test_account(3);
        let v3 = create_test_account(2);

        assert_eq!(transition.elected_validators[0], v5); // 5000 ZT
        assert_eq!(transition.elected_validators[1], v4); // 4000 ZT
        assert_eq!(transition.elected_validators[2], v3); // 3000 ZT
    }

    #[test]
    fn test_invalid_claim() {
        let mut pool = TicketPool::new(1, [0u8; 32]);
        let validator = create_test_account(1);
        let ticket = create_test_ticket(1, validator, 1000 * 10u128.pow(18));

        pool.submit_ticket(ticket.clone()).unwrap();

        // Wrong validator
        let wrong_validator = create_test_account(2);
        let claim = TicketClaim {
            ticket_id: ticket.ticket_id(),
            validator: wrong_validator,
            erased_signature: [1u8; 64],
            revealed_amount: 1000 * 10u128.pow(18),
        };

        let result = pool.claim_ticket(claim);
        assert!(result.is_err());
    }
}
