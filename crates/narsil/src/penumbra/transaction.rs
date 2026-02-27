//! penumbra transaction building for narsil syndicates
//!
//! converts ActionPlans into penumbra transactions using the penumbra-sdk.
//!
//! # flow
//!
//! ```text
//! ActionPlan → TransactionPlan → effect_hash → (OSST sign) → Transaction
//! ```
//!
//! the signature is created via OSST threshold signing on decaf377.
//!
//! # witness data
//!
//! to spend notes, we need merkle proofs from the state commitment tree (TCT).
//! these are obtained from the view service or maintained locally.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

#[cfg(feature = "penumbra")]
use {
    decaf377_rdsa::{Signature, SpendAuth},
    penumbra_sdk_asset::{Value as PenumbraValue, STAKING_TOKEN_ASSET_ID},
    penumbra_sdk_keys::{
        keys::{FullViewingKey, AddressIndex},
        Address,
    },
    penumbra_sdk_shielded_pool::{
        Note, Rseed,
        SpendPlan as PenumbraSpendPlan,
        OutputPlan as PenumbraOutputPlan,
    },
    penumbra_sdk_transaction::{
        plan::{TransactionPlan, ActionPlan as PenumbraActionPlan},
        Transaction,
        TransactionParameters,
        AuthorizationData as PenumbraAuthData,
        WitnessData,
    },
    penumbra_sdk_num::Amount,
    penumbra_sdk_tct as tct,
};

use super::action::{ActionPlan, SyndicateAction, SpendPlan, SwapPlan, DelegatePlan, Value, Address as NarsilAddress};
use super::note::{NoteSet, SyndicateNote};

/// witness data for a note (merkle proof)
#[cfg(feature = "penumbra")]
#[derive(Clone, Debug)]
pub struct NoteWitness {
    /// note commitment
    pub commitment: [u8; 32],
    /// position in TCT
    pub position: tct::Position,
    /// merkle auth path
    pub auth_path: tct::Proof,
}

/// penumbra transaction builder
#[cfg(feature = "penumbra")]
pub struct TransactionBuilder {
    /// full viewing key for the syndicate
    fvk: FullViewingKey,
    /// chain id
    chain_id: String,
    /// fee amount (in upenumbra)
    fee: u64,
}

#[cfg(feature = "penumbra")]
impl TransactionBuilder {
    /// create builder with FVK
    pub fn new(fvk: FullViewingKey, chain_id: impl Into<String>) -> Self {
        Self {
            fvk,
            chain_id: chain_id.into(),
            fee: 0,
        }
    }

    /// set transaction fee
    pub fn with_fee(mut self, fee: u64) -> Self {
        self.fee = fee;
        self
    }

    /// build transaction plan from action plan
    ///
    /// # arguments
    /// * `action` - the syndicate action to execute
    /// * `notes` - available notes for spending
    /// * `witnesses` - merkle proofs for notes we want to spend
    /// * `anchor` - current TCT root
    ///
    /// # returns
    /// * `TransactionPlan` - the plan to be signed
    /// * `Vec<u8>` - the effect hash bytes for OSST signing
    pub fn build_plan(
        &self,
        action: &ActionPlan,
        notes: &NoteSet,
        witnesses: &[NoteWitness],
        anchor: tct::Root,
    ) -> Result<(TransactionPlan, Vec<u8>), TransactionError> {
        match &action.action {
            SyndicateAction::Spend(spend_plan) => {
                self.build_spend_transaction(spend_plan, notes, witnesses, anchor, action.expiry_height)
            }
            SyndicateAction::Swap(swap_plan) => {
                self.build_swap_transaction(swap_plan, notes, witnesses, anchor, action.expiry_height)
            }
            SyndicateAction::Delegate(delegate_plan) => {
                self.build_delegate_transaction(delegate_plan, notes, witnesses, anchor, action.expiry_height)
            }
            SyndicateAction::Undelegate(_) => {
                Err(TransactionError::UnsupportedAction("undelegate not yet implemented"))
            }
            SyndicateAction::IbcTransfer(_) => {
                Err(TransactionError::UnsupportedAction("ibc transfer not yet implemented"))
            }
            SyndicateAction::Distribute(_) => {
                Err(TransactionError::UnsupportedAction("distribute - use multiple outputs"))
            }
        }
    }

