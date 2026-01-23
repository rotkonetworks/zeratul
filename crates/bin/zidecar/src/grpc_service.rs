//! gRPC service implementation

use crate::{
    compact::CompactBlock as InternalCompactBlock,
    epoch::EpochManager,
    error::{Result, ZidecarError},
    prover::HeaderChainProof,
    storage::Storage,
    zebrad::ZebradClient,
    checkpoint::EpochCheckpoint,
    zidecar::{
        self, zidecar_server::Zidecar, BlockHeader as ProtoBlockHeader, BlockId, BlockRange,
        CompactAction as ProtoCompactAction, CompactBlock as ProtoCompactBlock, Empty,
        HeaderProof, ProofRequest, SyncStatus, RawTransaction, SendResponse, TxFilter, TreeState,
        TransparentAddressFilter, UtxoList, Utxo, TxidList, BlockTransactions,
        sync_status::GigaproofStatus,
        // trustless v2 types
        TrustlessStateProof, FrostCheckpoint as ProtoFrostCheckpoint,
        FrostSignature as ProtoFrostSignature, EpochRequest, CommitmentQuery, CommitmentProof,
        NullifierQuery, NullifierProof, VerifiedBlock,
        // epoch boundary types
        EpochBoundary as ProtoEpochBoundary, EpochRangeRequest, EpochBoundaryList,
        // public outputs
        ProofPublicOutputs as ProtoPublicOutputs,
    },
};
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{info, warn, error};

pub struct ZidecarService {
    zebrad: ZebradClient,
    storage: Arc<Storage>,
    epoch_manager: Arc<EpochManager>,
    start_height: u32,
}

impl ZidecarService {
    pub fn new(
        zebrad: ZebradClient,
        storage: Arc<Storage>,
        epoch_manager: Arc<EpochManager>,
        start_height: u32,
    ) -> Self {
        Self {
            zebrad,
            storage,
            epoch_manager,
            start_height,
        }
    }

    /// fetch block headers for range
    async fn fetch_headers(
        &self,
        from_height: u32,
        to_height: u32,
    ) -> Result<Vec<ProtoBlockHeader>> {
        let mut headers = Vec::new();

        for height in from_height..=to_height {
            let hash = self.zebrad.get_block_hash(height).await?;
            let header = self.zebrad.get_block_header(&hash).await?;

            headers.push(ProtoBlockHeader {
                height: header.height,
                hash: hex::decode(&header.hash)
                    .map_err(|e| ZidecarError::Serialization(e.to_string()))?,
                prev_hash: hex::decode(&header.prev_hash)
                    .map_err(|e| ZidecarError::Serialization(e.to_string()))?,
                timestamp: header.timestamp,
                merkle_root: hex::decode(&header.merkle_root)
                    .map_err(|e| ZidecarError::Serialization(e.to_string()))?,
            });
        }

        Ok(headers)
    }
}

