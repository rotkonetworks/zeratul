//! penumbra actions that a syndicate can perform
//!
//! syndicates are limited to what a penumbra wallet can do:
//! - spend: transfer funds to an address
//! - swap: exchange assets via penumbra dex
//! - delegate: stake to validators
//! - undelegate: unstake from validators
//! - ibc transfer: move assets to other chains
//!
//! each action requires threshold approval via OSST.

use alloc::vec::Vec;
use alloc::string::String;
use sha2::{Digest, Sha256};

use crate::governance::ActionType;

/// asset identifier (simplified - real penumbra uses asset registry)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    pub fn native() -> Self {
        // UM token
        let mut hasher = Sha256::new();
        hasher.update(b"penumbra-native");
        Self(hasher.finalize().into())
    }

    pub fn from_denom(denom: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"penumbra-asset");
        hasher.update(denom.as_bytes());
        Self(hasher.finalize().into())
    }
}

/// amount with asset type
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub amount: u128,
    pub asset_id: AssetId,
}

impl Value {
    pub fn new(amount: u128, asset_id: AssetId) -> Self {
        Self { amount, asset_id }
    }

    pub fn native(amount: u128) -> Self {
        Self::new(amount, AssetId::native())
    }
}

/// address (simplified)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address(pub [u8; 80]);

impl Address {
    pub fn from_bytes(bytes: [u8; 80]) -> Self {
        Self(bytes)
    }
}

/// validator identity
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorId(pub [u8; 32]);

/// actions a syndicate can perform
#[derive(Clone, Debug)]
pub enum SyndicateAction {
    /// transfer funds to an address
    Spend(SpendPlan),
    /// swap assets via dex
    Swap(SwapPlan),
    /// delegate to validator
    Delegate(DelegatePlan),
    /// undelegate from validator
    Undelegate(UndelegatePlan),
    /// ibc transfer to another chain
    IbcTransfer(IbcTransferPlan),
    /// distribute funds to members pro-rata
    Distribute(DistributePlan),
}

impl SyndicateAction {
    /// what governance approval level does this action need?
    pub fn action_type(&self) -> ActionType {
        match self {
            // routine operations
            SyndicateAction::Spend(p) if p.value.amount < 1_000_000_000_000 => ActionType::Routine,
            SyndicateAction::Distribute(_) => ActionType::Routine,

            // major decisions
            SyndicateAction::Spend(_) => ActionType::Major,  // large spend
            SyndicateAction::Swap(_) => ActionType::Major,
            SyndicateAction::Delegate(_) => ActionType::Major,
            SyndicateAction::Undelegate(_) => ActionType::Major,
            SyndicateAction::IbcTransfer(_) => ActionType::Major,
        }
    }

    /// compute action hash for signing
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-action");
        match self {
            SyndicateAction::Spend(p) => {
                hasher.update(&[0u8]);
                hasher.update(&p.to_bytes());
            }
            SyndicateAction::Swap(p) => {
                hasher.update(&[1u8]);
                hasher.update(&p.to_bytes());
            }
            SyndicateAction::Delegate(p) => {
                hasher.update(&[2u8]);
                hasher.update(&p.to_bytes());
            }
            SyndicateAction::Undelegate(p) => {
                hasher.update(&[3u8]);
                hasher.update(&p.to_bytes());
            }
            SyndicateAction::IbcTransfer(p) => {
                hasher.update(&[4u8]);
                hasher.update(&p.to_bytes());
            }
            SyndicateAction::Distribute(p) => {
                hasher.update(&[5u8]);
                hasher.update(&p.to_bytes());
            }
        }
        hasher.finalize().into()
    }

    /// serialize for inclusion in BFT round
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        match self {
            SyndicateAction::Spend(p) => {
                buf.push(0);
                buf.extend_from_slice(&p.to_bytes());
            }
            SyndicateAction::Swap(p) => {
                buf.push(1);
                buf.extend_from_slice(&p.to_bytes());
            }
            SyndicateAction::Delegate(p) => {
                buf.push(2);
                buf.extend_from_slice(&p.to_bytes());
            }
            SyndicateAction::Undelegate(p) => {
                buf.push(3);
                buf.extend_from_slice(&p.to_bytes());
            }
            SyndicateAction::IbcTransfer(p) => {
                buf.push(4);
                buf.extend_from_slice(&p.to_bytes());
            }
            SyndicateAction::Distribute(p) => {
                buf.push(5);
                buf.extend_from_slice(&p.to_bytes());
            }
        }
        buf
    }
}

