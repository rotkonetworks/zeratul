//! peer information and identity

use alloc::string::String;

/// peer identifier (iroh endpoint id)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// short display format
    pub fn fmt_short(&self) -> String {
        use alloc::format;
        format!("{}..{}",
            hex::encode(&self.0[..2]),
            hex::encode(&self.0[30..])
        )
    }
}

/// information about a peer (syndicate member)
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// peer's iroh endpoint id
    pub id: PeerId,
    /// member index in syndicate (1-indexed)
    pub member_index: u32,
    /// member's personal public key (for auth)
    pub pubkey: [u8; 32],
    /// relay URL if known
    pub relay_url: Option<String>,
    /// direct addresses if known
    pub addresses: alloc::vec::Vec<String>,
    /// last seen timestamp
    pub last_seen: Option<u64>,
    /// connection status
    pub status: PeerStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PeerStatus {
    #[default]
    Unknown,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

impl PeerInfo {
    pub fn new(id: PeerId, member_index: u32, pubkey: [u8; 32]) -> Self {
        Self {
            id,
            member_index,
            pubkey,
            relay_url: None,
            addresses: alloc::vec::Vec::new(),
            last_seen: None,
            status: PeerStatus::Unknown,
        }
    }

    pub fn with_relay(mut self, relay_url: String) -> Self {
        self.relay_url = Some(relay_url);
        self
    }

    pub fn is_connected(&self) -> bool {
        self.status == PeerStatus::Connected
    }
}
