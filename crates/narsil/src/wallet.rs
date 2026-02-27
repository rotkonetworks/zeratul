//! private state wallet for shielded chains
//!
//! penumbra and zcash syndicates need to track private state:
//! - notes owned by syndicate (scanned with FVK)
//! - nullifiers (spent notes)
//! - merkle witnesses (for building spends)
//!
//! # key insight
//!
//! the viewing key is shared with ALL members (not threshold).
//! this means departing members can still see syndicate funds.
//!
//! solution: rotate to new wallet when membership changes.
//!
//! # wallet rotation
//!
//! when a member leaves:
//! 1. new DKG → new spending key → new FVK
//! 2. sweep all funds from old wallet to new
//! 3. departing member sees old history, not new funds
//!
//! ```text
//! ┌─────────────┐    sweep    ┌─────────────┐
//! │ OLD WALLET  │ ──────────▶ │ NEW WALLET  │
//! │ (frozen)    │             │ (active)    │
//! │             │             │             │
//! │ FVK known   │             │ FVK only to │
//! │ to departed │             │ current     │
//! │ member      │             │ members     │
//! └─────────────┘             └─────────────┘
//! ```

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::wire::Hash32;

/// wallet status
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalletStatus {
    /// active - can receive and spend
    Active,
    /// sweeping - transferring funds to new wallet
    Sweeping,
    /// frozen - view-only, no new spends
    Frozen,
    /// archived - no longer tracked
    Archived,
}

/// shielded note (penumbra or zcash)
#[derive(Clone, Debug)]
pub struct ShieldedNote {
    /// note commitment (unique identifier)
    pub commitment: Hash32,
    /// nullifier (computed from note + nullifier key)
    pub nullifier: Hash32,
    /// amount (asset-specific)
    pub amount: u128,
    /// asset id (penumbra) or pool (zcash)
    pub asset: Hash32,
    /// position in commitment tree
    pub position: u64,
    /// block height where note was created
    pub height: u64,
    /// encrypted memo (if any)
    pub memo: Option<Vec<u8>>,
    /// spent status
    pub spent: bool,
}

/// merkle witness for spending a note
#[derive(Clone, Debug)]
pub struct NoteWitness {
    /// note commitment
    pub commitment: Hash32,
    /// authentication path (sibling hashes)
    pub auth_path: Vec<Hash32>,
    /// position in tree
    pub position: u64,
    /// anchor (root at time of witness)
    pub anchor: Hash32,
}

/// syndicate wallet for shielded chains
#[derive(Clone, Debug)]
pub struct SyndicateWallet {
    /// wallet id (derived from spending key)
    pub id: Hash32,
    /// chain type
    pub chain: ShieldedChain,
    /// wallet status
    pub status: WalletStatus,
    /// full viewing key (shared with all members)
    /// stored encrypted, decrypted at runtime
    pub fvk_encrypted: Vec<u8>,
    /// spendable notes
    notes: Vec<ShieldedNote>,
    /// spent nullifiers
    nullifiers: Vec<Hash32>,
    /// cached witnesses (commitment -> witness)
    witnesses: BTreeMap<Hash32, NoteWitness>,
    /// last scanned height
    pub scan_height: u64,
    /// previous wallet (if this is a rotation)
    pub previous_wallet: Option<Hash32>,
    /// next wallet (if we rotated away)
    pub next_wallet: Option<Hash32>,
    /// rotation reason
    pub rotation_reason: Option<RotationReason>,
}

/// supported shielded chains
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShieldedChain {
    Penumbra,
    ZcashOrchard,
    ZcashSapling,
}

/// reason for wallet rotation
#[derive(Clone, Debug)]
pub enum RotationReason {
    /// member removed from syndicate
    MemberRemoved { member_pubkey: Hash32 },
    /// member left voluntarily
    MemberLeft { member_pubkey: Hash32 },
    /// key compromise suspected
    KeyCompromise,
    /// periodic rotation (security hygiene)
    ScheduledRotation { epoch: u64 },
}

