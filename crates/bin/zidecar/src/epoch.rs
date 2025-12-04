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

/// regenerate tip proof every N new blocks
const TIP_PROOF_REGEN_BLOCKS: u32 = 1; // real-time: regenerate on every new block

/// cached tip proof with its coverage
struct CachedTipProof {
    proof: Vec<u8>,
    from_height: u32,
    to_height: u32,
}

/// epoch proof manager
pub struct EpochManager {
    zebrad: ZebradClient,
    storage: Arc<Storage>,
    gigaproof_config: ProverConfig<BinaryElem32, BinaryElem128>,
    tip_config: ProverConfig<BinaryElem32, BinaryElem128>,
    start_height: u32,
    /// last complete epoch that has a gigaproof (in-memory cache)
    last_gigaproof_epoch: Arc<RwLock<Option<u32>>>,
    /// cached tip proof (pre-generated)
    cached_tip_proof: Arc<RwLock<Option<CachedTipProof>>>,
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
            cached_tip_proof: Arc::new(RwLock::new(None)),
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

        // pad trace to config size (2^26)
        let required_size = 1 << zync_core::GIGAPROOF_TRACE_LOG_SIZE;
        if trace.trace.len() < required_size {
            info!("padding gigaproof trace from {} to {} elements", trace.trace.len(), required_size);
            use ligerito_binary_fields::BinaryFieldElement;
            trace.trace.resize(required_size, ligerito_binary_fields::BinaryElem32::zero());
        }

        // generate proof with explicit gigaproof config (2^26)
        let proof = HeaderChainProof::prove(&self.gigaproof_config, &trace)?;

        // serialize full proof with public outputs
        let full_proof = proof.serialize_full()?;

        // cache the proof
        self.storage
            .store_proof(from_height, to_height, &full_proof)?;

        // persist gigaproof metadata
        self.storage.set_gigaproof_epoch(last_complete_epoch)?;
        self.storage.set_gigaproof_start(from_height)?;

        info!(
            "GIGAPROOF generated: {} blocks, {} KB (tip_hash: {})",
            to_height - from_height + 1,
            full_proof.len() / 1024,
            hex::encode(&proof.public_outputs.tip_hash[..8])
        );

        *self.last_gigaproof_epoch.write().await = Some(last_complete_epoch);

