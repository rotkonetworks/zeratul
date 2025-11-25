//! lightwalletd gRPC client (public endpoint fallback)

use anyhow::Result;
use super::{
    lightwalletd_proto::{
        compact_tx_streamer_client::CompactTxStreamerClient as GrpcClient,
        ChainSpec, BlockId, BlockRange, TxFilter, GetAddressUtxosArg,
        RawTransaction, Empty,
    },
    TreeState, Utxo, SendResult, CompactBlock, CompactAction,
};
use tonic::transport::Channel;
use tracing::{info, debug, warn};

pub struct LightwalletdClient {
    client: GrpcClient<Channel>,
    chain_name: String,
}

impl LightwalletdClient {
    pub async fn connect(url: &str) -> Result<Self> {
        info!("connecting to lightwalletd at {}", url);
        let client = GrpcClient::connect(url.to_string()).await?;
        Ok(Self {
            client,
            chain_name: String::new(),
        })
    }

    /// get lightwalletd server info
    pub async fn get_lightd_info(&mut self) -> Result<LightdInfo> {
        let request = tonic::Request::new(Empty {});
        let response = self.client.get_lightd_info(request).await?;
        let info = response.into_inner();

        self.chain_name = info.chain_name.clone();

        Ok(LightdInfo {
            version: info.version,
            vendor: info.vendor,
            taddr_support: info.taddr_support,
            chain_name: info.chain_name,
            sapling_activation_height: info.sapling_activation_height,
            consensus_branch_id: info.consensus_branch_id,
            block_height: info.block_height,
            estimated_height: info.estimated_height,
        })
    }

    /// get latest block height and hash
    pub async fn get_latest_block(&mut self) -> Result<(u64, Vec<u8>)> {
        let request = tonic::Request::new(ChainSpec {});
        let response = self.client.get_latest_block(request).await?;
        let block = response.into_inner();

        Ok((block.height, block.hash))
    }

    /// stream compact blocks for scanning
    pub async fn get_block_range(
        &mut self,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<CompactBlock>> {
        let request = tonic::Request::new(BlockRange {
            start: Some(BlockId { height: start_height, hash: vec![] }),
            end: Some(BlockId { height: end_height, hash: vec![] }),
        });

        let mut stream = self.client.get_block_range(request).await?.into_inner();
        let mut blocks = Vec::new();

        while let Some(block) = stream.message().await? {
            let mut actions = Vec::new();

            // extract orchard actions from transactions
            for tx in block.vtx {
                for action in tx.actions {
                    let mut cmx = [0u8; 32];
                    let mut ek = [0u8; 32];
                    let mut nf = [0u8; 32];
                    if action.cmx.len() == 32 { cmx.copy_from_slice(&action.cmx); }
                    if action.ephemeral_key.len() == 32 { ek.copy_from_slice(&action.ephemeral_key); }
                    if action.nullifier.len() == 32 { nf.copy_from_slice(&action.nullifier); }
                    actions.push(CompactAction {
                        cmx,
                        ephemeral_key: ek,
                        ciphertext: action.ciphertext,
                        nullifier: nf,
                    });
                }
            }

            blocks.push(CompactBlock {
                height: block.height as u32,
                hash: block.hash,
                actions,
            });
        }

        Ok(blocks)
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
            txid: String::new(), // lightwalletd doesn't return txid
            error_code: resp.error_code,
            error_message: resp.error_message,
        })
    }

    /// get transaction by hash
    pub async fn get_transaction(&mut self, txid: &[u8; 32]) -> Result<Vec<u8>> {
        let request = tonic::Request::new(TxFilter {
            block: None,
            index: 0,
            hash: txid.to_vec(),
        });
        let response = self.client.get_transaction(request).await?;
        Ok(response.into_inner().data)
    }

    /// get tree state at a given height
    pub async fn get_tree_state(&mut self, height: u64) -> Result<TreeState> {
        let request = tonic::Request::new(BlockId {
            height,
            hash: vec![],
        });
        let response = self.client.get_tree_state(request).await?;
        let state = response.into_inner();

        Ok(TreeState {
            height: state.height as u32,
            hash: hex::decode(&state.hash).unwrap_or_default(),
            time: state.time as u64,
            sapling_tree: state.sapling_tree,
            orchard_tree: state.orchard_tree,
        })
    }

    /// get transparent UTXOs for addresses
    pub async fn get_address_utxos(&mut self, addresses: Vec<String>) -> Result<Vec<Utxo>> {
        let request = tonic::Request::new(GetAddressUtxosArg {
            addresses,
            start_height: 0,
            max_entries: 0,
        });
        let response = self.client.get_address_utxos(request).await?;
        let utxos = response.into_inner().address_utxos;

        Ok(utxos.into_iter().map(|u| {
            let mut txid = [0u8; 32];
            if u.txid.len() == 32 { txid.copy_from_slice(&u.txid); }
            Utxo {
                address: u.address,
                txid,
                output_index: u.index as u32,
                script: u.script,
                value_zat: u.value_zat as u64,
                height: u.height as u32,
            }
        }).collect())
    }
}

/// lightwalletd server info
#[derive(Debug, Clone)]
pub struct LightdInfo {
    pub version: String,
    pub vendor: String,
    pub taddr_support: bool,
    pub chain_name: String,
    pub sapling_activation_height: u64,
    pub consensus_branch_id: String,
    pub block_height: u64,
    pub estimated_height: u64,
}