#[tonic::async_trait]
impl Zidecar for ZidecarService {
    async fn get_header_proof(
        &self,
        _request: Request<ProofRequest>,
    ) -> std::result::Result<Response<HeaderProof>, Status> {
        info!("header proof request (gigaproof + tip)");

        // get gigaproof + tip proof (both now contain public outputs)
        let (gigaproof, tip_proof) = match self.epoch_manager.get_proofs().await {
            Ok(p) => p,
            Err(e) => {
                error!("failed to get proofs: {}", e);
                return Err(Status::internal(e.to_string()));
            }
        };

        // deserialize public outputs from gigaproof
        let (giga_outputs, _, _) = HeaderChainProof::deserialize_full(&gigaproof)
            .map_err(|e| Status::internal(format!("failed to deserialize gigaproof: {}", e)))?;

        // deserialize public outputs from tip proof (if present)
        let tip_outputs = if !tip_proof.is_empty() {
            let (outputs, _, _) = HeaderChainProof::deserialize_full(&tip_proof)
                .map_err(|e| Status::internal(format!("failed to deserialize tip proof: {}", e)))?;
            Some(outputs)
        } else {
            None
        };

        // verify continuity: tip proof's start_prev_hash == gigaproof's tip_hash
        let continuity_verified = if let Some(ref tip) = tip_outputs {
            let is_continuous = tip.start_prev_hash == giga_outputs.tip_hash;
            if is_continuous {
                info!(
                    "✓ continuity verified: gigaproof tip {} -> tip proof start prev {}",
                    hex::encode(&giga_outputs.tip_hash[..8]),
                    hex::encode(&tip.start_prev_hash[..8])
                );
            } else {
                error!(
                    "✗ continuity FAILED: gigaproof tip {} != tip proof start prev {}",
                    hex::encode(&giga_outputs.tip_hash[..8]),
                    hex::encode(&tip.start_prev_hash[..8])
                );
            }
            is_continuous
        } else {
            // no tip proof, gigaproof covers everything
            true
        };

        // get current tip
        let tip_info = match self.zebrad.get_blockchain_info().await {
            Ok(info) => info,
            Err(e) => {
                error!("failed to get blockchain info: {}", e);
                return Err(Status::internal(e.to_string()));
            }
        };

        let tip_hash = hex::decode(&tip_info.bestblockhash)
            .map_err(|e| Status::internal(e.to_string()))?;

        // Skip header fetching - proof contains verified data
        // The public outputs contain the verified tip hash which is sufficient
        let headers = Vec::new();

        // combine gigaproof + tip proof with size prefix
        // format: [gigaproof_size: u32][gigaproof_bytes][tip_bytes]
        let gigaproof_size = gigaproof.len();
        let tip_size = tip_proof.len();

        let mut combined_proof = Vec::with_capacity(4 + gigaproof_size + tip_size);
        combined_proof.extend_from_slice(&(gigaproof_size as u32).to_le_bytes());
        combined_proof.extend_from_slice(&gigaproof);
        combined_proof.extend_from_slice(&tip_proof);

        info!(
            "serving proof: {} KB gigaproof + {} KB tip = {} KB total (continuity={})",
            gigaproof_size / 1024,
            tip_size / 1024,
            combined_proof.len() / 1024,
            continuity_verified
        );

        // convert public outputs to proto
        let giga_proto = public_outputs_to_proto(&giga_outputs);
        let tip_proto = tip_outputs.as_ref().map(public_outputs_to_proto);

        // Get current nullifier root from storage
        let nullifier_root = self.storage.get_nullifier_root().to_vec();

        Ok(Response::new(HeaderProof {
            ligerito_proof: combined_proof,
            from_height: self.start_height,
            to_height: tip_info.blocks,
            tip_hash,
            headers,
            gigaproof_outputs: Some(giga_proto),
            tip_proof_outputs: tip_proto,
            continuity_verified,
            nullifier_root,
        }))
    }

    type GetCompactBlocksStream = ReceiverStream<std::result::Result<ProtoCompactBlock, Status>>;