impl SyndicateWallet {
    /// create new wallet
    pub fn new(id: Hash32, chain: ShieldedChain, fvk_encrypted: Vec<u8>) -> Self {
        Self {
            id,
            chain,
            status: WalletStatus::Active,
            fvk_encrypted,
            notes: Vec::new(),
            nullifiers: Vec::new(),
            witnesses: BTreeMap::new(),
            scan_height: 0,
            previous_wallet: None,
            next_wallet: None,
            rotation_reason: None,
        }
    }

    /// create rotated wallet (successor to old wallet)
    pub fn rotated(
        id: Hash32,
        chain: ShieldedChain,
        fvk_encrypted: Vec<u8>,
        previous: Hash32,
        reason: RotationReason,
    ) -> Self {
        Self {
            id,
            chain,
            status: WalletStatus::Active,
            fvk_encrypted,
            notes: Vec::new(),
            nullifiers: Vec::new(),
            witnesses: BTreeMap::new(),
            scan_height: 0,
            previous_wallet: Some(previous),
            next_wallet: None,
            rotation_reason: Some(reason),
        }
    }

    /// add scanned note
    pub fn add_note(&mut self, note: ShieldedNote) {
        if !self.notes.iter().any(|n| n.commitment == note.commitment) {
            self.notes.push(note);
        }
    }

    /// mark note as spent
    pub fn spend_note(&mut self, nullifier: &Hash32) -> bool {
        if let Some(note) = self.notes.iter_mut().find(|n| &n.nullifier == nullifier) {
            note.spent = true;
            self.nullifiers.push(*nullifier);
            true
        } else {
            false
        }
    }

    /// check if nullifier is spent
    pub fn is_spent(&self, nullifier: &Hash32) -> bool {
        self.nullifiers.contains(nullifier)
    }

    /// get spendable notes
    pub fn spendable_notes(&self) -> Vec<&ShieldedNote> {
        self.notes.iter().filter(|n| !n.spent).collect()
    }

    /// get total balance for asset
    pub fn balance(&self, asset: &Hash32) -> u128 {
        self.notes
            .iter()
            .filter(|n| !n.spent && &n.asset == asset)
            .map(|n| n.amount)
            .sum()
    }

    /// get all balances
    pub fn balances(&self) -> BTreeMap<Hash32, u128> {
        let mut balances = BTreeMap::new();
        for note in self.spendable_notes() {
            *balances.entry(note.asset).or_insert(0) += note.amount;
        }
        balances
    }

    /// cache witness for note
    pub fn cache_witness(&mut self, commitment: Hash32, witness: NoteWitness) {
        self.witnesses.insert(commitment, witness);
    }

    /// get cached witness
    pub fn get_witness(&self, commitment: &Hash32) -> Option<&NoteWitness> {
        self.witnesses.get(commitment)
    }

    /// update scan height
    pub fn update_scan_height(&mut self, height: u64) {
        if height > self.scan_height {
            self.scan_height = height;
        }
    }

    /// begin rotation to new wallet
    pub fn begin_sweep(&mut self, new_wallet: Hash32) {
        self.status = WalletStatus::Sweeping;
        self.next_wallet = Some(new_wallet);
    }

    /// complete rotation (freeze old wallet)
    pub fn complete_sweep(&mut self) {
        self.status = WalletStatus::Frozen;
    }

    /// archive wallet (stop tracking)
    pub fn archive(&mut self) {
        self.status = WalletStatus::Archived;
    }

    /// check if wallet is active
    pub fn is_active(&self) -> bool {
        self.status == WalletStatus::Active
    }

    /// check if wallet can spend
    pub fn can_spend(&self) -> bool {
        matches!(self.status, WalletStatus::Active | WalletStatus::Sweeping)
    }
}

