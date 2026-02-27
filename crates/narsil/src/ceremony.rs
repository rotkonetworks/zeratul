//! syndicate formation ceremony
//!
//! coordinates the initial setup of a syndicate:
//! 1. members join with their pubkeys and viewing keys
//! 2. OSST distributed key generation (DKG)
//! 3. share distribution with zoda-vss backup
//! 4. initial state commitment
//!
//! # flow
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    FORMATION CEREMONY                           │
//! │                                                                 │
//! │  1. ANNOUNCE                                                    │
//! │     founder posts syndicate parameters to relay                 │
//! │                                                                 │
//! │  2. JOIN                                                        │
//! │     members post join requests with pubkeys                     │
//! │                                                                 │
//! │  3. COMMIT                                                      │
//! │     each member generates DKG commitment                        │
//! │                                                                 │
//! │  4. SHARE                                                       │
//! │     members exchange DKG shares (encrypted)                     │
//! │                                                                 │
//! │  5. FINALIZE                                                    │
//! │     combine shares, verify group key, create vss backups        │
//! │                                                                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::shares::TOTAL_SHARES;
use crate::vss::{ShareDistributor, VerifiableSharePackage};
use crate::wire::{Hash32, ShareId, GovernanceRules};

/// ceremony phase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeremonyPhase {
    /// waiting for members to join
    Joining,
    /// collecting DKG commitments
    Committing,
    /// exchanging DKG shares
    Sharing,
    /// verifying and finalizing
    Finalizing,
    /// ceremony complete
    Complete,
    /// ceremony failed
    Failed,
}

/// member joining the ceremony
#[derive(Clone, Debug)]
pub struct JoiningMember {
    /// member's public key
    pub pubkey: Hash32,
    /// member's viewing key (for mailbox)
    pub viewing_key: [u8; 32],
    /// human-readable name
    pub name: String,
    /// shares to be allocated
    pub shares: Vec<ShareId>,
}

/// DKG commitment from a member
#[derive(Clone, Debug)]
pub struct DkgCommitment {
    /// committer's pubkey
    pub pubkey: Hash32,
    /// commitment to secret polynomial
    pub commitment: [u8; 32],
    /// verification points (for feldman commitment)
    pub verification_points: Vec<[u8; 32]>,
}

/// DKG share from one member to another
#[derive(Clone, Debug)]
pub struct DkgShare {
    /// sender's pubkey
    pub from: Hash32,
    /// recipient's pubkey
    pub to: Hash32,
    /// share indices
    pub share_indices: Vec<ShareId>,
    /// encrypted share data
    pub encrypted_data: Vec<u8>,
}

/// formation ceremony state
#[derive(Clone, Debug)]
pub struct FormationCeremony {
    /// syndicate id (derived from parameters)
    pub syndicate_id: Hash32,
    /// current phase
    pub phase: CeremonyPhase,
    /// governance rules
    pub rules: GovernanceRules,
    /// threshold for signing
    pub threshold: u8,
    /// members who have joined
    pub members: Vec<JoiningMember>,
    /// DKG commitments
    pub commitments: Vec<DkgCommitment>,
    /// DKG shares received
    pub shares_received: Vec<DkgShare>,
}

impl FormationCeremony {
    /// create new ceremony
    pub fn new(rules: GovernanceRules, threshold: u8) -> Self {
        let syndicate_id = Self::derive_syndicate_id(&rules, threshold);
        Self {
            syndicate_id,
            phase: CeremonyPhase::Joining,
            rules,
            threshold,
            members: Vec::new(),
            commitments: Vec::new(),
            shares_received: Vec::new(),
        }
    }

    /// derive syndicate id from parameters
    fn derive_syndicate_id(rules: &GovernanceRules, threshold: u8) -> Hash32 {
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-syndicate-v1");
        hasher.update([threshold]);
        hasher.update([rules.routine_threshold]);
        hasher.update([rules.major_threshold]);
        hasher.update([rules.amendment_threshold]);
        hasher.update([rules.existential_threshold]);
        // include timestamp for uniqueness
        let ts = 0u64; // would be actual timestamp
        hasher.update(&ts.to_le_bytes());
        hasher.finalize().into()
    }