    async fn get_compact_blocks(
        &self,
        request: Request<BlockRange>,
    ) -> std::result::Result<Response<Self::GetCompactBlocksStream>, Status> {
        let range = request.into_inner();

        info!(
            "compact blocks request: {}..{}",
            range.start_height, range.end_height
        );

        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let zebrad = self.zebrad.clone();
        let start = range.start_height;
        let end = range.end_height;

        tokio::spawn(async move {
            for height in start..=end {
                match InternalCompactBlock::from_zebrad(&zebrad, height).await {
                    Ok(block) => {
                        let proto_block = ProtoCompactBlock {
                            height: block.height,
                            hash: block.hash,
                            actions: block
                                .actions
                                .into_iter()
                                .map(|a| ProtoCompactAction {
                                    cmx: a.cmx,
                                    ephemeral_key: a.ephemeral_key,
                                    ciphertext: a.ciphertext,
                                    nullifier: a.nullifier,
                                    txid: a.txid,
                                })
                                .collect(),
                        };

                        if tx.send(Ok(proto_block)).await.is_err() {
                            warn!("client disconnected during stream");
                            break;
                        }
                    }
                    Err(e) => {
                        error!("failed to fetch block {}: {}", height, e);
                        let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_tip(
        &self,
        _request: Request<Empty>,
    ) -> std::result::Result<Response<BlockId>, Status> {
        match self.zebrad.get_blockchain_info().await {
            Ok(info) => {
                let hash = hex::decode(&info.bestblockhash)
                    .map_err(|e| Status::internal(e.to_string()))?;

                Ok(Response::new(BlockId {
                    height: info.blocks,
                    hash,
                }))
            }
            Err(e) => {
                error!("failed to get tip: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    type SubscribeBlocksStream = ReceiverStream<std::result::Result<BlockId, Status>>;

    async fn subscribe_blocks(
        &self,
        _request: Request<Empty>,
    ) -> std::result::Result<Response<Self::SubscribeBlocksStream>, Status> {
        info!("new block subscription");

        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let zebrad = self.zebrad.clone();

        tokio::spawn(async move {
            let mut last_height = 0;

            loop {
                // poll for new blocks every 30s
                tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

                match zebrad.get_blockchain_info().await {
                    Ok(info) => {
                        if info.blocks > last_height {
                            last_height = info.blocks;

                            let hash = match hex::decode(&info.bestblockhash) {
                                Ok(h) => h,
                                Err(e) => {
                                    error!("invalid hash: {}", e);
                                    continue;
                                }
                            };

                            if tx
                                .send(Ok(BlockId {
                                    height: info.blocks,
                                    hash,
                                }))
                                .await
                                .is_err()
                            {
                                info!("client disconnected from subscription");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!("failed to poll blockchain: {}", e);
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_sync_status(
        &self,
        _request: Request<Empty>,
    ) -> std::result::Result<Response<SyncStatus>, Status> {
        // get current blockchain height
        let blockchain_info = match self.zebrad.get_blockchain_info().await {
            Ok(info) => info,
            Err(e) => {
                error!("failed to get blockchain info: {}", e);
                return Err(Status::internal(e.to_string()));
            }
        };

        let current_height = blockchain_info.blocks;
        let current_epoch = current_height / zync_core::EPOCH_SIZE;
        let blocks_in_epoch = current_height % zync_core::EPOCH_SIZE;

        // calculate complete epochs
        let complete_epochs = if blocks_in_epoch == 0 && current_height > 0 {
            current_epoch
        } else {
            current_epoch.saturating_sub(1)
        };

        // check gigaproof status
        let (gigaproof_status, last_gigaproof_height) = match self.epoch_manager.is_gigaproof_ready().await {
            Ok(true) => {
                let last_height = self.epoch_manager.last_complete_epoch_height().await
                    .unwrap_or(0);
                (GigaproofStatus::Ready as i32, last_height)
            }
            Ok(false) => {
                if complete_epochs == 0 {
                    (GigaproofStatus::WaitingForEpoch as i32, 0)
                } else {
                    (GigaproofStatus::Generating as i32, 0)
                }
            }
            Err(e) => {
                warn!("failed to check gigaproof status: {}", e);
                (GigaproofStatus::WaitingForEpoch as i32, 0)
            }
        };

        // calculate blocks until ready
        let blocks_until_ready = if complete_epochs == 0 {
            zync_core::EPOCH_SIZE - blocks_in_epoch
        } else {
            0
        };

        info!(
            "sync status: height={} epoch={}/{} gigaproof={:?}",
            current_height, blocks_in_epoch, zync_core::EPOCH_SIZE, gigaproof_status
        );

        Ok(Response::new(SyncStatus {
            current_height,
            current_epoch,
            blocks_in_epoch,
            complete_epochs,
            gigaproof_status,
            blocks_until_ready,
            last_gigaproof_height,
        }))
    }

    async fn send_transaction(
        &self,
        request: Request<RawTransaction>,
    ) -> std::result::Result<Response<SendResponse>, Status> {
        let raw_tx = request.into_inner();
        let tx_hex = hex::encode(&raw_tx.data);

        info!("send_transaction: {} bytes", raw_tx.data.len());

        match self.zebrad.send_raw_transaction(&tx_hex).await {
            Ok(txid) => {
                info!("transaction sent: {}", txid);
                Ok(Response::new(SendResponse {
                    txid,
                    error_code: 0,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("send_transaction failed: {}", e);
                Ok(Response::new(SendResponse {
                    txid: String::new(),
                    error_code: -1,
                    error_message: e.to_string(),
                }))
            }
        }
    }

    async fn get_transaction(
        &self,
        request: Request<TxFilter>,
    ) -> std::result::Result<Response<RawTransaction>, Status> {
        let filter = request.into_inner();
        let txid = hex::encode(&filter.hash);

        info!("get_transaction: {}", txid);

        match self.zebrad.get_raw_transaction(&txid).await {
            Ok(tx) => {
                let data = hex::decode(&tx.hex)
                    .map_err(|e| Status::internal(format!("invalid tx hex: {}", e)))?;

                Ok(Response::new(RawTransaction {
                    data,
                    height: tx.height.unwrap_or(0),
                }))
            }
            Err(e) => {
                error!("get_transaction failed: {}", e);
                Err(Status::not_found(e.to_string()))
            }
        }
    }

    /// privacy-preserving memo retrieval: get all transactions at a height
    /// client fetches entire block, server doesn't learn which tx they care about
    async fn get_block_transactions(
        &self,
        request: Request<BlockId>,
    ) -> std::result::Result<Response<BlockTransactions>, Status> {
        let block_id = request.into_inner();
        let height = block_id.height;

        info!("get_block_transactions: height {}", height);

        // get block hash at height
        let block_hash = match self.zebrad.get_block_hash(height).await {
            Ok(hash) => hash,
            Err(e) => {
                error!("failed to get block hash at {}: {}", height, e);
                return Err(Status::not_found(e.to_string()));
            }
        };

        // get full block with all transactions (verbosity 1 = include tx hashes)
        let block = match self.zebrad.get_block(&block_hash, 1).await {
            Ok(b) => b,
            Err(e) => {
                error!("failed to get block {}: {}", block_hash, e);
                return Err(Status::internal(e.to_string()));
            }
        };

        // fetch raw transaction bytes for each txid in the block
        let mut txs = Vec::new();
        for txid in &block.tx {
            match self.zebrad.get_raw_transaction(txid).await {
                Ok(tx) => {
                    let data = hex::decode(&tx.hex).unwrap_or_default();
                    txs.push(RawTransaction {
                        data,
                        height,
                    });
                }
                Err(e) => {
                    warn!("failed to get tx {}: {}", txid, e);
                    // continue with other txs
                }
            }
        }

        let hash = hex::decode(&block_hash).unwrap_or_default();

        info!("returning {} transactions for block {}", txs.len(), height);

        Ok(Response::new(BlockTransactions {
            height,
            hash,
            txs,
        }))
    }

    async fn get_tree_state(
        &self,
        request: Request<BlockId>,
    ) -> std::result::Result<Response<TreeState>, Status> {
        let block_id = request.into_inner();

        // use height as string for z_gettreestate
        let height_str = block_id.height.to_string();

        info!("get_tree_state: height {}", block_id.height);

        match self.zebrad.get_tree_state(&height_str).await {
            Ok(state) => {
                let hash = hex::decode(&state.hash)
                    .map_err(|e| Status::internal(format!("invalid hash: {}", e)))?;

                Ok(Response::new(TreeState {
                    height: state.height,
                    hash,
                    time: state.time,
                    sapling_tree: state.sapling.commitments.final_state,
                    orchard_tree: state.orchard.commitments.final_state,
                }))
            }
            Err(e) => {
                error!("get_tree_state failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn get_address_utxos(
        &self,
        request: Request<TransparentAddressFilter>,
    ) -> std::result::Result<Response<UtxoList>, Status> {
        let filter = request.into_inner();

        info!("get_address_utxos: {} addresses", filter.addresses.len());

        match self.zebrad.get_address_utxos(&filter.addresses).await {
            Ok(utxos) => {
                let proto_utxos: Vec<Utxo> = utxos
                    .into_iter()
                    .map(|u| {
                        Utxo {
                            address: u.address,
                            txid: hex::decode(&u.txid).unwrap_or_default(),
                            output_index: u.output_index,
                            script: hex::decode(&u.script).unwrap_or_default(),
                            value_zat: u.satoshis,
                            height: u.height,
                        }
                    })
                    .collect();

                info!("returning {} UTXOs", proto_utxos.len());
                Ok(Response::new(UtxoList { utxos: proto_utxos }))
            }
            Err(e) => {
                error!("get_address_utxos failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn get_taddress_txids(
        &self,
        request: Request<TransparentAddressFilter>,
    ) -> std::result::Result<Response<TxidList>, Status> {
        let filter = request.into_inner();

        // get current height for end range
        let end_height = match self.zebrad.get_blockchain_info().await {
            Ok(info) => info.blocks,
            Err(e) => {
                error!("failed to get blockchain info: {}", e);
                return Err(Status::internal(e.to_string()));
            }
        };

        let start_height = if filter.start_height > 0 {
            filter.start_height
        } else {
            1 // start from beginning
        };

        info!(
            "get_taddress_txids: {} addresses, height {}..{}",
            filter.addresses.len(),
            start_height,
            end_height
        );

        match self
            .zebrad
            .get_address_txids(&filter.addresses, start_height, end_height)
            .await
        {
            Ok(txids) => {
                let proto_txids: Vec<Vec<u8>> = txids
                    .into_iter()
                    .filter_map(|txid| hex::decode(&txid).ok())
                    .collect();

                info!("returning {} txids", proto_txids.len());
                Ok(Response::new(TxidList { txids: proto_txids }))
            }
            Err(e) => {
                error!("get_taddress_txids failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    // ===== TRUSTLESS STATE PROOFS (v2) =====

    async fn get_trustless_state_proof(
        &self,
        _request: Request<ProofRequest>,
    ) -> std::result::Result<Response<TrustlessStateProof>, Status> {
        info!("trustless state proof request");

        // get latest checkpoint
        let checkpoint_epoch = self.storage.get_latest_checkpoint_epoch()
            .map_err(|e| Status::internal(e.to_string()))?
            .unwrap_or(0);

        let checkpoint_bytes = self.storage.get_checkpoint(checkpoint_epoch)
            .map_err(|e| Status::internal(e.to_string()))?;

        let checkpoint = if let Some(bytes) = checkpoint_bytes {
            EpochCheckpoint::from_bytes(&bytes)
                .map_err(|e| Status::internal(e.to_string()))?
        } else {
            // use genesis checkpoint if none stored
            crate::checkpoint::mainnet_genesis_checkpoint()
        };

        // get gigaproof + tip proof (state transition proof)
        let (gigaproof, _tip_proof) = match self.epoch_manager.get_proofs().await {
            Ok(p) => p,
            Err(e) => {
                error!("failed to get proofs: {}", e);
                return Err(Status::internal(e.to_string()));
            }
        };

        // get current state
        let tip_info = self.zebrad.get_blockchain_info().await
            .map_err(|e| Status::internal(e.to_string()))?;

        let current_hash = hex::decode(&tip_info.bestblockhash)
            .map_err(|e| Status::internal(e.to_string()))?;

        // get state roots (from storage or compute)
        let (tree_root, nullifier_root) = self.storage
            .get_state_roots(tip_info.blocks)
            .map_err(|e| Status::internal(e.to_string()))?
            .unwrap_or(([0u8; 32], [0u8; 32]));

        info!(
            "serving trustless proof: checkpoint epoch {} -> height {}",
            checkpoint.epoch_index, tip_info.blocks
        );

        // get total action count from storage
        let num_actions = self.storage.get_action_count()
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(TrustlessStateProof {
            checkpoint: Some(checkpoint_to_proto(&checkpoint)),
            state_transition_proof: gigaproof,
            current_height: tip_info.blocks,
            current_hash,
            tree_root: tree_root.to_vec(),
            nullifier_root: nullifier_root.to_vec(),
            num_actions,
            proof_log_size: 20,  // 2^20 default
        }))
    }

    async fn get_commitment_proof(
        &self,
        request: Request<CommitmentQuery>,
    ) -> std::result::Result<Response<CommitmentProof>, Status> {
        let query = request.into_inner();

        if query.cmx.len() != 32 {
            return Err(Status::invalid_argument("cmx must be 32 bytes"));
        }

        let mut cmx = [0u8; 32];
        cmx.copy_from_slice(&query.cmx);

        info!("commitment proof request: {}", hex::encode(&cmx[..8]));

        let proof = self.storage.generate_commitment_proof(&cmx)
            .map_err(|e| Status::internal(e.to_string()))?;

        let height = query.at_height.max(
            self.storage.get_latest_state_height()
                .map_err(|e| Status::internal(e.to_string()))?
                .unwrap_or(0)
        );

        // get commitment position from storage (if tracked)
        let position = self.storage.get_commitment_position(&cmx)
            .map_err(|e| Status::internal(e.to_string()))?
            .unwrap_or(0);

        Ok(Response::new(CommitmentProof {
            cmx: cmx.to_vec(),
            position,
            tree_root: proof.root.to_vec(),
            height,
            proof_path: proof.path.iter().map(|p| p.to_vec()).collect(),
            proof_indices: proof.indices,
            exists: proof.exists,
        }))
    }

    async fn get_nullifier_proof(
        &self,
        request: Request<NullifierQuery>,
    ) -> std::result::Result<Response<NullifierProof>, Status> {
        let query = request.into_inner();

        if query.nullifier.len() != 32 {
            return Err(Status::invalid_argument("nullifier must be 32 bytes"));
        }

        let mut nullifier = [0u8; 32];
        nullifier.copy_from_slice(&query.nullifier);

        info!("nullifier proof request: {}", hex::encode(&nullifier[..8]));

        let proof = self.storage.generate_nullifier_proof(&nullifier)
            .map_err(|e| Status::internal(e.to_string()))?;

        let height = query.at_height.max(
            self.storage.get_latest_state_height()
                .map_err(|e| Status::internal(e.to_string()))?
                .unwrap_or(0)
        );

        Ok(Response::new(NullifierProof {
            nullifier: nullifier.to_vec(),
            nullifier_root: proof.root.to_vec(),
            height,
            proof_path: proof.path.iter().map(|p| p.to_vec()).collect(),
            proof_indices: proof.indices,
            is_spent: proof.exists,
        }))
    }

    type GetVerifiedBlocksStream = ReceiverStream<std::result::Result<VerifiedBlock, Status>>;

    async fn get_verified_blocks(
        &self,
        request: Request<BlockRange>,
    ) -> std::result::Result<Response<Self::GetVerifiedBlocksStream>, Status> {
        let range = request.into_inner();

        info!(
            "verified blocks request: {}..{}",
            range.start_height, range.end_height
        );

        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let zebrad = self.zebrad.clone();
        let storage = self.storage.clone();
        let start = range.start_height;
        let end = range.end_height;

        tokio::spawn(async move {
            for height in start..=end {
                match InternalCompactBlock::from_zebrad(&zebrad, height).await {
                    Ok(block) => {
                        // compute actions merkle root
                        let actions_root = compute_actions_root(&block.actions);

                        // get state roots after this block (if available)
                        let (tree_root_after, nullifier_root_after) = storage
                            .get_state_roots(height)
                            .unwrap_or(None)
                            .unwrap_or(([0u8; 32], [0u8; 32]));

                        // Note: merkle_path is empty - computing full merkle path from
                        // actions to block header requires maintaining the complete block
                        // merkle tree structure. For now, clients can verify actions_root
                        // against header.merkle_root directly if needed.
                        let verified_block = VerifiedBlock {
                            height: block.height,
                            hash: block.hash,
                            actions: block
                                .actions
                                .into_iter()
                                .map(|a| ProtoCompactAction {
                                    cmx: a.cmx,
                                    ephemeral_key: a.ephemeral_key,
                                    ciphertext: a.ciphertext,
                                    nullifier: a.nullifier,
                                    txid: a.txid,
                                })
                                .collect(),
                            actions_root: actions_root.to_vec(),
                            merkle_path: vec![],
                            tree_root_after: tree_root_after.to_vec(),
                            nullifier_root_after: nullifier_root_after.to_vec(),
                        };

                        if tx.send(Ok(verified_block)).await.is_err() {
                            warn!("client disconnected during stream");
                            break;
                        }
                    }
                    Err(e) => {
                        error!("failed to fetch block {}: {}", height, e);
                        let _ = tx.send(Err(Status::internal(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_checkpoint(
        &self,
        request: Request<EpochRequest>,
    ) -> std::result::Result<Response<ProtoFrostCheckpoint>, Status> {
        let req = request.into_inner();

        let epoch = if req.epoch_index == 0 {
            // get latest
            self.storage.get_latest_checkpoint_epoch()
                .map_err(|e| Status::internal(e.to_string()))?
                .unwrap_or(0)
        } else {
            req.epoch_index
        };

        info!("checkpoint request for epoch {}", epoch);

        let checkpoint_bytes = self.storage.get_checkpoint(epoch)
            .map_err(|e| Status::internal(e.to_string()))?;

        let checkpoint = if let Some(bytes) = checkpoint_bytes {
            EpochCheckpoint::from_bytes(&bytes)
                .map_err(|e| Status::internal(e.to_string()))?
        } else if epoch == 0 {
            crate::checkpoint::mainnet_genesis_checkpoint()
        } else {
            return Err(Status::not_found(format!("checkpoint {} not found", epoch)));
        };

        Ok(Response::new(checkpoint_to_proto(&checkpoint)))
    }

    async fn get_epoch_boundary(
        &self,
        request: Request<EpochRequest>,
    ) -> std::result::Result<Response<ProtoEpochBoundary>, Status> {
        let req = request.into_inner();

        let epoch = if req.epoch_index == 0 {
            // get latest complete epoch
            let info = self.zebrad.get_blockchain_info().await
                .map_err(|e| Status::internal(e.to_string()))?;
            let current_epoch = info.blocks / zync_core::EPOCH_SIZE;
            if info.blocks % zync_core::EPOCH_SIZE == 0 {
                current_epoch
            } else {
                current_epoch.saturating_sub(1)
            }
        } else {
            req.epoch_index as u32
        };

        info!("epoch boundary request for epoch {}", epoch);

        let boundary = self.storage.get_epoch_boundary(epoch)
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found(format!("epoch {} boundary not found", epoch)))?;

        Ok(Response::new(ProtoEpochBoundary {
            epoch: boundary.epoch,
            first_height: boundary.first_height,
            first_hash: boundary.first_hash.to_vec(),
            first_prev_hash: boundary.first_prev_hash.to_vec(),
            last_height: boundary.last_height,
            last_hash: boundary.last_hash.to_vec(),
        }))
    }

    async fn get_epoch_boundaries(
        &self,
        request: Request<EpochRangeRequest>,
    ) -> std::result::Result<Response<EpochBoundaryList>, Status> {
        let req = request.into_inner();

        let to_epoch = if req.to_epoch == 0 {
            // get latest complete epoch
            let info = self.zebrad.get_blockchain_info().await
                .map_err(|e| Status::internal(e.to_string()))?;
            let current_epoch = info.blocks / zync_core::EPOCH_SIZE;
            if info.blocks % zync_core::EPOCH_SIZE == 0 {
                current_epoch
            } else {
                current_epoch.saturating_sub(1)
            }
        } else {
            req.to_epoch
        };

        info!("epoch boundaries request: epochs {} -> {}", req.from_epoch, to_epoch);

        let mut boundaries = Vec::new();

        for epoch in req.from_epoch..=to_epoch {
            if let Ok(Some(boundary)) = self.storage.get_epoch_boundary(epoch) {
                boundaries.push(ProtoEpochBoundary {
                    epoch: boundary.epoch,
                    first_height: boundary.first_height,
                    first_hash: boundary.first_hash.to_vec(),
                    first_prev_hash: boundary.first_prev_hash.to_vec(),
                    last_height: boundary.last_height,
                    last_hash: boundary.last_hash.to_vec(),
                });
            }
        }

        info!("returning {} epoch boundaries", boundaries.len());

        Ok(Response::new(EpochBoundaryList { boundaries }))
    }
}

// helper: convert EpochCheckpoint to proto
fn checkpoint_to_proto(cp: &EpochCheckpoint) -> ProtoFrostCheckpoint {
    ProtoFrostCheckpoint {
        epoch_index: cp.epoch_index,
        height: cp.height,
        block_hash: cp.block_hash.to_vec(),
        tree_root: cp.tree_root.to_vec(),
        nullifier_root: cp.nullifier_root.to_vec(),
        timestamp: cp.timestamp,
        signature: Some(ProtoFrostSignature {
            r: cp.signature.r.to_vec(),
            s: cp.signature.s.to_vec(),
        }),
        signer_set_id: cp.signer_set_id.to_vec(),
    }
}

// helper: compute merkle root of compact actions
fn compute_actions_root(actions: &[crate::compact::CompactAction]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    if actions.is_empty() {
        return [0u8; 32];
    }

    // simple: hash all actions together
    // production would use proper merkle tree
    let mut hasher = Sha256::new();
    hasher.update(b"ZIDECAR_ACTIONS_ROOT");
    for action in actions {
        hasher.update(&action.cmx);
        hasher.update(&action.nullifier);
    }
    hasher.finalize().into()
}

// helper: convert ProofPublicOutputs to proto
fn public_outputs_to_proto(outputs: &crate::prover::ProofPublicOutputs) -> ProtoPublicOutputs {
    ProtoPublicOutputs {
        start_height: outputs.start_height,
        end_height: outputs.end_height,
        start_hash: outputs.start_hash.to_vec(),
        start_prev_hash: outputs.start_prev_hash.to_vec(),
        tip_hash: outputs.tip_hash.to_vec(),
        tip_prev_hash: outputs.tip_prev_hash.to_vec(),
        cumulative_difficulty: outputs.cumulative_difficulty,
        final_commitment: outputs.final_commitment.to_vec(),
        final_state_commitment: outputs.final_state_commitment.to_vec(),
        num_headers: outputs.num_headers,
    }
}
