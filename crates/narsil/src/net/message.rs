//! syndicate P2P messages

use alloc::vec::Vec;
use alloc::string::String;

/// message types for syndicate coordination
#[derive(Clone, Debug)]
pub enum SyndicateMessage {
    /// propose a new action
    Propose(ProposeMessage),
    /// vote on a proposal
    Vote(VoteMessage),
    /// OSST contribution for approved action
    Contribute(ContributeMessage),
    /// request state sync
    SyncRequest(SyncRequestMessage),
    /// state sync response
    SyncResponse(SyncResponseMessage),
    /// heartbeat / keepalive
    Ping(u64),
    /// heartbeat response
    Pong(u64),
}

impl SyndicateMessage {
    pub fn message_type(&self) -> MessageType {
        match self {
            Self::Propose(_) => MessageType::Propose,
            Self::Vote(_) => MessageType::Vote,
            Self::Contribute(_) => MessageType::Contribute,
            Self::SyncRequest(_) => MessageType::SyncRequest,
            Self::SyncResponse(_) => MessageType::SyncResponse,
            Self::Ping(_) => MessageType::Ping,
            Self::Pong(_) => MessageType::Pong,
        }
    }

    /// encode to bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.message_type() as u8);
        match self {
            Self::Propose(m) => buf.extend_from_slice(&m.encode()),
            Self::Vote(m) => buf.extend_from_slice(&m.encode()),
            Self::Contribute(m) => buf.extend_from_slice(&m.encode()),
            Self::SyncRequest(m) => buf.extend_from_slice(&m.encode()),
            Self::SyncResponse(m) => buf.extend_from_slice(&m.encode()),
            Self::Ping(n) => buf.extend_from_slice(&n.to_le_bytes()),
            Self::Pong(n) => buf.extend_from_slice(&n.to_le_bytes()),
        }
        buf
    }

    /// decode from bytes
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let msg_type = MessageType::try_from(bytes[0]).ok()?;
        let payload = &bytes[1..];
        Some(match msg_type {
            MessageType::Propose => Self::Propose(ProposeMessage::decode(payload)?),
            MessageType::Vote => Self::Vote(VoteMessage::decode(payload)?),
            MessageType::Contribute => Self::Contribute(ContributeMessage::decode(payload)?),
            MessageType::SyncRequest => Self::SyncRequest(SyncRequestMessage::decode(payload)?),
            MessageType::SyncResponse => Self::SyncResponse(SyncResponseMessage::decode(payload)?),
            MessageType::Ping => {
                let n = u64::from_le_bytes(payload.get(..8)?.try_into().ok()?);
                Self::Ping(n)
            }
            MessageType::Pong => {
                let n = u64::from_le_bytes(payload.get(..8)?.try_into().ok()?);
                Self::Pong(n)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Propose = 0,
    Vote = 1,
    Contribute = 2,
    SyncRequest = 3,
    SyncResponse = 4,
    Ping = 5,
    Pong = 6,
}

impl TryFrom<u8> for MessageType {
    type Error = ();
    fn try_from(v: u8) -> Result<Self, ()> {
        match v {
            0 => Ok(Self::Propose),
            1 => Ok(Self::Vote),
            2 => Ok(Self::Contribute),
            3 => Ok(Self::SyncRequest),
            4 => Ok(Self::SyncResponse),
            5 => Ok(Self::Ping),
            6 => Ok(Self::Pong),
            _ => Err(()),
        }
    }
}

/// proposal message
#[derive(Clone, Debug)]
pub struct ProposeMessage {
    /// proposal id
    pub proposal_id: u64,
    /// proposer's member index
    pub proposer: u32,
    /// action type (from governance)
    pub action_type: u8,
    /// action description
    pub description: String,
    /// serialized action plan
    pub action_data: Vec<u8>,
    /// proposer's signature
    pub signature: [u8; 64],
}

impl ProposeMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.proposal_id.to_le_bytes());
        buf.extend_from_slice(&self.proposer.to_le_bytes());
        buf.push(self.action_type);
        buf.extend_from_slice(&(self.description.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.description.as_bytes());
        buf.extend_from_slice(&(self.action_data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.action_data);
        buf.extend_from_slice(&self.signature);
        buf
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 + 4 + 1 + 4 {
            return None;
        }
        let proposal_id = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let proposer = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let action_type = bytes[12];
        let desc_len = u32::from_le_bytes(bytes[13..17].try_into().ok()?) as usize;
        if bytes.len() < 17 + desc_len + 4 {
            return None;
        }
        let description = String::from_utf8(bytes[17..17 + desc_len].to_vec()).ok()?;
        let data_len_offset = 17 + desc_len;
        let data_len = u32::from_le_bytes(bytes[data_len_offset..data_len_offset + 4].try_into().ok()?) as usize;
        let data_offset = data_len_offset + 4;
        if bytes.len() < data_offset + data_len + 64 {
            return None;
        }
        let action_data = bytes[data_offset..data_offset + data_len].to_vec();
        let sig_offset = data_offset + data_len;
        let signature: [u8; 64] = bytes[sig_offset..sig_offset + 64].try_into().ok()?;
        Some(Self {
            proposal_id,
            proposer,
            action_type,
            description,
            action_data,
            signature,
        })
    }
}

/// vote message
#[derive(Clone, Debug)]
pub struct VoteMessage {
    /// proposal being voted on
    pub proposal_id: u64,
    /// voter's member index
    pub voter: u32,
    /// approve or reject
    pub approve: bool,
    /// voter's signature
    pub signature: [u8; 64],
}

impl VoteMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.proposal_id.to_le_bytes());
        buf.extend_from_slice(&self.voter.to_le_bytes());
        buf.push(if self.approve { 1 } else { 0 });
        buf.extend_from_slice(&self.signature);
        buf
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 + 4 + 1 + 64 {
            return None;
        }
        Some(Self {
            proposal_id: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            voter: u32::from_le_bytes(bytes[8..12].try_into().ok()?),
            approve: bytes[12] == 1,
            signature: bytes[13..77].try_into().ok()?,
        })
    }
}

