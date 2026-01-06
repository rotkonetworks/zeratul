//! xcm message construction
//!
//! builds xcm messages for:
//! - asset transfers between parachains
//! - deposits/withdrawals to/from ghettobox
//! - hyperbridge bridging via kusama asset hub

use crate::error::{ChainError, Result};
use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};

/// xcm version we target
pub const XCM_VERSION: u32 = 4;

/// multi-location for xcm
#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub struct MultiLocation {
    pub parents: u8,
    pub interior: Junctions,
}

/// junction types
#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub enum Junctions {
    Here,
    X1(Junction),
    X2(Junction, Junction),
    X3(Junction, Junction, Junction),
}

#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub enum Junction {
    Parachain(u32),
    AccountId32 { network: Option<NetworkId>, id: [u8; 32] },
    AccountKey20 { network: Option<NetworkId>, key: [u8; 20] },
    PalletInstance(u8),
    GeneralIndex(u128),
    GeneralKey { length: u8, data: [u8; 32] },
    GlobalConsensus(NetworkId),
}

#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub enum NetworkId {
    Polkadot,
    Kusama,
    Ethereum { chain_id: u64 },
    BitcoinCore,
}

/// xcm asset
#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub struct XcmAsset {
    pub id: MultiLocation,
    pub fun: Fungibility,
}

#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub enum Fungibility {
    Fungible(u128),
    NonFungible(AssetInstance),
}

#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub enum AssetInstance {
    Undefined,
    Index(u128),
    Array4([u8; 4]),
    Array8([u8; 8]),
    Array16([u8; 16]),
    Array32([u8; 32]),
}

