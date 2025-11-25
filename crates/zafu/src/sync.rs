//! sync orchestrator - coordinates proof verification and scanning

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::{
    client::ZidecarClient,
    verifier::ProofVerifier,
    scanner::WalletScanner,
    storage::WalletStorage,
};

pub struct SyncOrchestrator {
    client: Arc<RwLock<ZidecarClient>>,
    verifier: Arc<ProofVerifier>,
    scanner: Arc<RwLock<WalletScanner>>,
    storage: Arc<WalletStorage>,
}

#[derive(Debug, Clone)]
pub struct SyncProgress {
    pub phase: SyncPhase,
    pub progress: f32,
    pub message: String,
    pub current_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncPhase {
    Connecting,
    VerifyingProofs,
    DownloadingBlocks,
    Scanning,
    Complete,
    Error,
}

impl SyncOrchestrator {
    pub fn new(
        client: Arc<RwLock<ZidecarClient>>,
        verifier: Arc<ProofVerifier>,
        scanner: Arc<RwLock<WalletScanner>>,
        storage: Arc<WalletStorage>,
    ) -> Self {
        Self {
            client,
            verifier,
            scanner,
            storage,
        }
    }

    /// full sync flow
    pub async fn sync(&self) -> Result<SyncProgress> {
        info!("starting sync");

        // phase 1: verify proofs
        let mut progress = self.verify_proofs().await?;
        if matches!(progress.phase, SyncPhase::Error) {
            return Ok(progress);
        }

        // phase 2: download and scan blocks
        progress = self.download_and_scan().await?;

        Ok(progress)
    }

    async fn verify_proofs(&self) -> Result<SyncProgress> {
        info!("phase 1: verifying proofs");

        let mut progress = SyncProgress {
            phase: SyncPhase::VerifyingProofs,
            progress: 0.1,
            message: "downloading proofs from zidecar...".into(),
            current_height: 0,
        };

        // get proofs from server
        let (proof_bytes, from_height, to_height) = {
            let mut client = self.client.write().await;
            match client.get_header_proof().await {
                Ok(p) => p,
                Err(e) => {
                    warn!("failed to get proofs: {}", e);
                    progress.phase = SyncPhase::Error;
                    progress.message = format!("failed to get proofs: {}", e);
                    return Ok(progress);
                }
            }
        };

        info!(
            "received proof: {} -> {} ({} KB)",
            from_height,
            to_height,
            proof_bytes.len() / 1024
        );

        progress.progress = 0.3;
        progress.message = "verifying gigaproof + tip proof...".into();

        // verify proofs
        match self.verifier.verify_proofs(&proof_bytes) {
            Ok((gigaproof_valid, tip_valid)) => {
                if gigaproof_valid && tip_valid {
                    info!("proofs verified successfully");
                    progress.progress = 0.5;
                    progress.current_height = to_height;
                    progress.message = format!("proofs verified! chain height: {}", to_height);
                } else {
                    warn!("proof verification failed");
                    progress.phase = SyncPhase::Error;
                    progress.message = "proof verification failed!".into();
                }
            }
            Err(e) => {
                warn!("proof verification error: {}", e);
                progress.phase = SyncPhase::Error;
                progress.message = format!("verification error: {}", e);
            }
        }

        Ok(progress)
    }

    async fn download_and_scan(&self) -> Result<SyncProgress> {
        info!("phase 2: downloading and scanning blocks");

        let mut progress = SyncProgress {
            phase: SyncPhase::DownloadingBlocks,
            progress: 0.5,
            message: "downloading compact blocks...".into(),
            current_height: 0,
        };

        // get last synced height
        let start_height = self
            .storage
            .get_last_sync_height()?
            .unwrap_or(zync_core::ORCHARD_ACTIVATION_HEIGHT);

        // get current tip
        let (tip_height, _tip_hash) = {
            let mut client = self.client.write().await;
            match client.get_tip().await {
                Ok(t) => t,
                Err(e) => {
                    warn!("failed to get tip: {}", e);
                    progress.phase = SyncPhase::Error;
                    progress.message = format!("failed to get tip: {}", e);
                    return Ok(progress);
                }
            }
        };

        if start_height >= tip_height {
            info!("already synced to tip");
            progress.phase = SyncPhase::Complete;
            progress.progress = 1.0;
            progress.current_height = tip_height;
            progress.message = "wallet is up to date".into();
            return Ok(progress);
        }

        let total_blocks = tip_height - start_height;
        info!(
            "scanning blocks {} -> {} ({} blocks)",
            start_height, tip_height, total_blocks
        );

        progress.phase = SyncPhase::Scanning;

        // stream compact blocks and scan
        let mut stream = {
            let mut client = self.client.write().await;
            client.get_compact_blocks(start_height, tip_height).await?
        };

        let mut scanned = 0;
        let mut total_found = 0;

        use tokio_stream::StreamExt;

        while let Some(result) = stream.next().await {
            match result {
                Ok(block) => {
                    scanned += 1;

                    // scan block
                    let found = {
                        let mut scanner = self.scanner.write().await;
                        scanner.scan_block(&block)?
                    };

                    total_found += found;

                    if found > 0 {
                        info!("found {} notes in block {}", found, block.height);
                    }

                    // update progress
                    progress.progress = 0.5 + (0.5 * (scanned as f32 / total_blocks as f32));
                    progress.current_height = block.height;
                    progress.message = format!(
                        "scanned {} / {} blocks ({} notes found)",
                        scanned, total_blocks, total_found
                    );

                    // update storage every 100 blocks
                    if scanned % 100 == 0 {
                        self.storage.set_last_sync_height(block.height)?;
                    }
                }
                Err(e) => {
                    warn!("error receiving block: {}", e);
                    progress.phase = SyncPhase::Error;
                    progress.message = format!("block download error: {}", e);
                    return Ok(progress);
                }
            }
        }

        // save final height
        self.storage.set_last_sync_height(tip_height)?;

        info!("sync complete: scanned {} blocks, found {} notes", scanned, total_found);

        progress.phase = SyncPhase::Complete;
        progress.progress = 1.0;
        progress.current_height = tip_height;
        progress.message = format!("sync complete! found {} notes", total_found);

        Ok(progress)
    }
}