    /// build a spend (transfer) transaction
    fn build_spend_transaction(
        &self,
        spend: &SpendPlan,
        notes: &NoteSet,
        witnesses: &[NoteWitness],
        _anchor: tct::Root,
        expiry_height: u64,
    ) -> Result<(TransactionPlan, Vec<u8>), TransactionError> {
        let mut rng = rand_core::OsRng;

        // select notes to cover spend + fee
        let total_needed = spend.value.amount + self.fee as u128;
        let selected = notes.select_notes(&spend.value.asset_id, total_needed)
            .ok_or(TransactionError::InsufficientFunds)?;

        // calculate change
        let total_input: u128 = selected.iter().map(|n| n.value.amount).sum();
        let change_amount = total_input.saturating_sub(total_needed);

        // convert to penumbra value
        let spend_value = self.convert_value(&spend.value)?;

        // build spend plans for each input note
        let mut actions: Vec<PenumbraActionPlan> = Vec::new();
        for syndicate_note in &selected {
            // find witness for this note
            let witness = witnesses.iter()
                .find(|w| w.commitment == syndicate_note.commitment)
                .ok_or(TransactionError::MissingWitness)?;

            // convert to penumbra Note
            let note = self.convert_note(syndicate_note)?;

            // create spend plan
            let spend_plan = PenumbraSpendPlan::new(&mut rng, note, witness.position);
            actions.push(PenumbraActionPlan::Spend(spend_plan));
        }

        // build output plan for destination
        let dest_address = self.parse_address(&spend.dest_address)?;
        let output_plan = PenumbraOutputPlan::new(&mut rng, spend_value, dest_address);
        actions.push(PenumbraActionPlan::Output(output_plan));

        // build change output if needed
        if change_amount > 0 {
            let change_address = self.fvk.payment_address(AddressIndex::new(0)).0;
            let change_value = PenumbraValue {
                amount: Amount::from(change_amount as u64),
                asset_id: spend_value.asset_id,
            };
            let change_plan = PenumbraOutputPlan::new(&mut rng, change_value, change_address);
            actions.push(PenumbraActionPlan::Output(change_plan));
        }

        // create transaction parameters
        let params = TransactionParameters {
            chain_id: self.chain_id.clone(),
            expiry_height,
            ..Default::default()
        };

        let plan = TransactionPlan {
            actions,
            transaction_parameters: params,
            detection_data: None,
            memo: None,
        };

        // compute effect hash - this is what OSST signs
        let effect_hash = plan.effect_hash(&self.fvk)
            .map_err(|_| TransactionError::PlanError)?;

        Ok((plan, effect_hash.0.to_vec()))
    }

    /// build a swap transaction
    fn build_swap_transaction(
        &self,
        swap: &SwapPlan,
        notes: &NoteSet,
        witnesses: &[NoteWitness],
        _anchor: tct::Root,
        _expiry_height: u64,
    ) -> Result<(TransactionPlan, Vec<u8>), TransactionError> {
        let mut rng = rand_core::OsRng;

        // select notes to cover input + fee
        let total_needed = swap.input.amount + self.fee as u128;
        let selected = notes.select_notes(&swap.input.asset_id, total_needed)
            .ok_or(TransactionError::InsufficientFunds)?;

        // build spend plans for input notes
        let mut actions: Vec<PenumbraActionPlan> = Vec::new();
        for syndicate_note in &selected {
            let witness = witnesses.iter()
                .find(|w| w.commitment == syndicate_note.commitment)
                .ok_or(TransactionError::MissingWitness)?;

            let note = self.convert_note(syndicate_note)?;
            let spend_plan = PenumbraSpendPlan::new(&mut rng, note, witness.position);
            actions.push(PenumbraActionPlan::Spend(spend_plan));
        }

        // TODO: add swap action
        // penumbra swaps are complex - they involve:
        // 1. SwapPlan with trading pair and amounts
        // 2. SwapClaimPlan to claim output after batch processing
        // for now, return error until swap support is complete

        Err(TransactionError::UnsupportedAction("swap action building incomplete"))
    }

    /// build a delegation transaction
    fn build_delegate_transaction(
        &self,
        delegate: &DelegatePlan,
        notes: &NoteSet,
        witnesses: &[NoteWitness],
        _anchor: tct::Root,
        _expiry_height: u64,
    ) -> Result<(TransactionPlan, Vec<u8>), TransactionError> {
        let mut rng = rand_core::OsRng;

        // delegation requires staking tokens
        let staking_asset = super::action::AssetId::native();
        let selected = notes.select_notes(&staking_asset, delegate.amount + self.fee as u128)
            .ok_or(TransactionError::InsufficientFunds)?;

        let mut actions: Vec<PenumbraActionPlan> = Vec::new();

        // spend the staking tokens
        for syndicate_note in &selected {
            let witness = witnesses.iter()
                .find(|w| w.commitment == syndicate_note.commitment)
                .ok_or(TransactionError::MissingWitness)?;

            let note = self.convert_note(syndicate_note)?;
            let spend_plan = PenumbraSpendPlan::new(&mut rng, note, witness.position);
            actions.push(PenumbraActionPlan::Spend(spend_plan));
        }

        // TODO: add delegate action
        // penumbra delegation involves:
        // 1. Delegate action with validator identity and amount
        // 2. output for delegation tokens
        // for now, return error until delegate support is complete

        Err(TransactionError::UnsupportedAction("delegate action building incomplete"))
    }