    /// add member to ceremony
    pub fn add_member(&mut self, member: JoiningMember) -> Result<(), CeremonyError> {
        if self.phase != CeremonyPhase::Joining {
            return Err(CeremonyError::WrongPhase {
                expected: CeremonyPhase::Joining,
                got: self.phase,
            });
        }

        // check not duplicate
        if self.members.iter().any(|m| m.pubkey == member.pubkey) {
            return Err(CeremonyError::DuplicateMember { pubkey: member.pubkey });
        }

        // check shares not already allocated
        for &sid in &member.shares {
            if self.members.iter().any(|m| m.shares.contains(&sid)) {
                return Err(CeremonyError::ShareAlreadyAllocated { share_id: sid });
            }
        }

        self.members.push(member);
        Ok(())
    }

    /// allocate shares to all members equally
    pub fn allocate_equal(&mut self) -> Result<(), CeremonyError> {
        if self.members.is_empty() {
            return Err(CeremonyError::NoMembers);
        }

        let n = self.members.len();
        let shares_each = TOTAL_SHARES / n as u8;
        let remainder = TOTAL_SHARES % n as u8;

        let mut next_share: ShareId = 1;
        for (i, member) in self.members.iter_mut().enumerate() {
            let count = shares_each + if (i as u8) < remainder { 1 } else { 0 };
            member.shares = (next_share..next_share + count).collect();
            next_share += count;
        }

        Ok(())
    }

    /// transition to committing phase
    pub fn start_committing(&mut self) -> Result<(), CeremonyError> {
        if self.phase != CeremonyPhase::Joining {
            return Err(CeremonyError::WrongPhase {
                expected: CeremonyPhase::Joining,
                got: self.phase,
            });
        }

        // verify we have members
        if self.members.is_empty() {
            return Err(CeremonyError::NoMembers);
        }

        // verify all 100 shares allocated
        let total: usize = self.members.iter().map(|m| m.shares.len()).sum();
        if total != TOTAL_SHARES as usize {
            return Err(CeremonyError::SharesNotFullyAllocated {
                allocated: total as u8,
                total: TOTAL_SHARES,
            });
        }

        self.phase = CeremonyPhase::Committing;
        Ok(())
    }

    /// add DKG commitment
    pub fn add_commitment(&mut self, commitment: DkgCommitment) -> Result<(), CeremonyError> {
        if self.phase != CeremonyPhase::Committing {
            return Err(CeremonyError::WrongPhase {
                expected: CeremonyPhase::Committing,
                got: self.phase,
            });
        }

        // check member exists
        if !self.members.iter().any(|m| m.pubkey == commitment.pubkey) {
            return Err(CeremonyError::UnknownMember { pubkey: commitment.pubkey });
        }

        // check not duplicate
        if self.commitments.iter().any(|c| c.pubkey == commitment.pubkey) {
            return Err(CeremonyError::DuplicateCommitment { pubkey: commitment.pubkey });
        }

        self.commitments.push(commitment);

        // check if all members committed
        if self.commitments.len() == self.members.len() {
            self.phase = CeremonyPhase::Sharing;
        }

        Ok(())
    }

    /// add DKG share
    pub fn add_share(&mut self, share: DkgShare) -> Result<(), CeremonyError> {
        if self.phase != CeremonyPhase::Sharing {
            return Err(CeremonyError::WrongPhase {
                expected: CeremonyPhase::Sharing,
                got: self.phase,
            });
        }

        // verify sender and recipient exist
        if !self.members.iter().any(|m| m.pubkey == share.from) {
            return Err(CeremonyError::UnknownMember { pubkey: share.from });
        }
        if !self.members.iter().any(|m| m.pubkey == share.to) {
            return Err(CeremonyError::UnknownMember { pubkey: share.to });
        }

        self.shares_received.push(share);

        // check if all shares exchanged
        // n members each send to n-1 others
        let expected = self.members.len() * (self.members.len() - 1);
        if self.shares_received.len() == expected {
            self.phase = CeremonyPhase::Finalizing;
        }

        Ok(())
    }

