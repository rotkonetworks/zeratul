//! ligerito pool component - the main entry point for the shielded pool

use anyhow::{Result, anyhow};
use tracing::{info, warn, debug, instrument};

use ligerito_shielded_pool::{
    note::NoteCommitment,
    nullifier::Nullifier,
    commitment::{StateRoot, StateCommitmentTree},
    channel::{ChannelId, SignedState},
    value::Amount,
    proof::{SpendProof, OutputProof, StateTransitionProof, ProofError},
};

use super::{
    state::{PoolState, OnChainChannel},
    actions::{PoolAction, ActionResult, ActionError, PoolEvent},
};

/// the ligerito shielded pool component
pub struct LigeritoPool {
    /// in-memory state (synced from NOMT)
    state: PoolState,
    /// commitment tree (full tree for generating proofs)
    commitment_tree: StateCommitmentTree,
    /// pending actions to include in next block
    mempool: Vec<PoolAction>,
    /// maximum actions per block
    max_actions_per_block: usize,
}

impl LigeritoPool {
    /// create a new pool with default settings
    pub fn new() -> Self {
        Self {
            state: PoolState::new(),
            commitment_tree: StateCommitmentTree::new(32), // 2^32 notes max
            mempool: Vec::new(),
            max_actions_per_block: 100,
        }
    }

    /// initialize from genesis
    pub fn init_genesis(&mut self, genesis_commitments: Vec<NoteCommitment>) {
        for commitment in genesis_commitments {
            self.commitment_tree.insert(commitment);
        }
        self.state.update_root(
            self.commitment_tree.root(),
            self.commitment_tree.len() as u64,
        );
        info!(
            note_count = self.commitment_tree.len(),
            root = ?self.state.commitment_root,
            "ligerito pool initialized"
        );
    }

    /// submit an action to the mempool
    pub fn submit_action(&mut self, action: PoolAction) -> Result<()> {
        // basic validation before accepting
        self.validate_action(&action)?;
        self.mempool.push(action);
        Ok(())
    }

