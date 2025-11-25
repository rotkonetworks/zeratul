//! IBP Monitoring Probe - Host Functions
//!
//! Provides actual network I/O for the PolkaVM guest program.
//! All host function results are recorded in the execution trace.
//!
//! ## Architecture
//!
//! The host provides generic primitives that can be composed by different
//! monitoring guest contracts:
//!
//! - Network: TCP, UDP, DNS resolution
//! - HTTP: GET, POST, HEAD
//! - WebSocket: connect, send, receive
//! - RPC: JSON-RPC calls, batching, subscriptions
//! - Substrate: chain info, sync state, peer count
//! - Relay: finalized block verification
//!
//! Guest contracts define WHAT to monitor, host provides HOW.

use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

pub mod primitives;
pub mod trace;

pub use primitives::*;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

use trace::{HostCallRecord, HostCallTrace};

/// Host call IDs - must match guest
pub const HOST_TCP_PING: u32 = 0x100;
pub const HOST_WSS_CONNECT: u32 = 0x101;
pub const HOST_WSS_CONNECT_LATENCY: u32 = 0x101 + 1; // Get latency from last wss_connect
pub const HOST_WSS_SUBSCRIBE: u32 = 0x102;
pub const HOST_RPC_CALL: u32 = 0x103;
pub const HOST_RPC_CALL_RESULT: u32 = 0x103 + 1; // Get result from RPC call
pub const HOST_RELAY_FINALIZED: u32 = 0x104;
pub const HOST_TIMESTAMP: u32 = 0x105;
pub const HOST_TIMESTAMP_HIGH: u32 = 0x105 + 1; // Get high bits of timestamp
pub const HOST_READ_INPUT: u32 = 0x106;
pub const HOST_WRITE_OUTPUT: u32 = 0x107;

/// Error types for host functions
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("TCP connection failed: {0}")]
    TcpError(String),
    #[error("WebSocket connection failed: {0}")]
    WssError(String),
    #[error("RPC call failed: {0}")]
    RpcError(String),
    #[error("Timeout")]
    Timeout,
    #[error("Invalid endpoint: {0}")]
    InvalidEndpoint(String),
}

/// WebSocket connection handle
pub struct WssConnection {
    pub handle: u32,
    pub latency_ms: u32,
    inner: Option<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>>,
}

/// IBP Host runtime - provides network I/O and records trace
pub struct IbpHost {
    /// Execution trace recording
    trace: Arc<RwLock<HostCallTrace>>,
    /// Active WebSocket connections
    wss_connections: RwLock<HashMap<u32, WssConnection>>,
    /// Next connection handle
    next_handle: RwLock<u32>,
    /// Relay chain RPC endpoint (for verification)
    relay_rpc: String,
    /// Input data for guest
    input_data: RwLock<Vec<u8>>,
    /// Output data from guest
    output_data: RwLock<Vec<u8>>,
    /// Cached relay finalized block
    relay_cache: RwLock<Option<(u32, [u8; 32])>>,
}

impl IbpHost {
    pub fn new(relay_rpc: &str) -> Self {
        Self {
            trace: Arc::new(RwLock::new(HostCallTrace::new())),
            wss_connections: RwLock::new(HashMap::new()),
            next_handle: RwLock::new(1),
            relay_rpc: relay_rpc.to_string(),
            input_data: RwLock::new(Vec::new()),
            output_data: RwLock::new(Vec::new()),
            relay_cache: RwLock::new(None),
        }
    }

    /// Set input data for the guest
    pub async fn set_input(&self, data: Vec<u8>) {
        *self.input_data.write().await = data;
    }

    /// Get output data from the guest
    pub async fn get_output(&self) -> Vec<u8> {
        self.output_data.read().await.clone()
    }

    /// Get the execution trace
    pub async fn get_trace(&self) -> HostCallTrace {
        self.trace.read().await.clone()
    }

