//! relay client for async coordination
//!
//! members post to and fetch from relays using pseudonymous mailboxes.
//! the relay cannot read message content (encrypted) or link mailboxes
//! to identities.
//!
//! # relay backends
//!
//! - IPFS: content-addressed, decentralized
//! - DHT: kademlia-style key-value store
//! - HTTP: simple REST endpoints (e.g., S3)
//!
//! all backends implement the `RelayClient` trait.

use alloc::vec::Vec;

use crate::mailbox::{MailboxId, BroadcastTopic};
use crate::wire::Hash32;

/// message stored in relay
#[derive(Clone, Debug)]
pub struct RelayMessage {
    /// unique message id (assigned by relay)
    pub id: Hash32,
    /// encrypted content
    pub content: Vec<u8>,
    /// timestamp (relay-assigned, untrusted)
    pub timestamp: u64,
}

/// fetch options
#[derive(Clone, Debug, Default)]
pub struct FetchOptions {
    /// only fetch messages after this id
    pub after: Option<Hash32>,
    /// maximum messages to fetch
    pub limit: Option<usize>,
    /// fetch timeout in milliseconds
    pub timeout_ms: Option<u64>,
}

/// relay error
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayError {
    /// connection failed
    ConnectionFailed(RelayErrorDetail),
    /// post failed
    PostFailed(RelayErrorDetail),
    /// fetch failed
    FetchFailed(RelayErrorDetail),
    /// timeout
    Timeout,
    /// not found
    NotFound,
    /// rate limited
    RateLimited,
    /// relay returned invalid data
    InvalidData,
}

/// error details (relay-specific)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayErrorDetail {
    pub code: u32,
    pub message: &'static str,
}

impl core::fmt::Display for RelayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ConnectionFailed(d) => write!(f, "connection failed: {}", d.message),
            Self::PostFailed(d) => write!(f, "post failed: {}", d.message),
            Self::FetchFailed(d) => write!(f, "fetch failed: {}", d.message),
            Self::Timeout => write!(f, "timeout"),
            Self::NotFound => write!(f, "not found"),
            Self::RateLimited => write!(f, "rate limited"),
            Self::InvalidData => write!(f, "invalid data from relay"),
        }
    }
}

/// relay client trait (async)
///
/// implementors provide specific relay backends:
/// - `IpfsRelay`: IPFS pubsub + content addressing
/// - `DhtRelay`: kademlia DHT
/// - `HttpRelay`: REST API (S3-compatible)
#[cfg(feature = "net")]
#[allow(async_fn_in_trait)]
pub trait RelayClient: Send + Sync {
    /// post message to a mailbox
    async fn post_to_mailbox(
        &self,
        mailbox: &MailboxId,
        message: &[u8],
    ) -> Result<Hash32, RelayError>;

    /// fetch messages from a mailbox
    async fn fetch_from_mailbox(
        &self,
        mailbox: &MailboxId,
        options: FetchOptions,
    ) -> Result<Vec<RelayMessage>, RelayError>;

    /// post to broadcast topic
    async fn broadcast(
        &self,
        topic: &BroadcastTopic,
        message: &[u8],
    ) -> Result<Hash32, RelayError>;

    /// subscribe to broadcast topic (returns message receiver)
    async fn subscribe(
        &self,
        topic: &BroadcastTopic,
    ) -> Result<BroadcastSubscription, RelayError>;

    /// check if relay is reachable
    async fn health_check(&self) -> Result<(), RelayError>;
}

/// broadcast subscription receiver
/// uses tokio channels instead of async trait methods for dyn compatibility
#[cfg(feature = "net")]
pub struct BroadcastSubscription {
    receiver: tokio::sync::mpsc::Receiver<RelayMessage>,
}

#[cfg(feature = "net")]
impl BroadcastSubscription {
    /// create new subscription with channel
    pub fn new(receiver: tokio::sync::mpsc::Receiver<RelayMessage>) -> Self {
        Self { receiver }
    }

    /// get next message (async)
    pub async fn next(&mut self) -> Option<RelayMessage> {
        self.receiver.recv().await
    }

    /// try to get next message without blocking
    pub fn try_next(&mut self) -> Option<RelayMessage> {
        self.receiver.try_recv().ok()
    }
}