/// wallet rotation session
#[derive(Clone, Debug)]
pub struct WalletRotation {
    /// old wallet id
    pub old_wallet: Hash32,
    /// new wallet id
    pub new_wallet: Hash32,
    /// reason for rotation
    pub reason: RotationReason,
    /// phase of rotation
    pub phase: RotationPhase,
    /// notes to sweep
    pub notes_to_sweep: Vec<Hash32>,
    /// notes successfully swept
    pub notes_swept: Vec<Hash32>,
    /// sweep transaction hash (if submitted)
    pub sweep_tx: Option<Hash32>,
}

/// rotation phase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationPhase {
    /// proposal to rotate
    Proposed,
    /// new wallet DKG in progress
    NewWalletDkg,
    /// DKG complete, ready to sweep
    ReadyToSweep,
    /// sweep transaction building
    BuildingSweep,
    /// sweep submitted, waiting confirmation
    SweepPending,
    /// sweep confirmed, old wallet frozen
    Complete,
    /// rotation failed
    Failed,
}

impl WalletRotation {
    /// create new rotation
    pub fn new(old_wallet: Hash32, new_wallet: Hash32, reason: RotationReason) -> Self {
        Self {
            old_wallet,
            new_wallet,
            reason,
            phase: RotationPhase::Proposed,
            notes_to_sweep: Vec::new(),
            notes_swept: Vec::new(),
            sweep_tx: None,
        }
    }

    /// advance to next phase
    pub fn advance(&mut self, next: RotationPhase) {
        self.phase = next;
    }

    /// set notes to sweep
    pub fn set_notes_to_sweep(&mut self, notes: Vec<Hash32>) {
        self.notes_to_sweep = notes;
    }

    /// mark note as swept
    pub fn mark_swept(&mut self, commitment: &Hash32) {
        if !self.notes_swept.contains(commitment) {
            self.notes_swept.push(*commitment);
        }
    }

    /// check if all notes swept
    pub fn all_swept(&self) -> bool {
        self.notes_to_sweep.len() == self.notes_swept.len()
    }

    /// set sweep transaction
    pub fn set_sweep_tx(&mut self, tx_hash: Hash32) {
        self.sweep_tx = Some(tx_hash);
    }

    /// check if complete
    pub fn is_complete(&self) -> bool {
        self.phase == RotationPhase::Complete
    }
}

/// wallet manager for multiple wallets (including rotated ones)
#[derive(Clone, Debug, Default)]
pub struct WalletManager {
    /// all wallets (including frozen)
    wallets: BTreeMap<Hash32, SyndicateWallet>,
    /// active wallet per chain
    active: BTreeMap<ShieldedChain, Hash32>,
    /// pending rotations
    rotations: Vec<WalletRotation>,
}

impl WalletManager {
    /// create new manager
    pub fn new() -> Self {
        Self::default()
    }

    /// add wallet
    pub fn add_wallet(&mut self, wallet: SyndicateWallet) {
        let id = wallet.id;
        let chain = wallet.chain;
        if wallet.is_active() {
            self.active.insert(chain, id);
        }
        self.wallets.insert(id, wallet);
    }

    /// get active wallet for chain
    pub fn active_wallet(&self, chain: ShieldedChain) -> Option<&SyndicateWallet> {
        self.active.get(&chain).and_then(|id| self.wallets.get(id))
    }

    /// get active wallet for chain (mutable)
    pub fn active_wallet_mut(&mut self, chain: ShieldedChain) -> Option<&mut SyndicateWallet> {
        if let Some(id) = self.active.get(&chain).copied() {
            self.wallets.get_mut(&id)
        } else {
            None
        }
    }

    /// get wallet by id
    pub fn get_wallet(&self, id: &Hash32) -> Option<&SyndicateWallet> {
        self.wallets.get(id)
    }

    /// get wallet by id (mutable)
    pub fn get_wallet_mut(&mut self, id: &Hash32) -> Option<&mut SyndicateWallet> {
        self.wallets.get_mut(id)
    }

