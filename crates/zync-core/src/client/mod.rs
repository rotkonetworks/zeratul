//! gRPC clients for Zcash light wallet services
//!
//! provides unified interface for both zidecar (our server) and lightwalletd (public fallback)

#[cfg(feature = "client")]
mod zidecar;
#[cfg(feature = "client")]
mod lightwalletd;

#[cfg(feature = "client")]
pub use zidecar::ZidecarClient;
#[cfg(feature = "client")]
pub use lightwalletd::LightwalletdClient;

/// public lightwalletd endpoints for fallback
pub const LIGHTWALLETD_MAINNET: &str = "https://mainnet.lightwalletd.com:9067";
pub const LIGHTWALLETD_MAINNET_ALT: &str = "https://lightwalletd.electriccoin.co:9067";
pub const LIGHTWALLETD_TESTNET: &str = "https://testnet.lightwalletd.com:9067";

/// generated protobuf types for zidecar
#[cfg(feature = "client")]
pub mod zidecar_proto {
    tonic::include_proto!("zidecar.v1");
}

/// generated protobuf types for lightwalletd
#[cfg(feature = "client")]
pub mod lightwalletd_proto {
    tonic::include_proto!("cash.z.wallet.sdk.rpc");
}

/// sync status from server
#[derive(Debug, Clone)]
pub struct SyncStatus {
    pub current_height: u32,
    pub current_epoch: u32,
    pub blocks_in_epoch: u32,
    pub complete_epochs: u32,
    pub gigaproof_ready: bool,
    pub blocks_until_ready: u32,
    pub last_gigaproof_height: u32,
}

/// tree state at a block height
#[derive(Debug, Clone)]
pub struct TreeState {
    pub height: u32,
    pub hash: Vec<u8>,
    pub time: u64,
    pub sapling_tree: String,
    pub orchard_tree: String,
}

/// transparent UTXO
#[derive(Debug, Clone)]
pub struct Utxo {
    pub address: String,
    pub txid: [u8; 32],
    pub output_index: u32,
    pub script: Vec<u8>,
    pub value_zat: u64,
    pub height: u32,
}

/// send transaction response
#[derive(Debug, Clone)]
pub struct SendResult {
    pub txid: String,
    pub error_code: i32,
    pub error_message: String,
}

impl SendResult {
    pub fn is_success(&self) -> bool {
        self.error_code == 0
    }
}

/// compact block for wallet scanning
#[derive(Debug, Clone)]
pub struct CompactBlock {
    pub height: u32,
    pub hash: Vec<u8>,
    pub actions: Vec<CompactAction>,
}

/// compact orchard action for trial decryption
#[derive(Debug, Clone)]
pub struct CompactAction {
    pub cmx: [u8; 32],
    pub ephemeral_key: [u8; 32],
    pub ciphertext: Vec<u8>,
    pub nullifier: [u8; 32],
}
