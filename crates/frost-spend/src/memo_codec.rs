// memo_codec.rs — ZIP-302 structured memo encoding/decoding
//
// canonical implementation of the zafu memo protocol.
// the TypeScript version in packages/wallet is a port of this.
//
// layout (ZIP-302 compliant):
//   byte 0:    0xFF  (arbitrary data per ZIP-302)
//   byte 1:    0x5A  (zafu magic — 'Z')
//   byte 2:    type  (MemoType enum)
//   byte 3:    seq   (0x00=standalone, high nibble=part 1-indexed, low=total)
//   bytes 4-19: messageId  (only if seq != 0x00, i.e. fragmented)
//   rest:      payload
//
// single memo:     4-byte header → 508 bytes payload
// fragmented memo: 20-byte header → 492 bytes per fragment, max 15 parts (~7.4KB)

/// memo field size per ZIP-302
pub const MEMO_SIZE: usize = 512;
/// ZIP-302 arbitrary data tag
const ARBITRARY_DATA: u8 = 0xFF;
/// zafu magic byte (ASCII 'Z')
const ZAFU_MAGIC: u8 = 0x5A;
/// max payload in a single (non-fragmented) memo
pub const PAYLOAD_SINGLE: usize = MEMO_SIZE - 4; // 508
/// max payload per fragment
pub const PAYLOAD_FRAGMENT: usize = MEMO_SIZE - 20; // 492
/// max fragments (4-bit nibble)
pub const MAX_FRAGMENTS: usize = 15;

// ── message types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MemoType {
    /// UTF-8 text message
    Text = 0x01,
    /// unified address (bech32m string or raw bytes)
    Address = 0x02,
    /// payment request
    PaymentRequest = 0x03,
    /// read receipt / ack
    Ack = 0x04,

    // FROST DKG (0x10-0x1f)
    DkgRound1 = 0x10,
    DkgRound2 = 0x11,
    DkgRound3 = 0x12,

    // FROST signing (0x20-0x2f)
    SignRequest = 0x20,
    SignCommitment = 0x21,
    SignShare = 0x22,
    SignResult = 0x23,
}

impl MemoType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Text),
            0x02 => Some(Self::Address),
            0x03 => Some(Self::PaymentRequest),
            0x04 => Some(Self::Ack),
            0x10 => Some(Self::DkgRound1),
            0x11 => Some(Self::DkgRound2),
            0x12 => Some(Self::DkgRound3),
            0x20 => Some(Self::SignRequest),
            0x21 => Some(Self::SignCommitment),
            0x22 => Some(Self::SignShare),
            0x23 => Some(Self::SignResult),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Text => "message",
            Self::Address => "address",
            Self::PaymentRequest => "payment request",
            Self::Ack => "read receipt",
            Self::DkgRound1 => "DKG round 1",
            Self::DkgRound2 => "DKG round 2",
            Self::DkgRound3 => "DKG round 3",
            Self::SignRequest => "sign request",
            Self::SignCommitment => "commitment",
            Self::SignShare => "signature share",
            Self::SignResult => "signature",
        }
    }

    pub fn is_frost(&self) -> bool {
        (*self as u8) >= 0x10
    }
}

// ── parsed memo ──

#[derive(Debug, Clone)]
pub struct ParsedMemo {
    pub memo_type: MemoType,
    /// 16-byte message ID (shared across fragments)
    pub message_id: [u8; 16],
    /// 1-indexed part number (1 for standalone)
    pub part: u8,
    /// total number of parts (1 for standalone)
    pub total: u8,
    /// raw payload bytes
    pub payload: Vec<u8>,
}

// ── encode ──

