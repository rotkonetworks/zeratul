//! osst contribution collection and aggregation
//!
//! collects threshold contributions from members and combines them
//! into a final threshold signature. any member can aggregate.
//!
//! # decentralized aggregation
//!
//! no designated aggregator - any member can collect contributions
//! from the relay and aggregate when threshold is reached.
//!
//! ```text
//! member 1 ──┐
//! member 2 ──┼──▶ contributions via relay ──▶ aggregator ──▶ signature
//! member 3 ──┘
//! ```

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::wire::{Hash32, ShareId, ProposalId, SignedContribution};

/// contribution collector
///
/// collects OSST contributions for a proposal until threshold is met
#[derive(Clone, Debug)]
pub struct ContributionCollector {
    /// proposal id
    proposal_id: ProposalId,
    /// required threshold (number of shares)
    threshold: u8,
    /// collected contributions: share_id -> contribution
    contributions: BTreeMap<ShareId, CollectedContribution>,
    /// seen contributors (to prevent duplicate processing)
    seen_contributors: Vec<Hash32>,
}

/// individual collected contribution
#[derive(Clone, Debug)]
pub struct CollectedContribution {
    /// contributor pubkey
    pub contributor: Hash32,
    /// osst contribution data (R, z)
    pub osst_data: Vec<u8>,
}

/// aggregation result
#[derive(Clone, Debug)]
pub enum AggregationResult {
    /// need more contributions
    NeedMore {
        collected: u8,
        required: u8,
    },
    /// ready to aggregate
    Ready,
    /// aggregation complete
    Complete {
        /// combined signature data
        signature: Vec<u8>,
    },
}

impl ContributionCollector {
    /// create collector for a proposal
    pub fn new(proposal_id: ProposalId, threshold: u8) -> Self {
        Self {
            proposal_id,
            threshold,
            contributions: BTreeMap::new(),
            seen_contributors: Vec::new(),
        }
    }

    /// get proposal id
    pub fn proposal_id(&self) -> ProposalId {
        self.proposal_id
    }

    /// count collected shares
    pub fn collected_count(&self) -> u8 {
        self.contributions.len() as u8
    }

    /// check if ready to aggregate
    pub fn is_ready(&self) -> bool {
        self.collected_count() >= self.threshold
    }

    /// add signed contribution
    ///
    /// returns Ok(true) if contribution was new and valid
    pub fn add_contribution(
        &mut self,
        signed: &SignedContribution,
    ) -> Result<bool, AggregationError> {
        // check proposal matches
        if signed.contribution.proposal_id != self.proposal_id {
            return Err(AggregationError::WrongProposal {
                expected: self.proposal_id,
                got: signed.contribution.proposal_id,
            });
        }

        // check contributor not already seen
        if self.seen_contributors.contains(&signed.contributor_pubkey) {
            return Ok(false); // duplicate, but not an error
        }
        self.seen_contributors.push(signed.contributor_pubkey);

        // add each share's contribution
        for &share_id in &signed.contribution.share_ids {
            // check share not already contributed
            if self.contributions.contains_key(&share_id) {
                continue; // skip duplicate shares
            }

            self.contributions.insert(
                share_id,
                CollectedContribution {
                    contributor: signed.contributor_pubkey,
                    osst_data: signed.contribution.osst_data.clone(),
                },
            );
        }

        Ok(true)
    }

    /// get current status
    pub fn status(&self) -> AggregationResult {
        if self.is_ready() {
            AggregationResult::Ready
        } else {
            AggregationResult::NeedMore {
                collected: self.collected_count(),
                required: self.threshold,
            }
        }
    }

    /// get share ids with contributions
    pub fn contributing_shares(&self) -> Vec<ShareId> {
        self.contributions.keys().copied().collect()
    }

    /// get all contributions for aggregation
    pub fn contributions(&self) -> &BTreeMap<ShareId, CollectedContribution> {
        &self.contributions
    }

    /// aggregate contributions into final signature
    ///
    /// this is a simplified aggregation - real impl uses OSST combine
    pub fn aggregate(&self) -> Result<Vec<u8>, AggregationError> {
        if !self.is_ready() {
            return Err(AggregationError::InsufficientContributions {
                collected: self.collected_count(),
                required: self.threshold,
            });
        }

        // take exactly threshold contributions
        let contribs: Vec<_> = self.contributions
            .iter()
            .take(self.threshold as usize)
            .collect();

        // aggregate osst data (simplified - real impl uses curve ops)
        let mut combined = Vec::new();

        // format: [num_shares][share_ids...][osst_data_len][combined_osst_data]
        combined.push(contribs.len() as u8);
        for (&share_id, _) in &contribs {
            combined.push(share_id);
        }

        // in real impl, this would be:
        // 1. extract R points from each contribution
        // 2. sum them: R_combined = R_1 + R_2 + ... + R_t
        // 3. extract z scalars from each contribution
        // 4. sum with lagrange coefficients: z_combined = Σ λ_i * z_i
        // 5. return (R_combined, z_combined) as the threshold signature

        // for now, concatenate osst data
        for (_, contrib) in &contribs {
            combined.extend_from_slice(&(contrib.osst_data.len() as u32).to_le_bytes());
            combined.extend_from_slice(&contrib.osst_data);
        }

        Ok(combined)
    }
}

