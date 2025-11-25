//! epoch management and gigaproof generation

use crate::{
    error::{Result, ZidecarError},
    header_chain::HeaderChainTrace,
    prover::HeaderChainProof,
    storage::Storage,
    zebrad::ZebradClient,
};
use ligerito::ProverConfig;
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

/// epoch proof manager
pub struct EpochManager {
    zebrad: ZebradClient,
    storage: Arc<Storage>,
    gigaproof_config: ProverConfig<BinaryElem32, BinaryElem128>,
    tip_config: ProverConfig<BinaryElem32, BinaryElem128>,
    start_height: u32,
    /// last complete epoch that has a gigaproof
    last_gigaproof_epoch: Arc<RwLock<Option<u32>>>,
}

impl EpochManager {
    pub fn new(
        zebrad: ZebradClient,
        storage: Arc<Storage>,
        gigaproof_config: ProverConfig<BinaryElem32, BinaryElem128>,
        tip_config: ProverConfig<BinaryElem32, BinaryElem128>,
        start_height: u32,
    ) -> Self {
        Self {
            zebrad,
            storage,
            gigaproof_config,
            tip_config,
            start_height,
            last_gigaproof_epoch: Arc::new(RwLock::new(None)),
        }
    }

    /// get current chain tip
    async fn get_current_height(&self) -> Result<u32> {
        let info = self.zebrad.get_blockchain_info().await?;
        Ok(info.blocks)
    }

    /// calculate epoch for height
    fn epoch_for_height(&self, height: u32) -> u32 {
        height / zync_core::EPOCH_SIZE
    }

    /// get epoch boundary heights
    fn epoch_range(&self, epoch: u32) -> (u32, u32) {
        let start = epoch * zync_core::EPOCH_SIZE;
        let end = start + zync_core::EPOCH_SIZE - 1;
        (start, end)
    }

    /// check if gigaproof exists for epoch range
    async fn has_gigaproof(&self, from_epoch: u32, to_epoch: u32) -> Result<bool> {
        let from_height = self.epoch_range(from_epoch).0;
        let to_height = self.epoch_range(to_epoch).1;

        Ok(self.storage.get_proof(from_height, to_height)?.is_some())
    }

    /// generate gigaproof from start to last complete epoch
    pub async fn generate_gigaproof(&self) -> Result<()> {
        let current_height = self.get_current_height().await?;
        let current_epoch = self.epoch_for_height(current_height);

        // last complete epoch (current epoch might not be complete yet)
        let last_complete_epoch = if current_height % zync_core::EPOCH_SIZE == 0 {
            current_epoch
        } else {
            current_epoch.saturating_sub(1)
        };

        // check if we're still in epoch 0 and haven't completed it
        if current_epoch == 0 && current_height < zync_core::EPOCH_SIZE {
            info!("no complete epochs yet (at block {} / {})", current_height, zync_core::EPOCH_SIZE);
            return Ok(());
        }

        // check if we already have this gigaproof
        let start_epoch = self.epoch_for_height(self.start_height);
        if self.has_gigaproof(start_epoch, last_complete_epoch).await? {
            info!("gigaproof already exists for epochs {} -> {}", start_epoch, last_complete_epoch);
            *self.last_gigaproof_epoch.write().await = Some(last_complete_epoch);
            return Ok(());
        }

        let (from_height, to_height) = (
            self.start_height,
            self.epoch_range(last_complete_epoch).1,
        );

        info!(
            "generating GIGAPROOF: height {} -> {} (epochs {} -> {})",
            from_height, to_height, start_epoch, last_complete_epoch
        );

        // build trace
        let mut trace = HeaderChainTrace::build(&self.zebrad, from_height, to_height).await?;

        // generate proof (auto-select config based on trace size)
        let proof = HeaderChainProof::prove_auto(&mut trace)?;

        // cache it
        self.storage
            .store_proof(from_height, to_height, &proof.proof_bytes)?;

        info!(
            "GIGAPROOF generated: {} blocks, {} KB",
            to_height - from_height + 1,
            proof.proof_bytes.len() / 1024
        );

        *self.last_gigaproof_epoch.write().await = Some(last_complete_epoch);

        Ok(())
    }

    /// background task: generate gigaproof every epoch
    pub async fn run_background_prover(self: Arc<Self>) {
        info!("starting background gigaproof generator");

        loop {
            match self.generate_gigaproof().await {
                Ok(_) => {
                    info!("gigaproof generation complete");
                }
                Err(e) => {
                    error!("gigaproof generation failed: {}", e);
                }
            }

            // check every hour
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    }

    /// get gigaproof + tip proof for current chain state
    pub async fn get_proofs(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let current_height = self.get_current_height().await?;
        let current_epoch = self.epoch_for_height(current_height);

        let last_gigaproof = self.last_gigaproof_epoch.read().await;

        let gigaproof_epoch = match *last_gigaproof {
            Some(e) => e,
            None => {
                return Err(ZidecarError::ProofGeneration(
                    "no gigaproof available yet".into(),
                ))
            }
        };

        // get cached gigaproof
        let gigaproof_to = self.epoch_range(gigaproof_epoch).1;
        let gigaproof = self
            .storage
            .get_proof(self.start_height, gigaproof_to)?
            .ok_or_else(|| {
                ZidecarError::ProofGeneration("gigaproof not found in cache".into())
            })?;

        // generate tip proof (from last gigaproof to current tip)
        let tip_from = gigaproof_to + 1;

        if tip_from > current_height {
            // no tip needed, gigaproof is fresh
            info!("gigaproof is up to date, no tip proof needed");
            return Ok((gigaproof, vec![]));
        }

        // check if tip crosses epoch boundary (should be max 1 epoch)
        let blocks_in_tip = current_height - tip_from + 1;
        if blocks_in_tip > zync_core::EPOCH_SIZE {
            warn!(
                "tip proof spans {} blocks (> 1 epoch), gigaproof may be stale",
                blocks_in_tip
            );
        }

        info!(
            "generating tip proof: {} -> {} ({} blocks)",
            tip_from,
            current_height,
            blocks_in_tip
        );

        // build tip trace
        let mut tip_trace = HeaderChainTrace::build(&self.zebrad, tip_from, current_height).await?;

        // generate tip proof (auto-select config)
        let tip_proof = HeaderChainProof::prove_auto(&mut tip_trace)?;

        info!("tip proof generated: {} KB", tip_proof.proof_bytes.len() / 1024);

        Ok((gigaproof, tip_proof.proof_bytes))
    }

    /// get last complete epoch height
    pub async fn last_complete_epoch_height(&self) -> Result<u32> {
        let current_height = self.get_current_height().await?;
        let current_epoch = self.epoch_for_height(current_height);

        let last_complete = if current_height % zync_core::EPOCH_SIZE == 0 {
            current_epoch
        } else {
            current_epoch.saturating_sub(1)
        };

        Ok(self.epoch_range(last_complete).1)
    }

    /// check if any gigaproof is available
    pub async fn is_gigaproof_ready(&self) -> Result<bool> {
        Ok(self.last_gigaproof_epoch.read().await.is_some())
    }
}