    /// finalize ceremony
    pub fn finalize(&mut self) -> Result<FormationResult, CeremonyError> {
        if self.phase != CeremonyPhase::Finalizing {
            return Err(CeremonyError::WrongPhase {
                expected: CeremonyPhase::Finalizing,
                got: self.phase,
            });
        }

        // in real impl:
        // 1. combine DKG shares to get individual OSST shares
        // 2. compute group public key
        // 3. verify all shares are consistent
        // 4. create zoda-vss backups

        // simplified: derive group key from all commitments
        let mut group_key = [0u8; 32];
        for commitment in &self.commitments {
            for (i, byte) in commitment.commitment.iter().enumerate() {
                group_key[i] ^= byte;
            }
        }

        // create vss backup packages (simplified)
        let backup_packages: Vec<VerifiableSharePackage> = self
            .members
            .iter()
            .map(|member| {
                let recipients: Vec<Hash32> = self
                    .members
                    .iter()
                    .filter(|m| m.pubkey != member.pubkey)
                    .map(|m| m.pubkey)
                    .collect();

                // backup threshold: need at least 1, at most recipients.len()
                let backup_threshold = core::cmp::min(2, recipients.len() as u8).max(1);
                let distributor = ShareDistributor::new(backup_threshold);

                // mock encrypted share data
                let encrypted_share = [0u8; 32].to_vec();
                let mut rng = MockRng;
                distributor.create_package(
                    member.pubkey,
                    &encrypted_share,
                    &recipients,
                    &mut rng,
                )
            })
            .collect();

        self.phase = CeremonyPhase::Complete;

        Ok(FormationResult {
            syndicate_id: self.syndicate_id,
            group_key,
            threshold: self.threshold,
            members: self.members.clone(),
            backup_packages,
        })
    }

    /// check if ceremony is complete
    pub fn is_complete(&self) -> bool {
        self.phase == CeremonyPhase::Complete
    }
}

/// result of successful formation
#[derive(Clone, Debug)]
pub struct FormationResult {
    /// syndicate id
    pub syndicate_id: Hash32,
    /// group public key (for verification)
    pub group_key: [u8; 32],
    /// signing threshold
    pub threshold: u8,
    /// finalized members
    pub members: Vec<JoiningMember>,
    /// vss backup packages
    pub backup_packages: Vec<VerifiableSharePackage>,
}

/// ceremony errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CeremonyError {
    /// wrong phase for operation
    WrongPhase {
        expected: CeremonyPhase,
        got: CeremonyPhase,
    },
    /// no members in ceremony
    NoMembers,
    /// duplicate member
    DuplicateMember { pubkey: Hash32 },
    /// share already allocated
    ShareAlreadyAllocated { share_id: ShareId },
    /// shares not fully allocated
    SharesNotFullyAllocated { allocated: u8, total: u8 },
    /// unknown member
    UnknownMember { pubkey: Hash32 },
    /// duplicate commitment
    DuplicateCommitment { pubkey: Hash32 },
    /// verification failed
    VerificationFailed,
}

impl core::fmt::Display for CeremonyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongPhase { expected, got } => {
                write!(f, "wrong phase: expected {:?}, got {:?}", expected, got)
            }
            Self::NoMembers => write!(f, "no members in ceremony"),
            Self::DuplicateMember { .. } => write!(f, "duplicate member"),
            Self::ShareAlreadyAllocated { share_id } => {
                write!(f, "share {} already allocated", share_id)
            }
            Self::SharesNotFullyAllocated { allocated, total } => {
                write!(f, "shares not fully allocated: {}/{}", allocated, total)
            }
            Self::UnknownMember { .. } => write!(f, "unknown member"),
            Self::DuplicateCommitment { .. } => write!(f, "duplicate commitment"),
            Self::VerificationFailed => write!(f, "verification failed"),
        }
    }
}

/// mock RNG for testing
struct MockRng;

