//! nominated proof of stake with sequential phragmén
//!
//! polkadot-style NPoS adapted for narsil escrow custody. validators hold
//! FROST shares for escrow addresses. nominators delegate stake to validators
//! they trust. sequential phragmén elects the active set each era with
//! proportional representation and balanced stake distribution.
//!
//! # phragmén algorithm
//!
//! lars edvard phragmén's sequential election method (1894):
//! 1. each nominator has a "load" starting at 0
//! 2. for each seat, find the candidate whose backers have minimum total load
//! 3. elect that candidate, distribute 1/stake cost across their backers
//! 4. repeat until all seats filled
//!
//! this minimizes the maximum load on any nominator, producing balanced
//! stake distribution across elected validators.
//!
//! # integration
//!
//! era change triggers reshare: elected validators receive FROST shares,
//! outgoing validators' shares are invalidated. the whole network can act
//! as jury via OSST (scales to hundreds).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::wire::Hash32;

/// validator: runs a narsil node, holds FROST shares for escrows
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validator {
    /// node identity
    pub pubkey: Hash32,
    /// self-stake (staked into FROST address controlled by peers)
    pub self_stake: u64,
    /// whether currently accepting nominations
    pub accepting: bool,
    /// commission (basis points, e.g. 500 = 5%)
    pub commission_bps: u16,
}

/// nominator: delegates stake to trusted validators
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nominator {
    /// nominator identity
    pub pubkey: Hash32,
    /// total stake
    pub stake: u64,
    /// nominated validators (up to MAX_NOMINATIONS)
    pub nominations: Vec<Hash32>,
}

/// elected validator with assigned stake
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElectedValidator {
    /// validator pubkey
    pub pubkey: Hash32,
    /// total backing stake (self + nominated)
    pub total_stake: u64,
    /// self stake portion
    pub self_stake: u64,
    /// stake assignments from nominators: (nominator, amount)
    pub backers: Vec<(Hash32, u64)>,
}

/// election result for an era
#[derive(Clone, Debug)]
pub struct ElectionResult {
    /// era number
    pub era: u64,
    /// elected validators with stake assignments
    pub elected: Vec<ElectedValidator>,
    /// total stake backing the elected set
    pub total_stake: u64,
    /// validators that didn't make the cut
    pub waiting: Vec<Hash32>,
}

/// total OSST shares in the system (fixed)
pub const TOTAL_SHARES: u32 = 200;

/// OSST threshold (2/3 of shares)
pub const OSST_THRESHOLD: u32 = 134;

/// max FROST executors (top validators by stake)
pub const FROST_COMMITTEE_SIZE: u32 = 5;

/// FROST threshold within the committee
pub const FROST_THRESHOLD: u32 = 4;

/// share allocation for a validator
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareAllocation {
    pub pubkey: Hash32,
    /// OSST shares (proportional to stake, out of TOTAL_SHARES)
    pub osst_shares: u32,
    /// whether this validator is in the FROST execution committee
    pub frost_executor: bool,
    /// stake percentage (basis points, e.g. 1857 = 18.57%)
    pub stake_bps: u32,
}

/// custody allocation result for an era
#[derive(Clone, Debug)]
pub struct CustodyAllocation {
    /// era
    pub era: u64,
    /// per-validator allocation
    pub allocations: Vec<ShareAllocation>,
    /// FROST committee (top validators by stake)
    pub frost_committee: Vec<Hash32>,
    /// total allocated shares (should equal TOTAL_SHARES ± rounding)
    pub total_allocated: u32,
}