/// mock relay for testing
#[derive(Clone, Debug, Default)]
pub struct MockRelay {
    /// stored messages per mailbox
    #[cfg(feature = "std")]
    mailboxes: std::sync::Arc<std::sync::Mutex<
        alloc::collections::BTreeMap<[u8; 32], Vec<RelayMessage>>
    >>,
    /// broadcast messages per topic
    #[cfg(feature = "std")]
    broadcasts: std::sync::Arc<std::sync::Mutex<
        alloc::collections::BTreeMap<[u8; 32], Vec<RelayMessage>>
    >>,
    /// next message id
    #[cfg(feature = "std")]
    next_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(feature = "std")]
impl MockRelay {
    /// create new mock relay
    pub fn new() -> Self {
        Self {
            mailboxes: std::sync::Arc::new(std::sync::Mutex::new(
                alloc::collections::BTreeMap::new()
            )),
            broadcasts: std::sync::Arc::new(std::sync::Mutex::new(
                alloc::collections::BTreeMap::new()
            )),
            next_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    fn generate_id(&self) -> Hash32 {
        use sha2::{Digest, Sha256};
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let hash: [u8; 32] = Sha256::digest(&id.to_le_bytes()).into();
        hash
    }

    /// post to mailbox (sync version for testing)
    pub fn post_sync(&self, mailbox: &MailboxId, message: &[u8]) -> Hash32 {
        let id = self.generate_id();
        let msg = RelayMessage {
            id,
            content: message.to_vec(),
            timestamp: 0, // mock timestamp
        };
        let mut mailboxes = self.mailboxes.lock().unwrap();
        mailboxes.entry(mailbox.0).or_default().push(msg);
        id
    }

    /// fetch from mailbox (sync version for testing)
    pub fn fetch_sync(&self, mailbox: &MailboxId) -> Vec<RelayMessage> {
        let mailboxes = self.mailboxes.lock().unwrap();
        mailboxes.get(&mailbox.0).cloned().unwrap_or_default()
    }

    /// post to broadcast (sync version)
    pub fn broadcast_sync(&self, topic: &BroadcastTopic, message: &[u8]) -> Hash32 {
        let id = self.generate_id();
        let msg = RelayMessage {
            id,
            content: message.to_vec(),
            timestamp: 0,
        };
        let mut broadcasts = self.broadcasts.lock().unwrap();
        broadcasts.entry(topic.0).or_default().push(msg);
        id
    }

    /// fetch from broadcast (sync version)
    pub fn broadcast_fetch_sync(&self, topic: &BroadcastTopic) -> Vec<RelayMessage> {
        let broadcasts = self.broadcasts.lock().unwrap();
        broadcasts.get(&topic.0).cloned().unwrap_or_default()
    }
}

#[cfg(all(feature = "net", feature = "std"))]
impl RelayClient for MockRelay {
    async fn post_to_mailbox(
        &self,
        mailbox: &MailboxId,
        message: &[u8],
    ) -> Result<Hash32, RelayError> {
        Ok(self.post_sync(mailbox, message))
    }

    async fn fetch_from_mailbox(
        &self,
        mailbox: &MailboxId,
        options: FetchOptions,
    ) -> Result<Vec<RelayMessage>, RelayError> {
        let mut messages = self.fetch_sync(mailbox);

        // apply after filter
        if let Some(after) = options.after {
            if let Some(pos) = messages.iter().position(|m| m.id == after) {
                messages = messages.split_off(pos + 1);
            }
        }

        // apply limit
        if let Some(limit) = options.limit {
            messages.truncate(limit);
        }

        Ok(messages)
    }

    async fn broadcast(
        &self,
        topic: &BroadcastTopic,
        message: &[u8],
    ) -> Result<Hash32, RelayError> {
        Ok(self.broadcast_sync(topic, message))
    }

    async fn subscribe(
        &self,
        topic: &BroadcastTopic,
    ) -> Result<BroadcastSubscription, RelayError> {
        // create channel for mock subscription
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        // spawn task to poll and send existing messages
        let relay = self.clone();
        let topic = *topic;
        tokio::spawn(async move {
            let messages = relay.broadcast_fetch_sync(&topic);
            for msg in messages {
                if tx.send(msg).await.is_err() {
                    break;
                }
            }
        });

        Ok(BroadcastSubscription::new(rx))
    }

    async fn health_check(&self) -> Result<(), RelayError> {
        Ok(())
    }
}


// ============================================================================
// HTTP RELAY CLIENT (reqwest-based)
// ============================================================================

/// HTTP relay client configuration
#[cfg(feature = "net")]
#[derive(Clone, Debug)]
pub struct HttpRelayConfig {
    /// relay base URL (e.g., "https://relay.narsil.network")
    pub base_url: alloc::string::String,
    /// optional auth token
    pub auth_token: Option<alloc::string::String>,
    /// timeout in milliseconds
    pub timeout_ms: u64,
    /// max message size in bytes
    pub max_message_size: usize,
}

#[cfg(feature = "net")]
impl HttpRelayConfig {
    /// create config with base URL
    pub fn new(base_url: impl Into<alloc::string::String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token: None,
            timeout_ms: 30_000,
            max_message_size: 64 * 1024,
        }
    }

    /// set auth token
    pub fn with_auth(mut self, token: impl Into<alloc::string::String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// set timeout
    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }
}

/// HTTP-based relay client using reqwest
#[cfg(feature = "net")]
pub struct HttpRelayClient {
    config: HttpRelayConfig,
    client: reqwest::Client,
}

#[cfg(feature = "net")]
impl HttpRelayClient {
    /// create new HTTP relay client
    pub fn new(config: HttpRelayConfig) -> Result<Self, RelayError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|_| RelayError::ConnectionFailed(RelayErrorDetail {
                code: 1,
                message: "failed to create HTTP client",
            }))?;

