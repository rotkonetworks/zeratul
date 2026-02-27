//! 100-share ownership model
//!
//! syndicates have exactly 100 OSST key shares. each share is one
//! cryptographic unit and one governance unit. members may own
//! multiple shares.
//!
//! share ownership determines:
//! - voting weight (1 share = 1 vote)
//! - signing weight (1 share = 1 OSST contribution)
//! - distribution rights (pro-rata payouts)

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::wire::{Hash32, ShareId};

/// total shares in any syndicate
pub const TOTAL_SHARES: u8 = 100;

/// maximum shares one member can hold
pub const MAX_SHARES_PER_MEMBER: u8 = 100;

/// share registry tracking ownership
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareRegistry {
    /// share_id (1-100) -> owner pubkey
    owners: BTreeMap<ShareId, Hash32>,
}

impl ShareRegistry {
    /// create empty registry
    pub fn new() -> Self {
        Self {
            owners: BTreeMap::new(),
        }
    }

    /// create registry with initial allocation
    pub fn with_allocation(allocations: &[(Hash32, u8)]) -> Result<Self, ShareError> {
        let mut registry = Self::new();
        let mut next_share: ShareId = 1;

        for (pubkey, count) in allocations {
            if *count == 0 {
                continue;
            }
            for _ in 0..*count {
                if next_share > TOTAL_SHARES {
                    return Err(ShareError::ExceedsTotalShares);
                }
                registry.owners.insert(next_share, *pubkey);
                next_share += 1;
            }
        }

        Ok(registry)
    }

    /// allocate all 100 shares to founder
    pub fn founder_controlled(founder_pubkey: Hash32) -> Self {
        let mut owners = BTreeMap::new();
        for i in 1..=TOTAL_SHARES {
            owners.insert(i, founder_pubkey);
        }
        Self { owners }
    }

    /// equal split among founders
    pub fn equal_split(founders: &[Hash32]) -> Result<Self, ShareError> {
        if founders.is_empty() {
            return Err(ShareError::NoFounders);
        }
        if founders.len() > TOTAL_SHARES as usize {
            return Err(ShareError::TooManyFounders);
        }

        let shares_each = TOTAL_SHARES / founders.len() as u8;
        let remainder = TOTAL_SHARES % founders.len() as u8;

        let mut allocations: Vec<(Hash32, u8)> = founders
            .iter()
            .map(|pk| (*pk, shares_each))
            .collect();

        // distribute remainder to first founders
        for i in 0..remainder as usize {
            allocations[i].1 += 1;
        }

        Self::with_allocation(&allocations)
    }

    /// get owner of a share
    pub fn owner(&self, share_id: ShareId) -> Option<&Hash32> {
        self.owners.get(&share_id)
    }

    /// get all shares owned by a pubkey
    pub fn shares_of(&self, pubkey: &Hash32) -> Vec<ShareId> {
        self.owners
            .iter()
            .filter_map(|(id, owner)| {
                if owner == pubkey {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    }

    /// count shares owned by a pubkey
    pub fn share_count(&self, pubkey: &Hash32) -> u8 {
        self.shares_of(pubkey).len() as u8
    }

    /// get all unique owners
    pub fn owners(&self) -> Vec<Hash32> {
        let mut seen = Vec::new();
        for owner in self.owners.values() {
            if !seen.contains(owner) {
                seen.push(*owner);
            }
        }
        seen
    }

    /// count allocated shares
    pub fn allocated_shares(&self) -> u8 {
        self.owners.len() as u8
    }

    /// check if all 100 shares are allocated
    pub fn is_fully_allocated(&self) -> bool {
        self.allocated_shares() == TOTAL_SHARES
    }

    /// transfer shares from one owner to another
    /// returns error if sender doesn't own the shares
    pub fn transfer(
        &mut self,
        share_ids: &[ShareId],
        from: &Hash32,
        to: &Hash32,
    ) -> Result<(), ShareError> {
        // verify ownership first
        for &id in share_ids {
            match self.owners.get(&id) {
                Some(owner) if owner == from => {}
                Some(_) => return Err(ShareError::NotOwner { share_id: id }),
                None => return Err(ShareError::ShareNotFound { share_id: id }),
            }
        }

        // transfer
        for &id in share_ids {
            self.owners.insert(id, *to);
        }

        Ok(())
    }

    /// check if pubkey owns all specified shares
    pub fn owns_all(&self, pubkey: &Hash32, share_ids: &[ShareId]) -> bool {
        share_ids.iter().all(|id| {
            self.owners.get(id).map(|o| o == pubkey).unwrap_or(false)
        })
    }

    /// check if shares meet threshold
    pub fn meets_threshold(&self, share_ids: &[ShareId], threshold: u8) -> bool {
        // deduplicate and validate
        let mut valid_count = 0u8;
        let mut seen = [false; 101]; // index 0 unused

        for &id in share_ids {
            if id >= 1 && id <= TOTAL_SHARES && !seen[id as usize] {
                if self.owners.contains_key(&id) {
                    valid_count += 1;
                    seen[id as usize] = true;
                }
            }
        }

        valid_count >= threshold
    }
}

impl Default for ShareRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// errors for share operations
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareError {
    /// allocation exceeds 100 shares
    ExceedsTotalShares,
    /// no founders provided
    NoFounders,
    /// more founders than shares
    TooManyFounders,
    /// share not found
    ShareNotFound { share_id: ShareId },
    /// sender doesn't own share
    NotOwner { share_id: ShareId },
    /// invalid share id (must be 1-100)
    InvalidShareId { share_id: ShareId },
}

impl core::fmt::Display for ShareError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ExceedsTotalShares => write!(f, "allocation exceeds 100 shares"),
            Self::NoFounders => write!(f, "no founders provided"),
            Self::TooManyFounders => write!(f, "more founders than shares"),
            Self::ShareNotFound { share_id } => write!(f, "share {} not found", share_id),
            Self::NotOwner { share_id } => write!(f, "not owner of share {}", share_id),
            Self::InvalidShareId { share_id } => write!(f, "invalid share id {}", share_id),
        }
    }
}