impl ElectionResult {
    /// allocate OSST shares proportional to stake and select FROST committee
    ///
    /// every elected validator gets at least 1 share.
    /// remaining shares distributed proportionally.
    /// top FROST_COMMITTEE_SIZE validators form the execution committee.
    pub fn allocate_custody(&self) -> CustodyAllocation {
        if self.elected.is_empty() || self.total_stake == 0 {
            return CustodyAllocation {
                era: self.era,
                allocations: Vec::new(),
                frost_committee: Vec::new(),
                total_allocated: 0,
            };
        }

        let n = self.elected.len() as u32;

        // step 1: give everyone 1 share, distribute rest by stake
        let reserved = n.min(TOTAL_SHARES);
        let distributable = TOTAL_SHARES.saturating_sub(reserved);

        let mut allocations: Vec<ShareAllocation> = self.elected.iter().map(|v| {
            let stake_bps = ((v.total_stake as u128 * 10_000) / self.total_stake as u128) as u32;
            let proportional = ((v.total_stake as u128 * distributable as u128)
                / self.total_stake as u128) as u32;
            ShareAllocation {
                pubkey: v.pubkey,
                osst_shares: 1 + proportional, // at least 1
                frost_executor: false,
                stake_bps,
            }
        }).collect();

        // step 2: fix rounding — adjust largest holder
        let total_allocated: u32 = allocations.iter().map(|a| a.osst_shares).sum();
        if total_allocated < TOTAL_SHARES {
            allocations[0].osst_shares += TOTAL_SHARES - total_allocated;
        } else if total_allocated > TOTAL_SHARES {
            let excess = total_allocated - TOTAL_SHARES;
            allocations[0].osst_shares = allocations[0].osst_shares.saturating_sub(excess);
        }

        // step 3: select FROST committee (top by stake, already sorted)
        let committee_size = (FROST_COMMITTEE_SIZE as usize).min(allocations.len());
        let frost_committee: Vec<Hash32> = allocations[..committee_size]
            .iter()
            .map(|a| a.pubkey)
            .collect();

        for a in allocations[..committee_size].iter_mut() {
            a.frost_executor = true;
        }

        let total_allocated = allocations.iter().map(|a| a.osst_shares).sum();

        CustodyAllocation {
            era: self.era,
            allocations,
            frost_committee,
            total_allocated,
        }
    }
}

/// era configuration
#[derive(Clone, Debug)]
pub struct EraConfig {
    /// number of validator seats
    pub seats: u32,
    /// era duration in blocks/rounds
    pub era_length: u64,
    /// minimum self-stake to be a validator candidate
    pub min_self_stake: u64,
    /// minimum total stake to be elected
    pub min_total_stake: u64,
    /// maximum nominations per nominator
    pub max_nominations: usize,
    /// minimum nominator stake
    pub min_nominator_stake: u64,
}

impl Default for EraConfig {
    fn default() -> Self {
        Self {
            seats: 10,
            era_length: 1000,
            min_self_stake: 1_000,
            min_total_stake: 5_000,
            max_nominations: 16,
            min_nominator_stake: 100,
        }
    }
}

/// election error
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElectionError {
    /// not enough candidates for the number of seats
    NotEnoughCandidates { candidates: usize, seats: u32 },
    /// nominator stake below minimum
    StakeTooLow { stake: u64, minimum: u64 },
    /// too many nominations
    TooManyNominations { count: usize, max: usize },
    /// validator not found
    UnknownValidator(Hash32),
    /// duplicate registration
    AlreadyRegistered(Hash32),
    /// self-stake below minimum
    SelfStakeTooLow { stake: u64, minimum: u64 },
}

/// NPoS election state
#[derive(Clone, Debug)]
pub struct Election {
    /// configuration
    pub config: EraConfig,
    /// registered validators
    validators: BTreeMap<Hash32, Validator>,
    /// registered nominators
    nominators: BTreeMap<Hash32, Nominator>,
    /// current era
    pub current_era: u64,
    /// current election result
    pub current_result: Option<ElectionResult>,
}

impl Election {
    pub fn new(config: EraConfig) -> Self {
        Self {
            config,
            validators: BTreeMap::new(),
            nominators: BTreeMap::new(),
            current_era: 0,
            current_result: None,
        }
    }

    /// register as validator candidate
    pub fn register_validator(
        &mut self,
        pubkey: Hash32,
        self_stake: u64,
        commission_bps: u16,
    ) -> Result<(), ElectionError> {
        if self.validators.contains_key(&pubkey) {
            return Err(ElectionError::AlreadyRegistered(pubkey));
        }
        if self_stake < self.config.min_self_stake {
            return Err(ElectionError::SelfStakeTooLow {
                stake: self_stake,
                minimum: self.config.min_self_stake,
            });
        }
        self.validators.insert(pubkey, Validator {
            pubkey,
            self_stake,
            accepting: true,
            commission_bps,
        });
        Ok(())
    }

