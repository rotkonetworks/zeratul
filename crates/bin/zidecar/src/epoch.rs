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

/// maximum epochs to cover with tip proof before regenerating gigaproof
/// Tip proof uses 2^24 config which handles up to 2M elements
/// 2M / 8 fields = 256K headers = ~250 epochs
/// We use 200 as a safe threshold (~200K blocks, ~1.3s proof)
const GIGAPROOF_REGEN_THRESHOLD: u32 = 200;

/// epoch proof manager
pub struct EpochManager {
    zebrad: ZebradClient,
    storage: Arc<Storage>,
    gigaproof_config: ProverConfig<BinaryElem32, BinaryElem128>,
    tip_config: ProverConfig<BinaryElem32, BinaryElem128>,
    start_height: u32,
    /// last complete epoch that has a gigaproof (in-memory cache)
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
    /// Uses incremental strategy: only regenerate when significantly behind
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
        let start_epoch = self.epoch_for_height(self.start_height);
        if last_complete_epoch < start_epoch {
            info!("no complete epochs yet (at block {} / {})", current_height, zync_core::EPOCH_SIZE);
            return Ok(());
        }

        // load persisted gigaproof epoch from storage
        let cached_epoch = self.storage.get_gigaproof_epoch()?;

        // check if we already have the latest gigaproof
        if let Some(cached) = cached_epoch {
            if cached >= last_complete_epoch {
                // already up to date, ensure in-memory cache matches
                *self.last_gigaproof_epoch.write().await = Some(cached);
                info!("gigaproof already exists for epochs {} -> {} (cached)", start_epoch, cached);
                return Ok(());
            }

            // check how many epochs behind we are
            let epochs_behind = last_complete_epoch - cached;
            if epochs_behind < GIGAPROOF_REGEN_THRESHOLD {
                // not far enough behind - tip proof will cover the gap
                info!(
                    "gigaproof {} epochs behind (threshold {}), using tip proof for gap",
                    epochs_behind, GIGAPROOF_REGEN_THRESHOLD
                );
                *self.last_gigaproof_epoch.write().await = Some(cached);
                return Ok(());
            }

            info!(
                "gigaproof {} epochs behind (>= threshold {}), regenerating",
                epochs_behind, GIGAPROOF_REGEN_THRESHOLD
            );
        }

        let (from_height, to_height) = (
            self.start_height,
            self.epoch_range(last_complete_epoch).1,
        );

        info!(
            "generating GIGAPROOF: height {} -> {} (epochs {} -> {})",
            from_height, to_height, start_epoch, last_complete_epoch
        );

        // build trace (with caching - headers already fetched won't be refetched)
        let mut trace = HeaderChainTrace::build(&self.zebrad, &self.storage, from_height, to_height).await?;

        // Store epoch boundary hashes for chain continuity verification
        self.store_epoch_boundaries(start_epoch, last_complete_epoch).await?;

        // generate proof (auto-select config based on trace size)
        let proof = HeaderChainProof::prove_auto(&mut trace)?;

        // cache the proof
        self.storage
            .store_proof(from_height, to_height, &proof.proof_bytes)?;

        // persist gigaproof metadata
        self.storage.set_gigaproof_epoch(last_complete_epoch)?;
        self.storage.set_gigaproof_start(from_height)?;

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

        // try in-memory cache first, then persistent storage
        let gigaproof_epoch = {
            let cached = self.last_gigaproof_epoch.read().await;
            match *cached {
                Some(e) => e,
                None => {
                    // try loading from persistent storage
                    match self.storage.get_gigaproof_epoch()? {
                        Some(e) => {
                            drop(cached);
                            *self.last_gigaproof_epoch.write().await = Some(e);
                            e
                        }
                        None => {
                            return Err(ZidecarError::ProofGeneration(
                                "no gigaproof available yet".into(),
                            ))
                        }
                    }
                }
            }
        };

        // get cached gigaproof
        let gigaproof_to = self.epoch_range(gigaproof_epoch).1;
        let gigaproof_start = self.storage.get_gigaproof_start()?.unwrap_or(self.start_height);

        let gigaproof = self
            .storage
            .get_proof(gigaproof_start, gigaproof_to)?
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

        // check if tip crosses epoch boundary - warn if > threshold
        let blocks_in_tip = current_height - tip_from + 1;
        let max_tip_blocks = GIGAPROOF_REGEN_THRESHOLD * zync_core::EPOCH_SIZE;
        if blocks_in_tip > max_tip_blocks {
            warn!(
                "tip proof spans {} blocks (> {} epoch threshold), consider regenerating gigaproof",
                blocks_in_tip, GIGAPROOF_REGEN_THRESHOLD
            );
        }

        info!(
            "generating tip proof: {} -> {} ({} blocks)",
            tip_from,
            current_height,
            blocks_in_tip
        );

        // build tip trace (with caching)
        let mut tip_trace = HeaderChainTrace::build(&self.zebrad, &self.storage, tip_from, current_height).await?;

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