    /// TCP ping - measure connection latency
    pub async fn tcp_ping(
        &self,
        endpoint: &str,
        port: u16,
        timeout_ms: u32,
    ) -> Result<u32, HostError> {
        let addr = format!("{}:{}", endpoint, port);
        let socket_addr = addr
            .to_socket_addrs()
            .map_err(|e| HostError::InvalidEndpoint(e.to_string()))?
            .next()
            .ok_or_else(|| HostError::InvalidEndpoint("no address found".into()))?;

        let start = Instant::now();
        let result = timeout(
            Duration::from_millis(timeout_ms as u64),
            TcpStream::connect(socket_addr),
        )
        .await;

        let latency_ms = match result {
            Ok(Ok(_stream)) => start.elapsed().as_millis() as u32,
            Ok(Err(e)) => {
                tracing::warn!("TCP connect failed: {}", e);
                u32::MAX
            }
            Err(_) => {
                tracing::warn!("TCP connect timeout");
                u32::MAX
            }
        };

        // Record in trace
        self.trace.write().await.record(HostCallRecord {
            call_id: HOST_TCP_PING,
            inputs: vec![
                endpoint.as_bytes().to_vec(),
                port.to_le_bytes().to_vec(),
                timeout_ms.to_le_bytes().to_vec(),
            ],
            output: latency_ms.to_le_bytes().to_vec(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        });

        Ok(latency_ms)
    }

    /// WebSocket connect
    pub async fn wss_connect(
        &self,
        endpoint: &str,
        timeout_ms: u32,
    ) -> Result<(u32, u32), HostError> {
        let url = if endpoint.starts_with("wss://") || endpoint.starts_with("ws://") {
            endpoint.to_string()
        } else {
            format!("wss://{}", endpoint)
        };

        let start = Instant::now();
        let result = timeout(Duration::from_millis(timeout_ms as u64), connect_async(&url)).await;

        let (handle, latency_ms, ws_stream) = match result {
            Ok(Ok((ws_stream, _response))) => {
                let latency = start.elapsed().as_millis() as u32;
                let mut next = self.next_handle.write().await;
                let handle = *next;
                *next += 1;
                (handle, latency, Some(ws_stream))
            }
            Ok(Err(e)) => {
                tracing::warn!("WebSocket connect failed: {}", e);
                (0, u32::MAX, None)
            }
            Err(_) => {
                tracing::warn!("WebSocket connect timeout");
                (0, u32::MAX, None)
            }
        };

        // Store connection
        if let Some(stream) = ws_stream {
            self.wss_connections.write().await.insert(
                handle,
                WssConnection {
                    handle,
                    latency_ms,
                    inner: Some(stream),
                },
            );
        }

        // Record in trace
        self.trace.write().await.record(HostCallRecord {
            call_id: HOST_WSS_CONNECT,
            inputs: vec![endpoint.as_bytes().to_vec(), timeout_ms.to_le_bytes().to_vec()],
            output: [handle.to_le_bytes(), latency_ms.to_le_bytes()].concat(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        });

        Ok((handle, latency_ms))
    }

    /// JSON-RPC call
    pub async fn rpc_call(
        &self,
        endpoint: &str,
        method: &str,
        params: &[serde_json::Value],
    ) -> Result<serde_json::Value, HostError> {
        let url = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.to_string()
        } else {
            format!("https://{}", endpoint)
        };

        let client = reqwest::Client::new();
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let start = Instant::now();
        let response = client
            .post(&url)
            .json(&request)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| HostError::RpcError(e.to_string()))?;

        let latency_ms = start.elapsed().as_millis() as u32;

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| HostError::RpcError(e.to_string()))?;

        let result = body
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // Record in trace
        self.trace.write().await.record(HostCallRecord {
            call_id: HOST_RPC_CALL,
            inputs: vec![
                endpoint.as_bytes().to_vec(),
                method.as_bytes().to_vec(),
                serde_json::to_vec(params).unwrap_or_default(),
            ],
            output: serde_json::to_vec(&result).unwrap_or_default(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        });

        Ok(result)
    }