/// batch contribution helper
///
/// when a member holds N shares, they generate N OSST contributions
/// but can batch them locally before sending
#[derive(Clone, Debug)]
pub struct BatchedContribution {
    /// shares contributing
    pub share_ids: Vec<ShareId>,
    /// combined R point (sum of individual R_i)
    pub combined_r: Vec<u8>,
    /// combined z scalar (sum of individual z_i)
    pub combined_z: Vec<u8>,
}

impl BatchedContribution {
    /// create from individual contributions
    ///
    /// caller must ensure contributions are for the same message
    /// and lagrange coefficients are computed correctly
    pub fn new(
        share_ids: Vec<ShareId>,
        combined_r: Vec<u8>,
        combined_z: Vec<u8>,
    ) -> Self {
        Self {
            share_ids,
            combined_r,
            combined_z,
        }
    }

    /// number of shares in this batch
    pub fn share_count(&self) -> u8 {
        self.share_ids.len() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_founder_controlled() {
        let founder = [1u8; 32];
        let registry = ShareRegistry::founder_controlled(founder);

        assert!(registry.is_fully_allocated());
        assert_eq!(registry.share_count(&founder), 100);
        assert_eq!(registry.owners().len(), 1);
    }

    #[test]
    fn test_equal_split_3_founders() {
        let founders = [
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
        ];
        let registry = ShareRegistry::equal_split(&founders).unwrap();

        assert!(registry.is_fully_allocated());
        // 100 / 3 = 33 each, remainder 1 goes to first
        assert_eq!(registry.share_count(&founders[0]), 34);
        assert_eq!(registry.share_count(&founders[1]), 33);
        assert_eq!(registry.share_count(&founders[2]), 33);
    }

    #[test]
    fn test_equal_split_5_founders() {
        let founders: Vec<Hash32> = (0..5).map(|i| [i as u8; 32]).collect();
        let registry = ShareRegistry::equal_split(&founders).unwrap();

        assert!(registry.is_fully_allocated());
        // 100 / 5 = 20 each, no remainder
        for founder in &founders {
            assert_eq!(registry.share_count(founder), 20);
        }
    }

    #[test]
    fn test_custom_allocation() {
        let alice = [1u8; 32];
        let bob = [2u8; 32];
        let carol = [3u8; 32];

        let registry = ShareRegistry::with_allocation(&[
            (alice, 30),
            (bob, 30),
            (carol, 40),
        ]).unwrap();

        assert!(registry.is_fully_allocated());
        assert_eq!(registry.share_count(&alice), 30);
        assert_eq!(registry.share_count(&bob), 30);
        assert_eq!(registry.share_count(&carol), 40);
    }

    #[test]
    fn test_over_allocation_fails() {
        let alice = [1u8; 32];
        let bob = [2u8; 32];

        let result = ShareRegistry::with_allocation(&[
            (alice, 60),
            (bob, 50), // total 110 > 100
        ]);

        assert_eq!(result, Err(ShareError::ExceedsTotalShares));
    }

    #[test]
    fn test_transfer() {
        let alice = [1u8; 32];
        let bob = [2u8; 32];

        let mut registry = ShareRegistry::founder_controlled(alice);
        assert_eq!(registry.share_count(&alice), 100);
        assert_eq!(registry.share_count(&bob), 0);

        // transfer 30 shares to bob
        let to_transfer: Vec<ShareId> = (1..=30).collect();
        registry.transfer(&to_transfer, &alice, &bob).unwrap();

        assert_eq!(registry.share_count(&alice), 70);
        assert_eq!(registry.share_count(&bob), 30);
    }

    #[test]
    fn test_transfer_not_owner_fails() {
        let alice = [1u8; 32];
        let bob = [2u8; 32];
        let carol = [3u8; 32];

        let mut registry = ShareRegistry::founder_controlled(alice);

        // bob tries to transfer alice's shares
        let result = registry.transfer(&[1, 2, 3], &bob, &carol);
        assert!(matches!(result, Err(ShareError::NotOwner { .. })));
    }

    #[test]
    fn test_meets_threshold() {
        let alice = [1u8; 32];
        let registry = ShareRegistry::founder_controlled(alice);

        // 67 shares should meet 67% threshold
        let shares: Vec<ShareId> = (1..=67).collect();
        assert!(registry.meets_threshold(&shares, 67));
        assert!(!registry.meets_threshold(&shares, 68));

        // 50 shares shouldn't meet 51% threshold
        let shares: Vec<ShareId> = (1..=50).collect();
        assert!(!registry.meets_threshold(&shares, 51));
    }

    #[test]
    fn test_shares_of() {
        let alice = [1u8; 32];
        let bob = [2u8; 32];

        let registry = ShareRegistry::with_allocation(&[
            (alice, 30),
            (bob, 70),
        ]).unwrap();

        let alice_shares = registry.shares_of(&alice);
        assert_eq!(alice_shares.len(), 30);
        assert!(alice_shares.contains(&1));
        assert!(alice_shares.contains(&30));
        assert!(!alice_shares.contains(&31));

        let bob_shares = registry.shares_of(&bob);
        assert_eq!(bob_shares.len(), 70);
        assert!(bob_shares.contains(&31));
        assert!(bob_shares.contains(&100));
    }
}
