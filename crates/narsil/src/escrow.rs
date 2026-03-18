//! escrow lifecycle for poker tables
//!
//! each poker table has a 2-of-3 FROST escrow address:
//!   share 1 = player A
//!   share 2 = player B
//!   share 3 = narsil jury (threshold-signed by jury panel via OSST)
//!
//! the escrow wraps a state channel — players exchange co-signed state
//! updates off-chain. each update increments the nonce and records
//! current balances. on dispute the jury only replays from the latest
//! agreed state, not the entire session.
//!
//! happy path: A + B sign final state → spend from escrow
//! dispute:    jury replays from latest SignedState → signs correction
//! timeout:    pre-signed refund tx unlocks after N blocks

use crate::tiers::Tier;

/// escrow state machine
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscrowState {
    /// DKG in progress between players + jury
    Setup,
    /// escrow address derived, waiting for deposits
    AwaitingDeposits {
        address: [u8; 32],
        expected: [u64; 2],
        received: [bool; 2],
    },
    /// both deposited, channel open, game in progress
    Active {
        address: [u8; 32],
        deposits: [u64; 2],
        channel: ChannelState,
    },
    /// settlement signed by both players (happy path)
    Settled,
    /// settlement signed via dispute (player + jury)
    Disputed {
        verdict: JuryVerdict,
    },
    /// refund after timeout
    Refunded,
}

/// channel state tracked by escrow (mirrors state-channel::Channel)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelState {
    /// latest co-signed nonce (monotonically increasing)
    pub nonce: u64,
    /// hash of latest co-signed state
    pub state_hash: [u8; 32],
    /// current balances per seat from latest signed state
    pub balances: Vec<u64>,
    /// number of hands played in this session
    pub hands_played: u64,
}

impl ChannelState {
    pub fn new(initial_balances: Vec<u64>) -> Self {
        Self {
            nonce: 0,
            state_hash: [0u8; 32],
            balances: initial_balances,
            hands_played: 0,
        }
    }
}

/// escrow configuration for a table
#[derive(Clone, Debug)]
pub struct EscrowConfig {
    pub tier: Tier,
    /// player A public key
    pub player_a: [u8; 32],
    /// player B public key
    pub player_b: [u8; 32],
    /// jury panel node IDs (narsil syndicate members assigned to this table)
    pub jury_panel: Vec<[u8; 32]>,
    /// timeout in blocks for refund
    pub refund_timeout_blocks: u32,
}

/// escrow keys produced by DKG
#[derive(Clone, Debug)]
pub struct EscrowKeys {
    /// the shared escrow address (group public key)
    pub address: [u8; 32],
    /// player A's key share index
    pub share_a: u16,
    /// player B's key share index
    pub share_b: u16,
    /// jury's key share index (backed by OSST internally)
    pub share_jury: u16,
    /// threshold (always 2 for 2-of-3)
    pub threshold: u16,
}

/// a co-signed state update from both players
#[derive(Clone, Debug)]
pub struct SignedStateUpdate {
    pub nonce: u64,
    pub state_hash: [u8; 32],
    pub balances: Vec<u64>,
    pub hands_played: u64,
    pub sig_a: [u8; 64],
    pub sig_b: [u8; 64],
}

/// jury verdict after replaying a disputed hand
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JuryVerdict {
    /// hand number that was disputed
    pub hand_number: u64,
    /// nonce of the last agreed state before the dispute
    pub last_agreed_nonce: u64,
    /// correct final balances after replaying the disputed hand
    pub correct_balances: Vec<u64>,
    /// who initiated the dispute incorrectly (pays jury deposit)
    pub deposit_loser: Option<DisputeLoser>,
    /// hash of the action log that was replayed
    pub action_log_hash: [u8; 32],
}

/// who lost the dispute (pays jury deposit)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisputeLoser {
    PlayerA,
    PlayerB,
}

/// full escrow lifecycle
#[derive(Clone, Debug)]
pub struct Escrow {
    pub config: EscrowConfig,
    pub state: EscrowState,
    pub keys: Option<EscrowKeys>,
}

impl Escrow {
    pub fn new(config: EscrowConfig) -> Self {
        Self {
            config,
            state: EscrowState::Setup,
            keys: None,
        }
    }

    /// transition after DKG completes
    pub fn dkg_complete(&mut self, keys: EscrowKeys) {
        let half_pot = self.config.tier.max_pot() / 2;
        let deposit = self.config.tier.jury_deposit();
        let expected = [half_pot + deposit, half_pot + deposit];
        self.state = EscrowState::AwaitingDeposits {
            address: keys.address,
            expected,
            received: [false; 2],
        };
        self.keys = Some(keys);
    }