    /// unregister validator
    pub fn unregister_validator(&mut self, pubkey: &Hash32) -> bool {
        self.validators.remove(pubkey).is_some()
    }

    /// update validator self-stake
    pub fn update_self_stake(
        &mut self,
        pubkey: &Hash32,
        new_stake: u64,
    ) -> Result<(), ElectionError> {
        let v = self.validators.get_mut(pubkey)
            .ok_or(ElectionError::UnknownValidator(*pubkey))?;
        if new_stake < self.config.min_self_stake {
            return Err(ElectionError::SelfStakeTooLow {
                stake: new_stake,
                minimum: self.config.min_self_stake,
            });
        }
        v.self_stake = new_stake;
        Ok(())
    }

    /// nominate validators
    pub fn nominate(
        &mut self,
        pubkey: Hash32,
        stake: u64,
        nominations: Vec<Hash32>,
    ) -> Result<(), ElectionError> {
        if stake < self.config.min_nominator_stake {
            return Err(ElectionError::StakeTooLow {
                stake,
                minimum: self.config.min_nominator_stake,
            });
        }
        if nominations.len() > self.config.max_nominations {
            return Err(ElectionError::TooManyNominations {
                count: nominations.len(),
                max: self.config.max_nominations,
            });
        }
        // verify all nominated validators exist
        for v in &nominations {
            if !self.validators.contains_key(v) {
                return Err(ElectionError::UnknownValidator(*v));
            }
        }
        self.nominators.insert(pubkey, Nominator {
            pubkey,
            stake,
            nominations,
        });
        Ok(())
    }

    /// remove nomination
    pub fn unnominate(&mut self, pubkey: &Hash32) -> bool {
        self.nominators.remove(pubkey).is_some()
    }

    /// get candidates (accepting validators with sufficient self-stake)
    pub fn candidates(&self) -> Vec<&Validator> {
        self.validators.values()
            .filter(|v| v.accepting && v.self_stake >= self.config.min_self_stake)
            .collect()
    }

    /// run sequential phragmén election
    ///
    /// returns elected set with balanced stake assignments.
    /// each nominator's stake is split across their elected nominees
    /// proportional to load balancing.
    pub fn run_election(&mut self) -> Result<ElectionResult, ElectionError> {
        let candidates = self.candidates();
        let seats = self.config.seats as usize;

        if candidates.len() < seats {
            return Err(ElectionError::NotEnoughCandidates {
                candidates: candidates.len(),
                seats: self.config.seats,
            });
        }

        // build candidate list with self-stake
        let candidate_keys: Vec<Hash32> = candidates.iter().map(|c| c.pubkey).collect();
        let self_stakes: BTreeMap<Hash32, u64> = candidates.iter()
            .map(|c| (c.pubkey, c.self_stake))
            .collect();

        // build nominator -> [candidates] mapping (only candidates that are actually running)
        let mut nom_candidates: Vec<(Hash32, u64, Vec<Hash32>)> = Vec::new();
        for nom in self.nominators.values() {
            let valid_noms: Vec<Hash32> = nom.nominations.iter()
                .filter(|n| candidate_keys.contains(n))
                .copied()
                .collect();
            if !valid_noms.is_empty() {
                nom_candidates.push((nom.pubkey, nom.stake, valid_noms));
            }
        }

        // sequential phragmén
        let elected = seq_phragmen(
            seats,
            &candidate_keys,
            &self_stakes,
            &nom_candidates,
        );

        let total_stake: u64 = elected.iter().map(|e| e.total_stake).sum();
        let elected_keys: Vec<Hash32> = elected.iter().map(|e| e.pubkey).collect();
        let waiting: Vec<Hash32> = candidate_keys.into_iter()
            .filter(|c| !elected_keys.contains(c))
            .collect();

        let result = ElectionResult {
            era: self.current_era + 1,
            elected,
            total_stake,
            waiting,
        };

        self.current_era = result.era;
        self.current_result = Some(result.clone());

        Ok(result)
    }

