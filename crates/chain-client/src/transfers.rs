//! deposit/withdraw transfers
//!
//! handles cross-chain asset movements:
//! - xcm from asset hubs (polkadot/kusama)
//! - ibc from cosmos chains (osmosis)
//! - hyperbridge assets via kusama asset hub xcm

use crate::{
    config::{Asset, TransferDestination},
    error::{ChainError, Result},
};

use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};

/// transfer request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferRequest {
    /// asset to transfer
    pub asset: Asset,
    /// amount (in smallest unit)
    pub amount: u128,
    /// source address
    pub from: String,
    /// destination
    pub to: TransferDestination,
    /// optional memo (for ibc)
    pub memo: Option<String>,
}

/// transfer status
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TransferStatus {
    /// pending submission
    Pending,
    /// submitted to source chain
    Submitted { tx_hash: String },
    /// confirmed on source chain
    SourceConfirmed { block: u32 },
    /// xcm/ibc message sent
    InFlight { msg_id: String },
    /// received on destination
    Completed { dest_tx: String },
    /// failed
    Failed { error: String },
}

/// deposit route - how to get assets into ghettobox
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DepositRoute {
    /// direct transfer (if already on ghettobox)
    Direct,
    /// xcm from polkadot asset hub
    XcmPolkadotAssetHub { asset_id: Option<u32> },
    /// xcm from kusama asset hub (including hyperbridge assets)
    XcmKusamaAssetHub { asset_id: Option<u32> },
    /// ibc from osmosis
    IbcOsmosis { channel: String },
    /// ibc from other cosmos chain
    IbcCosmos { chain_id: String, channel: String },
}

impl DepositRoute {
    /// get deposit route for asset
    pub fn for_asset(asset: &Asset) -> Self {
        match asset {
            Asset::Native => DepositRoute::Direct,
            Asset::Usdt => DepositRoute::XcmPolkadotAssetHub { asset_id: Some(1984) },
            Asset::Usdc => DepositRoute::XcmPolkadotAssetHub { asset_id: Some(1337) },
            Asset::Weth => {
                // weth comes via hyperbridge on kusama asset hub
                DepositRoute::XcmKusamaAssetHub { asset_id: None }
            }
            Asset::AssetHub { asset_id } => {
                DepositRoute::XcmPolkadotAssetHub { asset_id: Some(*asset_id) }
            }
            Asset::Ibc { channel, .. } => {
                if channel.starts_with("channel-0") {
                    DepositRoute::IbcOsmosis { channel: channel.clone() }
                } else {
                    DepositRoute::IbcCosmos {
                        chain_id: "unknown".into(),
                        channel: channel.clone(),
                    }
                }
            }
        }
    }
}

/// withdraw route - how to get assets out of ghettobox
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WithdrawRoute {
    /// keep on ghettobox
    Direct,
    /// xcm to polkadot asset hub
    XcmPolkadotAssetHub,
    /// xcm to kusama asset hub (for hyperbridge bridging)
    XcmKusamaAssetHub,
    /// xcm to specific parachain
    XcmParachain { para_id: u32 },
    /// ibc to osmosis
    IbcOsmosis { channel: String },
    /// ibc to other cosmos chain
    IbcCosmos { chain_id: String, channel: String },
}

/// deposit manager
pub struct DepositManager {
    /// pending deposits
    pub pending: Vec<TransferRequest>,
}

impl DepositManager {
    pub fn new() -> Self {
        Self { pending: Vec::new() }
    }

    /// initiate deposit from asset hub
    pub async fn deposit_from_asset_hub(
        &mut self,
        asset: Asset,
        amount: u128,
        from: String,
        to_poker_address: String,
    ) -> Result<TransferRequest> {
        let route = DepositRoute::for_asset(&asset);

        let to = match route {
            DepositRoute::XcmPolkadotAssetHub { .. } => TransferDestination::Xcm {
                para_id: 2000, // ghettobox para id (example)
                address: to_poker_address,
            },
            DepositRoute::XcmKusamaAssetHub { .. } => TransferDestination::Xcm {
                para_id: 2000,
                address: to_poker_address,
            },
            _ => TransferDestination::Local { address: to_poker_address },
        };

        let request = TransferRequest {
            asset,
            amount,
            from,
            to,
            memo: None,
        };

        self.pending.push(request.clone());
        Ok(request)
    }

    /// initiate deposit from cosmos/osmosis
    pub async fn deposit_from_cosmos(
        &mut self,
        denom: String,
        channel: String,
        amount: u128,
        from: String,
        to_poker_address: String,
    ) -> Result<TransferRequest> {
        let request = TransferRequest {
            asset: Asset::Ibc { channel: channel.clone(), denom },
            amount,
            from,
            to: TransferDestination::Ibc {
                channel,
                address: to_poker_address,
            },
            memo: Some("ghettobox poker deposit".into()),
        };

        self.pending.push(request.clone());
        Ok(request)
    }

