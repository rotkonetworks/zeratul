//! zidecar gRPC client

use anyhow::Result;
use super::{
    zidecar_proto::{
        zidecar_client::ZidecarClient as GrpcClient,
        Empty, ProofRequest, BlockRange, BlockId, TxFilter,
        TransparentAddressFilter, RawTransaction,
    },
    SyncStatus, TreeState, Utxo, SendResult, CompactBlock, CompactAction,
};
use tonic::transport::Channel;
use tracing::{info, debug};

pub struct ZidecarClient {
    client: GrpcClient<Channel>,
}

impl ZidecarClient {
    pub async fn connect(url: &str) -> Result<Self> {
        info!("connecting to zidecar at {}", url);
        let client = GrpcClient::connect(url.to_string()).await?;
        Ok(Self { client })
    }

    /// get gigaproof + tip proof for full chain
    pub async fn get_header_proof(&mut self) -> Result<(Vec<u8>, u32, u32)> {
        let request = tonic::Request::new(ProofRequest {
            from_height: 0,
            to_height: 0, // 0 = tip
        });

        let response = self.client.get_header_proof(request).await?;
        let proof = response.into_inner();

        debug!(
            "received proof: {} -> {} ({} bytes)",
            proof.from_height,
            proof.to_height,
            proof.ligerito_proof.len()
        );

        Ok((proof.ligerito_proof, proof.from_height, proof.to_height))
    }

    /// get current chain tip
    pub async fn get_tip(&mut self) -> Result<(u32, Vec<u8>)> {
        let request = tonic::Request::new(Empty {});
        let response = self.client.get_tip(request).await?;
        let tip = response.into_inner();

        Ok((tip.height, tip.hash))
    }

    /// stream compact blocks for scanning
    pub async fn get_compact_blocks(
        &mut self,
        start_height: u32,
        end_height: u32,
    ) -> Result<Vec<CompactBlock>> {
        let request = tonic::Request::new(BlockRange {
            start_height,
            end_height,
        });

        let mut stream = self.client.get_compact_blocks(request).await?.into_inner();
        let mut blocks = Vec::new();

        while let Some(block) = stream.message().await? {
            let actions: Vec<CompactAction> = block.actions.into_iter().map(|a| {
                let mut cmx = [0u8; 32];
                let mut ek = [0u8; 32];
                let mut nf = [0u8; 32];
                if a.cmx.len() == 32 { cmx.copy_from_slice(&a.cmx); }
                if a.ephemeral_key.len() == 32 { ek.copy_from_slice(&a.ephemeral_key); }
                if a.nullifier.len() == 32 { nf.copy_from_slice(&a.nullifier); }
                CompactAction {
                    cmx,
                    ephemeral_key: ek,
                    ciphertext: a.ciphertext,
                    nullifier: nf,
                }
            }).collect();

            blocks.push(CompactBlock {
                height: block.height,
                hash: block.hash,
                actions,
            });
        }

        Ok(blocks)
    }

    /// get sync status (blockchain height, epoch progress, gigaproof status)
    pub async fn get_sync_status(&mut self) -> Result<SyncStatus> {
        let request = tonic::Request::new(Empty {});
        let response = self.client.get_sync_status(request).await?;
        let status = response.into_inner();

        Ok(SyncStatus {
            current_height: status.current_height,
            current_epoch: status.current_epoch,
            blocks_in_epoch: status.blocks_in_epoch,
            complete_epochs: status.complete_epochs,
            gigaproof_ready: status.gigaproof_status == 2, // READY
            blocks_until_ready: status.blocks_until_ready,
            last_gigaproof_height: status.last_gigaproof_height,
        })
    }

    /// send raw transaction to the network
    pub async fn send_transaction(&mut self, tx_data: Vec<u8>) -> Result<SendResult> {
        let request = tonic::Request::new(RawTransaction {
            data: tx_data,
            height: 0,
        });
        let response = self.client.send_transaction(request).await?;
        let resp = response.into_inner();
        Ok(SendResult {
            txid: resp.txid,
            error_code: resp.error_code,
            error_message: resp.error_message,
        })
    }

    /// get transaction by hash
    pub async fn get_transaction(&mut self, txid: &[u8; 32]) -> Result<Vec<u8>> {
        let request = tonic::Request::new(TxFilter {
            hash: txid.to_vec(),
        });
        let response = self.client.get_transaction(request).await?;
        Ok(response.into_inner().data)
    }

    /// get tree state at a given height
    pub async fn get_tree_state(&mut self, height: u32) -> Result<TreeState> {
        let request = tonic::Request::new(BlockId {
            height,
            hash: vec![],
        });
        let response = self.client.get_tree_state(request).await?;
        let state = response.into_inner();
        Ok(TreeState {
            height: state.height,
            hash: state.hash,
            time: state.time,
            sapling_tree: state.sapling_tree,
            orchard_tree: state.orchard_tree,
        })
    }

    /// get transparent UTXOs for addresses
    pub async fn get_address_utxos(&mut self, addresses: Vec<String>) -> Result<Vec<Utxo>> {
        let request = tonic::Request::new(TransparentAddressFilter {
            addresses,
            start_height: 0,
            max_entries: 0,
        });
        let response = self.client.get_address_utxos(request).await?;
        let utxos = response.into_inner().utxos;

        Ok(utxos.into_iter().map(|u| {
            let mut txid = [0u8; 32];
            if u.txid.len() == 32 { txid.copy_from_slice(&u.txid); }
            Utxo {
                address: u.address,
                txid,
                output_index: u.output_index,
                script: u.script,
                value_zat: u.value_zat,
                height: u.height,
            }
        }).collect())
    }

    /// get transparent transaction IDs for addresses
    pub async fn get_taddress_txids(&mut self, addresses: Vec<String>, start_height: u32) -> Result<Vec<[u8; 32]>> {
        let request = tonic::Request::new(TransparentAddressFilter {
            addresses,
            start_height,
            max_entries: 0,
        });
        let response = self.client.get_taddress_txids(request).await?;
        let txids = response.into_inner().txids;

        Ok(txids.into_iter().filter_map(|t| {
            if t.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&t);
                Some(arr)
            } else {
                None
            }
        }).collect())
    }
}