/// plan to spend funds to an address
#[derive(Clone, Debug)]
pub struct SpendPlan {
    /// value to send
    pub value: Value,
    /// destination address
    pub dest_address: Address,
    /// memo (encrypted, only recipient can read)
    pub memo: Vec<u8>,
}

impl SpendPlan {
    pub fn new(value: Value, dest_address: Address) -> Self {
        Self {
            value,
            dest_address,
            memo: Vec::new(),
        }
    }

    pub fn with_memo(mut self, memo: Vec<u8>) -> Self {
        self.memo = memo;
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.value.amount.to_le_bytes());
        buf.extend_from_slice(&self.value.asset_id.0);
        buf.extend_from_slice(&self.dest_address.0);
        buf.extend_from_slice(&(self.memo.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.memo);
        buf
    }
}

/// plan to swap assets
#[derive(Clone, Debug)]
pub struct SwapPlan {
    /// what we're selling
    pub input: Value,
    /// what we want to buy
    pub target_asset: AssetId,
    /// minimum acceptable output (slippage protection)
    pub min_output: u128,
    /// claim address for swap output
    pub claim_address: Address,
}

impl SwapPlan {
    pub fn new(input: Value, target_asset: AssetId, min_output: u128, claim_address: Address) -> Self {
        Self {
            input,
            target_asset,
            min_output,
            claim_address,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.input.amount.to_le_bytes());
        buf.extend_from_slice(&self.input.asset_id.0);
        buf.extend_from_slice(&self.target_asset.0);
        buf.extend_from_slice(&self.min_output.to_le_bytes());
        buf.extend_from_slice(&self.claim_address.0);
        buf
    }
}

/// plan to delegate to validator
#[derive(Clone, Debug)]
pub struct DelegatePlan {
    /// amount to delegate
    pub amount: u128,
    /// validator to delegate to
    pub validator: ValidatorId,
}

impl DelegatePlan {
    pub fn new(amount: u128, validator: ValidatorId) -> Self {
        Self { amount, validator }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(&self.validator.0);
        buf
    }
}

/// plan to undelegate from validator
#[derive(Clone, Debug)]
pub struct UndelegatePlan {
    /// amount to undelegate (in delegation tokens)
    pub amount: u128,
    /// validator to undelegate from
    pub validator: ValidatorId,
}

impl UndelegatePlan {
    pub fn new(amount: u128, validator: ValidatorId) -> Self {
        Self { amount, validator }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(&self.validator.0);
        buf
    }
}

/// plan to transfer via IBC
#[derive(Clone, Debug)]
pub struct IbcTransferPlan {
    /// value to transfer
    pub value: Value,
    /// destination chain
    pub dest_chain: String,
    /// destination address on remote chain
    pub dest_address: String,
    /// timeout height
    pub timeout_height: u64,
}