    /// background task: track state roots at epoch boundaries
    pub async fn run_background_state_tracker(self: Arc<Self>) {
        info!("starting background state root tracker");

        let mut last_tracked_height = self.start_height;

        loop {
            match self.track_state_roots(last_tracked_height).await {
                Ok(new_height) => {
                    if new_height > last_tracked_height {
                        info!(
                            "tracked state roots up to height {} ({} new epochs)",
                            new_height,
                            (new_height - last_tracked_height) / zync_core::EPOCH_SIZE
                        );
                        last_tracked_height = new_height;
                    }
                }
                Err(e) => {
                    warn!("state root tracking failed: {}", e);
                }
            }

            // check every 10 minutes
            tokio::time::sleep(tokio::time::Duration::from_secs(600)).await;
        }
    }

    /// store epoch boundary hashes for chain continuity verification
    async fn store_epoch_boundaries(&self, from_epoch: u32, to_epoch: u32) -> Result<()> {
        info!("storing epoch boundary hashes for epochs {} -> {}", from_epoch, to_epoch);

        for epoch in from_epoch..=to_epoch {
            // skip if already stored
            if self.storage.get_epoch_boundary(epoch)?.is_some() {
                continue;
            }

            let (first_height, last_height) = self.epoch_range(epoch);

            // Get first block of epoch
            let first_header = self.storage.get_header(first_height)?
                .ok_or_else(|| ZidecarError::BlockNotFound(first_height))?;
            let first_hash = hex_to_bytes32(&first_header.0)?;
            let first_prev_hash = if first_header.1.is_empty() {
                [0u8; 32] // genesis
            } else {
                hex_to_bytes32(&first_header.1)?
            };

            // Get last block of epoch
            let last_header = self.storage.get_header(last_height)?
                .ok_or_else(|| ZidecarError::BlockNotFound(last_height))?;
            let last_hash = hex_to_bytes32(&last_header.0)?;

            // Store boundary
            self.storage.store_epoch_boundary(
                epoch,
                first_height,
                &first_hash,
                &first_prev_hash,
                last_height,
                &last_hash,
            )?;

            info!(
                "epoch {} boundary: first={}@{} last={}@{}",
                epoch,
                hex::encode(&first_hash[..4]),
                first_height,
                hex::encode(&last_hash[..4]),
                last_height
            );
        }

        // Verify chain continuity between epochs
        for epoch in (from_epoch + 1)..=to_epoch {
            if !self.storage.verify_epoch_continuity(epoch)? {
                warn!("epoch {} continuity check failed!", epoch);
            }
        }

        Ok(())
    }

    /// track state roots at epoch boundaries from zebrad
    async fn track_state_roots(&self, from_height: u32) -> Result<u32> {
        let current_height = self.get_current_height().await?;
        let current_epoch = self.epoch_for_height(current_height);
        let from_epoch = self.epoch_for_height(from_height);

        let mut latest_tracked = from_height;

        // iterate through epochs and get tree state at each boundary
        for epoch in from_epoch..=current_epoch {
            let epoch_end = self.epoch_range(epoch).1;

            // skip if beyond current height
            if epoch_end > current_height {
                break;
            }

            // skip if already tracked
            if self.storage.get_state_roots(epoch_end)?.is_some() {
                latest_tracked = epoch_end;
                continue;
            }

            // get tree state from zebrad
            match self.zebrad.get_tree_state(&epoch_end.to_string()).await {
                Ok(state) => {
                    // parse orchard tree root from state
                    let tree_root = parse_tree_root(&state.orchard.commitments.final_state);

                    // for nullifier root, we use NOMT which tracks separately
                    // for now use a placeholder derived from tree root
                    let nullifier_root = derive_nullifier_root(&tree_root);

                    // store roots
                    self.storage.store_state_roots(epoch_end, &tree_root, &nullifier_root)?;

                    info!(
                        "stored state roots at height {}: tree={} nullifier={}",
                        epoch_end,
                        hex::encode(&tree_root[..8]),
                        hex::encode(&nullifier_root[..8])
                    );

                    latest_tracked = epoch_end;
                }
                Err(e) => {
                    warn!("failed to get tree state at height {}: {}", epoch_end, e);
                }
            }
        }

        Ok(latest_tracked)
    }
}

/// convert hex string to [u8; 32]
fn hex_to_bytes32(hex: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex)
        .map_err(|e| ZidecarError::Serialization(format!("invalid hex: {}", e)))?;
    if bytes.len() != 32 {
        return Err(ZidecarError::Serialization(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut result = [0u8; 32];
    result.copy_from_slice(&bytes);
    Ok(result)
}

/// parse tree root from zebrad hex-encoded final state
fn parse_tree_root(final_state: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    // zebrad returns a hex-encoded frontier, we hash it to get a 32-byte root
    // (in production, properly parse the Orchard Frontier)
    let mut hasher = Sha256::new();
    hasher.update(b"ZIDECAR_TREE_ROOT");
    hasher.update(final_state.as_bytes());
    hasher.finalize().into()
}

/// derive nullifier root from tree root (placeholder)
/// in production, would track actual nullifiers seen
fn derive_nullifier_root(tree_root: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"ZIDECAR_NULLIFIER_ROOT");
    hasher.update(tree_root);
    hasher.finalize().into()
}