    /// convert narsil note to penumbra Note
    fn convert_note(&self, note: &SyndicateNote) -> Result<Note, TransactionError> {
        // get address from stored diversifier index or use default
        let address_index = note.address_index.unwrap_or(0);
        let address = self.fvk.payment_address(AddressIndex::new(address_index)).0;

        // convert value - for now use native staking token
        // TODO: proper asset id conversion from note.value.asset_id
        let value = PenumbraValue {
            amount: Amount::from(note.value.amount as u64),
            asset_id: *STAKING_TOKEN_ASSET_ID,
        };

        // reconstruct rseed from stored bytes or generate new (should never happen for real notes)
        let rseed = if let Some(rseed_bytes) = note.rseed {
            Rseed(rseed_bytes)
        } else {
            // fallback for notes without stored rseed (testing only)
            Rseed::generate(&mut rand_core::OsRng)
        };

        Note::from_parts(address, value, rseed)
            .map_err(|_| TransactionError::InvalidNote)
    }

    /// convert narsil value to penumbra Value
    fn convert_value(&self, value: &Value) -> Result<PenumbraValue, TransactionError> {
        // would need proper asset id conversion
        Ok(PenumbraValue {
            amount: Amount::from(value.amount as u64),
            asset_id: *STAKING_TOKEN_ASSET_ID, // placeholder
        })
    }

    /// parse address from narsil format to penumbra Address
    fn parse_address(&self, addr: &NarsilAddress) -> Result<Address, TransactionError> {
        Address::try_from(addr.0.as_slice())
            .map_err(|_| TransactionError::InvalidAddress)
    }

    /// finalize transaction with authorization data from OSST signing
    ///
    /// # arguments
    /// * `plan` - the transaction plan
    /// * `witness_data` - merkle proofs for all spent notes
    /// * `auth_data` - spend auth signatures from OSST
    /// * `effect_hash` - the effect hash that was signed (for verification)
    pub fn finalize(
        &self,
        plan: TransactionPlan,
        witness_data: WitnessData,
        auth_data: AuthorizationData,
        effect_hash: Option<[u8; 64]>,
    ) -> Result<Transaction, TransactionError> {
        // convert effect hash to penumbra type
        let effect_hash_typed = effect_hash.map(|h| {
            penumbra_sdk_txhash::EffectHash(h)
        });

        // convert our auth data to penumbra format
        let penumbra_auth = PenumbraAuthData {
            effect_hash: effect_hash_typed,
            spend_auths: auth_data.spend_auths.iter()
                .map(|sig| Signature::<SpendAuth>::from(*sig))
                .collect(),
            delegator_vote_auths: auth_data.delegator_vote_auths.iter()
                .map(|sig| Signature::<SpendAuth>::from(*sig))
                .collect(),
            lqt_vote_auths: auth_data.lqt_vote_auths.iter()
                .map(|sig| Signature::<SpendAuth>::from(*sig))
                .collect(),
        };

        // build the transaction
        plan.build(&self.fvk, &witness_data, &penumbra_auth)
            .map_err(|_| TransactionError::BuildFailed)
    }

    /// get the number of spend authorizations needed for a plan
    pub fn required_spend_auths(plan: &TransactionPlan) -> usize {
        plan.actions.iter()
            .filter(|a| matches!(a, PenumbraActionPlan::Spend(_)))
            .count()
    }

    /// build WitnessData from our NoteWitness array and anchor
    ///
    /// this converts narsil's witness format to penumbra's WitnessData
    pub fn build_witness_data(
        witnesses: &[NoteWitness],
        anchor: tct::Root,
    ) -> WitnessData {
        use penumbra_sdk_shielded_pool::note::StateCommitment;

        let mut proofs = alloc::collections::BTreeMap::new();
        for witness in witnesses {
            // convert commitment bytes to StateCommitment
            if let Ok(commitment) = StateCommitment::try_from(witness.commitment.as_slice()) {
                proofs.insert(commitment, witness.auth_path.clone());
            }
        }

        WitnessData {
            anchor,
            state_commitment_proofs: proofs,
        }
    }
}

/// authorization data from OSST signing
#[cfg(feature = "penumbra")]
#[derive(Clone, Debug, Default)]
pub struct AuthorizationData {
    /// spend auth signatures (one per spend action)
    /// each signature is 64 bytes (decaf377-rdsa)
    pub spend_auths: Vec<[u8; 64]>,
    /// delegator vote authorizations (if any)
    pub delegator_vote_auths: Vec<[u8; 64]>,
    /// lqt vote authorizations (if any)
    pub lqt_vote_auths: Vec<[u8; 64]>,
}