impl rand_core::RngCore for MockRng {
    fn next_u32(&mut self) -> u32 {
        42
    }
    fn next_u64(&mut self) -> u64 {
        42
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for byte in dest {
            *byte = 42;
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_member(id: u8, shares: Vec<ShareId>) -> JoiningMember {
        JoiningMember {
            pubkey: [id; 32],
            viewing_key: [id + 100; 32],
            name: format!("member{}", id),
            shares,
        }
    }

    #[test]
    fn test_ceremony_join_phase() {
        let mut ceremony = FormationCeremony::new(GovernanceRules::default(), 67);

        ceremony.add_member(make_member(1, vec![1, 2, 3])).unwrap();
        ceremony.add_member(make_member(2, vec![4, 5, 6])).unwrap();

        assert_eq!(ceremony.phase, CeremonyPhase::Joining);
        assert_eq!(ceremony.members.len(), 2);
    }

    #[test]
    fn test_duplicate_member() {
        let mut ceremony = FormationCeremony::new(GovernanceRules::default(), 67);

        ceremony.add_member(make_member(1, vec![1])).unwrap();

        let result = ceremony.add_member(make_member(1, vec![2]));
        assert!(matches!(result, Err(CeremonyError::DuplicateMember { .. })));
    }

    #[test]
    fn test_share_conflict() {
        let mut ceremony = FormationCeremony::new(GovernanceRules::default(), 67);

        ceremony.add_member(make_member(1, vec![1, 2, 3])).unwrap();

        let result = ceremony.add_member(make_member(2, vec![3, 4, 5])); // share 3 conflict
        assert!(matches!(
            result,
            Err(CeremonyError::ShareAlreadyAllocated { .. })
        ));
    }

    #[test]
    fn test_allocate_equal() {
        let mut ceremony = FormationCeremony::new(GovernanceRules::default(), 67);

        // add 3 members with no shares
        ceremony.add_member(make_member(1, vec![])).unwrap();
        ceremony.add_member(make_member(2, vec![])).unwrap();
        ceremony.add_member(make_member(3, vec![])).unwrap();

        ceremony.allocate_equal().unwrap();

        // 100 / 3 = 33 each, remainder 1 to first
        assert_eq!(ceremony.members[0].shares.len(), 34);
        assert_eq!(ceremony.members[1].shares.len(), 33);
        assert_eq!(ceremony.members[2].shares.len(), 33);

        // verify total
        let total: usize = ceremony.members.iter().map(|m| m.shares.len()).sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn test_phase_transitions() {
        let mut ceremony = FormationCeremony::new(GovernanceRules::default(), 67);

        // add members
        ceremony.add_member(make_member(1, vec![])).unwrap();
        ceremony.add_member(make_member(2, vec![])).unwrap();
        ceremony.allocate_equal().unwrap();

        // start committing
        ceremony.start_committing().unwrap();
        assert_eq!(ceremony.phase, CeremonyPhase::Committing);

        // add commitments
        ceremony
            .add_commitment(DkgCommitment {
                pubkey: [1u8; 32],
                commitment: [11u8; 32],
                verification_points: vec![],
            })
            .unwrap();

        ceremony
            .add_commitment(DkgCommitment {
                pubkey: [2u8; 32],
                commitment: [22u8; 32],
                verification_points: vec![],
            })
            .unwrap();

        // should transition to sharing
        assert_eq!(ceremony.phase, CeremonyPhase::Sharing);
    }

    #[test]
    fn test_full_ceremony() {
        let mut ceremony = FormationCeremony::new(GovernanceRules::default(), 67);

        // add members
        ceremony.add_member(make_member(1, vec![])).unwrap();
        ceremony.add_member(make_member(2, vec![])).unwrap();
        ceremony.allocate_equal().unwrap();
        ceremony.start_committing().unwrap();

        // commitments
        ceremony
            .add_commitment(DkgCommitment {
                pubkey: [1u8; 32],
                commitment: [11u8; 32],
                verification_points: vec![],
            })
            .unwrap();
        ceremony
            .add_commitment(DkgCommitment {
                pubkey: [2u8; 32],
                commitment: [22u8; 32],
                verification_points: vec![],
            })
            .unwrap();

        // shares (each member sends to the other)
        ceremony
            .add_share(DkgShare {
                from: [1u8; 32],
                to: [2u8; 32],
                share_indices: vec![1],
                encrypted_data: vec![1, 2, 3],
            })
            .unwrap();
        ceremony
            .add_share(DkgShare {
                from: [2u8; 32],
                to: [1u8; 32],
                share_indices: vec![51],
                encrypted_data: vec![4, 5, 6],
            })
            .unwrap();

        assert_eq!(ceremony.phase, CeremonyPhase::Finalizing);

        // finalize
        let result = ceremony.finalize().unwrap();

        assert_eq!(result.syndicate_id, ceremony.syndicate_id);
        assert_eq!(result.threshold, 67);
        assert_eq!(result.members.len(), 2);
        assert_eq!(result.backup_packages.len(), 2);
        assert!(ceremony.is_complete());
    }
}