/// encode a single (non-fragmented) 512-byte memo
pub fn encode_memo(memo_type: MemoType, payload: &[u8]) -> Result<[u8; MEMO_SIZE], String> {
    if payload.len() > PAYLOAD_SINGLE {
        return Err(format!(
            "payload {} exceeds single memo capacity {}",
            payload.len(),
            PAYLOAD_SINGLE
        ));
    }
    let mut memo = [0u8; MEMO_SIZE];
    memo[0] = ARBITRARY_DATA;
    memo[1] = ZAFU_MAGIC;
    memo[2] = memo_type as u8;
    memo[3] = 0x00; // standalone
    memo[4..4 + payload.len()].copy_from_slice(payload);
    Ok(memo)
}

/// encode a text message, automatically fragmenting if needed
pub fn encode_text(text: &str) -> Result<Vec<[u8; MEMO_SIZE]>, String> {
    let payload = text.as_bytes();
    if payload.len() <= PAYLOAD_SINGLE {
        return Ok(vec![encode_memo(MemoType::Text, payload)?]);
    }
    encode_fragmented(MemoType::Text, payload)
}

/// encode arbitrary payload across multiple memos with fragmentation
pub fn encode_fragmented(
    memo_type: MemoType,
    payload: &[u8],
) -> Result<Vec<[u8; MEMO_SIZE]>, String> {
    let total_parts = (payload.len() + PAYLOAD_FRAGMENT - 1) / PAYLOAD_FRAGMENT;
    if total_parts > MAX_FRAGMENTS {
        return Err(format!(
            "message too large: {} fragments (max {})",
            total_parts, MAX_FRAGMENTS
        ));
    }

    let mut message_id = [0u8; 16];
    use rand_core::{OsRng, RngCore};
    OsRng.fill_bytes(&mut message_id);

    let mut memos = Vec::with_capacity(total_parts);

    for i in 0..total_parts {
        let start = i * PAYLOAD_FRAGMENT;
        let end = std::cmp::min(start + PAYLOAD_FRAGMENT, payload.len());
        let chunk = &payload[start..end];

        let mut memo = [0u8; MEMO_SIZE];
        memo[0] = ARBITRARY_DATA;
        memo[1] = ZAFU_MAGIC;
        memo[2] = memo_type as u8;
        memo[3] = (((i + 1) as u8) << 4) | (total_parts as u8);
        memo[4..20].copy_from_slice(&message_id);
        memo[20..20 + chunk.len()].copy_from_slice(chunk);
        memos.push(memo);
    }

    Ok(memos)
}

// ── decode ──

/// check if a 512-byte memo is a zafu structured memo
pub fn is_structured(memo: &[u8; MEMO_SIZE]) -> bool {
    memo[0] == ARBITRARY_DATA && memo[1] == ZAFU_MAGIC
}

/// decode a 512-byte memo. returns None for non-zafu memos.
pub fn decode_memo(memo: &[u8; MEMO_SIZE]) -> Option<ParsedMemo> {
    if memo[0] != ARBITRARY_DATA || memo[1] != ZAFU_MAGIC {
        return None;
    }

    let memo_type = MemoType::from_byte(memo[2])?;
    let seq = memo[3];

    if seq == 0x00 {
        // standalone — deterministic ID from first 16 payload bytes
        let mut message_id = [0u8; 16];
        message_id.copy_from_slice(&memo[4..20]);

        // strip trailing zeros for text
        let end = if memo_type == MemoType::Text {
            let mut e = MEMO_SIZE;
            while e > 4 && memo[e - 1] == 0 {
                e -= 1;
            }
            e
        } else {
            MEMO_SIZE
        };

        return Some(ParsedMemo {
            memo_type,
            message_id,
            part: 1,
            total: 1,
            payload: memo[4..end].to_vec(),
        });
    }

    // fragmented
    let part = (seq >> 4) & 0x0f;
    let total = seq & 0x0f;
    let mut message_id = [0u8; 16];
    message_id.copy_from_slice(&memo[4..20]);

    let end = if memo_type == MemoType::Text && part == total {
        let mut e = MEMO_SIZE;
        while e > 20 && memo[e - 1] == 0 {
            e -= 1;
        }
        e
    } else {
        MEMO_SIZE
    };

    Some(ParsedMemo {
        memo_type,
        message_id,
        part,
        total,
        payload: memo[20..end].to_vec(),
    })
}

