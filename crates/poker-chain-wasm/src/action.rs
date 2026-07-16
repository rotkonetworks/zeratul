//! canonical poker action codec
//!
//! single wire format shared by the ui submit path, the optimistic
//! predictor, and the chain tx. layout:
//!
//! [type:1][channel_id:32][nonce:8][seat:1][payload]
//!
//! type codes:
//! - 0x01 bet    payload [amount:8 le]
//! - 0x02 fold   payload []
//! - 0x03 call   payload [amount:8 le]
//! - 0x04 raise  payload [amount:8 le][new_bet:8 le]
//! - 0x05 check  payload []

/// header size: type + channel_id + nonce + seat
pub const ACTION_HEADER_LEN: usize = 1 + 32 + 8 + 1;

/// poker action kind
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Bet { amount: u64 },
    Fold,
    Call { amount: u64 },
    Raise { amount: u64, new_bet: u64 },
    Check,
}

/// decoded poker action
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub channel_id: [u8; 32],
    pub nonce: u64,
    pub seat: u8,
    pub kind: ActionKind,
}

impl Action {
    /// wire type code
    pub fn type_code(&self) -> u8 {
        match self.kind {
            ActionKind::Bet { .. } => 0x01,
            ActionKind::Fold => 0x02,
            ActionKind::Call { .. } => 0x03,
            ActionKind::Raise { .. } => 0x04,
            ActionKind::Check => 0x05,
        }
    }

    /// encode to wire bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ACTION_HEADER_LEN + 16);
        out.push(self.type_code());
        out.extend_from_slice(&self.channel_id);
        out.extend_from_slice(&self.nonce.to_le_bytes());
        out.push(self.seat);
        match self.kind {
            ActionKind::Bet { amount } | ActionKind::Call { amount } => {
                out.extend_from_slice(&amount.to_le_bytes());
            }
            ActionKind::Raise { amount, new_bet } => {
                out.extend_from_slice(&amount.to_le_bytes());
                out.extend_from_slice(&new_bet.to_le_bytes());
            }
            ActionKind::Fold | ActionKind::Check => {}
        }
        out
    }

    /// decode from wire bytes, none if malformed or unknown type
    pub fn decode(bytes: &[u8]) -> Option<Action> {
        if bytes.len() < ACTION_HEADER_LEN {
            return None;
        }
        let mut channel_id = [0u8; 32];
        channel_id.copy_from_slice(&bytes[1..33]);
        let nonce = u64::from_le_bytes(bytes[33..41].try_into().ok()?);
        let seat = bytes[41];
        let payload = &bytes[ACTION_HEADER_LEN..];

        let read_u64 = |off: usize| -> Option<u64> {
            payload
                .get(off..off + 8)?
                .try_into()
                .ok()
                .map(u64::from_le_bytes)
        };

        let kind = match bytes[0] {
            0x01 => ActionKind::Bet { amount: read_u64(0)? },
            0x02 => ActionKind::Fold,
            0x03 => ActionKind::Call { amount: read_u64(0)? },
            0x04 => ActionKind::Raise {
                amount: read_u64(0)?,
                new_bet: read_u64(8)?,
            },
            0x05 => ActionKind::Check,
            _ => return None,
        };

        Some(Action {
            channel_id,
            nonce,
            seat,
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(kind: ActionKind) {
        let action = Action {
            channel_id: [7u8; 32],
            nonce: 42,
            seat: 3,
            kind,
        };
        let bytes = action.encode();
        assert_eq!(Action::decode(&bytes), Some(action));
    }

    #[test]
    fn test_roundtrip_all_kinds() {
        roundtrip(ActionKind::Bet { amount: 100 });
        roundtrip(ActionKind::Fold);
        roundtrip(ActionKind::Call { amount: 55 });
        roundtrip(ActionKind::Raise {
            amount: 200,
            new_bet: 250,
        });
        roundtrip(ActionKind::Check);
    }

    #[test]
    fn test_decode_rejects_malformed() {
        assert!(Action::decode(&[]).is_none());
        assert!(Action::decode(&[0x02; 41]).is_none()); // short header
        // bet without amount payload
        let mut bytes = Action {
            channel_id: [0u8; 32],
            nonce: 1,
            seat: 0,
            kind: ActionKind::Bet { amount: 1 },
        }
        .encode();
        bytes.truncate(ACTION_HEADER_LEN);
        assert!(Action::decode(&bytes).is_none());
        // unknown type
        let mut fold = Action {
            channel_id: [0u8; 32],
            nonce: 1,
            seat: 0,
            kind: ActionKind::Fold,
        }
        .encode();
        fold[0] = 0x99;
        assert!(Action::decode(&fold).is_none());
    }
}
