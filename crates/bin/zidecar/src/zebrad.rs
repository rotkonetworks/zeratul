//! zebrad RPC client

use crate::error::{Result, ZidecarError};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct ZebradClient {
    url: String,
    client: Client,
}

impl ZebradClient {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            client: Client::new(),
        }
    }

    async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": "zidecar",
            "method": method,
            "params": params,
        });

        let response = self
            .client
            .post(&self.url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))?;

        let json: RpcResponse = response
            .json()
            .await
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))?;

        if let Some(error) = json.error {
            return Err(ZidecarError::ZebradRpc(format!(
                "RPC error {}: {}",
                error.code, error.message
            )));
        }

        json.result
            .ok_or_else(|| ZidecarError::ZebradRpc("no result in response".into()))
    }

    pub async fn get_blockchain_info(&self) -> Result<BlockchainInfo> {
        let result = self.call("getblockchaininfo", vec![]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    pub async fn get_block_hash(&self, height: u32) -> Result<String> {
        let result = self.call("getblockhash", vec![json!(height)]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    pub async fn get_block(&self, hash: &str, verbosity: u8) -> Result<Block> {
        let result = self.call("getblock", vec![json!(hash), json!(verbosity)]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    /// Get block with full transaction data (verbosity=2)
    pub async fn get_block_with_txs(&self, hash: &str) -> Result<BlockWithTxs> {
        let result = self.call("getblock", vec![json!(hash), json!(2)]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    pub async fn get_block_header(&self, hash: &str) -> Result<BlockHeader> {
        let block = self.get_block(hash, 1).await?;
        Ok(BlockHeader {
            height: block.height,
            hash: block.hash,
            prev_hash: block.previousblockhash.unwrap_or_default(),
            timestamp: block.time,
            merkle_root: block.merkleroot,
        })
    }

    pub async fn get_raw_transaction(&self, txid: &str) -> Result<RawTransaction> {
        let result = self.call("getrawtransaction", vec![json!(txid), json!(1)]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    /// send raw transaction hex to the network
    pub async fn send_raw_transaction(&self, tx_hex: &str) -> Result<String> {
        let result = self.call("sendrawtransaction", vec![json!(tx_hex)]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    /// get tree state at a given block height or hash
    pub async fn get_tree_state(&self, height_or_hash: &str) -> Result<TreeState> {
        let result = self.call("z_gettreestate", vec![json!(height_or_hash)]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    /// get transparent UTXOs for addresses
    pub async fn get_address_utxos(&self, addresses: &[String]) -> Result<Vec<AddressUtxo>> {
        let result = self.call("getaddressutxos", vec![json!({"addresses": addresses})]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    /// get transaction IDs for transparent addresses
    pub async fn get_address_txids(&self, addresses: &[String], start: u32, end: u32) -> Result<Vec<String>> {
        let result = self.call("getaddresstxids", vec![json!({
            "addresses": addresses,
            "start": start,
            "end": end
        })]).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }

    /// get subtrees by index for sapling/orchard commitment trees
    /// returns precomputed subtree roots for efficient witness reconstruction
    pub async fn get_subtrees_by_index(&self, pool: &str, start_index: u32, limit: Option<u32>) -> Result<SubtreeResponse> {
        let mut params = vec![json!(pool), json!(start_index)];
        if let Some(l) = limit {
            params.push(json!(l));
        }
        let result = self.call("z_getsubtreesbyindex", params).await?;
        serde_json::from_value(result)
            .map_err(|e| ZidecarError::ZebradRpc(e.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize)]
pub struct BlockchainInfo {
    pub chain: String,
    pub blocks: u32,
    pub bestblockhash: String,
    pub difficulty: f64,
}

#[derive(Debug, Deserialize)]
pub struct Block {
    pub hash: String,
    pub height: u32,
    pub version: u32,
    pub merkleroot: String,
    pub time: u64,
    pub nonce: String,
    pub bits: String,
    pub difficulty: f64,
    pub previousblockhash: Option<String>,
    pub nextblockhash: Option<String>,
    pub tx: Vec<String>,
}

/// Block with full transaction data (verbosity=2)
#[derive(Debug, Deserialize)]
pub struct BlockWithTxs {
    pub hash: String,
    pub height: u32,
    pub version: u32,
    pub merkleroot: String,
    pub time: u64,
    pub nonce: String,
    pub bits: String,
    pub difficulty: f64,
    pub previousblockhash: Option<String>,
    pub nextblockhash: Option<String>,
    pub tx: Vec<TransactionData>,
}

/// Transaction data from block (verbosity=2)
#[derive(Debug, Deserialize)]
pub struct TransactionData {
    pub txid: String,
    pub version: u32,
    #[serde(default)]
    pub orchard: Option<OrchardBundle>,
}

/// Orchard bundle from transaction
#[derive(Debug, Deserialize)]
pub struct OrchardBundle {
    #[serde(default)]
    pub actions: Vec<OrchardAction>,
}

#[derive(Debug, Clone)]
pub struct BlockHeader {
    pub height: u32,
    pub hash: String,
    pub prev_hash: String,
    pub timestamp: u64,
    pub merkle_root: String,
}

#[derive(Debug, Deserialize)]
pub struct RawTransaction {
    pub txid: String,
    pub version: u32,
    pub hex: String,
    /// Block height (only present if confirmed)
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub orchard: Option<OrchardData>,
}

#[derive(Debug, Deserialize)]
pub struct OrchardData {
    #[serde(default)]
    pub actions: Vec<OrchardAction>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OrchardAction {
    pub cv: String,
    pub nullifier: String,
    pub rk: String,
    pub cmx: String,
    #[serde(rename = "ephemeralKey")]
    pub ephemeral_key: String,
    #[serde(rename = "encCiphertext")]
    pub enc_ciphertext: String,
    #[serde(rename = "outCiphertext")]
    pub out_ciphertext: String,
}

impl OrchardAction {
    /// Parse nullifier hex string to bytes
    pub fn nullifier_bytes(&self) -> Option<[u8; 32]> {
        hex::decode(&self.nullifier)
            .ok()
            .and_then(|b| b.try_into().ok())
    }

    /// Parse cmx hex string to bytes
    pub fn cmx_bytes(&self) -> Option<[u8; 32]> {
        hex::decode(&self.cmx)
            .ok()
            .and_then(|b| b.try_into().ok())
    }
}

/// tree state from z_gettreestate RPC
#[derive(Debug, Deserialize)]
pub struct TreeState {
    pub height: u32,
    pub hash: String,
    pub time: u64,
    pub sapling: TreeCommitment,
    pub orchard: TreeCommitment,
}

#[derive(Debug, Deserialize)]
pub struct TreeCommitment {
    #[serde(rename = "commitments")]
    pub commitments: TreeCommitmentData,
}

#[derive(Debug, Deserialize)]
pub struct TreeCommitmentData {
    #[serde(rename = "finalState")]
    pub final_state: String,
}

/// transparent UTXO from getaddressutxos
#[derive(Debug, Deserialize)]
pub struct AddressUtxo {
    pub address: String,
    pub txid: String,
    #[serde(rename = "outputIndex")]
    pub output_index: u32,
    pub script: String,
    pub satoshis: u64,
    pub height: u32,
}

/// response from z_getsubtreesbyindex
#[derive(Debug, Deserialize)]
pub struct SubtreeResponse {
    pub pool: String,
    #[serde(rename = "start_index")]
    pub start_index: u32,
    pub subtrees: Vec<Subtree>,
}

/// individual subtree from z_getsubtreesbyindex
/// each subtree covers 2^16 = 65536 leaves
#[derive(Debug, Deserialize, Clone)]
pub struct Subtree {
    /// merkle root of this subtree (hex)
    pub root: String,
    /// block height where subtree was completed
    #[serde(rename = "end_height")]
    pub end_height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires zebrad running
    async fn test_zebrad_connection() {
        let client = ZebradClient::new("http://127.0.0.1:8232");
        let info = client.get_blockchain_info().await.unwrap();
        assert!(!info.chain.is_empty());
    }
}
