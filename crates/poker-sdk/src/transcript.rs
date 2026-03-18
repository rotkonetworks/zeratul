//! deterministic action transcript for poker games
//!
//! both players independently build the same transcript from signed actions.
//! the transcript is a hash chain — each action's hash depends on all previous
//! actions. this makes the transcript non-repudiable and tamper-evident.
//!
//! on dispute, the jury (or WIM prover) replays the transcript to determine
//! the correct payout.
//!
//! # hash chain
//!
//! ```text
//! action_hash[n] = H("poker/action/v1" || seat || action || amount || seq || action_hash[n-1])
//! ```
//!
//! # settlement
//!
//! ```text
//! settlement_hash = H("poker/settle/v1" || final_action_hash || a_payout || b_payout)
//! ```
//!
//! both players compute the same settlement_hash. the frostito escrow signs it.

use sha2::{Sha256, Digest};

/// a signed action in the transcript (non-repudiable)
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SignedAction {
    /// player seat (0 = A, 1 = B)
    pub seat: u8,
    /// action type
    pub action: Action,
    /// bet/raise amount (0 for check/fold/call)
    pub amount: u64,
    /// sequence number (1-indexed, monotonic)
    pub seq: u64,
    /// penumbra block height at time of action (trustless clock)
    pub block_height: u64,
    /// penumbra block hash (32 bytes, proves the block existed)
    pub block_hash: [u8; 32],
    /// chained hash: H(seat || action || amount || seq || block_height || block_hash || prev_hash)
    pub hash: [u8; 32],
    /// player's signature over hash (proves they authorized this action)
    /// in production: ed25519 or RedPallas signature
    pub sig: [u8; 64],
}

/// timeout configuration for a game
#[derive(Debug, Clone, Copy)]
pub struct TimeoutConfig {
    /// blocks per turn before timeout (e.g. 6 blocks = ~30s on penumbra)
    pub turn_timeout_blocks: u64,
    /// blocks for the entire hand before force-settlement
    pub hand_timeout_blocks: u64,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            turn_timeout_blocks: 6,   // ~30s at 5s/block
            hand_timeout_blocks: 120, // ~10min
        }
    }
}

/// timeout check result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutResult {
    /// action is within time limits
    Ok,
    /// player exceeded turn timeout — they forfeit
    TurnTimeout { seat: u8, blocks_late: u64 },
    /// hand exceeded total timeout — force settle current stacks
    HandTimeout,
}

/// poker actions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Action {
    Fold,
    Check,
    Call,
    Bet,
    Raise,
    AllIn,
}

impl Action {
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Fold => "fold",
            Action::Check => "check",
            Action::Call => "call",
            Action::Bet => "bet",
            Action::Raise => "raise",
            Action::AllIn => "allin",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "fold" => Some(Action::Fold),
            "check" => Some(Action::Check),
            "call" => Some(Action::Call),
            "bet" => Some(Action::Bet),
            "raise" => Some(Action::Raise),
            "allin" | "all-in" | "all_in" => Some(Action::AllIn),
            _ => None,
        }
    }
}

/// deterministic action transcript with block-based timing
///
/// both players maintain an identical copy. actions are chained via
/// hash — the hash of action N depends on all previous actions AND
/// the penumbra block hash at the time of the action.
///
/// timing: each action references a block height. the jury (or WIM)
/// can verify that actions were submitted within timeout limits by
/// checking block height differences.
pub struct Transcript {
    actions: Vec<SignedAction>,
    sequence: u64,
    prev_hash: [u8; 32],
    /// block height of the first action (hand start)
    start_block: Option<u64>,
    /// timeout configuration
    timeout: TimeoutConfig,
}

impl Transcript {
    pub fn new() -> Self {
        Self::with_timeout(TimeoutConfig::default())
    }

    pub fn with_timeout(timeout: TimeoutConfig) -> Self {
        Self {
            actions: Vec::new(),
            sequence: 0,
            prev_hash: [0u8; 32],
            start_block: None,
            timeout,
        }
    }

    /// record an action anchored to a penumbra block.
    /// returns the hash that the player should sign.
    pub fn record(
        &mut self,
        seat: u8,
        action: Action,
        amount: u64,
        block_height: u64,
        block_hash: [u8; 32],
    ) -> [u8; 32] {
        self.sequence += 1;
        if self.start_block.is_none() {
            self.start_block = Some(block_height);
        }

        let hash = compute_action_hash(
            seat, action, amount, self.sequence,
            block_height, &block_hash, &self.prev_hash,
        );

        self.prev_hash = hash;
        hash
    }

