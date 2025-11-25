//! gRPC client re-exports from zync-core

pub use zync_core::client::{
    ZidecarClient,
    LightwalletdClient,
    SyncStatus,
    TreeState,
    Utxo,
    SendResult,
    CompactBlock,
    CompactAction,
    LIGHTWALLETD_MAINNET,
    LIGHTWALLETD_MAINNET_ALT,
    LIGHTWALLETD_TESTNET,
};
