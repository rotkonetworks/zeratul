//! Penumbra network client - connects to https://penumbra.rotko.net

pub mod grpc_client;

use anyhow::Result;
use penumbra_proto::core::component::dex::v1::{
    dex_query_service_client::DexQueryServiceClient,
    LiquidityPositionsRequest,
};
use penumbra_keys::{Address, FullViewingKey};
use penumbra_asset::asset::Id as AssetId;

use super::{NetworkClient, OrderBook, TradingPair, Trade, Order, Level};

/// Penumbra network client
pub struct PenumbraClient {
    /// gRPC endpoint
    endpoint: String,

    /// Full viewing key for queries
    fvk: FullViewingKey,

    /// Address
    address: Address,
}

impl PenumbraClient {
    /// Connect to Penumbra network
    pub async fn connect(endpoint: impl Into<String>, fvk: FullViewingKey) -> Result<Self> {
        let endpoint = endpoint.into();
        let address = fvk.payment_address(Default::default()).0;

        Ok(Self {
            endpoint,
            fvk,
            address,
        })
    }

    /// Connect to Rotko's Penumbra node
    pub async fn connect_rotko(fvk: FullViewingKey) -> Result<Self> {
        Self::connect("https://penumbra.rotko.net", fvk).await
    }
}

#[async_trait::async_trait]
impl NetworkClient for PenumbraClient {
    async fn get_order_book(&mut self, pair: &TradingPair) -> Result<OrderBook> {
        let mut client = DexQueryServiceClient::connect(self.endpoint.clone()).await?;

        // Query liquidity positions for this trading pair
        let request = LiquidityPositionsRequest {
            // Filter by trading pair
            // TODO: Implement proper filtering
            ..Default::default()
        };

        let response = client.liquidity_positions(request).await?;
        let positions = response.into_inner();

        // Convert Penumbra liquidity positions to order book
        // TODO: Implement conversion from concentrated liquidity positions

        Ok(OrderBook {
            pair: pair.clone(),
            bids: vec![],
            asks: vec![],
        })
    }

    async fn get_recent_trades(&mut self, _pair: &TradingPair, _limit: usize) -> Result<Vec<Trade>> {
        // TODO: Query Penumbra for recent swap executions
        Ok(vec![])
    }

    async fn submit_order(&mut self, _order: Order) -> Result<String> {
        // TODO: Create Penumbra swap transaction
        // This requires:
        // 1. Build swap plaintext
        // 2. Generate ZK proof
        // 3. Submit transaction

        anyhow::bail!("Order submission not yet implemented")
    }

    fn address(&self) -> String {
        self.address.to_string()
    }
}

/// Helper to create test FVK for development
pub fn test_fvk() -> FullViewingKey {
    use penumbra_keys::keys::{SeedPhrase, SpendSeed};

    // Use a test seed phrase (DO NOT use in production!)
    let seed_phrase = SeedPhrase::from_randomness(&[0u8; 32]);
    let spend_seed = SpendSeed::from_seed_phrase(seed_phrase, 0);

    spend_seed.to_spend_key().full_viewing_key().clone()
}