/// xcm message builder
pub struct XcmBuilder {
    messages: Vec<XcmInstruction>,
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum XcmInstruction {
    WithdrawAsset(Vec<XcmAsset>),
    DepositAsset { assets: AssetFilter, beneficiary: MultiLocation },
    InitiateReserveWithdraw { assets: AssetFilter, reserve: MultiLocation, xcm: Vec<XcmInstruction> },
    InitiateTeleport { assets: AssetFilter, dest: MultiLocation, xcm: Vec<XcmInstruction> },
    BuyExecution { fees: XcmAsset, weight_limit: WeightLimit },
    DepositReserveAsset { assets: AssetFilter, dest: MultiLocation, xcm: Vec<XcmInstruction> },
    ExchangeAsset { give: AssetFilter, want: Vec<XcmAsset>, maximal: bool },
    SetAppendix(Vec<XcmInstruction>),
    ClearError,
    RefundSurplus,
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum AssetFilter {
    Definite(Vec<XcmAsset>),
    Wild(WildAsset),
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum WildAsset {
    All,
    AllOf { id: MultiLocation, fun: WildFungibility },
    AllCounted(u32),
    AllOfCounted { id: MultiLocation, fun: WildFungibility, count: u32 },
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum WildFungibility {
    Fungible,
    NonFungible,
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum WeightLimit {
    Unlimited,
    Limited(u64),
}

impl XcmBuilder {
    pub fn new() -> Self {
        Self { messages: Vec::new() }
    }

    /// withdraw asset from origin
    pub fn withdraw_asset(mut self, asset: XcmAsset) -> Self {
        self.messages.push(XcmInstruction::WithdrawAsset(vec![asset]));
        self
    }

    /// buy execution with fees
    pub fn buy_execution(mut self, fee_asset: XcmAsset) -> Self {
        self.messages.push(XcmInstruction::BuyExecution {
            fees: fee_asset,
            weight_limit: WeightLimit::Unlimited,
        });
        self
    }

    /// deposit to beneficiary
    pub fn deposit_asset(mut self, beneficiary: MultiLocation) -> Self {
        self.messages.push(XcmInstruction::DepositAsset {
            assets: AssetFilter::Wild(WildAsset::All),
            beneficiary,
        });
        self
    }

    /// build the xcm message
    pub fn build(self) -> Vec<XcmInstruction> {
        self.messages
    }
}

impl Default for XcmBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// build xcm for deposit from polkadot asset hub to ghettobox
pub fn build_deposit_from_polkadot_asset_hub(
    asset_id: u32,
    amount: u128,
    dest_account: [u8; 32],
    ghettobox_para_id: u32,
) -> Vec<XcmInstruction> {
    // asset location on asset hub
    let asset_location = MultiLocation {
        parents: 0,
        interior: Junctions::X2(
            Junction::PalletInstance(50), // Assets pallet
            Junction::GeneralIndex(asset_id as u128),
        ),
    };

    let asset = XcmAsset {
        id: asset_location.clone(),
        fun: Fungibility::Fungible(amount),
    };

    // destination on ghettobox
    let dest = MultiLocation {
        parents: 1,
        interior: Junctions::X2(
            Junction::Parachain(ghettobox_para_id),
            Junction::AccountId32 { network: None, id: dest_account },
        ),
    };

    XcmBuilder::new()
        .withdraw_asset(asset.clone())
        .buy_execution(asset)
        .deposit_asset(dest)
        .build()
}

/// build xcm for deposit from kusama asset hub (including hyperbridge assets)
pub fn build_deposit_from_kusama_asset_hub(
    asset_id: Option<u32>, // None for native KSM
    amount: u128,
    dest_account: [u8; 32],
    ghettobox_para_id: u32,
) -> Vec<XcmInstruction> {
    let asset_location = match asset_id {
        Some(id) => MultiLocation {
            parents: 0,
            interior: Junctions::X2(
                Junction::PalletInstance(50),
                Junction::GeneralIndex(id as u128),
            ),
        },
        None => MultiLocation {
            parents: 1,
            interior: Junctions::Here,
        },
    };

    let asset = XcmAsset {
        id: asset_location,
        fun: Fungibility::Fungible(amount),
    };

    let dest = MultiLocation {
        parents: 1, // up to relay
        interior: Junctions::X2(
            Junction::Parachain(ghettobox_para_id),
            Junction::AccountId32 { network: None, id: dest_account },
        ),
    };

    XcmBuilder::new()
        .withdraw_asset(asset.clone())
        .buy_execution(asset)
        .deposit_asset(dest)
        .build()
}

/// build xcm for withdrawal from ghettobox to asset hub
pub fn build_withdraw_to_asset_hub(
    amount: u128,
    dest_account: [u8; 32],
    asset_hub_para_id: u32,
) -> Vec<XcmInstruction> {
    // native token from ghettobox
    let asset = XcmAsset {
        id: MultiLocation {
            parents: 0,
            interior: Junctions::Here,
        },
        fun: Fungibility::Fungible(amount),
    };

    let dest = MultiLocation {
        parents: 1,
        interior: Junctions::X2(
            Junction::Parachain(asset_hub_para_id),
            Junction::AccountId32 { network: None, id: dest_account },
        ),
    };

    XcmBuilder::new()
        .withdraw_asset(asset.clone())
        .buy_execution(asset)
        .deposit_asset(dest)
        .build()
}

/// estimate xcm execution fee
pub fn estimate_xcm_fee(
    _instructions: &[XcmInstruction],
    _dest_chain: u32,
) -> u128 {
    // simplified estimate - real impl would query weight and convert
    1_000_000_000 // 0.001 token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xcm_builder() {
        let asset = XcmAsset {
            id: MultiLocation {
                parents: 0,
                interior: Junctions::Here,
            },
            fun: Fungibility::Fungible(1_000_000),
        };

        let dest = MultiLocation {
            parents: 1,
            interior: Junctions::X1(Junction::Parachain(1000)),
        };

        let xcm = XcmBuilder::new()
            .withdraw_asset(asset.clone())
            .buy_execution(asset)
            .deposit_asset(dest)
            .build();

        assert_eq!(xcm.len(), 3);
    }

    #[test]
    fn test_deposit_from_asset_hub() {
        let xcm = build_deposit_from_polkadot_asset_hub(
            1984, // USDT
            1_000_000,
            [1u8; 32],
            2000, // ghettobox
        );

        assert_eq!(xcm.len(), 3);
    }

    #[test]
    fn test_withdraw_to_asset_hub() {
        let xcm = build_withdraw_to_asset_hub(
            1_000_000_000_000,
            [1u8; 32],
            1000, // asset hub
        );

        assert_eq!(xcm.len(), 3);
    }
}