        Ok(Self { config, client })
    }

    /// build URL for mailbox endpoint
    fn mailbox_url(&self, mailbox: &MailboxId) -> alloc::string::String {
        alloc::format!("{}/mailbox/{}", self.config.base_url, hex::encode(mailbox.0))
    }

    /// build URL for broadcast endpoint
    fn broadcast_url(&self, topic: &BroadcastTopic) -> alloc::string::String {
        alloc::format!("{}/broadcast/{}", self.config.base_url, hex::encode(topic.0))
    }
}

/// response from POST /mailbox/:id
#[cfg(feature = "net")]
#[derive(serde::Deserialize)]
struct PostResponse {
    id: alloc::string::String,
}

/// response from GET /mailbox/:id
#[cfg(feature = "net")]
#[derive(serde::Deserialize)]
struct FetchResponse {
    messages: Vec<MessageItem>,
}

#[cfg(feature = "net")]
#[derive(serde::Deserialize)]
struct MessageItem {
    id: alloc::string::String,
    timestamp: u64,
    data: alloc::string::String, // base64
}

#[cfg(feature = "net")]
impl RelayClient for HttpRelayClient {
    async fn post_to_mailbox(
        &self,
        mailbox: &MailboxId,
        message: &[u8],
    ) -> Result<Hash32, RelayError> {
        if message.len() > self.config.max_message_size {
            return Err(RelayError::PostFailed(RelayErrorDetail {
                code: 413,
                message: "message too large",
            }));
        }

        let url = self.mailbox_url(mailbox);
        let mut req = self.client.post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(message.to_vec());

        if let Some(token) = &self.config.auth_token {
            req = req.header("Authorization", alloc::format!("Bearer {}", token));
        }

        let resp = req.send().await.map_err(|_| RelayError::PostFailed(RelayErrorDetail {
            code: 0,
            message: "network error",
        }))?;

        if resp.status() == 429 {
            return Err(RelayError::RateLimited);
        }

        if !resp.status().is_success() {
            return Err(RelayError::PostFailed(RelayErrorDetail {
                code: resp.status().as_u16() as u32,
                message: "relay returned error",
            }));
        }

        let post_resp: PostResponse = resp.json().await.map_err(|_| RelayError::InvalidData)?;

        // parse hex id to Hash32
        let id_bytes = hex::decode(&post_resp.id).map_err(|_| RelayError::InvalidData)?;
        if id_bytes.len() != 32 {
            return Err(RelayError::InvalidData);
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&id_bytes);
        Ok(id)
    }