    /// record a deposit from a player (called by scanner)
    pub fn record_deposit(&mut self, player_idx: usize, amount: u64) -> Result<(), EscrowError> {
        match &mut self.state {
            EscrowState::AwaitingDeposits { expected, received, address } => {
                if player_idx > 1 {
                    return Err(EscrowError::InvalidPlayer);
                }
                if amount < expected[player_idx] {
                    return Err(EscrowError::InsufficientDeposit {
                        expected: expected[player_idx],
                        received: amount,
                    });
                }
                received[player_idx] = true;

                if received[0] && received[1] {
                    let balances = vec![expected[0], expected[1]];
                    self.state = EscrowState::Active {
                        address: *address,
                        deposits: *expected,
                        channel: ChannelState::new(balances),
                    };
                }
                Ok(())
            }
            _ => Err(EscrowError::InvalidState),
        }
    }

    /// accept a co-signed state update from both players
    /// this advances the channel nonce and records latest balances
    pub fn update_state(&mut self, update: SignedStateUpdate) -> Result<(), EscrowError> {
        match &mut self.state {
            EscrowState::Active { channel, deposits, .. } => {
                if update.nonce <= channel.nonce {
                    return Err(EscrowError::StaleNonce {
                        current: channel.nonce,
                        received: update.nonce,
                    });
                }

                // balances must not exceed total deposits
                let total_deposits: u64 = deposits.iter().sum();
                let total_balances: u64 = update.balances.iter().sum();
                if total_balances > total_deposits {
                    return Err(EscrowError::BalanceOverflow);
                }

                channel.nonce = update.nonce;
                channel.state_hash = update.state_hash;
                channel.balances = update.balances;
                channel.hands_played = update.hands_played;
                Ok(())
            }
            _ => Err(EscrowError::InvalidState),
        }
    }

    /// get the latest agreed channel state (for dispute starting point)
    pub fn latest_state(&self) -> Option<&ChannelState> {
        match &self.state {
            EscrowState::Active { channel, .. } => Some(channel),
            _ => None,
        }
    }

    /// happy path: both players agreed on final settlement
    pub fn settle_cooperative(&mut self) -> Result<(), EscrowError> {
        match &self.state {
            EscrowState::Active { .. } => {
                self.state = EscrowState::Settled;
                Ok(())
            }
            _ => Err(EscrowError::InvalidState),
        }
    }

    /// dispute path: jury replayed from latest agreed state and produced verdict
    pub fn settle_dispute(&mut self, verdict: JuryVerdict) -> Result<(), EscrowError> {
        match &self.state {
            EscrowState::Active { deposits, channel, .. } => {
                // verdict must reference a nonce >= last agreed
                if verdict.last_agreed_nonce > channel.nonce {
                    return Err(EscrowError::InvalidNonceReference);
                }

                let total = deposits[0] + deposits[1];
                let verdict_total: u64 = verdict.correct_balances.iter().sum();
                if verdict_total > total {
                    return Err(EscrowError::PayoutExceedsDeposits);
                }

                self.state = EscrowState::Disputed { verdict };
                Ok(())
            }
            _ => Err(EscrowError::InvalidState),
        }
    }

