//! public relay protocol
//!
//! simple mailbox-based relay that anyone can run.
//! syndicates use pseudonymous mailbox IDs - relay sees nothing.
//!
//! # design goals
//!
//! - **stateless**: relay just stores/forwards, no logic
//! - **pseudonymous**: mailbox IDs are hashes, no identity
//! - **public**: anyone can run a relay, anyone can use it
//! - **simple**: HTTP or WebSocket, trivial to implement
//!
//! # protocol
//!
//! ```text
//! POST /mailbox/{id}
//!   body: encrypted message bytes
//!   returns: message_id
//!
//! GET /mailbox/{id}?after={cursor}
//!   returns: [{ id, timestamp, data }, ...]
//!
//! WS /subscribe/{id}
//!   streams: new messages as they arrive
//!
//! DELETE /mailbox/{id}/{message_id}
//!   optional: cleanup old messages
//! ```
//!
//! # privacy
//!
//! - messages are encrypted end-to-end (relay can't read)
//! - mailbox IDs are derived from syndicate viewing key (unlinkable)
//! - relay only sees: mailbox ID, message size, timestamp
//! - multiple relays can be used for redundancy
//!
//! # hosting options
//!
//! - **community relays**: run by ecosystem (like iroh relays)
//! - **self-hosted**: anyone can run `narsil-relay` binary
//! - **ipfs/web3.storage**: use decentralized storage as relay
//! - **cloudflare workers**: serverless, free tier available

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::wire::Hash32;

/// relay endpoint configuration
#[derive(Clone, Debug)]
pub struct RelayEndpoint {
    /// base URL (e.g., "https://relay1.narsil.network")
    pub url: String,
    /// optional auth token (for private relays)
    pub auth: Option<String>,
    /// priority (lower = preferred)
    pub priority: u8,
}

impl RelayEndpoint {
    /// create public relay endpoint
    pub fn public(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth: None,
            priority: 0,
        }
    }

    /// create authenticated endpoint
    pub fn authenticated(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth: Some(token.into()),
            priority: 0,
        }
    }

    /// set priority
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
}

/// message posted to relay
#[derive(Clone, Debug)]
pub struct RelayMessage {
    /// message ID (assigned by relay)
    pub id: Hash32,
    /// timestamp (unix seconds)
    pub timestamp: u64,
    /// encrypted payload
    pub data: Vec<u8>,
}

/// relay request types
#[derive(Clone, Debug)]
pub enum RelayRequest {
    /// post message to mailbox
    Post {
        mailbox: Hash32,
        data: Vec<u8>,
    },
    /// fetch messages from mailbox
    Fetch {
        mailbox: Hash32,
        after: Option<Hash32>,
        limit: u32,
    },
    /// subscribe to mailbox (websocket)
    Subscribe {
        mailbox: Hash32,
    },
    /// delete old message
    Delete {
        mailbox: Hash32,
        message_id: Hash32,
    },
}

/// relay response types
#[derive(Clone, Debug)]
pub enum RelayResponse {
    /// post successful
    Posted { message_id: Hash32 },
    /// fetched messages
    Messages(Vec<RelayMessage>),
    /// subscription event
    NewMessage(RelayMessage),
    /// deleted
    Deleted,
    /// error
    Error(RelayError),
}

/// relay errors
#[derive(Clone, Debug)]
pub enum RelayError {
    /// network error
    Network(String),
    /// mailbox not found (or empty)
    NotFound,
    /// rate limited
    RateLimited,
    /// message too large
    TooLarge,
    /// relay unavailable
    Unavailable,
}

/// relay client configuration
#[derive(Clone, Debug)]
pub struct RelayConfig {
    /// relay endpoints (will try in priority order)
    pub endpoints: Vec<RelayEndpoint>,
    /// max message size (default 64KB)
    pub max_message_size: usize,
    /// message TTL in seconds (default 7 days)
    pub message_ttl: u64,
    /// retry count on failure
    pub retries: u8,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            endpoints: vec![],
            max_message_size: 64 * 1024,
            message_ttl: 7 * 24 * 3600,
            retries: 3,
        }
    }
}

impl RelayConfig {
    /// create with public relays
    pub fn public() -> Self {
        Self {
            endpoints: vec![
                // these would be real community relays
                RelayEndpoint::public("https://relay1.narsil.network").with_priority(0),
                RelayEndpoint::public("https://relay2.narsil.network").with_priority(1),
            ],
            ..Default::default()
        }
    }

    /// add endpoint
    pub fn with_endpoint(mut self, endpoint: RelayEndpoint) -> Self {
        self.endpoints.push(endpoint);
        self.endpoints.sort_by_key(|e| e.priority);
        self
    }

    /// set message TTL
    pub fn with_ttl(mut self, seconds: u64) -> Self {
        self.message_ttl = seconds;
        self
    }
}