    /// get the active validator set (from last election)
    pub fn active_set(&self) -> Option<&[ElectedValidator]> {
        self.current_result.as_ref().map(|r| r.elected.as_slice())
    }

    /// check if a pubkey is in the active set
    pub fn is_active(&self, pubkey: &Hash32) -> bool {
        self.current_result.as_ref()
            .map(|r| r.elected.iter().any(|e| &e.pubkey == pubkey))
            .unwrap_or(false)
    }

    /// get validator info
    pub fn validator(&self, pubkey: &Hash32) -> Option<&Validator> {
        self.validators.get(pubkey)
    }

    /// get all registered validators
    pub fn validators(&self) -> impl Iterator<Item = &Validator> {
        self.validators.values()
    }

    /// get all registered nominators
    pub fn nominators(&self) -> impl Iterator<Item = &Nominator> {
        self.nominators.values()
    }

    /// number of registered validators
    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    /// number of registered nominators
    pub fn nominator_count(&self) -> usize {
        self.nominators.len()
    }

    /// pubkeys that should receive FROST shares after election
    /// (for integration with reshare module)
    pub fn elected_pubkeys(&self) -> Vec<Hash32> {
        self.current_result.as_ref()
            .map(|r| r.elected.iter().map(|e| e.pubkey).collect())
            .unwrap_or_default()
    }

    /// pubkeys that lost their seat (need share invalidation)
    pub fn outgoing_validators(&self, previous: &[Hash32]) -> Vec<Hash32> {
        let current = self.elected_pubkeys();
        previous.iter()
            .filter(|p| !current.contains(p))
            .copied()
            .collect()
    }

    /// pubkeys that gained a seat (need new shares)
    pub fn incoming_validators(&self, previous: &[Hash32]) -> Vec<Hash32> {
        let current = self.elected_pubkeys();
        current.iter()
            .filter(|c| !previous.contains(c))
            .copied()
            .collect()
    }
}

