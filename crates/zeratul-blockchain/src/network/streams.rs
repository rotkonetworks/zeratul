//! JAMNP-S Stream Protocols
//!
//! Implements JAM stream kinds:
//! - UP 0-127: Unique Persistent streams
//! - CE 128+: Common Ephemeral streams

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Stream kind identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamKind {
    // UP (Unique Persistent) - 0-127
    BlockAnnouncement,  // UP 0

    // CE (Common Ephemeral) - 128+
    BlockRequest,           // CE 128
    StateRequest,           // CE 129
    SafroleTicketGen,       // CE 131
    SafroleTicketProxy,     // CE 132
    WorkPackageSubmit,      // CE 133
    WorkPackageShare,       // CE 134
    WorkReportDistribute,   // CE 135
    WorkReportRequest,      // CE 136
    ShardDistribute,        // CE 137
    AuditShardRequest,      // CE 138
    SegmentShardRequest1,   // CE 139
    SegmentShardRequest2,   // CE 140
    AssuranceDistribute,    // CE 141
    PreimageAnnounce,       // CE 142
    PreimageRequest,        // CE 143
    AuditAnnounce,          // CE 144
    JudgmentPublish,        // CE 145
    WorkPackageBundleSubmit, // CE 146
    BundleRequest,          // CE 147
    SegmentRequest,         // CE 148

    // Custom (Zeratul-specific) - 200+
    DKGBroadcast,           // CE 200
    DKGRequest,             // CE 201
    DKGComplete,            // CE 202
}

impl StreamKind {
    /// Convert to wire format (single byte)
    pub fn to_byte(self) -> u8 {
        match self {
            Self::BlockAnnouncement => 0,

            Self::BlockRequest => 128,
            Self::StateRequest => 129,
            Self::SafroleTicketGen => 131,
            Self::SafroleTicketProxy => 132,
            Self::WorkPackageSubmit => 133,
            Self::WorkPackageShare => 134,
            Self::WorkReportDistribute => 135,
            Self::WorkReportRequest => 136,
            Self::ShardDistribute => 137,
            Self::AuditShardRequest => 138,
            Self::SegmentShardRequest1 => 139,
            Self::SegmentShardRequest2 => 140,
            Self::AssuranceDistribute => 141,
            Self::PreimageAnnounce => 142,
            Self::PreimageRequest => 143,
            Self::AuditAnnounce => 144,
            Self::JudgmentPublish => 145,
            Self::WorkPackageBundleSubmit => 146,
            Self::BundleRequest => 147,
            Self::SegmentRequest => 148,

            Self::DKGBroadcast => 200,
            Self::DKGRequest => 201,
            Self::DKGComplete => 202,
        }
    }

    /// Convert from wire format
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::BlockAnnouncement),

            128 => Some(Self::BlockRequest),
            129 => Some(Self::StateRequest),
            131 => Some(Self::SafroleTicketGen),
            132 => Some(Self::SafroleTicketProxy),
            133 => Some(Self::WorkPackageSubmit),
            134 => Some(Self::WorkPackageShare),
            135 => Some(Self::WorkReportDistribute),
            136 => Some(Self::WorkReportRequest),
            137 => Some(Self::ShardDistribute),
            138 => Some(Self::AuditShardRequest),
            139 => Some(Self::SegmentShardRequest1),
            140 => Some(Self::SegmentShardRequest2),
            141 => Some(Self::AssuranceDistribute),
            142 => Some(Self::PreimageAnnounce),
            143 => Some(Self::PreimageRequest),
            144 => Some(Self::AuditAnnounce),
            145 => Some(Self::JudgmentPublish),
            146 => Some(Self::WorkPackageBundleSubmit),
            147 => Some(Self::BundleRequest),
            148 => Some(Self::SegmentRequest),

            200 => Some(Self::DKGBroadcast),
            201 => Some(Self::DKGRequest),
            202 => Some(Self::DKGComplete),

            _ => None,
        }
    }

    /// Is this a UP (Unique Persistent) stream?
    pub fn is_up(&self) -> bool {
        self.to_byte() < 128
    }

    /// Is this a CE (Common Ephemeral) stream?
    pub fn is_ce(&self) -> bool {
        self.to_byte() >= 128
    }
}

/// Stream handler trait
pub trait StreamHandler: Send + Sync {
    /// Handle incoming stream
    fn handle_stream(&self, kind: StreamKind, data: Vec<u8>) -> Result<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_kind_conversion() {
        let kind = StreamKind::BlockAnnouncement;
        assert_eq!(kind.to_byte(), 0);
        assert!(kind.is_up());
        assert!(!kind.is_ce());

        let kind = StreamKind::DKGBroadcast;
        assert_eq!(kind.to_byte(), 200);
        assert!(!kind.is_up());
        assert!(kind.is_ce());
    }

    #[test]
    fn test_roundtrip() {
        for kind in [
            StreamKind::BlockAnnouncement,
            StreamKind::BlockRequest,
            StreamKind::DKGBroadcast,
        ] {
            let byte = kind.to_byte();
            let recovered = StreamKind::from_byte(byte).unwrap();
            assert_eq!(kind, recovered);
        }
    }
}