    /// add a signed action to the transcript (after chain + timing verification)
    pub fn add_signed(&mut self, action: SignedAction) {
        if self.start_block.is_none() {
            self.start_block = Some(action.block_height);
        }
        self.prev_hash = action.hash;
        self.sequence = action.seq;
        self.actions.push(action);
    }

    /// verify that a signed action chains correctly and is within time limits
    pub fn verify_action(&self, action: &SignedAction) -> (bool, TimeoutResult) {
        // check sequence
        let expected_seq = self.sequence + 1;
        if action.seq != expected_seq {
            return (false, TimeoutResult::Ok);
        }

        // check hash chain
        let expected_hash = compute_action_hash(
            action.seat, action.action, action.amount, action.seq,
            action.block_height, &action.block_hash, &self.prev_hash,
        );
        if action.hash != expected_hash {
            return (false, TimeoutResult::Ok);
        }

        // check block height is monotonic
        if let Some(last) = self.actions.last() {
            if action.block_height < last.block_height {
                return (false, TimeoutResult::Ok);
            }

            // check turn timeout
            let blocks_elapsed = action.block_height - last.block_height;
            if blocks_elapsed > self.timeout.turn_timeout_blocks {
                return (true, TimeoutResult::TurnTimeout {
                    seat: action.seat,
                    blocks_late: blocks_elapsed - self.timeout.turn_timeout_blocks,
                });
            }
        }

        // check hand timeout
        if let Some(start) = self.start_block {
            let total_blocks = action.block_height - start;
            if total_blocks > self.timeout.hand_timeout_blocks {
                return (true, TimeoutResult::HandTimeout);
            }
        }

        (true, TimeoutResult::Ok)
    }

    /// check if a player has timed out given the current block height
    pub fn check_timeout(&self, current_block: u64) -> TimeoutResult {
        if let Some(last) = self.actions.last() {
            let blocks_since = current_block.saturating_sub(last.block_height);
            if blocks_since > self.timeout.turn_timeout_blocks {
                // the OTHER player (not the one who last acted) timed out
                let timed_out_seat = 1 - last.seat;
                return TimeoutResult::TurnTimeout {
                    seat: timed_out_seat,
                    blocks_late: blocks_since - self.timeout.turn_timeout_blocks,
                };
            }
        }
        if let Some(start) = self.start_block {
            if current_block - start > self.timeout.hand_timeout_blocks {
                return TimeoutResult::HandTimeout;
            }
        }
        TimeoutResult::Ok
    }

    /// compute deterministic settlement hash
    pub fn settlement_hash(&self, a_payout: u64, b_payout: u64) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"poker/settle/v1");
        hasher.update(&self.prev_hash);
        hasher.update(a_payout.to_le_bytes());
        hasher.update(b_payout.to_le_bytes());
        hasher.finalize().into()
    }

    /// get the current chain head hash
    pub fn head_hash(&self) -> [u8; 32] {
        self.prev_hash
    }

    /// get all signed actions
    pub fn actions(&self) -> &[SignedAction] {
        &self.actions
    }

    /// current sequence number
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// timeout config
    pub fn timeout(&self) -> &TimeoutConfig {
        &self.timeout
    }
}

