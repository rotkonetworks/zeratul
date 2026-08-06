// transport.rs — pluggable transport trait for FROST coordination
//
// "your server as a function" — the transport is a composable filter.
// DKG and signing orchestration use this trait. swap relay for memos
// for QUIC without touching protocol logic.
//
// three backends:
//   RelayTransport — gRPC/WebSocket to frost relay (fast, interactive)
//   MemoTransport  — ZIP-302 memos as dust notes (slow, serverless)
//   QuicTransport  — QUIC direct peer-to-peer (fast, no relay server)

use std::future::Future;
use std::pin::Pin;

/// a round message from one participant
#[derive(Debug, Clone)]
pub struct RoundMessage {
    /// participant identifier (FROST Identifier, derived from ed25519 vk)
    pub sender: Vec<u8>,
    /// raw protocol data (hex-encoded FROST round output)
    pub payload: Vec<u8>,
}

/// transport errors
#[derive(Debug)]
pub enum TransportError {
    /// connection or I/O failure
    Io(String),
    /// timed out waiting for peers
    Timeout,
    /// peer sent invalid data
    InvalidMessage(String),
    /// transport-specific error
    Other(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "transport io: {}", s),
            Self::Timeout => write!(f, "transport timeout"),
            Self::InvalidMessage(s) => write!(f, "invalid message: {}", s),
            Self::Other(s) => write!(f, "transport: {}", s),
        }
    }
}

impl std::error::Error for TransportError {}

/// result type for transport operations
pub type TransportResult<T> = Result<T, TransportError>;

/// pluggable transport for FROST round coordination.
///
/// DKG and signing call broadcast() then collect() for each round.
/// the transport handles delivery and gathering — relay, memos, QUIC, whatever.
pub trait FrostTransport: Send + Sync {
    /// broadcast our round data to all peers.
    /// `round` identifies which protocol round (1, 2, 3 for DKG; 1, 2 for signing).
    fn broadcast(
        &self,
        round: u8,
        data: &[u8],
    ) -> Pin<Box<dyn Future<Output = TransportResult<()>> + Send + '_>>;

    /// collect round data from all other participants.
    /// blocks until `expected` messages are received or timeout.
    fn collect(
        &self,
        round: u8,
        expected: usize,
    ) -> Pin<Box<dyn Future<Output = TransportResult<Vec<RoundMessage>>> + Send + '_>>;
}

/// transport-agnostic DKG coordinator.
/// runs 3 rounds over any FrostTransport.
pub async fn run_dkg(
    transport: &dyn FrostTransport,
    max_signers: u16,
    min_signers: u16,
    peer_count: usize,
) -> TransportResult<crate::orchestrate::Dkg3Result> {
    // round 1: generate + broadcast commitment
    let r1 = crate::orchestrate::dkg_part1(max_signers, min_signers)
        .map_err(|e| TransportError::Other(e.to_string()))?;
    transport.broadcast(1, r1.broadcast_hex.as_bytes()).await?;

    // collect peer round 1 broadcasts
    let peer_r1 = transport.collect(1, peer_count).await?;
    let peer_broadcasts: Vec<String> = peer_r1
        .iter()
        .map(|m| String::from_utf8_lossy(&m.payload).to_string())
        .collect();

    // round 2: process + broadcast peer packages
    let r2 = crate::orchestrate::dkg_part2(&r1.secret_hex, &peer_broadcasts)
        .map_err(|e| TransportError::Other(e.to_string()))?;
    for pkg in &r2.peer_packages {
        transport.broadcast(2, pkg.as_bytes()).await?;
    }

    // collect peer round 2 packages
    let peer_r2 = transport.collect(2, peer_count).await?;
    let peer_packages: Vec<String> = peer_r2
        .iter()
        .map(|m| String::from_utf8_lossy(&m.payload).to_string())
        .collect();

    // round 3: finalize
    let r3 = crate::orchestrate::dkg_part3(&r2.secret_hex, &peer_broadcasts, &peer_packages)
        .map_err(|e| TransportError::Other(e.to_string()))?;

    Ok(r3)
}