/// batched contribution aggregator
///
/// handles contributions where a single member contributes for multiple shares
#[derive(Clone, Debug)]
pub struct BatchedAggregator {
    /// underlying collector
    collector: ContributionCollector,
}

impl BatchedAggregator {
    /// create aggregator
    pub fn new(proposal_id: ProposalId, threshold: u8) -> Self {
        Self {
            collector: ContributionCollector::new(proposal_id, threshold),
        }
    }

    /// add batched contribution
    pub fn add(&mut self, signed: &SignedContribution) -> Result<bool, AggregationError> {
        self.collector.add_contribution(signed)
    }

    /// check if ready
    pub fn is_ready(&self) -> bool {
        self.collector.is_ready()
    }

    /// aggregate if ready
    pub fn finalize(&self) -> Result<Vec<u8>, AggregationError> {
        self.collector.aggregate()
    }

    /// get status
    pub fn status(&self) -> AggregationResult {
        self.collector.status()
    }
}

/// aggregation errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AggregationError {
    /// contribution for wrong proposal
    WrongProposal {
        expected: ProposalId,
        got: ProposalId,
    },
    /// not enough contributions
    InsufficientContributions {
        collected: u8,
        required: u8,
    },
    /// invalid osst data
    InvalidContribution,
    /// signature verification failed
    SignatureInvalid,
}

impl core::fmt::Display for AggregationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongProposal { expected, got } => {
                write!(f, "wrong proposal: expected {}, got {}", expected, got)
            }
            Self::InsufficientContributions { collected, required } => {
                write!(f, "insufficient contributions: {}/{}", collected, required)
            }
            Self::InvalidContribution => write!(f, "invalid contribution"),
            Self::SignatureInvalid => write!(f, "signature verification failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Contribution as WireContribution;

    fn make_contribution(
        proposal_id: ProposalId,
        contributor: Hash32,
        share_ids: Vec<ShareId>,
    ) -> SignedContribution {
        SignedContribution {
            contribution: WireContribution {
                proposal_id,
                share_ids,
                osst_data: vec![1, 2, 3, 4], // mock osst data
            },
            contributor_pubkey: contributor,
            signature: [0u8; 64],
        }
    }

    #[test]
    fn test_collector_basic() {
        let mut collector = ContributionCollector::new(1, 3);

        assert_eq!(collector.collected_count(), 0);
        assert!(!collector.is_ready());

        // add contribution from member with 2 shares
        let contrib1 = make_contribution(1, [1u8; 32], vec![1, 2]);
        assert!(collector.add_contribution(&contrib1).unwrap());
        assert_eq!(collector.collected_count(), 2);
        assert!(!collector.is_ready());

        // add contribution from member with 1 share
        let contrib2 = make_contribution(1, [2u8; 32], vec![3]);
        assert!(collector.add_contribution(&contrib2).unwrap());
        assert_eq!(collector.collected_count(), 3);
        assert!(collector.is_ready());
    }

    #[test]
    fn test_duplicate_contributor() {
        let mut collector = ContributionCollector::new(1, 3);

        let contrib = make_contribution(1, [1u8; 32], vec![1]);
        assert!(collector.add_contribution(&contrib).unwrap()); // first
        assert!(!collector.add_contribution(&contrib).unwrap()); // duplicate
    }

    #[test]
    fn test_wrong_proposal() {
        let mut collector = ContributionCollector::new(1, 3);

        let contrib = make_contribution(2, [1u8; 32], vec![1]); // wrong proposal
        let result = collector.add_contribution(&contrib);
        assert!(matches!(result, Err(AggregationError::WrongProposal { .. })));
    }

    #[test]
    fn test_aggregate() {
        let mut collector = ContributionCollector::new(1, 2);

        let contrib1 = make_contribution(1, [1u8; 32], vec![1]);
        let contrib2 = make_contribution(1, [2u8; 32], vec![2]);

        collector.add_contribution(&contrib1).unwrap();
        collector.add_contribution(&contrib2).unwrap();

        let signature = collector.aggregate().unwrap();
        assert!(!signature.is_empty());
    }

    #[test]
    fn test_aggregate_insufficient() {
        let collector = ContributionCollector::new(1, 3);

        // no contributions, should fail
        let result = collector.aggregate();
        assert!(matches!(
            result,
            Err(AggregationError::InsufficientContributions { .. })
        ));
    }

    #[test]
    fn test_batched_aggregator() {
        let mut agg = BatchedAggregator::new(1, 5);

        // member with 3 shares
        let contrib1 = make_contribution(1, [1u8; 32], vec![1, 2, 3]);
        agg.add(&contrib1).unwrap();
        assert!(!agg.is_ready());

        // member with 2 shares
        let contrib2 = make_contribution(1, [2u8; 32], vec![4, 5]);
        agg.add(&contrib2).unwrap();
        assert!(agg.is_ready());

        let sig = agg.finalize().unwrap();
        assert!(!sig.is_empty());
    }
}