/// compute the chained action hash (used by both record and verify)
fn compute_action_hash(
    seat: u8,
    action: Action,
    amount: u64,
    seq: u64,
    block_height: u64,
    block_hash: &[u8; 32],
    prev_hash: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"poker/action/v2");
    hasher.update([seat]);
    hasher.update(action.as_str().as_bytes());
    hasher.update(amount.to_le_bytes());
    hasher.update(seq.to_le_bytes());
    hasher.update(block_height.to_le_bytes());
    hasher.update(block_hash);
    hasher.update(prev_hash);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK_HASH: [u8; 32] = [0xAB; 32];

    #[test]
    fn test_transcript_deterministic() {
        let mut t1 = Transcript::new();
        let mut t2 = Transcript::new();

        // both players record the same actions with the same block
        let h1a = t1.record(0, Action::Check, 0, 100, BLOCK_HASH);
        let h2a = t2.record(0, Action::Check, 0, 100, BLOCK_HASH);
        assert_eq!(h1a, h2a, "same action + block should produce same hash");

        let h1b = t1.record(1, Action::Bet, 100, 101, BLOCK_HASH);
        let h2b = t2.record(1, Action::Bet, 100, 101, BLOCK_HASH);
        assert_eq!(h1b, h2b, "chained hashes should match");

        let s1 = t1.settlement_hash(1100, 900);
        let s2 = t2.settlement_hash(1100, 900);
        assert_eq!(s1, s2, "settlement hashes must match");

        assert_ne!(s1, t1.settlement_hash(900, 1100));
    }

    #[test]
    fn test_chain_verification() {
        let mut transcript = Transcript::new();
        let hash = transcript.record(0, Action::Bet, 50, 100, BLOCK_HASH);

        let action = SignedAction {
            seat: 0, action: Action::Bet, amount: 50, seq: 1,
            block_height: 100, block_hash: BLOCK_HASH,
            hash, sig: [0u8; 64],
        };

        let mut fresh = Transcript::new();
        let (valid, timeout) = fresh.verify_action(&action);
        assert!(valid);
        assert_eq!(timeout, TimeoutResult::Ok);

        fresh.add_signed(action);

        let hash2 = transcript.record(1, Action::Call, 50, 102, BLOCK_HASH);
        let action2 = SignedAction {
            seat: 1, action: Action::Call, amount: 50, seq: 2,
            block_height: 102, block_hash: BLOCK_HASH,
            hash: hash2, sig: [0u8; 64],
        };
        let (valid, _) = fresh.verify_action(&action2);
        assert!(valid);
    }

    #[test]
    fn test_turn_timeout() {
        let timeout = TimeoutConfig {
            turn_timeout_blocks: 6,
            hand_timeout_blocks: 120,
        };
        let mut transcript = Transcript::with_timeout(timeout);
        let hash = transcript.record(0, Action::Bet, 50, 100, BLOCK_HASH);

        let action = SignedAction {
            seat: 0, action: Action::Bet, amount: 50, seq: 1,
            block_height: 100, block_hash: BLOCK_HASH,
            hash, sig: [0u8; 64],
        };
        transcript.add_signed(action);

        // opponent responds 10 blocks later (> 6 block timeout)
        let late_hash = compute_action_hash(
            1, Action::Call, 50, 2, 110, &BLOCK_HASH, &transcript.head_hash(),
        );
        let late_action = SignedAction {
            seat: 1, action: Action::Call, amount: 50, seq: 2,
            block_height: 110, block_hash: BLOCK_HASH,
            hash: late_hash, sig: [0u8; 64],
        };

        let (valid, timeout) = transcript.verify_action(&late_action);
        assert!(valid, "hash chain is still valid");
        assert_eq!(timeout, TimeoutResult::TurnTimeout { seat: 1, blocks_late: 4 });
    }

    #[test]
    fn test_check_timeout_current_block() {
        let timeout = TimeoutConfig {
            turn_timeout_blocks: 6,
            hand_timeout_blocks: 120,
        };
        let mut transcript = Transcript::with_timeout(timeout);
        let hash = transcript.record(0, Action::Check, 0, 100, BLOCK_HASH);
        transcript.add_signed(SignedAction {
            seat: 0, action: Action::Check, amount: 0, seq: 1,
            block_height: 100, block_hash: BLOCK_HASH,
            hash, sig: [0u8; 64],
        });

        // within timeout
        assert_eq!(transcript.check_timeout(104), TimeoutResult::Ok);

        // seat 1 (opponent) should have acted by now
        assert_eq!(
            transcript.check_timeout(108),
            TimeoutResult::TurnTimeout { seat: 1, blocks_late: 2 }
        );
    }

    #[test]
    fn test_tampered_action_fails() {
        let mut transcript = Transcript::new();
        let hash = transcript.record(0, Action::Check, 0, 100, BLOCK_HASH);

        let tampered = SignedAction {
            seat: 0, action: Action::Bet, amount: 0, seq: 1,
            block_height: 100, block_hash: BLOCK_HASH,
            hash, sig: [0u8; 64],
        };

        let fresh = Transcript::new();
        let (valid, _) = fresh.verify_action(&tampered);
        assert!(!valid, "tampered action should fail");
    }

    #[test]
    fn test_hand_timeout() {
        let timeout = TimeoutConfig {
            turn_timeout_blocks: 60,
            hand_timeout_blocks: 10,
        };
        let mut transcript = Transcript::with_timeout(timeout);
        let hash = transcript.record(0, Action::Check, 0, 100, BLOCK_HASH);
        transcript.add_signed(SignedAction {
            seat: 0, action: Action::Check, amount: 0, seq: 1,
            block_height: 100, block_hash: BLOCK_HASH,
            hash, sig: [0u8; 64],
        });

        assert_eq!(transcript.check_timeout(115), TimeoutResult::HandTimeout);
    }
}