// ── reassembly ──

/// reassemble fragmented memos. returns None if incomplete or inconsistent.
pub fn reassemble(fragments: &[ParsedMemo]) -> Option<Vec<u8>> {
    if fragments.is_empty() {
        return None;
    }

    let total = fragments[0].total as usize;
    let expected_type = fragments[0].memo_type;

    if fragments.len() < total {
        return None;
    }

    // all fragments must have the same type
    if !fragments.iter().all(|f| f.memo_type == expected_type) {
        return None;
    }

    // sort by part number
    let mut sorted: Vec<_> = fragments.iter().collect();
    sorted.sort_by_key(|f| f.part);

    // verify completeness (parts 1..=total)
    for (i, f) in sorted.iter().enumerate().take(total) {
        if f.part != (i + 1) as u8 {
            return None;
        }
    }

    // concatenate
    let mut result = Vec::new();
    for f in sorted.iter().take(total) {
        result.extend_from_slice(&f.payload);
    }
    Some(result)
}

// ── convenience ──

/// encode a FROST DKG round message
pub fn encode_dkg_round(round: u8, data: &[u8]) -> Result<Vec<[u8; MEMO_SIZE]>, String> {
    let memo_type = match round {
        1 => MemoType::DkgRound1,
        2 => MemoType::DkgRound2,
        3 => MemoType::DkgRound3,
        _ => return Err(format!("invalid DKG round: {}", round)),
    };
    if data.len() <= PAYLOAD_SINGLE {
        return Ok(vec![encode_memo(memo_type, data)?]);
    }
    encode_fragmented(memo_type, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single_text() {
        let text = "hello from zafu";
        let memos = encode_text(text).unwrap();
        assert_eq!(memos.len(), 1);
        assert!(is_structured(&memos[0]));

        let parsed = decode_memo(&memos[0]).unwrap();
        assert_eq!(parsed.memo_type, MemoType::Text);
        assert_eq!(parsed.part, 1);
        assert_eq!(parsed.total, 1);
        assert_eq!(std::str::from_utf8(&parsed.payload).unwrap(), text);
    }

    #[test]
    fn roundtrip_fragmented_text() {
        let text = "a".repeat(1000); // > 508 bytes
        let memos = encode_text(&text).unwrap();
        assert!(memos.len() > 1);

        let fragments: Vec<ParsedMemo> = memos.iter().map(|m| decode_memo(m).unwrap()).collect();

        // all fragments share the same message_id
        let id = fragments[0].message_id;
        assert!(fragments.iter().all(|f| f.message_id == id));

        let payload = reassemble(&fragments).unwrap();
        assert_eq!(std::str::from_utf8(&payload).unwrap(), text);
    }

    #[test]
    fn rejects_non_zafu_memo() {
        let mut memo = [0u8; MEMO_SIZE];
        memo[0] = 0xFF;
        memo[1] = 0x00; // not zafu magic
        assert!(!is_structured(&memo));
        assert!(decode_memo(&memo).is_none());
    }

    #[test]
    fn mixed_types_rejected() {
        let a = ParsedMemo {
            memo_type: MemoType::Text,
            message_id: [0; 16],
            part: 1,
            total: 2,
            payload: vec![0x41],
        };
        let b = ParsedMemo {
            memo_type: MemoType::DkgRound1, // wrong type
            message_id: [0; 16],
            part: 2,
            total: 2,
            payload: vec![0x42],
        };
        assert!(reassemble(&[a, b]).is_none());
    }

    #[test]
    fn deterministic_standalone_id() {
        let memo = encode_memo(MemoType::Address, b"u1test").unwrap();
        let p1 = decode_memo(&memo).unwrap();
        let p2 = decode_memo(&memo).unwrap();
        assert_eq!(p1.message_id, p2.message_id);
    }
}