        Ok(())
    }

    /// background task: generate gigaproof every epoch
    pub async fn run_background_prover(self: Arc<Self>) {
        info!("starting background gigaproof generator");

        loop {
            // check every 60 seconds for new complete epochs
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

            match self.generate_gigaproof().await {
                Ok(_) => {}
                Err(e) => {
                    error!("gigaproof generation failed: {}", e);
                }
            }
        }
    }

    /// background task: keep tip proof up-to-date (real-time proving)
    pub async fn run_background_tip_prover(self: Arc<Self>) {
        info!("starting background tip proof generator");

        let mut last_proven_height: u32 = 0;

        loop {
            // check every second for new blocks (real-time proving)
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            let current_height = match self.get_current_height().await {
                Ok(h) => h,
                Err(e) => {
                    warn!("failed to get current height: {}", e);
                    continue;
                }
            };

            // check if we need to regenerate (N new blocks since last proof)
            let blocks_since = current_height.saturating_sub(last_proven_height);
            if blocks_since < TIP_PROOF_REGEN_BLOCKS {
                continue; // not enough new blocks
            }

            // get gigaproof endpoint
            let gigaproof_epoch = {
                let cached = self.last_gigaproof_epoch.read().await;
                match *cached {
                    Some(e) => e,
                    None => match self.storage.get_gigaproof_epoch() {
                        Ok(Some(e)) => e,
                        _ => continue, // no gigaproof yet
                    }
                }
            };

            let tip_from = self.epoch_range(gigaproof_epoch).1 + 1;
            if tip_from > current_height {
                // gigaproof is fresh, no tip needed
                last_proven_height = current_height;
                continue;
            }

            let blocks_in_tip = current_height - tip_from + 1;
            info!(
                "generating tip proof: {} -> {} ({} blocks, {} new since last)",
                tip_from, current_height, blocks_in_tip, blocks_since
            );

            // build and generate tip proof
            match HeaderChainTrace::build(&self.zebrad, &self.storage, tip_from, current_height).await {
                Ok(mut tip_trace) => {
                    // pad trace to tip config size (2^20)
                    let required_size = 1 << zync_core::TIP_TRACE_LOG_SIZE;
                    if tip_trace.trace.len() < required_size {
                        use ligerito_binary_fields::BinaryFieldElement;
                        tip_trace.trace.resize(required_size, ligerito_binary_fields::BinaryElem32::zero());
                    }
                    match HeaderChainProof::prove(&self.tip_config, &tip_trace) {
                        Ok(tip_proof) => {
                            match tip_proof.serialize_full() {
                                Ok(tip_proof_bytes) => {
                                    info!(
                                        "tip proof ready: {} KB, covers {} -> {} (tip: {})",
                                        tip_proof_bytes.len() / 1024,
                                        tip_from,
                                        current_height,
                                        hex::encode(&tip_proof.public_outputs.tip_hash[..8])
                                    );

                                    // cache the tip proof
                                    *self.cached_tip_proof.write().await = Some(CachedTipProof {
                                        proof: tip_proof_bytes,
                                        from_height: tip_from,
                                        to_height: current_height,
                                    });

                                    last_proven_height = current_height;
                                }
                                Err(e) => error!("tip proof serialization failed: {}", e),
                            }
                        }
                        Err(e) => error!("tip proof generation failed: {}", e),
                    }
                }
                Err(e) => error!("tip trace build failed: {}", e),
            }
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

        // get tip proof (from last gigaproof to current tip)
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

        // try to use cached tip proof first (from background prover)
        {
            let cached = self.cached_tip_proof.read().await;
            if let Some(ref tip) = *cached {
                // use cached if it starts from the right place and is reasonably fresh
                // (within TIP_PROOF_REGEN_BLOCKS of current height)
                if tip.from_height == tip_from &&
                   current_height.saturating_sub(tip.to_height) < TIP_PROOF_REGEN_BLOCKS * 2 {
                    info!(
                        "using cached tip proof: {} -> {} ({} KB)",
                        tip.from_height, tip.to_height, tip.proof.len() / 1024
                    );
                    return Ok((gigaproof, tip.proof.clone()));
                }
            }
        }

        // fallback: generate on-demand (should be rare with background prover)
        info!(
            "generating tip proof on-demand: {} -> {} ({} blocks)",
            tip_from,
            current_height,
            blocks_in_tip
        );

        // build tip trace (with caching)
        let mut tip_trace = HeaderChainTrace::build(&self.zebrad, &self.storage, tip_from, current_height).await?;

        // pad trace to tip config size (2^20)
        let required_size = 1 << zync_core::TIP_TRACE_LOG_SIZE;
        if tip_trace.trace.len() < required_size {
            use ligerito_binary_fields::BinaryFieldElement;
            tip_trace.trace.resize(required_size, ligerito_binary_fields::BinaryElem32::zero());
        }

        // generate tip proof with explicit config (2^20)
        let tip_proof = HeaderChainProof::prove(&self.tip_config, &tip_trace)?;
        let tip_proof_bytes = tip_proof.serialize_full()?;

        info!(
            "tip proof generated: {} KB (tip_hash: {})",
            tip_proof_bytes.len() / 1024,
            hex::encode(&tip_proof.public_outputs.tip_hash[..8])
        );

        // cache this proof for future requests
        *self.cached_tip_proof.write().await = Some(CachedTipProof {
            proof: tip_proof_bytes.clone(),
            from_height: tip_from,
            to_height: current_height,
        });

        Ok((gigaproof, tip_proof_bytes))
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

        // Calculate first epoch that we have complete data for
        // (first epoch may be partial if start_height isn't at epoch boundary)
        let first_full_epoch = if self.start_height % zync_core::EPOCH_SIZE == 0 {
            from_epoch
        } else {
            from_epoch + 1 // skip partial first epoch
        };

        for epoch in first_full_epoch..=to_epoch {
            // skip if already stored
            if self.storage.get_epoch_boundary(epoch)?.is_some() {
                continue;
            }

            let (first_height, last_height) = self.epoch_range(epoch);

            // Skip if first block is before our start height (partial epoch)
            if first_height < self.start_height {
                info!("skipping partial epoch {} (starts at {} < start {})", epoch, first_height, self.start_height);
                continue;
            }

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
        // Skip start_epoch + 1 since we don't have start_epoch - 1 boundary data
        let start_epoch = self.epoch_for_height(self.start_height);
        for epoch in (from_epoch + 1)..=to_epoch {
            if epoch <= start_epoch + 1 {
                continue; // can't verify - no prior epoch boundary data
            }
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

                    // get nullifier root from nomt (populated by nullifier sync)
                    let nullifier_root = self.storage.get_nullifier_root();

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

/// Nullifier extracted from a shielded spend
#[derive(Debug, Clone)]
pub struct ExtractedNullifier {
    pub nullifier: [u8; 32],
    pub pool: NullifierPool,
}

/// Which shielded pool the nullifier came from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullifierPool {
    Sapling,
    Orchard,
}

impl EpochManager {
    /// Extract all nullifiers from a block
    pub async fn extract_nullifiers_from_block(&self, height: u32) -> Result<Vec<ExtractedNullifier>> {
        let hash = self.zebrad.get_block_hash(height).await?;
        let block = self.zebrad.get_block_with_txs(&hash).await?;

        let mut nullifiers = Vec::new();

        for tx in &block.tx {
            // Sapling spends
            if let Some(ref spends) = tx.sapling_spends {
                for spend in spends {
                    if let Some(nf) = spend.nullifier_bytes() {
                        nullifiers.push(ExtractedNullifier {
                            nullifier: nf,
                            pool: NullifierPool::Sapling,
                        });
                    }
                }
            }

            // Orchard actions (each action has a nullifier)
            if let Some(ref orchard) = tx.orchard {
                for action in &orchard.actions {
                    if let Some(nf) = action.nullifier_bytes() {
                        nullifiers.push(ExtractedNullifier {
                            nullifier: nf,
                            pool: NullifierPool::Orchard,
                        });
                    }
                }
            }
        }

        Ok(nullifiers)
    }

    /// Sync nullifiers from a range of blocks into nomt
    pub async fn sync_nullifiers(&self, from_height: u32, to_height: u32) -> Result<u32> {
        let mut total_nullifiers = 0u32;

        for height in from_height..=to_height {
            let nullifiers = self.extract_nullifiers_from_block(height).await?;

            if !nullifiers.is_empty() {
                // Batch insert into nomt
                let nf_bytes: Vec<[u8; 32]> = nullifiers.iter().map(|n| n.nullifier).collect();
                self.storage.batch_insert_nullifiers(&nf_bytes, height)?;
                total_nullifiers += nullifiers.len() as u32;
            }

            // Update sync progress
            self.storage.set_nullifier_sync_height(height)?;

            // Log progress every 1000 blocks
            if height % 1000 == 0 {
                info!(
                    "nullifier sync progress: height {}/{} ({} nullifiers so far)",
                    height, to_height, total_nullifiers
                );
            }
        }

        Ok(total_nullifiers)
    }

    /// Background task: sync nullifiers incrementally
    pub async fn run_background_nullifier_sync(self: Arc<Self>) {
        info!("starting background nullifier sync");

        // Get current sync progress
        let mut last_synced = self.storage.get_nullifier_sync_height()
            .unwrap_or(None)
            .unwrap_or(self.start_height.saturating_sub(1));

        info!("nullifier sync starting from height {}", last_synced + 1);

        loop {
            // Get current chain height
            let current_height = match self.get_current_height().await {
                Ok(h) => h,
                Err(e) => {
                    warn!("failed to get current height for nullifier sync: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    continue;
                }
            };

            // Sync in batches of 100 blocks
            if last_synced < current_height {
                let batch_end = (last_synced + 100).min(current_height);

                match self.sync_nullifiers(last_synced + 1, batch_end).await {
                    Ok(count) => {
                        if count > 0 {
                            info!(
                                "synced {} nullifiers for blocks {} -> {} (root: {})",
                                count, last_synced + 1, batch_end,
                                hex::encode(&self.storage.get_nullifier_root()[..8])
                            );
                        }
                        last_synced = batch_end;
                    }
                    Err(e) => {
                        error!("nullifier sync failed at height {}: {}", last_synced + 1, e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                        continue;
                    }
                }
            }

            // Check every 5 seconds for new blocks
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }

    /// Get current nullifier root from nomt
    pub fn get_nullifier_root(&self) -> [u8; 32] {
        self.storage.get_nullifier_root()
    }
}