impl IbcTransferPlan {
    pub fn new(value: Value, dest_chain: String, dest_address: String, timeout_height: u64) -> Self {
        Self {
            value,
            dest_chain,
            dest_address,
            timeout_height,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.value.amount.to_le_bytes());
        buf.extend_from_slice(&self.value.asset_id.0);
        buf.extend_from_slice(&(self.dest_chain.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.dest_chain.as_bytes());
        buf.extend_from_slice(&(self.dest_address.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.dest_address.as_bytes());
        buf.extend_from_slice(&self.timeout_height.to_le_bytes());
        buf
    }
}

/// plan to distribute funds to members
#[derive(Clone, Debug)]
pub struct DistributePlan {
    /// total value to distribute
    pub value: Value,
    /// member addresses (indexed by member_id)
    pub member_addresses: Vec<Address>,
    /// explicit allocations (if empty, use share-weighted)
    pub allocations: Vec<u128>,
}

impl DistributePlan {
    /// distribute pro-rata based on share ownership
    pub fn pro_rata(value: Value, member_addresses: Vec<Address>) -> Self {
        Self {
            value,
            member_addresses,
            allocations: Vec::new(),  // empty = pro-rata
        }
    }

    /// distribute with explicit amounts
    pub fn explicit(value: Value, member_addresses: Vec<Address>, allocations: Vec<u128>) -> Self {
        Self {
            value,
            member_addresses,
            allocations,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.value.amount.to_le_bytes());
        buf.extend_from_slice(&self.value.asset_id.0);
        buf.extend_from_slice(&(self.member_addresses.len() as u32).to_le_bytes());
        for addr in &self.member_addresses {
            buf.extend_from_slice(&addr.0);
        }
        buf.extend_from_slice(&(self.allocations.len() as u32).to_le_bytes());
        for alloc in &self.allocations {
            buf.extend_from_slice(&alloc.to_le_bytes());
        }
        buf
    }
}

/// a complete action plan ready for signing
#[derive(Clone, Debug)]
pub struct ActionPlan {
    /// sequence number (prevents replay)
    pub sequence: u64,
    /// the action to perform
    pub action: SyndicateAction,
    /// fee to pay
    pub fee: u128,
    /// expiry height (action invalid after this)
    pub expiry_height: u64,
}

impl ActionPlan {
    pub fn new(sequence: u64, action: SyndicateAction, fee: u128, expiry_height: u64) -> Self {
        Self {
            sequence,
            action,
            fee,
            expiry_height,
        }
    }

    /// the payload to sign with OSST
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"narsil-action-plan-v1");
        buf.extend_from_slice(&self.sequence.to_le_bytes());
        buf.extend_from_slice(&self.action.hash());
        buf.extend_from_slice(&self.fee.to_le_bytes());
        buf.extend_from_slice(&self.expiry_height.to_le_bytes());
        buf
    }

    pub fn action_type(&self) -> ActionType {
        self.action.action_type()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spend_plan_serialization() {
        let plan = SpendPlan::new(
            Value::native(1_000_000),
            Address([0u8; 80]),
        ).with_memo(b"test".to_vec());

        let bytes = plan.to_bytes();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_action_type_thresholds() {
        // small spend = routine
        let small_spend = SyndicateAction::Spend(SpendPlan::new(
            Value::native(100),
            Address([0u8; 80]),
        ));
        assert_eq!(small_spend.action_type(), ActionType::Routine);

        // large spend = major
        let large_spend = SyndicateAction::Spend(SpendPlan::new(
            Value::native(10_000_000_000_000),
            Address([0u8; 80]),
        ));
        assert_eq!(large_spend.action_type(), ActionType::Major);

        // swap = major
        let swap = SyndicateAction::Swap(SwapPlan::new(
            Value::native(100),
            AssetId::from_denom("usdc"),
            90,
            Address([0u8; 80]),
        ));
        assert_eq!(swap.action_type(), ActionType::Major);
    }

    #[test]
    fn test_action_plan_signing_payload() {
        let plan = ActionPlan::new(
            1,
            SyndicateAction::Spend(SpendPlan::new(
                Value::native(1000),
                Address([0u8; 80]),
            )),
            100,
            1000,
        );

        let payload1 = plan.signing_payload();
        let payload2 = plan.signing_payload();
        assert_eq!(payload1, payload2);  // deterministic

        // different sequence = different payload
        let plan2 = ActionPlan::new(
            2,
            SyndicateAction::Spend(SpendPlan::new(
                Value::native(1000),
                Address([0u8; 80]),
            )),
            100,
            1000,
        );
        assert_ne!(plan.signing_payload(), plan2.signing_payload());
    }
}