/// trait for relay implementations
#[cfg(feature = "std")]
pub trait RelayClient: Send + Sync {
    /// post message to mailbox
    fn post(&self, mailbox: &Hash32, data: &[u8]) -> Result<Hash32, RelayError>;

    /// fetch messages from mailbox
    fn fetch(&self, mailbox: &Hash32, after: Option<Hash32>) -> Result<Vec<RelayMessage>, RelayError>;

    /// check if relay is reachable
    fn ping(&self) -> bool;
}

/// multi-relay client (tries endpoints in order)
#[cfg(feature = "std")]
pub struct MultiRelayClient {
    config: RelayConfig,
}

#[cfg(feature = "std")]
impl MultiRelayClient {
    /// create with config
    pub fn new(config: RelayConfig) -> Self {
        Self { config }
    }

    /// create with public relays
    pub fn public() -> Self {
        Self::new(RelayConfig::public())
    }

    /// post to first available relay
    pub fn post(&self, mailbox: &Hash32, data: &[u8]) -> Result<Hash32, RelayError> {
        if data.len() > self.config.max_message_size {
            return Err(RelayError::TooLarge);
        }

        for endpoint in &self.config.endpoints {
            // TODO: actual HTTP request
            // let url = format!("{}/mailbox/{}", endpoint.url, hex::encode(mailbox));
            // match http_post(&url, data, &endpoint.auth) { ... }
            let _ = endpoint;
        }

        // mock: return hash of data as message ID
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data);
        let id: [u8; 32] = hasher.finalize().into();
        Ok(id)
    }

    /// fetch from first available relay
    pub fn fetch(&self, mailbox: &Hash32, after: Option<Hash32>) -> Result<Vec<RelayMessage>, RelayError> {
        for endpoint in &self.config.endpoints {
            // TODO: actual HTTP request
            // let url = format!("{}/mailbox/{}", endpoint.url, hex::encode(mailbox));
            let _ = (endpoint, mailbox, after);
        }

        Ok(vec![])
    }

    /// broadcast to all relays (for redundancy)
    pub fn broadcast(&self, mailbox: &Hash32, data: &[u8]) -> Vec<Result<Hash32, RelayError>> {
        self.config.endpoints.iter()
            .map(|_endpoint| {
                // post to each endpoint
                self.post(mailbox, data)
            })
            .collect()
    }
}

/// simple relay server spec (for implementers)
///
/// any HTTP server implementing these endpoints is a valid narsil relay:
///
/// ```text
/// POST /mailbox/:id
///   Content-Type: application/octet-stream
///   Body: <encrypted bytes>
///   Response: { "id": "<hex message id>" }
///
/// GET /mailbox/:id?after=<cursor>&limit=<n>
///   Response: {
///     "messages": [
///       { "id": "<hex>", "timestamp": <unix>, "data": "<base64>" }
///     ],
///     "cursor": "<next cursor or null>"
///   }
///
/// DELETE /mailbox/:id/:message_id
///   Response: { "ok": true }
///
/// GET /health
///   Response: { "status": "ok", "version": "0.1" }
/// ```
///
/// storage can be:
/// - sqlite (simplest)
/// - redis (fast, auto-expiry)
/// - s3/r2 (scalable)
/// - ipfs (decentralized)
pub struct RelayServerSpec;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_creation() {
        let public = RelayEndpoint::public("https://relay.example.com");
        assert!(public.auth.is_none());
        assert_eq!(public.priority, 0);

        let auth = RelayEndpoint::authenticated("https://private.example.com", "token123")
            .with_priority(5);
        assert_eq!(auth.auth, Some("token123".into()));
        assert_eq!(auth.priority, 5);
    }

    #[test]
    fn test_config_defaults() {
        let config = RelayConfig::default();
        assert_eq!(config.max_message_size, 64 * 1024);
        assert_eq!(config.message_ttl, 7 * 24 * 3600);
        assert_eq!(config.retries, 3);
    }

    #[test]
    fn test_config_public() {
        let config = RelayConfig::public();
        assert_eq!(config.endpoints.len(), 2);
    }

    #[test]
    fn test_multi_relay_client() {
        let client = MultiRelayClient::public();
        let mailbox = [1u8; 32];
        let data = b"test message";

        // mock post
        let result = client.post(&mailbox, data);
        assert!(result.is_ok());

        // mock fetch
        let messages = client.fetch(&mailbox, None);
        assert!(messages.is_ok());
    }

    #[test]
    fn test_message_too_large() {
        let config = RelayConfig::default();
        let client = MultiRelayClient::new(config);
        let mailbox = [1u8; 32];
        let large_data = vec![0u8; 100 * 1024]; // 100KB > 64KB limit

        let result = client.post(&mailbox, &large_data);
        assert!(matches!(result, Err(RelayError::TooLarge)));
    }
}