    /// validate an action (cheap checks before mempool)
    fn validate_action(&self, action: &PoolAction) -> Result<()> {
        match action {
            PoolAction::Withdraw { spend_proof, .. } => {
                // check nullifier not already spent
                if self.state.is_spent(&spend_proof.nullifier) {
                    return Err(anyhow!("nullifier already spent"));
                }
                // check anchor is recent (simplified - would check last N roots)
                if spend_proof.anchor != self.state.commitment_root {
                    return Err(anyhow!("invalid merkle anchor"));
                }
            }
            PoolAction::Transfer { spend_proofs, .. } => {
                for proof in spend_proofs {
                    if self.state.is_spent(&proof.nullifier) {
                        return Err(anyhow!("nullifier already spent"));
                    }
                }
            }
            PoolAction::OpenChannel { channel_id, .. } => {
                if self.state.is_channel_active(channel_id) {
                    return Err(anyhow!("channel already exists"));
                }
            }
            PoolAction::SettleChannel { channel_id, .. }
            | PoolAction::ForceCloseChannel { channel_id, .. } => {
                if !self.state.is_channel_active(channel_id) {
                    return Err(anyhow!("channel not found"));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// process actions for a new block
    #[instrument(skip(self, verifier))]
    pub fn process_block<V: ProofVerifier>(
        &mut self,
        verifier: &V,
        block_height: u64,
    ) -> Vec<ActionResult> {
        let actions: Vec<_> = self.mempool.drain(..).collect();
        let mut results = Vec::with_capacity(actions.len());

        for action in actions.into_iter().take(self.max_actions_per_block) {
            let result = self.execute_action(&action, verifier, block_height);
            results.push(result);
        }

        results
    }

    /// execute a single action
    fn execute_action<V: ProofVerifier>(
        &mut self,
        action: &PoolAction,
        verifier: &V,
        block_height: u64,
    ) -> ActionResult {
        match action {
            PoolAction::Deposit { value, output_proof, encrypted_note } => {
                self.execute_deposit(value, output_proof, encrypted_note, verifier)
            }
            PoolAction::Withdraw { spend_proof, destination } => {
                self.execute_withdraw(spend_proof, destination, verifier)
            }
            PoolAction::Transfer { spend_proofs, output_proofs, encrypted_notes } => {
                self.execute_transfer(spend_proofs, output_proofs, encrypted_notes, verifier)
            }
            PoolAction::OpenChannel { participants, deposit_proofs, channel_id } => {
                self.execute_open_channel(participants, deposit_proofs, channel_id, verifier, block_height)
            }
            PoolAction::SettleChannel { channel_id, final_state, settlement_proofs, encrypted_notes } => {
                self.execute_settle_channel(channel_id, final_state, settlement_proofs, encrypted_notes, verifier)
            }
            PoolAction::UpdateChannelState { channel_id, signed_state, transition_proof } => {
                self.execute_update_channel(channel_id, signed_state, transition_proof, verifier)
            }
            PoolAction::ForceCloseChannel { channel_id, latest_state, proof_chain } => {
                self.execute_force_close(channel_id, latest_state, proof_chain, verifier)
            }
        }
    }

    fn execute_deposit<V: ProofVerifier>(
        &mut self,
        value: &ligerito_shielded_pool::value::Value,
        output_proof: &OutputProof,
        _encrypted_note: &[u8],
        verifier: &V,
    ) -> ActionResult {
        // verify output proof
        if !verifier.verify_output(output_proof) {
            return ActionResult::Failed { reason: ActionError::InvalidProof };
        }

        // add commitment to tree
        let commitment = output_proof.note_commitment;
        self.commitment_tree.insert(commitment);
        self.state.update_root(
            self.commitment_tree.root(),
            self.commitment_tree.len() as u64,
        );

        ActionResult::Success {
            nullifiers: vec![],
            commitments: vec![commitment],
            events: vec![
                PoolEvent::Deposit {
                    amount: value.amount,
                    asset_id: value.asset_id,
                    commitment,
                },
                PoolEvent::NoteCreated { commitment },
            ],
        }
    }

    fn execute_withdraw<V: ProofVerifier>(
        &mut self,
        spend_proof: &SpendProof,
        _destination: &[u8; 32],
        verifier: &V,
    ) -> ActionResult {
        // check anchor
        if spend_proof.anchor != self.state.commitment_root {
            return ActionResult::Failed { reason: ActionError::InvalidAnchor };
        }

        // check nullifier not spent
        if self.state.is_spent(&spend_proof.nullifier) {
            return ActionResult::Failed { reason: ActionError::NullifierAlreadySpent };
        }

        // verify spend proof
        if !verifier.verify_spend(spend_proof) {
            return ActionResult::Failed { reason: ActionError::InvalidProof };
        }

        // add nullifier
        self.state.add_nullifier(spend_proof.nullifier);

        ActionResult::Success {
            nullifiers: vec![spend_proof.nullifier],
            commitments: vec![],
            events: vec![
                PoolEvent::NoteSpent { nullifier: spend_proof.nullifier },
                // withdrawal event would include amount from proof
            ],
        }
    }

    fn execute_transfer<V: ProofVerifier>(
        &mut self,
        spend_proofs: &[SpendProof],
        output_proofs: &[OutputProof],
        _encrypted_notes: &[Vec<u8>],
        verifier: &V,
    ) -> ActionResult {
        let mut nullifiers = Vec::new();
        let mut commitments = Vec::new();
        let mut events = Vec::new();

        // verify all spends
        for proof in spend_proofs {
            if proof.anchor != self.state.commitment_root {
                return ActionResult::Failed { reason: ActionError::InvalidAnchor };
            }
            if self.state.is_spent(&proof.nullifier) {
                return ActionResult::Failed { reason: ActionError::NullifierAlreadySpent };
            }
            if !verifier.verify_spend(proof) {
                return ActionResult::Failed { reason: ActionError::InvalidProof };
            }
            nullifiers.push(proof.nullifier);
            events.push(PoolEvent::NoteSpent { nullifier: proof.nullifier });
        }

        // verify all outputs
        for proof in output_proofs {
            if !verifier.verify_output(proof) {
                return ActionResult::Failed { reason: ActionError::InvalidProof };
            }
            commitments.push(proof.note_commitment);
            events.push(PoolEvent::NoteCreated { commitment: proof.note_commitment });
        }

        // apply state changes
        for nf in &nullifiers {
            self.state.add_nullifier(*nf);
        }
        for cm in &commitments {
            self.commitment_tree.insert(*cm);
        }
        self.state.update_root(
            self.commitment_tree.root(),
            self.commitment_tree.len() as u64,
        );

        ActionResult::Success { nullifiers, commitments, events }
    }

    fn execute_open_channel<V: ProofVerifier>(
        &mut self,
        participants: &[(ligerito_shielded_pool::keys::PublicKey, Amount)],
        deposit_proofs: &[SpendProof],
        channel_id: &ChannelId,
        verifier: &V,
        block_height: u64,
    ) -> ActionResult {
        // verify deposit proofs
        let mut nullifiers = Vec::new();
        for proof in deposit_proofs {
            if !verifier.verify_spend(proof) {
                return ActionResult::Failed { reason: ActionError::InvalidProof };
            }
            if self.state.is_spent(&proof.nullifier) {
                return ActionResult::Failed { reason: ActionError::NullifierAlreadySpent };
            }
            nullifiers.push(proof.nullifier);
        }

        // apply nullifiers
        for nf in &nullifiers {
            self.state.add_nullifier(*nf);
        }

        // register channel
        self.state.open_channel(*channel_id);

        let pks: Vec<_> = participants.iter().map(|(pk, _)| *pk).collect();
        let total: Amount = participants.iter()
            .fold(Amount::ZERO, |acc, (_, amt)| acc.saturating_add(*amt));

        ActionResult::Success {
            nullifiers,
            commitments: vec![],
            events: vec![
                PoolEvent::ChannelOpened {
                    channel_id: *channel_id,
                    participants: pks,
                    total_locked: total,
                },
            ],
        }
    }

    fn execute_settle_channel<V: ProofVerifier>(
        &mut self,
        channel_id: &ChannelId,
        final_state: &SignedState,
        settlement_proofs: &[OutputProof],
        _encrypted_notes: &[Vec<u8>],
        verifier: &V,
    ) -> ActionResult {
        // check channel exists
        if !self.state.is_channel_active(channel_id) {
            return ActionResult::Failed { reason: ActionError::ChannelNotFound };
        }

        // check all participants signed
        if !final_state.is_fully_signed() {
            return ActionResult::Failed { reason: ActionError::Unauthorized };
        }

        // verify settlement proofs
        let mut commitments = Vec::new();
        for proof in settlement_proofs {
            if !verifier.verify_output(proof) {
                return ActionResult::Failed { reason: ActionError::InvalidProof };
            }
            commitments.push(proof.note_commitment);
        }

        // add settlement notes to tree
        for cm in &commitments {
            self.commitment_tree.insert(*cm);
        }
        self.state.update_root(
            self.commitment_tree.root(),
            self.commitment_tree.len() as u64,
        );

        // close channel
        self.state.close_channel(channel_id);

        let final_balances: Vec<_> = final_state.state.participants.iter()
            .map(|p| (p.public_key, p.balance))
            .collect();

        ActionResult::Success {
            nullifiers: vec![],
            commitments,
            events: vec![
                PoolEvent::ChannelSettled {
                    channel_id: *channel_id,
                    final_balances,
                },
            ],
        }
    }

    fn execute_update_channel<V: ProofVerifier>(
        &mut self,
        channel_id: &ChannelId,
        signed_state: &SignedState,
        transition_proof: &StateTransitionProof,
        verifier: &V,
    ) -> ActionResult {
        if !self.state.is_channel_active(channel_id) {
            return ActionResult::Failed { reason: ActionError::ChannelNotFound };
        }

        // verify transition proof
        if !verifier.verify_state_transition(transition_proof) {
            return ActionResult::Failed { reason: ActionError::InvalidProof };
        }

        // store updated state (for dispute resolution)
        // in production, would store in NOMT

        ActionResult::Success {
            nullifiers: vec![],
            commitments: vec![],
            events: vec![
                PoolEvent::ChannelStateUpdated {
                    channel_id: *channel_id,
                    nonce: signed_state.state.nonce,
                },
            ],
        }
    }

    fn execute_force_close<V: ProofVerifier>(
        &mut self,
        channel_id: &ChannelId,
        latest_state: &SignedState,
        _proof_chain: &Option<Vec<StateTransitionProof>>,
        _verifier: &V,
    ) -> ActionResult {
        if !self.state.is_channel_active(channel_id) {
            return ActionResult::Failed { reason: ActionError::ChannelNotFound };
        }

        // in production: verify proof chain, start challenge period
        // for now: just close with latest state

        self.state.close_channel(channel_id);

        let final_balances: Vec<_> = latest_state.state.participants.iter()
            .map(|p| (p.public_key, p.balance))
            .collect();

        ActionResult::Success {
            nullifiers: vec![],
            commitments: vec![],
            events: vec![
                PoolEvent::ChannelSettled {
                    channel_id: *channel_id,
                    final_balances,
                },
            ],
        }
    }

    /// get current state root
    pub fn state_root(&self) -> StateRoot {
        self.state.commitment_root
    }

    /// get note count
    pub fn note_count(&self) -> u64 {
        self.state.note_count
    }
}

impl Default for LigeritoPool {
    fn default() -> Self {
        Self::new()
    }
}

/// trait for proof verification (implemented by PolkaVM verifier)
pub trait ProofVerifier {
    fn verify_spend(&self, proof: &SpendProof) -> bool;
    fn verify_output(&self, proof: &OutputProof) -> bool;
    fn verify_state_transition(&self, proof: &StateTransitionProof) -> bool;
}

/// mock verifier for testing
#[cfg(test)]
pub struct MockVerifier;

#[cfg(test)]
impl ProofVerifier for MockVerifier {
    fn verify_spend(&self, _proof: &SpendProof) -> bool { true }
    fn verify_output(&self, _proof: &OutputProof) -> bool { true }
    fn verify_state_transition(&self, _proof: &StateTransitionProof) -> bool { true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let pool = LigeritoPool::new();
        assert_eq!(pool.note_count(), 0);
        assert_eq!(pool.state_root(), StateRoot::empty());
    }
}