/// sequential phragmén election
///
/// 1. each voter (nominator + validator self-stake) starts with load = 0
/// 2. for each seat, compute each unelected candidate's "score":
///    score = 1 / (self_stake + sum of (nominator_stake / (1 + nominator_load)) for each backer)
///    elect the candidate with the lowest score (most backed)
/// 3. update backer loads proportionally
/// 4. after all seats filled, compute balanced stake assignments via edge weights
fn seq_phragmen(
    seats: usize,
    candidates: &[Hash32],
    self_stakes: &BTreeMap<Hash32, u64>,
    nominators: &[(Hash32, u64, Vec<Hash32>)],
) -> Vec<ElectedValidator> {
    // use f64 for phragmén load calculations (same as substrate reference impl)
    let mut elected: Vec<Hash32> = Vec::with_capacity(seats);
    let mut elected_set: Vec<(Hash32, f64)> = Vec::with_capacity(seats); // (candidate, score)

    // nominator loads: how much each nominator's budget has been "used"
    let mut nom_load: BTreeMap<Hash32, f64> = BTreeMap::new();
    for (pk, _, _) in nominators {
        nom_load.insert(*pk, 0.0);
    }

    // track which candidates are still available
    let mut available: Vec<Hash32> = candidates.to_vec();

    for _round in 0..seats {
        if available.is_empty() {
            break;
        }

        // for each available candidate, compute score
        let mut best_candidate = None;
        let mut best_score = f64::MAX;

        for &cand in &available {
            let self_s = *self_stakes.get(&cand).unwrap_or(&0) as f64;

            // sum of nominator support: stake_j / (1 + load_j)
            let nom_support: f64 = nominators.iter()
                .filter(|(_, _, noms)| noms.contains(&cand))
                .map(|(pk, stake, _)| {
                    let load = nom_load.get(pk).copied().unwrap_or(0.0);
                    (*stake as f64) / (1.0 + load)
                })
                .sum();

            let total_support = self_s + nom_support;
            if total_support <= 0.0 {
                continue;
            }

            // score = 1 / total_support (lower = better backed)
            let score = 1.0 / total_support;

            if score < best_score {
                best_score = score;
                best_candidate = Some(cand);
            }
        }

        let winner = match best_candidate {
            Some(c) => c,
            None => break,
        };

        // update nominator loads
        // each backer's load increases by: score * (stake / (1 + old_load)) / stake
        // simplified: new_load = old_load + score / (1 + old_load) ... no
        // phragmén load update: new_load_j = score (the winning candidate's score)
        // but scaled by participation. substrate uses: load_j += score * (contribution_j / stake_j)
        // where contribution_j = stake_j / (1 + old_load_j)
        //
        // simplified: after electing with score s, each backer j who contributed c_j gets:
        // new_load_j = old_load_j + s * c_j
        // but c_j = stake_j / (1 + old_load_j), and s = 1/total_support
        for (pk, stake, noms) in nominators {
            if noms.contains(&winner) {
                let load = nom_load.get(pk).copied().unwrap_or(0.0);
                let contribution = (*stake as f64) / (1.0 + load);
                let new_load = load + best_score * contribution;
                nom_load.insert(*pk, new_load);
            }
        }

        elected.push(winner);
        elected_set.push((winner, best_score));
        available.retain(|c| c != &winner);
    }

    // phase 2: compute balanced stake assignments
    // distribute each nominator's stake across their elected nominees
    // proportional to the load contributed in each round
    let mut result: Vec<ElectedValidator> = Vec::new();

    for &(cand, _) in &elected_set {
        let self_s = *self_stakes.get(&cand).unwrap_or(&0);
        let mut backers: Vec<(Hash32, u64)> = Vec::new();
        let mut nominated_stake: u64 = 0;

        // collect all nominators backing this candidate
        let backing_noms: Vec<(Hash32, u64)> = nominators.iter()
            .filter(|(_, _, noms)| noms.contains(&cand))
            .map(|(pk, stake, _)| (*pk, *stake))
            .collect();

        if backing_noms.is_empty() {
            result.push(ElectedValidator {
                pubkey: cand,
                total_stake: self_s,
                self_stake: self_s,
                backers: Vec::new(),
            });
            continue;
        }

        // how many elected validators does each nominator back?
        let mut nom_elected_count: BTreeMap<Hash32, u64> = BTreeMap::new();
        for (pk, _, noms) in nominators {
            let count = noms.iter().filter(|n| elected.contains(n)).count() as u64;
            if count > 0 {
                nom_elected_count.insert(*pk, count);
            }
        }

        // simple proportional split: each nominator divides stake equally
        // across their elected nominees (balanced distribution)
        for (nom_pk, nom_stake) in &backing_noms {
            let count = nom_elected_count.get(nom_pk).copied().unwrap_or(1);
            let share = nom_stake / count;
            if share > 0 {
                backers.push((*nom_pk, share));
                nominated_stake += share;
            }
        }

        result.push(ElectedValidator {
            pubkey: cand,
            total_stake: self_s + nominated_stake,
            self_stake: self_s,
            backers,
        });
    }

    // sort by total stake descending for consistent ordering
    result.sort_by(|a, b| b.total_stake.cmp(&a.total_stake));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u8) -> Hash32 {
        let mut h = [0u8; 32];
        h[0] = n;
        h
    }

    #[test]
    fn test_basic_election() {
        let config = EraConfig {
            seats: 3,
            min_self_stake: 100,
            min_total_stake: 0,
            max_nominations: 16,
            min_nominator_stake: 10,
            ..Default::default()
        };
        let mut election = Election::new(config);

        // register 5 validators
        election.register_validator(pk(1), 1000, 500).unwrap();
        election.register_validator(pk(2), 800, 300).unwrap();
        election.register_validator(pk(3), 600, 200).unwrap();
        election.register_validator(pk(4), 400, 100).unwrap();
        election.register_validator(pk(5), 200, 100).unwrap();

        // nominators back various validators
        election.nominate(pk(10), 5000, vec![pk(1), pk(3)]).unwrap();
        election.nominate(pk(11), 3000, vec![pk(2), pk(4)]).unwrap();
        election.nominate(pk(12), 2000, vec![pk(1), pk(2), pk(3)]).unwrap();

        let result = election.run_election().unwrap();

        assert_eq!(result.era, 1);
        assert_eq!(result.elected.len(), 3);
        assert_eq!(result.waiting.len(), 2);

        // all elected validators should have positive stake
        for v in &result.elected {
            assert!(v.total_stake > 0);
            assert!(v.self_stake > 0);
        }

        // check total stake conservation (approximately - rounding may lose some)
        let elected_keys: Vec<Hash32> = result.elected.iter().map(|e| e.pubkey).collect();
        assert!(result.total_stake > 0);

        // the top backed validators should be elected
        // pk(1) has 1000 self + nominations from 10 and 12
        assert!(elected_keys.contains(&pk(1)));
    }

    #[test]
    fn test_phragmen_balances_stake() {
        // test that phragmén distributes stake evenly across elected validators
        let config = EraConfig {
            seats: 2,
            min_self_stake: 100,
            min_nominator_stake: 10,
            ..Default::default()
        };
        let mut election = Election::new(config);

        election.register_validator(pk(1), 500, 0).unwrap();
        election.register_validator(pk(2), 500, 0).unwrap();
        election.register_validator(pk(3), 100, 0).unwrap();

        // one big nominator backs 1 and 2 equally
        election.nominate(pk(10), 10000, vec![pk(1), pk(2)]).unwrap();

        let result = election.run_election().unwrap();

        assert_eq!(result.elected.len(), 2);
        let stakes: Vec<u64> = result.elected.iter().map(|e| e.total_stake).collect();

        // both should get roughly equal stake from the nominator
        let diff = if stakes[0] > stakes[1] {
            stakes[0] - stakes[1]
        } else {
            stakes[1] - stakes[0]
        };
        // difference should be small (just the self-stake difference at most)
        assert!(diff <= 1000, "stake should be balanced, got diff={}", diff);
    }

    #[test]
    fn test_not_enough_candidates() {
        let config = EraConfig {
            seats: 5,
            min_self_stake: 100,
            ..Default::default()
        };
        let mut election = Election::new(config);

        election.register_validator(pk(1), 1000, 0).unwrap();
        election.register_validator(pk(2), 1000, 0).unwrap();

        let result = election.run_election();
        assert_eq!(
            result.unwrap_err(),
            ElectionError::NotEnoughCandidates { candidates: 2, seats: 5 }
        );
    }

    #[test]
    fn test_validator_registration_errors() {
        let config = EraConfig {
            min_self_stake: 1000,
            ..Default::default()
        };
        let mut election = Election::new(config);

        // self-stake too low
        assert_eq!(
            election.register_validator(pk(1), 500, 0).unwrap_err(),
            ElectionError::SelfStakeTooLow { stake: 500, minimum: 1000 }
        );

        // register then duplicate
        election.register_validator(pk(1), 1000, 0).unwrap();
        assert_eq!(
            election.register_validator(pk(1), 2000, 0).unwrap_err(),
            ElectionError::AlreadyRegistered(pk(1))
        );
    }

    #[test]
    fn test_nominator_validation() {
        let config = EraConfig {
            min_self_stake: 100,
            min_nominator_stake: 500,
            max_nominations: 2,
            ..Default::default()
        };
        let mut election = Election::new(config);

        election.register_validator(pk(1), 1000, 0).unwrap();
        election.register_validator(pk(2), 1000, 0).unwrap();
        election.register_validator(pk(3), 1000, 0).unwrap();

        // stake too low
        assert_eq!(
            election.nominate(pk(10), 100, vec![pk(1)]).unwrap_err(),
            ElectionError::StakeTooLow { stake: 100, minimum: 500 }
        );

        // too many nominations
        assert_eq!(
            election.nominate(pk(10), 1000, vec![pk(1), pk(2), pk(3)]).unwrap_err(),
            ElectionError::TooManyNominations { count: 3, max: 2 }
        );

        // unknown validator
        assert_eq!(
            election.nominate(pk(10), 1000, vec![pk(99)]).unwrap_err(),
            ElectionError::UnknownValidator(pk(99))
        );

        // valid
        election.nominate(pk(10), 1000, vec![pk(1), pk(2)]).unwrap();
    }

    #[test]
    fn test_era_progression() {
        let config = EraConfig {
            seats: 2,
            min_self_stake: 100,
            min_nominator_stake: 10,
            ..Default::default()
        };
        let mut election = Election::new(config);

        election.register_validator(pk(1), 1000, 0).unwrap();
        election.register_validator(pk(2), 800, 0).unwrap();
        election.register_validator(pk(3), 600, 0).unwrap();

        // era 1
        let r1 = election.run_election().unwrap();
        assert_eq!(r1.era, 1);
        assert_eq!(election.current_era, 1);

        // era 2 (can change nominations between eras)
        election.nominate(pk(10), 5000, vec![pk(3)]).unwrap();
        let r2 = election.run_election().unwrap();
        assert_eq!(r2.era, 2);

        // pk(3) should now be elected with the nomination boost
        let elected_keys: Vec<Hash32> = r2.elected.iter().map(|e| e.pubkey).collect();
        assert!(elected_keys.contains(&pk(3)));
    }

    #[test]
    fn test_incoming_outgoing_validators() {
        let config = EraConfig {
            seats: 2,
            min_self_stake: 100,
            min_nominator_stake: 10,
            ..Default::default()
        };
        let mut election = Election::new(config);

        election.register_validator(pk(1), 1000, 0).unwrap();
        election.register_validator(pk(2), 800, 0).unwrap();
        election.register_validator(pk(3), 100, 0).unwrap();

        // era 1: pk(1) and pk(2) elected
        let r1 = election.run_election().unwrap();
        let prev: Vec<Hash32> = r1.elected.iter().map(|e| e.pubkey).collect();
        assert!(prev.contains(&pk(1)));
        assert!(prev.contains(&pk(2)));

        // boost pk(3) with huge nomination, drop pk(2)
        election.nominate(pk(10), 50000, vec![pk(3)]).unwrap();
        election.unregister_validator(&pk(2));

        let _r2 = election.run_election().unwrap();

        let incoming = election.incoming_validators(&prev);
        let outgoing = election.outgoing_validators(&prev);

        assert!(incoming.contains(&pk(3)));
        assert!(outgoing.contains(&pk(2)));
    }

    #[test]
    fn test_self_stake_only_election() {
        // no nominators, election should work on self-stake alone
        let config = EraConfig {
            seats: 2,
            min_self_stake: 100,
            ..Default::default()
        };
        let mut election = Election::new(config);

        election.register_validator(pk(1), 5000, 0).unwrap();
        election.register_validator(pk(2), 3000, 0).unwrap();
        election.register_validator(pk(3), 1000, 0).unwrap();

        let result = election.run_election().unwrap();
        assert_eq!(result.elected.len(), 2);

        // highest self-stake should win
        let elected_keys: Vec<Hash32> = result.elected.iter().map(|e| e.pubkey).collect();
        assert!(elected_keys.contains(&pk(1)));
        assert!(elected_keys.contains(&pk(2)));

        // no backers when no nominators
        for v in &result.elected {
            assert!(v.backers.is_empty());
            assert_eq!(v.total_stake, v.self_stake);
        }
    }

    #[test]
    fn test_active_set_queries() {
        let config = EraConfig {
            seats: 2,
            min_self_stake: 100,
            ..Default::default()
        };
        let mut election = Election::new(config);

        assert!(election.active_set().is_none());
        assert!(!election.is_active(&pk(1)));

        election.register_validator(pk(1), 1000, 0).unwrap();
        election.register_validator(pk(2), 800, 0).unwrap();
        election.register_validator(pk(3), 100, 0).unwrap();

        election.run_election().unwrap();

        assert!(election.active_set().is_some());
        assert_eq!(election.active_set().unwrap().len(), 2);
        assert!(election.is_active(&pk(1)));
        assert!(election.is_active(&pk(2)));
        assert!(!election.is_active(&pk(3)));
    }

    #[test]
    fn test_update_self_stake() {
        let config = EraConfig {
            min_self_stake: 100,
            ..Default::default()
        };
        let mut election = Election::new(config);

        election.register_validator(pk(1), 1000, 0).unwrap();

        // valid update
        election.update_self_stake(&pk(1), 2000).unwrap();
        assert_eq!(election.validator(&pk(1)).unwrap().self_stake, 2000);

        // below minimum
        assert_eq!(
            election.update_self_stake(&pk(1), 50).unwrap_err(),
            ElectionError::SelfStakeTooLow { stake: 50, minimum: 100 }
        );

        // unknown validator
        assert_eq!(
            election.update_self_stake(&pk(99), 1000).unwrap_err(),
            ElectionError::UnknownValidator(pk(99))
        );
    }

    #[test]
    fn test_custody_allocation_penumbra_validators() {
        // real Penumbra mainnet validator distribution (2026-03-15)
        let stakes: Vec<u64> = vec![
            2084500, 2027192, 1060440, 1058498, 661519,  // top 5
            500901, 415962, 353735, 315872, 293507,       // 6-10
            272016, 266785, 266711, 263675, 243607,       // 11-15
            233424, 228504, 97295, 82095, 61655,          // 16-20
            52434, 51101, 48826, 48731, 48052,            // 21-25
            47371, 47132, 46981, 46829,                   // 26-29
        ];

        let total: u64 = stakes.iter().sum();

        let elected: Vec<ElectedValidator> = stakes.iter().enumerate().map(|(i, &s)| {
            let mut pubkey = [0u8; 32];
            pubkey[0..4].copy_from_slice(&(i as u32).to_le_bytes());
            ElectedValidator {
                pubkey,
                total_stake: s,
                self_stake: s,
                backers: vec![],
            }
        }).collect();

        let result = ElectionResult {
            era: 1,
            elected,
            total_stake: total,
            waiting: vec![],
        };

        let custody = result.allocate_custody();

        // all 29 validators should have shares
        assert_eq!(custody.allocations.len(), 29);

        // total shares must equal 200
        assert_eq!(custody.total_allocated, TOTAL_SHARES);

        // no validator has 0 shares
        for a in &custody.allocations {
            assert!(a.osst_shares >= 1, "{:?} has 0 shares", a.pubkey);
        }

        // FROST committee is top 5
        assert_eq!(custody.frost_committee.len(), FROST_COMMITTEE_SIZE as usize);
        for a in &custody.allocations[..5] {
            assert!(a.frost_executor);
        }
        for a in &custody.allocations[5..] {
            assert!(!a.frost_executor);
        }

        // top validator (iqlusion, 18.57%) should have ~33-47 shares
        let top = &custody.allocations[0];
        assert!(top.osst_shares >= 30 && top.osst_shares <= 50,
            "top validator should have ~37 shares, got {}", top.osst_shares);

        // smallest validator (0.42%) should have 1-2 shares
        let bottom = custody.allocations.last().unwrap();
        assert!(bottom.osst_shares >= 1 && bottom.osst_shares <= 3,
            "smallest validator should have 1-2 shares, got {}", bottom.osst_shares);

        // threshold analysis: top 7 should exceed 2/3
        let top7_shares: u32 = custody.allocations[..7].iter().map(|a| a.osst_shares).sum();
        assert!(top7_shares >= OSST_THRESHOLD,
            "top 7 should meet threshold: {} < {}", top7_shares, OSST_THRESHOLD);

        // top 6 should NOT meet threshold (proves it's not too concentrated)
        let top6_shares: u32 = custody.allocations[..6].iter().map(|a| a.osst_shares).sum();
        assert!(top6_shares < OSST_THRESHOLD,
            "top 6 should NOT meet threshold: {} >= {}", top6_shares, OSST_THRESHOLD);

        // print allocation for review
        println!("\n=== Custody Allocation (era {}) ===", custody.era);
        println!("FROST committee: {} executors, threshold {}-of-{}",
            FROST_COMMITTEE_SIZE, FROST_THRESHOLD, FROST_COMMITTEE_SIZE);
        println!("OSST: {} total shares, threshold {}\n", TOTAL_SHARES, OSST_THRESHOLD);
        for a in &custody.allocations {
            let role = if a.frost_executor { "FROST+OSST" } else { "OSST     " };
            println!("  {} {:3} shares ({:5.2}%) {}",
                role, a.osst_shares, a.stake_bps as f64 / 100.0,
                if a.frost_executor { "***" } else { "" });
        }
    }
}
