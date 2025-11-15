//! Game logic and player actions

use crate::{MoveTransaction, PlayerId, BlockNumber, consensus::Consensus, network::Network};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::info;

/// Player in the multiplayer game
pub struct Player {
    pub id: PlayerId,
    consensus: Consensus,
    network: Network,
    last_finalized_block: BlockNumber,
}

impl Player {
    pub fn new(
        id: PlayerId,
        consensus: Consensus,
        network: Network,
    ) -> Self {
        Self {
            id,
            consensus,
            network,
            last_finalized_block: 0,
        }
    }

    /// Make a move (click cell)
    pub async fn make_move(&mut self, x: usize, y: usize) -> Result<(), Box<dyn std::error::Error>> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis() as u64;

        let block_number = self.consensus.current_block(now_ms);

        info!("Making move at ({}, {}) for block {}", x, y, block_number);

        // Create move transaction
        // TODO: Generate actual proof
        let tx = MoveTransaction {
            player: self.id,
            x,
            y,
            block_number,
            proof: vec![0; 32], // Placeholder proof
            signature: [0; 64], // Placeholder signature
        };

        // Add to local consensus
        self.consensus.add_move(tx.clone())?;

        // Broadcast to network
        self.network.broadcast_move(tx).await?;

        Ok(())
    }

    /// Run game loop: finalize blocks every 1 second
    pub async fn run(&mut self, mut network_rx: mpsc::Receiver<crate::network::NetworkMessage>) {
        info!("Player {:?} starting game loop", &self.id[..8]);

        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(100));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.tick().await;
                }

                Some(msg) = network_rx.recv() => {
                    self.handle_network_message(msg).await;
                }
            }
        }
    }

    async fn tick(&mut self) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let current_block = self.consensus.current_block(now_ms);

        // Finalize blocks that are ready
        while self.last_finalized_block < current_block {
            self.last_finalized_block += 1;
            self.consensus.finalize_block(self.last_finalized_block);
        }
    }

    async fn handle_network_message(&mut self, msg: crate::network::NetworkMessage) {
        match msg {
            crate::network::NetworkMessage::Move(tx) => {
                if let Err(e) = self.consensus.add_move(tx) {
                    tracing::warn!("Failed to add move: {}", e);
                }
            }
            _ => {}
        }
    }

    /// Get current game state
    pub fn state(&self) -> &crate::GameState {
        self.consensus.current_state()
    }
}