    /// timeout refund
    pub fn refund(&mut self) -> Result<(), EscrowError> {
        match &self.state {
            EscrowState::AwaitingDeposits { .. } | EscrowState::Active { .. } => {
                self.state = EscrowState::Refunded;
                Ok(())
            }
            _ => Err(EscrowError::InvalidState),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscrowError {
    InvalidState,
    InvalidPlayer,
    InsufficientDeposit { expected: u64, received: u64 },
    PayoutExceedsDeposits,
    StaleNonce { current: u64, received: u64 },
    BalanceOverflow,
    InvalidNonceReference,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> EscrowConfig {
        EscrowConfig {
            tier: Tier::Regular,
            player_a: [1u8; 32],
            player_b: [2u8; 32],
            jury_panel: vec![[10u8; 32], [11u8; 32], [12u8; 32]],
            refund_timeout_blocks: 100,
        }
    }

    fn test_keys() -> EscrowKeys {
        EscrowKeys {
            address: [42u8; 32],
            share_a: 1,
            share_b: 2,
            share_jury: 3,
            threshold: 2,
        }
    }

    fn setup_active_escrow() -> Escrow {
        let mut escrow = Escrow::new(test_config());
        escrow.dkg_complete(test_keys());
        let deposit = Tier::Regular.max_pot() / 2 + Tier::Regular.jury_deposit();
        escrow.record_deposit(0, deposit).unwrap();
        escrow.record_deposit(1, deposit).unwrap();
        escrow
    }

    #[test]
    fn test_happy_path() {
        let mut escrow = setup_active_escrow();
        assert!(matches!(escrow.state, EscrowState::Active { .. }));

        // play some hands, update state
        let update = SignedStateUpdate {
            nonce: 1,
            state_hash: [0xAA; 32],
            balances: vec![55_000_000, 45_500_000],
            hands_played: 3,
            sig_a: [0; 64],
            sig_b: [0; 64],
        };
        escrow.update_state(update).unwrap();

        let channel = escrow.latest_state().unwrap();
        assert_eq!(channel.nonce, 1);
        assert_eq!(channel.hands_played, 3);

        escrow.settle_cooperative().unwrap();
        assert_eq!(escrow.state, EscrowState::Settled);
    }

    #[test]
    fn test_state_updates_advance_nonce() {
        let mut escrow = setup_active_escrow();

        for i in 1..=5 {
            let update = SignedStateUpdate {
                nonce: i,
                state_hash: [i as u8; 32],
                balances: vec![50_000_000 + i * 1000, 50_500_000 - i * 1000],
                hands_played: i,
                sig_a: [0; 64],
                sig_b: [0; 64],
            };
            escrow.update_state(update).unwrap();
        }

        let channel = escrow.latest_state().unwrap();
        assert_eq!(channel.nonce, 5);
        assert_eq!(channel.hands_played, 5);
    }

    #[test]
    fn test_stale_nonce_rejected() {
        let mut escrow = setup_active_escrow();

        let update = SignedStateUpdate {
            nonce: 3,
            state_hash: [0; 32],
            balances: vec![50_000_000, 50_500_000],
            hands_played: 1,
            sig_a: [0; 64],
            sig_b: [0; 64],
        };
        escrow.update_state(update).unwrap();

        // nonce 2 is stale (< 3)
        let stale = SignedStateUpdate {
            nonce: 2,
            state_hash: [0; 32],
            balances: vec![50_000_000, 50_500_000],
            hands_played: 1,
            sig_a: [0; 64],
            sig_b: [0; 64],
        };
        assert!(matches!(
            escrow.update_state(stale).unwrap_err(),
            EscrowError::StaleNonce { current: 3, received: 2 }
        ));
    }

    #[test]
    fn test_dispute_from_latest_state() {
        let mut escrow = setup_active_escrow();

        // advance to nonce 10 (10 hands played)
        let update = SignedStateUpdate {
            nonce: 10,
            state_hash: [0xBB; 32],
            balances: vec![60_000_000, 40_500_000],
            hands_played: 10,
            sig_a: [0; 64],
            sig_b: [0; 64],
        };
        escrow.update_state(update).unwrap();

        // dispute hand 11 — jury replays only hand 11 from nonce 10 state
        let verdict = JuryVerdict {
            hand_number: 11,
            last_agreed_nonce: 10,
            correct_balances: vec![65_000_000, 35_500_000],
            deposit_loser: Some(DisputeLoser::PlayerB),
            action_log_hash: [0xCC; 32],
        };

        escrow.settle_dispute(verdict.clone()).unwrap();
        assert_eq!(escrow.state, EscrowState::Disputed { verdict });
    }

    #[test]
    fn test_balance_overflow_rejected() {
        let mut escrow = setup_active_escrow();
        let deposit = Tier::Regular.max_pot() / 2 + Tier::Regular.jury_deposit();
        let total = deposit * 2;

        let bad_update = SignedStateUpdate {
            nonce: 1,
            state_hash: [0; 32],
            balances: vec![total + 1, 0],
            hands_played: 1,
            sig_a: [0; 64],
            sig_b: [0; 64],
        };
        assert_eq!(
            escrow.update_state(bad_update).unwrap_err(),
            EscrowError::BalanceOverflow,
        );
    }

    #[test]
    fn test_refund_from_awaiting() {
        let mut escrow = Escrow::new(test_config());
        escrow.dkg_complete(test_keys());
        escrow.refund().unwrap();
        assert_eq!(escrow.state, EscrowState::Refunded);
    }
}
