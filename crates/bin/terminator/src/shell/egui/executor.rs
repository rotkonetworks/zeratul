//! Effect Executor - executes side effects from core

use anyhow::Result;
use crate::core::{Effect, AppCore};
use crate::wallet::Wallet;

/// Executes effects (side effects from core logic)
pub struct EffectExecutor {
    wallet: Option<Wallet>,
}

impl EffectExecutor {
    pub async fn new(wallet: Option<Wallet>) -> Result<Self> {
        Ok(Self { wallet })
    }

    /// Execute a single effect
    pub async fn execute(&mut self, effect: Effect, _core: &mut AppCore) -> Result<()> {
        match effect {
            Effect::SubmitPosition { side, price, size, fee_bps } => {
                println!("Would submit position: {:?} {} @ {} (fee: {}bps)", side, size, price, fee_bps);
                // TODO: Implement actual position submission
            }

            Effect::ClosePosition { position_id } => {
                println!("Would close position: {}", position_id);
                // TODO: Implement actual position closing
            }

            Effect::Render(_) => {
                // egui handles rendering automatically
            }

            Effect::Exit => {
                // Handle exit
                std::process::exit(0);
            }

            _ => {
                // Ignore unknown effects
            }
        }

        Ok(())
    }
}