    /// start wallet rotation
    pub fn start_rotation(
        &mut self,
        chain: ShieldedChain,
        new_wallet: SyndicateWallet,
        reason: RotationReason,
    ) -> Option<WalletRotation> {
        let old_id = self.active.get(&chain).copied()?;
        let new_id = new_wallet.id;

        // mark old wallet as sweeping
        if let Some(old) = self.wallets.get_mut(&old_id) {
            old.begin_sweep(new_id);
        }

        // add new wallet
        self.wallets.insert(new_id, new_wallet);

        // create rotation
        let mut rotation = WalletRotation::new(old_id, new_id, reason);

        // collect notes to sweep
        if let Some(old) = self.wallets.get(&old_id) {
            let notes: Vec<Hash32> = old.spendable_notes()
                .iter()
                .map(|n| n.commitment)
                .collect();
            rotation.set_notes_to_sweep(notes);
        }

        rotation.advance(RotationPhase::ReadyToSweep);
        self.rotations.push(rotation.clone());

        Some(rotation)
    }

    /// complete rotation (after sweep confirmed)
    pub fn complete_rotation(&mut self, rotation_idx: usize) -> bool {
        if rotation_idx >= self.rotations.len() {
            return false;
        }

        let rotation = &mut self.rotations[rotation_idx];

        // freeze old wallet
        if let Some(old) = self.wallets.get_mut(&rotation.old_wallet) {
            old.complete_sweep();
        }

        // activate new wallet
        if let Some(new) = self.wallets.get(&rotation.new_wallet) {
            self.active.insert(new.chain, new.id);
        }

        rotation.advance(RotationPhase::Complete);
        true
    }

    /// get wallet history (chain of rotations)
    pub fn wallet_history(&self, chain: ShieldedChain) -> Vec<&SyndicateWallet> {
        let mut history = Vec::new();

        if let Some(current_id) = self.active.get(&chain) {
            let mut id = *current_id;
            while let Some(wallet) = self.wallets.get(&id) {
                history.push(wallet);
                if let Some(prev) = wallet.previous_wallet {
                    id = prev;
                } else {
                    break;
                }
            }
        }

        history.reverse();
        history
    }

    /// get all frozen wallets
    pub fn frozen_wallets(&self) -> Vec<&SyndicateWallet> {
        self.wallets.values()
            .filter(|w| w.status == WalletStatus::Frozen)
            .collect()
    }