/// OSST contribution message
#[derive(Clone, Debug)]
pub struct ContributeMessage {
    /// proposal this contribution is for
    pub proposal_id: u64,
    /// contributor's member index
    pub contributor: u32,
    /// serialized OSST contribution
    pub contribution: Vec<u8>,
}

impl ContributeMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.proposal_id.to_le_bytes());
        buf.extend_from_slice(&self.contributor.to_le_bytes());
        buf.extend_from_slice(&(self.contribution.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.contribution);
        buf
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 + 4 + 4 {
            return None;
        }
        let proposal_id = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let contributor = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let len = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
        if bytes.len() < 16 + len {
            return None;
        }
        let contribution = bytes[16..16 + len].to_vec();
        Some(Self {
            proposal_id,
            contributor,
            contribution,
        })
    }
}

/// state sync request
#[derive(Clone, Debug)]
pub struct SyncRequestMessage {
    /// requester's current height
    pub current_height: u64,
    /// requester's state root
    pub state_root: [u8; 32],
}

impl SyncRequestMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.current_height.to_le_bytes());
        buf.extend_from_slice(&self.state_root);
        buf
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 + 32 {
            return None;
        }
        Some(Self {
            current_height: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            state_root: bytes[8..40].try_into().ok()?,
        })
    }
}

/// state sync response
#[derive(Clone, Debug)]
pub struct SyncResponseMessage {
    /// responder's current height
    pub height: u64,
    /// responder's state root
    pub state_root: [u8; 32],
    /// blocks to sync (if requester is behind)
    pub blocks: Vec<Vec<u8>>,
}

impl SyncResponseMessage {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.state_root);
        buf.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for block in &self.blocks {
            buf.extend_from_slice(&(block.len() as u32).to_le_bytes());
            buf.extend_from_slice(block);
        }
        buf
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 + 32 + 4 {
            return None;
        }
        let height = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let state_root: [u8; 32] = bytes[8..40].try_into().ok()?;
        let num_blocks = u32::from_le_bytes(bytes[40..44].try_into().ok()?) as usize;

        let mut blocks = Vec::with_capacity(num_blocks);
        let mut offset = 44;
        for _ in 0..num_blocks {
            if offset + 4 > bytes.len() {
                return None;
            }
            let len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().ok()?) as usize;
            offset += 4;
            if offset + len > bytes.len() {
                return None;
            }
            blocks.push(bytes[offset..offset + len].to_vec());
            offset += len;
        }

        Some(Self {
            height,
            state_root,
            blocks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vote_roundtrip() {
        let msg = VoteMessage {
            proposal_id: 42,
            voter: 3,
            approve: true,
            signature: [0xab; 64],
        };
        let encoded = msg.encode();
        let decoded = VoteMessage::decode(&encoded).unwrap();
        assert_eq!(msg.proposal_id, decoded.proposal_id);
        assert_eq!(msg.voter, decoded.voter);
        assert_eq!(msg.approve, decoded.approve);
        assert_eq!(msg.signature, decoded.signature);
    }

    #[test]
    fn test_syndicate_message_roundtrip() {
        let msg = SyndicateMessage::Vote(VoteMessage {
            proposal_id: 123,
            voter: 1,
            approve: false,
            signature: [0x11; 64],
        });
        let encoded = msg.encode();
        let decoded = SyndicateMessage::decode(&encoded).unwrap();

        match decoded {
            SyndicateMessage::Vote(v) => {
                assert_eq!(v.proposal_id, 123);
                assert!(!v.approve);
            }
            _ => panic!("wrong message type"),
        }
    }

    #[test]
    fn test_ping_pong() {
        let ping = SyndicateMessage::Ping(12345);
        let encoded = ping.encode();
        let decoded = SyndicateMessage::decode(&encoded).unwrap();
        match decoded {
            SyndicateMessage::Ping(n) => assert_eq!(n, 12345),
            _ => panic!("wrong type"),
        }
    }
}
