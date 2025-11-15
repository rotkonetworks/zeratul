//! Effect Executor - executes side effects from core

use anyhow::Result;
use tokio::sync::mpsc;

use crate::core::{Effect, Event, AppCore, NotificationLevel};
use crate::network::penumbra::grpc_client::PenumbraGrpcClient;
use crate::wallet::Wallet;
use penumbra_dex::TradingPair;

/// Executes effects from the core
pub struct EffectExecutor {
    /// Wallet for signing transactions
    wallet: Option<Wallet>,

    /// Penumbra gRPC client
    client: Option<PenumbraGrpcClient>,

    /// Channel to send updates back to core
    event_tx: Option<mpsc::UnboundedSender<Event>>,
}

impl EffectExecutor {
    /// Create new executor
    pub async fn new(wallet: Option<Wallet>) -> Result<Self> {
        // Get RPC URL from environment or use default
        let rpc_url = std::env::var("PENUMBRA_RPC_URL")
            .unwrap_or_else(|_| "https://penumbra.rotko.net".to_string());

        eprintln!("Connecting to Penumbra RPC: {}", rpc_url);

        // Try to connect to Penumbra node
        let (mut client, _rx) = PenumbraGrpcClient::new(&rpc_url);

        // Connect to the node
        match client.connect().await {
            Ok(_) => eprintln!("✓ Connected to Penumbra node"),
            Err(e) => eprintln!("⚠ Failed to connect to Penumbra node: {}", e),
        }

        Ok(Self {
            wallet,
            client: Some(client),
            event_tx: None,
        })
    }

    /// Set event channel for sending updates back to core
    pub fn set_event_channel(&mut self, tx: mpsc::UnboundedSender<Event>) {
        self.event_tx = Some(tx);
    }

    /// Execute a single effect
    pub async fn execute(&mut self, effect: Effect) -> Result<()> {
        match effect {
            Effect::Render(_) => {
                // Rendering is handled by the shell directly
                Ok(())
            }

            Effect::SubmitPosition { side, price, size, fee_bps } => {
                self.submit_position(side, price, size, fee_bps).await
            }

            Effect::ClosePosition { position_id } => {
                self.close_position(&position_id).await
            }

            Effect::WithdrawPosition { position_id } => {
                self.withdraw_position(&position_id).await
            }

            Effect::StreamOrderBook { pair } => {
                self.stream_order_book(pair).await
            }

            Effect::FetchTrades { start_height, end_height } => {
                // TODO: Pass actual pair from context
                self.fetch_trades(start_height, end_height).await
            }

            Effect::FetchCandles { pair, duration_secs } => {
                self.fetch_candles(pair, duration_secs).await
            }

            Effect::RefreshBalances => {
                self.refresh_balances().await
            }

            Effect::ShowNotification { message, level } => {
                // TODO: Store notifications in a queue for display
                match level {
                    NotificationLevel::Info => println!("ℹ️  {}", message),
                    NotificationLevel::Success => println!("✅ {}", message),
                    NotificationLevel::Warning => println!("⚠️  {}", message),
                    NotificationLevel::Error => eprintln!("❌ {}", message),
                }
                Ok(())
            }

            Effect::ShowConfirmation { message, on_confirm } => {
                // TODO: Show modal confirmation dialog
                println!("⚠️  Confirmation needed: {}", message);
                Ok(())
            }

            Effect::SetCursor { x, y } => {
                // Terminal cursor positioning - handled by renderer
                Ok(())
            }

            Effect::Exit => {
                // Exit is handled by shell
                Ok(())
            }

            Effect::None => Ok(()),
        }
    }

    /// Poll for network updates and send events to core
    pub async fn poll_updates(&mut self, core: &mut AppCore) -> Result<()> {
        // TODO: Check for new trades, order book updates, balance changes
        // For now, just return OK
        Ok(())
    }

    // === Private implementation methods ===

    async fn submit_position(
        &mut self,
        side: crate::core::Side,
        price: rust_decimal::Decimal,
        size: rust_decimal::Decimal,
        fee_bps: u32,
    ) -> Result<()> {
        // Check wallet
        let _wallet = self.wallet.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Cannot submit position: No wallet loaded. Run wallet initialization first."))?;

        // Check connection
        let _client = self.client.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Cannot submit position: Not connected to Penumbra node. Check your network connection."))?;

        // TODO: Build and submit position transaction
        eprintln!("Submitting position: {} {} @ {} (fee: {}bps)",
                 side, size, price, fee_bps);

        // This would call wallet.submit_position() or similar
        // For now, just return error to prevent silent failures
        Err(anyhow::anyhow!("Position submission not yet implemented. Coming soon!"))
    }

    async fn close_position(&mut self, position_id: &str) -> Result<()> {
        let wallet = self.wallet.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No wallet loaded"))?;

        println!("Closing position: {}", position_id);

        // TODO: Build and submit close transaction
        Ok(())
    }

    async fn withdraw_position(&mut self, position_id: &str) -> Result<()> {
        let wallet = self.wallet.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No wallet loaded"))?;

        println!("Withdrawing position: {}", position_id);

        // TODO: Build and submit withdrawal transaction
        Ok(())
    }

    async fn stream_order_book(&mut self, pair: TradingPair) -> Result<()> {
        let client = self.client.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to Penumbra node"))?;

        println!("Starting order book stream for {:?}", pair);

        // TODO: Start gRPC stream and send OrderBookUpdated events
        // This should spawn a task that continuously polls and sends updates

        Ok(())
    }

    async fn fetch_trades(
        &mut self,
        start_height: u64,
        end_height: u64,
    ) -> Result<()> {
        let _client = self.client.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to Penumbra node"))?;

        println!("Fetching trades from block {} to {}", start_height, end_height);

        // TODO: Query SwapExecutions and send TradeExecuted events

        Ok(())
    }

    async fn fetch_candles(
        &mut self,
        pair: TradingPair,
        duration_secs: u64,
    ) -> Result<()> {
        let _client = self.client.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to Penumbra node"))?;

        println!("Fetching candles for {:?} with duration {}s", pair, duration_secs);

        // TODO: Query CandlestickDataStream and send CandleUpdated events

        Ok(())
    }

    async fn refresh_balances(&mut self) -> Result<()> {
        let wallet = self.wallet.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No wallet loaded"))?;

        println!("Refreshing balances...");

        // TODO: Query wallet balances and send BalancesUpdated event

        Ok(())
    }
}