#[cfg(feature = "penumbra")]
impl AuthorizationData {
    /// create empty auth data
    pub fn new() -> Self {
        Self::default()
    }

    /// add a spend authorization signature
    pub fn add_spend_auth(&mut self, sig: [u8; 64]) {
        self.spend_auths.push(sig);
    }
}

/// transaction building errors
#[derive(Clone, Debug)]
pub enum TransactionError {
    /// not enough funds to cover spend + fee
    InsufficientFunds,
    /// action type not yet supported
    UnsupportedAction(&'static str),
    /// failed to build transaction plan
    PlanError,
    /// missing merkle witness for a note
    MissingWitness,
    /// invalid note data
    InvalidNote,
    /// invalid address format
    InvalidAddress,
    /// transaction build failed
    BuildFailed,
    /// not implemented
    NotImplemented,
}

impl core::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InsufficientFunds => write!(f, "insufficient funds"),
            Self::UnsupportedAction(s) => write!(f, "unsupported action: {}", s),
            Self::PlanError => write!(f, "failed to build transaction plan"),
            Self::MissingWitness => write!(f, "missing merkle witness for note"),
            Self::InvalidNote => write!(f, "invalid note data"),
            Self::InvalidAddress => write!(f, "invalid address format"),
            Self::BuildFailed => write!(f, "transaction build failed"),
            Self::NotImplemented => write!(f, "not implemented"),
        }
    }
}

// ============================================================================
// OSST INTEGRATION FOR THRESHOLD SIGNING
// ============================================================================

/// helper for integrating OSST threshold signing with penumbra
///
/// # signing flow for syndicates
///
/// ```text
/// coordinator                    members (threshold required)
/// -----------                    ---------------------------
/// 1. build_plan() -> (plan, effect_hash)
/// 2. broadcast effect_hash    ->
///                             <- 3. each creates OSST contribution
/// 4. aggregate contributions
/// 5. derive_spend_auths()
/// 6. finalize() -> Transaction
/// 7. broadcast to chain
/// ```
///
/// the key insight is that penumbra uses decaf377-rdsa signatures
/// which we generate via OSST threshold signing on the decaf377 curve.
#[cfg(feature = "penumbra")]
pub mod osst_integration {
    use super::*;

    /// the message type for OSST signing is the effect hash
    pub type SigningMessage = [u8; 64];

    /// prepare the signing message from effect hash
    ///
    /// penumbra spend auth uses decaf377-rdsa which signs 64-byte messages
    /// the effect hash is 64 bytes (two field elements)
    pub fn prepare_signing_message(effect_hash: &[u8]) -> Result<SigningMessage, TransactionError> {
        if effect_hash.len() != 64 {
            return Err(TransactionError::PlanError);
        }
        let mut msg = [0u8; 64];
        msg.copy_from_slice(effect_hash);
        Ok(msg)
    }

    /// derive spend authorizations from threshold signature
    ///
    /// in penumbra, each spend action in a transaction needs its own
    /// spend auth signature. for a syndicate:
    ///
    /// 1. the syndicate's spending key is split via OSST
    /// 2. for each spend, we derive a randomized spend auth key
    /// 3. OSST threshold-signs the effect hash with each derived key
    ///
    /// this is a simplified version - in practice, the spend auth key
    /// derivation is more complex and involves the transaction plan's
    /// randomizers for each spend action.
    pub fn derive_spend_auths(
        threshold_signature: &[u8; 64],
        num_spends: usize,
    ) -> Vec<[u8; 64]> {
        // TODO: proper implementation would use the spend auth key derivation
        // from penumbra_sdk_keys::SpendKey::spend_auth_key() with each
        // spend action's randomizer
        //
        // for now, return the same signature for all spends
        // this works when there's exactly one spend, which is common
        vec![*threshold_signature; num_spends]
    }

    /// build authorization data from threshold signatures
    ///
    /// call this after aggregating OSST contributions for the effect hash
    pub fn build_auth_data(
        spend_sigs: Vec<[u8; 64]>,
    ) -> AuthorizationData {
        AuthorizationData {
            spend_auths: spend_sigs,
            delegator_vote_auths: Vec::new(),
            lqt_vote_auths: Vec::new(),
        }
    }
}

#[cfg(all(test, feature = "penumbra"))]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_error_display() {
        let err = TransactionError::InsufficientFunds;
        assert_eq!(format!("{}", err), "insufficient funds");

        let err = TransactionError::MissingWitness;
        assert_eq!(format!("{}", err), "missing merkle witness for note");
    }

    #[test]
    fn test_auth_data() {
        let mut auth = AuthorizationData::new();
        assert!(auth.spend_auths.is_empty());

        auth.add_spend_auth([1u8; 64]);
        auth.add_spend_auth([2u8; 64]);
        assert_eq!(auth.spend_auths.len(), 2);
    }
}
