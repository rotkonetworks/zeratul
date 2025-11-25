//! encrypted chat over zcash shielded memos
//! messages sent as dust transactions (0.0001 ZEC) with memo field

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// minimum dust amount for message transactions (10000 zatoshis = 0.0001 ZEC)
pub const MESSAGE_DUST_ZATOSHIS: u64 = 10_000;

/// message status
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MessageStatus {
    Pending,
    Sent,
    Confirmed,
    Failed,
}

/// a chat message (memo content)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub contact_id: String,
    pub content: String,
    pub timestamp: u64,
    pub outgoing: bool,
    pub status: MessageStatus,
    pub tx_id: Option<String>,
    pub block_height: Option<u32>,
}

impl ChatMessage {
    /// create new outgoing message
    pub fn outgoing(contact_id: &str, content: &str) -> Self {
        use sha2::{Sha256, Digest};
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut hasher = Sha256::new();
        hasher.update(contact_id.as_bytes());
        hasher.update(content.as_bytes());
        hasher.update(timestamp.to_le_bytes());
        let id = hex::encode(&hasher.finalize()[..12]);

        Self {
            id,
            contact_id: contact_id.to_string(),
            content: content.to_string(),
            timestamp,
            outgoing: true,
            status: MessageStatus::Pending,
            tx_id: None,
            block_height: None,
        }
    }

    /// create incoming message from received memo
    pub fn incoming(contact_id: &str, content: &str, tx_id: &str, block_height: u32) -> Self {
        use sha2::{Sha256, Digest};
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut hasher = Sha256::new();
        hasher.update(tx_id.as_bytes());
        let id = hex::encode(&hasher.finalize()[..12]);

        Self {
            id,
            contact_id: contact_id.to_string(),
            content: content.to_string(),
            timestamp,
            outgoing: false,
            status: MessageStatus::Confirmed,
            tx_id: Some(tx_id.to_string()),
            block_height: Some(block_height),
        }
    }

    /// format timestamp for display
    pub fn format_time(&self) -> String {
        use std::time::{Duration, UNIX_EPOCH};
        let datetime = UNIX_EPOCH + Duration::from_secs(self.timestamp);
        // simple time formatting
        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let diff = now.saturating_sub(self.timestamp);
        if diff < 60 {
            "just now".to_string()
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else if diff < 86400 {
            format!("{}h ago", diff / 3600)
        } else {
            format!("{}d ago", diff / 86400)
        }
    }
}

/// chat history for a single contact
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ChatHistory {
    messages: Vec<ChatMessage>,
}

impl ChatHistory {
    pub fn new() -> Self {
        Self { messages: Vec::new() }
    }

    /// add a message
    pub fn add(&mut self, message: ChatMessage) {
        self.messages.push(message);
        self.messages.sort_by_key(|m| m.timestamp);
    }

    /// get all messages
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// get last message
    pub fn last(&self) -> Option<&ChatMessage> {
        self.messages.last()
    }

    /// update message status by id
    pub fn update_status(&mut self, msg_id: &str, status: MessageStatus, tx_id: Option<String>) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == msg_id) {
            msg.status = status;
            if let Some(txid) = tx_id {
                msg.tx_id = Some(txid);
            }
        }
    }

    /// check if message exists by tx_id
    pub fn has_tx(&self, tx_id: &str) -> bool {
        self.messages.iter().any(|m| m.tx_id.as_deref() == Some(tx_id))
    }

    /// count messages
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// all chats storage
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ChatStorage {
    chats: HashMap<String, ChatHistory>,
}

impl ChatStorage {
    pub fn new() -> Self {
        Self { chats: HashMap::new() }
    }

    /// get or create chat history for contact
    pub fn get_or_create(&mut self, contact_id: &str) -> &mut ChatHistory {
        self.chats.entry(contact_id.to_string()).or_insert_with(ChatHistory::new)
    }

    /// get chat history
    pub fn get(&self, contact_id: &str) -> Option<&ChatHistory> {
        self.chats.get(contact_id)
    }

    /// get mutable chat history
    pub fn get_mut(&mut self, contact_id: &str) -> Option<&mut ChatHistory> {
        self.chats.get_mut(contact_id)
    }

    /// add message to contact's chat
    pub fn add_message(&mut self, contact_id: &str, message: ChatMessage) {
        self.get_or_create(contact_id).add(message);
    }

    /// get last message time for contact
    pub fn last_message_time(&self, contact_id: &str) -> Option<u64> {
        self.chats.get(contact_id)
            .and_then(|h| h.last())
            .map(|m| m.timestamp)
    }

    /// get preview text for contact
    pub fn preview(&self, contact_id: &str) -> Option<String> {
        self.chats.get(contact_id)
            .and_then(|h| h.last())
            .map(|m| {
                let prefix = if m.outgoing { "you: " } else { "" };
                let content = if m.content.len() > 30 {
                    format!("{}...", &m.content[..30])
                } else {
                    m.content.clone()
                };
                format!("{}{}", prefix, content)
            })
    }
}

/// encode message for memo field
/// format: [MSG:v1]<content>
pub fn encode_memo(content: &str) -> Vec<u8> {
    let header = "[MSG:v1]";
    let mut memo = header.as_bytes().to_vec();
    memo.extend_from_slice(content.as_bytes());
    // pad to 512 bytes (zcash memo size)
    memo.resize(512, 0);
    memo
}

/// decode message from memo field
pub fn decode_memo(memo: &[u8]) -> Option<String> {
    let header = b"[MSG:v1]";
    if memo.len() < header.len() || &memo[..header.len()] != header {
        return None;
    }
    // find null terminator or end
    let content_start = header.len();
    let content_end = memo[content_start..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| content_start + p)
        .unwrap_or(memo.len());

    String::from_utf8(memo[content_start..content_end].to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memo_encoding() {
        let content = "hello, this is a test message!";
        let encoded = encode_memo(content);
        assert_eq!(encoded.len(), 512);

        let decoded = decode_memo(&encoded);
        assert_eq!(decoded, Some(content.to_string()));
    }

    #[test]
    fn test_chat_history() {
        let mut history = ChatHistory::new();
        let msg = ChatMessage::outgoing("contact1", "hello!");
        history.add(msg);
        assert_eq!(history.len(), 1);
        assert_eq!(history.last().unwrap().content, "hello!");
    }
}