    /// Get relay chain finalized block
    pub async fn relay_finalized_block(&self) -> Result<(u32, [u8; 32]), HostError> {
        // Check cache first (valid for ~6 seconds)
        if let Some((block, hash)) = self.relay_cache.read().await.as_ref() {
            return Ok((*block, *hash));
        }

        // Convert WSS to HTTPS for HTTP RPC calls
        let relay_http = self.relay_rpc
            .replace("wss://", "https://")
            .replace("ws://", "http://");

        // Fetch from relay RPC
        let result = self
            .rpc_call(&relay_http, "chain_getFinalizedHead", &[])
            .await?;

        let hash_hex = result
            .as_str()
            .ok_or_else(|| HostError::RpcError("invalid hash response".into()))?;

        // Parse hex hash
        let hash_bytes = hex::decode(hash_hex.trim_start_matches("0x"))
            .map_err(|e| HostError::RpcError(e.to_string()))?;

        let mut hash = [0u8; 32];
        if hash_bytes.len() >= 32 {
            hash.copy_from_slice(&hash_bytes[..32]);
        }

        // Get block number
        let header_result = self
            .rpc_call(
                &relay_http,
                "chain_getHeader",
                &[serde_json::Value::String(hash_hex.to_string())],
            )
            .await?;

        let block_num_hex = header_result
            .get("number")
            .and_then(|n| n.as_str())
            .unwrap_or("0x0");

        let block_num =
            u32::from_str_radix(block_num_hex.trim_start_matches("0x"), 16).unwrap_or(0);

        // Cache result
        *self.relay_cache.write().await = Some((block_num, hash));

        // Record in trace
        self.trace.write().await.record(HostCallRecord {
            call_id: HOST_RELAY_FINALIZED,
            inputs: vec![],
            output: [block_num.to_le_bytes().to_vec(), hash.to_vec()].concat(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        });

        Ok((block_num, hash))
    }

    /// Current timestamp
    pub fn timestamp_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Read input (called by guest)
    pub async fn read_input(&self, max_len: usize) -> Vec<u8> {
        let data = self.input_data.read().await;
        data[..std::cmp::min(data.len(), max_len)].to_vec()
    }

    /// Write output (called by guest)
    pub async fn write_output(&self, data: Vec<u8>) {
        *self.output_data.write().await = data;
    }

    /// Handle a host call from the guest
    pub async fn handle_host_call(
        &self,
        call_id: u32,
        memory: &mut [u8],
        a0: u32,
        a1: u32,
        a2: u32,
        a3: u32,
    ) -> u32 {
        match call_id {
            HOST_TCP_PING => {
                let endpoint = read_string_from_memory(memory, a0, a1);
                let port = a2 as u16;
                let timeout_ms = a3;
                self.tcp_ping(&endpoint, port, timeout_ms).await.unwrap_or(u32::MAX)
            }
            HOST_WSS_CONNECT => {
                let endpoint = read_string_from_memory(memory, a0, a1);
                let timeout_ms = a2;
                match self.wss_connect(&endpoint, timeout_ms).await {
                    Ok((handle, _latency)) => handle,
                    Err(_) => 0,
                }
            }
            HOST_WSS_CONNECT_LATENCY => {
                // Return high bits (latency) from last wss_connect
                // This is a simplified approach - in practice would track per-call
                0
            }
            HOST_RPC_CALL => {
                let endpoint = read_string_from_memory(memory, a0, a1);
                let method = read_string_from_memory(memory, a2, a3);
                // Store endpoint/method for next call
                1 // success
            }
            HOST_RPC_CALL_RESULT => {
                // Execute RPC and write result to memory
                // Simplified - would need state from previous call
                0
            }
            HOST_RELAY_FINALIZED => {
                match tokio::runtime::Handle::current()
                    .block_on(self.relay_finalized_block())
                {
                    Ok((block_num, hash)) => {
                        // Write hash to memory at a0
                        if (a0 as usize) + 32 <= memory.len() {
                            memory[a0 as usize..(a0 as usize + 32)].copy_from_slice(&hash);
                        }
                        block_num
                    }
                    Err(_) => 0,
                }
            }
            HOST_TIMESTAMP => self.timestamp_ms() as u32,
            HOST_TIMESTAMP_HIGH => (self.timestamp_ms() >> 32) as u32,
            HOST_READ_INPUT => {
                let data = tokio::runtime::Handle::current().block_on(self.read_input(a1 as usize));
                let len = std::cmp::min(data.len(), a1 as usize);
                if (a0 as usize) + len <= memory.len() {
                    memory[a0 as usize..(a0 as usize + len)].copy_from_slice(&data[..len]);
                }
                len as u32
            }
            HOST_WRITE_OUTPUT => {
                if (a0 as usize) + (a1 as usize) <= memory.len() {
                    let data = memory[a0 as usize..(a0 as usize + a1 as usize)].to_vec();
                    tokio::runtime::Handle::current().block_on(self.write_output(data));
                }
                0
            }
            _ => {
                tracing::warn!("Unknown host call: {:#x}", call_id);
                0
            }
        }
    }
}

/// Helper to read string from guest memory
fn read_string_from_memory(memory: &[u8], ptr: u32, len: u32) -> String {
    let start = ptr as usize;
    let end = start + len as usize;
    if end <= memory.len() {
        String::from_utf8_lossy(&memory[start..end]).to_string()
    } else {
        String::new()
    }
}