    /// get pending rotations
    pub fn pending_rotations(&self) -> Vec<&WalletRotation> {
        self.rotations.iter()
            .filter(|r| !r.is_complete())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_note(commitment: u8, amount: u128) -> ShieldedNote {
        ShieldedNote {
            commitment: [commitment; 32],
            nullifier: [commitment + 100; 32],
            amount,
            asset: [0u8; 32], // native asset
            position: commitment as u64,
            height: 1000,
            memo: None,
            spent: false,
        }
    }

    #[test]
    fn test_wallet_creation() {
        let wallet = SyndicateWallet::new(
            [1u8; 32],
            ShieldedChain::Penumbra,
            vec![0u8; 64], // encrypted FVK
        );

        assert!(wallet.is_active());
        assert!(wallet.can_spend());
        assert_eq!(wallet.spendable_notes().len(), 0);
    }

    #[test]
    fn test_add_and_spend_notes() {
        let mut wallet = SyndicateWallet::new(
            [1u8; 32],
            ShieldedChain::ZcashOrchard,
            vec![],
        );

        wallet.add_note(make_note(1, 1000));
        wallet.add_note(make_note(2, 2000));
        wallet.add_note(make_note(3, 3000));

        assert_eq!(wallet.spendable_notes().len(), 3);
        assert_eq!(wallet.balance(&[0u8; 32]), 6000);

        // spend one note
        wallet.spend_note(&[101u8; 32]);
        assert_eq!(wallet.spendable_notes().len(), 2);
        assert_eq!(wallet.balance(&[0u8; 32]), 5000);
        assert!(wallet.is_spent(&[101u8; 32]));
    }

    #[test]
    fn test_wallet_rotation() {
        let mut manager = WalletManager::new();

        // create initial wallet
        let mut wallet1 = SyndicateWallet::new(
            [1u8; 32],
            ShieldedChain::Penumbra,
            vec![],
        );
        wallet1.add_note(make_note(1, 1000));
        wallet1.add_note(make_note(2, 2000));
        manager.add_wallet(wallet1);

        // verify active
        assert!(manager.active_wallet(ShieldedChain::Penumbra).is_some());

        // create new wallet for rotation
        let wallet2 = SyndicateWallet::rotated(
            [2u8; 32],
            ShieldedChain::Penumbra,
            vec![],
            [1u8; 32],
            RotationReason::MemberRemoved { member_pubkey: [99u8; 32] },
        );

        // start rotation
        let rotation = manager.start_rotation(
            ShieldedChain::Penumbra,
            wallet2,
            RotationReason::MemberRemoved { member_pubkey: [99u8; 32] },
        );
        assert!(rotation.is_some());
        let rotation = rotation.unwrap();
        assert_eq!(rotation.notes_to_sweep.len(), 2);

        // old wallet should be sweeping
        let old = manager.get_wallet(&[1u8; 32]).unwrap();
        assert_eq!(old.status, WalletStatus::Sweeping);

        // complete rotation
        manager.complete_rotation(0);

        // old wallet frozen
        let old = manager.get_wallet(&[1u8; 32]).unwrap();
        assert_eq!(old.status, WalletStatus::Frozen);

        // new wallet active
        let active = manager.active_wallet(ShieldedChain::Penumbra).unwrap();
        assert_eq!(active.id, [2u8; 32]);
    }

    #[test]
    fn test_wallet_history() {
        let mut manager = WalletManager::new();

        // wallet 1 (original)
        let wallet1 = SyndicateWallet::new([1u8; 32], ShieldedChain::ZcashOrchard, vec![]);
        manager.add_wallet(wallet1);

        // rotate to wallet 2
        let wallet2 = SyndicateWallet::rotated(
            [2u8; 32],
            ShieldedChain::ZcashOrchard,
            vec![],
            [1u8; 32],
            RotationReason::ScheduledRotation { epoch: 1 },
        );
        manager.start_rotation(ShieldedChain::ZcashOrchard, wallet2, RotationReason::ScheduledRotation { epoch: 1 });
        manager.complete_rotation(0);

        // rotate to wallet 3
        let wallet3 = SyndicateWallet::rotated(
            [3u8; 32],
            ShieldedChain::ZcashOrchard,
            vec![],
            [2u8; 32],
            RotationReason::MemberLeft { member_pubkey: [50u8; 32] },
        );
        manager.start_rotation(ShieldedChain::ZcashOrchard, wallet3, RotationReason::MemberLeft { member_pubkey: [50u8; 32] });
        manager.complete_rotation(1);

        // check history
        let history = manager.wallet_history(ShieldedChain::ZcashOrchard);
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].id, [1u8; 32]);
        assert_eq!(history[1].id, [2u8; 32]);
        assert_eq!(history[2].id, [3u8; 32]);

        // check frozen wallets
        let frozen = manager.frozen_wallets();
        assert_eq!(frozen.len(), 2);
    }

    #[test]
    fn test_multiple_chains() {
        let mut manager = WalletManager::new();

        let penumbra = SyndicateWallet::new([1u8; 32], ShieldedChain::Penumbra, vec![]);
        let zcash = SyndicateWallet::new([2u8; 32], ShieldedChain::ZcashOrchard, vec![]);

        manager.add_wallet(penumbra);
        manager.add_wallet(zcash);

        assert!(manager.active_wallet(ShieldedChain::Penumbra).is_some());
        assert!(manager.active_wallet(ShieldedChain::ZcashOrchard).is_some());
        assert!(manager.active_wallet(ShieldedChain::ZcashSapling).is_none());
    }
}
