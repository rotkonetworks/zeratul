//! gRPC service implementation

use crate::{
    compact::CompactBlock as InternalCompactBlock,
    epoch::EpochManager,
    error::{Result, ZidecarError},
    storage::Storage,
    zebrad::ZebradClient,
    zidecar::{
        self, zidecar_server::Zidecar, BlockHeader as ProtoBlockHeader, BlockId, BlockRange,
        CompactAction as ProtoCompactAction, CompactBlock as ProtoCompactBlock, Empty,
        HeaderProof, ProofRequest, SyncStatus, RawTransaction, SendResponse, TxFilter, TreeState,
        TransparentAddressFilter, UtxoList, Utxo, TxidList,
        sync_status::GigaproofStatus,
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

        // get gigaproof + tip proof
        let (gigaproof, tip_proof) = match self.epoch_manager.get_proofs().await {
            Ok(p) => p,
            Err(e) => {
                error!("failed to get proofs: {}", e);
                return Err(Status::internal(e.to_string()));
            }
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

        // fetch headers for verification (just epoch boundaries for now)
        let last_epoch_height = self
            .epoch_manager
            .last_complete_epoch_height()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let headers = match self
            .fetch_headers(self.start_height, last_epoch_height)
            .await
        {
            Ok(h) => h,
            Err(e) => {
                error!("failed to fetch headers: {}", e);
                return Err(Status::internal(e.to_string()));
            }
        };

        // combine gigaproof + tip proof with size prefix
        // format: [gigaproof_size: u32][gigaproof_bytes][tip_bytes]
        let gigaproof_size = gigaproof.len();
        let tip_size = tip_proof.len();

        let mut combined_proof = Vec::with_capacity(4 + gigaproof_size + tip_size);
        combined_proof.extend_from_slice(&(gigaproof_size as u32).to_le_bytes());
        combined_proof.extend_from_slice(&gigaproof);
        combined_proof.extend_from_slice(&tip_proof);

        info!(
            "serving proof: {} KB gigaproof + {} KB tip = {} KB total",
            gigaproof_size / 1024,
            tip_size / 1024,
            combined_proof.len() / 1024
        );

        Ok(Response::new(HeaderProof {
            ligerito_proof: combined_proof,
            from_height: self.start_height,
            to_height: tip_info.blocks,
            tip_hash,
            headers,
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
                    height: 0, // TODO: get block height from tx info
                }))
            }
            Err(e) => {
                error!("get_transaction failed: {}", e);
                Err(Status::not_found(e.to_string()))
            }
        }
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
}