    /// clear completed
    pub fn clear_completed(&mut self) {
        // in real impl: track status and clear completed
    }
}

impl Default for DepositManager {
    fn default() -> Self {
        Self::new()
    }
}

/// withdrawal manager
pub struct WithdrawManager {
    /// pending withdrawals
    pub pending: Vec<TransferRequest>,
}

impl WithdrawManager {
    pub fn new() -> Self {
        Self { pending: Vec::new() }
    }

    /// withdraw to asset hub
    pub async fn withdraw_to_asset_hub(
        &mut self,
        asset: Asset,
        amount: u128,
        from_poker_address: String,
        to: String,
    ) -> Result<TransferRequest> {
        let request = TransferRequest {
            asset,
            amount,
            from: from_poker_address,
            to: TransferDestination::Xcm {
                para_id: 1000, // asset hub
                address: to,
            },
            memo: None,
        };

        self.pending.push(request.clone());
        Ok(request)
    }

    /// withdraw to ethereum via kusama asset hub + hyperbridge
    pub async fn withdraw_to_ethereum(
        &mut self,
        amount: u128,
        from_poker_address: String,
        eth_address: String,
    ) -> Result<TransferRequest> {
        // first xcm to kusama asset hub, then hyperbridge to eth
        let request = TransferRequest {
            asset: Asset::Weth,
            amount,
            from: from_poker_address,
            to: TransferDestination::Hyperbridge {
                chain_id: 1, // ethereum mainnet
                address: eth_address,
            },
            memo: Some("ghettobox → kusama-asset-hub → hyperbridge → ethereum".into()),
        };

        self.pending.push(request.clone());
        Ok(request)
    }

    /// withdraw to osmosis
    pub async fn withdraw_to_osmosis(
        &mut self,
        denom: String,
        amount: u128,
        from_poker_address: String,
        osmo_address: String,
    ) -> Result<TransferRequest> {
        let request = TransferRequest {
            asset: Asset::Ibc {
                channel: "channel-0".into(), // ghettobox → osmosis channel
                denom,
            },
            amount,
            from: from_poker_address,
            to: TransferDestination::Ibc {
                channel: "channel-0".into(),
                address: osmo_address,
            },
            memo: None,
        };

        self.pending.push(request.clone());
        Ok(request)
    }
}

impl Default for WithdrawManager {
    fn default() -> Self {
        Self::new()
    }
}

/// supported deposit sources
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DepositSource {
    pub name: String,
    pub chain_type: ChainType,
    pub assets: Vec<Asset>,
    pub estimated_time_secs: u32,
    pub fee_estimate: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChainType {
    Substrate,
    Cosmos,
    Ethereum,
}

/// get available deposit sources
pub fn available_deposit_sources() -> Vec<DepositSource> {
    vec![
        DepositSource {
            name: "Polkadot Asset Hub".into(),
            chain_type: ChainType::Substrate,
            assets: vec![Asset::Usdt, Asset::Usdc, Asset::AssetHub { asset_id: 0 }],
            estimated_time_secs: 60,
            fee_estimate: 1_000_000_000, // 0.001 DOT
        },
        DepositSource {
            name: "Kusama Asset Hub".into(),
            chain_type: ChainType::Substrate,
            assets: vec![Asset::Weth, Asset::AssetHub { asset_id: 0 }],
            estimated_time_secs: 30,
            fee_estimate: 100_000_000_000, // 0.1 KSM
        },
        DepositSource {
            name: "Osmosis".into(),
            chain_type: ChainType::Cosmos,
            assets: vec![
                Asset::Ibc { channel: "channel-0".into(), denom: "uosmo".into() },
                Asset::Ibc { channel: "channel-0".into(), denom: "uatom".into() },
            ],
            estimated_time_secs: 120,
            fee_estimate: 5000, // 0.005 OSMO
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deposit_route() {
        assert!(matches!(
            DepositRoute::for_asset(&Asset::Native),
            DepositRoute::Direct
        ));

        assert!(matches!(
            DepositRoute::for_asset(&Asset::Usdt),
            DepositRoute::XcmPolkadotAssetHub { .. }
        ));

        assert!(matches!(
            DepositRoute::for_asset(&Asset::Weth),
            DepositRoute::XcmKusamaAssetHub { .. }
        ));
    }

    #[tokio::test]
    async fn test_deposit_manager() {
        let mut manager = DepositManager::new();

        let request = manager.deposit_from_asset_hub(
            Asset::Usdt,
            1_000_000, // 1 USDT
            "5GrwvaEF".into(),
            "5Poker...".into(),
        ).await.unwrap();

        assert_eq!(request.amount, 1_000_000);
        assert_eq!(manager.pending.len(), 1);
    }

    #[test]
    fn test_available_sources() {
        let sources = available_deposit_sources();
        assert!(sources.len() >= 3);
    }
}