    async fn fetch_from_mailbox(
        &self,
        mailbox: &MailboxId,
        options: FetchOptions,
    ) -> Result<Vec<RelayMessage>, RelayError> {
        let mut url = self.mailbox_url(mailbox);

        // add query params
        let mut params = Vec::new();
        if let Some(after) = options.after {
            params.push(alloc::format!("after={}", hex::encode(after)));
        }
        if let Some(limit) = options.limit {
            params.push(alloc::format!("limit={}", limit));
        }
        if !params.is_empty() {
            url = alloc::format!("{}?{}", url, params.join("&"));
        }

        let mut req = self.client.get(&url);
        if let Some(token) = &self.config.auth_token {
            req = req.header("Authorization", alloc::format!("Bearer {}", token));
        }

        let resp = req.send().await.map_err(|_| RelayError::FetchFailed(RelayErrorDetail {
            code: 0,
            message: "network error",
        }))?;

        if resp.status() == 404 {
            return Ok(Vec::new()); // empty mailbox
        }

        if resp.status() == 429 {
            return Err(RelayError::RateLimited);
        }

        if !resp.status().is_success() {
            return Err(RelayError::FetchFailed(RelayErrorDetail {
                code: resp.status().as_u16() as u32,
                message: "relay returned error",
            }));
        }

        let fetch_resp: FetchResponse = resp.json().await.map_err(|_| RelayError::InvalidData)?;

        // convert to RelayMessage
        let mut messages = Vec::new();
        for item in fetch_resp.messages {
            let id_bytes = hex::decode(&item.id).map_err(|_| RelayError::InvalidData)?;
            if id_bytes.len() != 32 {
                continue; // skip invalid
            }
            let mut id = [0u8; 32];
            id.copy_from_slice(&id_bytes);

            // decode base64 data
            use base64::Engine;
            let content = base64::engine::general_purpose::STANDARD
                .decode(&item.data)
                .map_err(|_| RelayError::InvalidData)?;

            messages.push(RelayMessage {
                id,
                content,
                timestamp: item.timestamp,
            });
        }

        Ok(messages)
    }

    async fn broadcast(
        &self,
        topic: &BroadcastTopic,
        message: &[u8],
    ) -> Result<Hash32, RelayError> {
        // broadcast uses same POST mechanism as mailbox
        let mailbox = MailboxId(topic.0);
        self.post_to_mailbox(&mailbox, message).await
    }

    async fn subscribe(
        &self,
        _topic: &BroadcastTopic,
    ) -> Result<BroadcastSubscription, RelayError> {
        // HTTP polling-based subscription (websocket upgrade could be added)
        Err(RelayError::ConnectionFailed(RelayErrorDetail {
            code: 501,
            message: "subscriptions require websocket (not implemented)",
        }))
    }

    async fn health_check(&self) -> Result<(), RelayError> {
        let url = alloc::format!("{}/health", self.config.base_url);
        let resp = self.client.get(&url).send().await.map_err(|_| {
            RelayError::ConnectionFailed(RelayErrorDetail {
                code: 0,
                message: "health check failed",
            })
        })?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(RelayError::ConnectionFailed(RelayErrorDetail {
                code: resp.status().as_u16() as u32,
                message: "relay unhealthy",
            }))
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn test_mock_relay_mailbox() {
        let relay = MockRelay::new();
        let syndicate_id = [1u8; 32];
        let viewing_key = [2u8; 32];
        let mailbox = MailboxId::derive(&viewing_key, &syndicate_id);

        // post message
        let id1 = relay.post_sync(&mailbox, b"hello");
        let id2 = relay.post_sync(&mailbox, b"world");
        assert_ne!(id1, id2);

        // fetch messages
        let messages = relay.fetch_sync(&mailbox);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, b"hello");
        assert_eq!(messages[1].content, b"world");
    }

    #[test]
    fn test_mock_relay_broadcast() {
        let relay = MockRelay::new();
        let syndicate_id = [1u8; 32];
        let topic = BroadcastTopic::derive(&syndicate_id);

        // broadcast
        relay.broadcast_sync(&topic, b"proposal");
        relay.broadcast_sync(&topic, b"vote");

        // fetch
        let messages = relay.broadcast_fetch_sync(&topic);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_mailbox_isolation() {
        let relay = MockRelay::new();
        let syndicate_id = [1u8; 32];

        let alice_mailbox = MailboxId::derive(&[1u8; 32], &syndicate_id);
        let bob_mailbox = MailboxId::derive(&[2u8; 32], &syndicate_id);

        // post to alice
        relay.post_sync(&alice_mailbox, b"for alice");

        // bob's mailbox is empty
        assert!(relay.fetch_sync(&bob_mailbox).is_empty());

        // alice has the message
        assert_eq!(relay.fetch_sync(&alice_mailbox).len(), 1);
    }
}
