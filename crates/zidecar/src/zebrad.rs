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
    #[serde(default)]
    pub orchard: Option<OrchardData>,
}

#[derive(Debug, Deserialize)]
pub struct OrchardData {
    #[serde(default)]
    pub actions: Vec<OrchardAction>,
}

#[derive(Debug, Deserialize)]
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
