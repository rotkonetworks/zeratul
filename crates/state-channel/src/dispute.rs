//! dispute resolution for poker state channels
//!
//! handles fraud proofs and on-chain verification

use alloc::vec::Vec;
use scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

use crate::state::PokerState;
use crate::types::*;

/// types of disputes that can be raised
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub enum DisputeType {
    /// player submitted invalid shuffle (ligerito proof fails)
    InvalidShuffle {
        /// accused player's seat
        accused_seat: Seat,
        /// the invalid shuffle commitment
        shuffle_commitment: ShuffleCommitment,
        /// the proof that should verify but doesn't
        proof: LigeritoProof,
    },

    /// player submitted invalid reveal token (chaum-pedersen fails)
    InvalidReveal {
        accused_seat: Seat,
        /// the invalid reveal token
        token: RevealToken,
    },

    /// player timed out (didn't act within timeout)
    Timeout {
        timed_out_seat: Seat,
        /// last known state
        last_state: SignedState<PokerState>,
        /// timestamp proving timeout
        timestamp: u64,
    },

    /// player submitted outdated/invalid state
    InvalidState {
        accused_seat: Seat,
        /// the invalid state update
        invalid_state: SignedState<PokerState>,
        /// the correct latest state
        correct_state: SignedState<PokerState>,
    },
}

/// dispute submission
#[derive(Clone, Debug, Encode, Decode, TypeInfo)]
pub struct Dispute {
    pub channel_id: ChannelId,
    /// who is raising the dispute
    pub challenger: AccountId,
    /// the type of dispute
    pub dispute_type: DisputeType,
    /// block number when dispute was raised
    pub raised_at: u64,
}

/// result of dispute verification
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub enum DisputeResult {
    /// dispute is valid, challenger wins
    ChallengerWins {
        /// amount slashed from accused
        slashed_amount: Balance,
        /// amount awarded to challenger
        reward_amount: Balance,
    },

    /// dispute is invalid, challenger loses deposit
    ChallengerLoses {
        /// amount slashed from challenger
        slashed_amount: Balance,
    },

    /// dispute requires more evidence
    NeedsMoreEvidence,
}

/// verify a shuffle dispute using ligerito
pub fn verify_shuffle_dispute(
    shuffle_commitment: &ShuffleCommitment,
    proof: &LigeritoProof,
) -> bool {
    // TODO: integrate actual ligerito verification
    // ligerito::verify(&proof.0, &shuffle_commitment.0)

    // for now, assume verification passes (placeholder)
    // in production, this calls into ligerito verifier
    let _ = (shuffle_commitment, proof);
    true
}

/// verify a reveal token dispute using chaum-pedersen
pub fn verify_reveal_dispute(
    token: &RevealToken,
    _encryption_key: &[u8],
) -> bool {
    // TODO: integrate actual chaum-pedersen verification
    // batch_chaum_pedersen::verify(&token.proof, &token.token, encryption_key)

    let _ = token;
    true
}

/// process a dispute and determine outcome
pub fn process_dispute(
    dispute: &Dispute,
    channel_participants: &[Participant],
    current_block: u64,
) -> DisputeResult {
    match &dispute.dispute_type {
        DisputeType::InvalidShuffle { accused_seat, shuffle_commitment, proof } => {
            // verify the shuffle proof
            let is_valid = verify_shuffle_dispute(shuffle_commitment, proof);

            if !is_valid {
                // shuffle is indeed invalid, challenger wins
                if let Some(accused) = channel_participants.iter().find(|p| p.seat == *accused_seat) {
                    return DisputeResult::ChallengerWins {
                        slashed_amount: accused.stake,
                        reward_amount: accused.stake / 2, // half goes to challenger, half burned
                    };
                }
            }

            // shuffle was valid, challenger made false claim
            DisputeResult::ChallengerLoses {
                slashed_amount: 100, // small penalty for false dispute
            }
        }

        DisputeType::InvalidReveal { accused_seat, token } => {
            if let Some(accused) = channel_participants.iter().find(|p| p.seat == *accused_seat) {
                let is_valid = verify_reveal_dispute(token, &accused.encryption_key);

                if !is_valid {
                    return DisputeResult::ChallengerWins {
                        slashed_amount: accused.stake,
                        reward_amount: accused.stake / 2,
                    };
                }
            }

            DisputeResult::ChallengerLoses {
                slashed_amount: 100,
            }
        }

        DisputeType::Timeout { timed_out_seat, last_state, timestamp } => {
            const TIMEOUT_BLOCKS: u64 = 60; // ~1 minute at 1 block/sec

            // verify timeout has actually occurred
            if current_block >= *timestamp + TIMEOUT_BLOCKS {
                if let Some(accused) = channel_participants.iter().find(|p| p.seat == *timed_out_seat) {
                    // verify the state is signed properly
                    if last_state.is_fully_signed(channel_participants.len()) {
                        return DisputeResult::ChallengerWins {
                            slashed_amount: accused.stake / 10, // 10% penalty for timeout
                            reward_amount: accused.stake / 20,
                        };
                    }
                }
            }

            DisputeResult::NeedsMoreEvidence
        }

        DisputeType::InvalidState { invalid_state, correct_state, .. } => {
            // compare nonces - higher nonce wins
            if correct_state.nonce > invalid_state.nonce {
                // verify correct state is properly signed
                if correct_state.is_fully_signed(channel_participants.len()) {
                    return DisputeResult::ChallengerWins {
                        slashed_amount: 500,
                        reward_amount: 250,
                    };
                }
            }

            DisputeResult::ChallengerLoses {
                slashed_amount: 100,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_participants() -> Vec<Participant> {
        vec![
            Participant {
                account: PublicKey::from_raw([1u8; 32]),
                seat: 0,
                stake: 1000,
                encryption_key: vec![1, 2, 3],
            },
            Participant {
                account: PublicKey::from_raw([2u8; 32]),
                seat: 1,
                stake: 1000,
                encryption_key: vec![4, 5, 6],
            },
        ]
    }

    #[test]
    fn test_shuffle_dispute() {
        let dispute = Dispute {
            channel_id: H256::zero(),
            challenger: PublicKey::from_raw([1u8; 32]),
            dispute_type: DisputeType::InvalidShuffle {
                accused_seat: 1,
                shuffle_commitment: ShuffleCommitment::default(),
                proof: LigeritoProof(vec![]),
            },
            raised_at: 100,
        };

        let result = process_dispute(&dispute, &mock_participants(), 150);

        // with placeholder verification, shuffle appears valid
        // so challenger loses
        assert!(matches!(result, DisputeResult::ChallengerLoses { .. }));
    }

    #[test]
    fn test_timeout_dispute() {
        let dispute = Dispute {
            channel_id: H256::zero(),
            challenger: PublicKey::from_raw([1u8; 32]),
            dispute_type: DisputeType::Timeout {
                timed_out_seat: 1,
                last_state: SignedState {
                    state: PokerState::new(H256::zero(), &mock_participants(), 10),
                    nonce: 5,
                    signatures: vec![Some(Signature::from_raw([0u8; 64])), Some(Signature::from_raw([0u8; 64]))],
                },
                timestamp: 100,
            },
            raised_at: 100,
        };

        // before timeout
        let result = process_dispute(&dispute, &mock_participants(), 150);
        assert!(matches!(result, DisputeResult::NeedsMoreEvidence));

        // after timeout
        let result = process_dispute(&dispute, &mock_participants(), 200);
        assert!(matches!(result, DisputeResult::ChallengerWins { .. }));
    }
}
