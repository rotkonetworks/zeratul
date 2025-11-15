//! General P2P networking primitives using litep2p with QUIC transport
//!
//! Designed for low-latency trading and blockchain applications.
//!
//! ## Features
//!
//! - **QUIC transport**: UDP-based, 10-50ms latency, built-in encryption
//! - **Gossipsub**: Efficient message broadcasting
//! - **Consensus primitives**: Block-based ordering with proof verification
//! - **Trading support**: Order matching, execution proofs
//!
//! ## Performance Targets
//!
//! - Proof verification: <1ms (512μs with Ligerito)
//! - Network latency: 10-50ms (QUIC over internet)
//! - Block time: 100ms-1s (configurable)
//! - Total order execution: <100ms end-to-end
//!
//! ## Architecture
//!
//! ```text
//! Trader 1                    P2P Network (QUIC)              Trader 2
//!    │                               │                           │
//!    │ Submit order (100ms)          │                           │
//!    ├──────────────────────────────►│                           │
//!    │                               │◄──────────────────────────┤
//!    │                               │   Submit order (100ms)    │
//!    │                               │                           │
//!    │                        Consensus Engine                   │
//!    │                        - Verify proofs (512μs)            │
//!    │                        - Match orders                     │
//!    │                        - Finalize block (100ms)           │
//!    │                               │                           │
//!    │◄──────────────────────────────┤                           │
//!    │   Trade executed              │──────────────────────────►│
//!    │                               │      Trade executed       │
//! ```

pub mod gossip;
pub mod types;
pub mod consensus;
pub mod trading;
pub mod jamnp;
pub mod zswap;
pub mod zswap_pvm;
pub mod privacy;
pub mod bft;
pub mod delegation_tokens;
pub mod staking_rewards;
pub mod staked_pool;
pub mod slashing;

pub use gossip::GossipNetwork;
pub use types::{PeerId, Message};
pub use consensus::{Genesis, ProvenTransaction, ConsensusEngine, BlockNumber};
pub use trading::{Order, OrderBook, Trade, Side};
pub use jamnp::{JamCertificate, AlpnId, StreamKind, Ed25519PublicKey, Ed25519Signature};
pub use zswap::{
    TradingPair, SwapIntent, BatchSwap, LiquidityPosition, DexState,
    StakePosition, Delegation, ZT_ASSET_ID, MIN_STAKE_ZT, TOTAL_SUPPLY_ZT,
};
pub use zswap_pvm::{ZSwapPVM, SwapProof};
pub use privacy::{PedersenCommitment, RangeProof};
pub use bft::{BatchProposal, StakeSignature, BftConsensus, BatchError, SlashingEvidence};
pub use delegation_tokens::{DelegationState, DelegationPool, ExchangeRate, ValidatorId, DelegationError};
pub use staking_rewards::{StakingRewards, RewardStats, BASE_INFLATION_BPS, TARGET_STAKING_RATIO_BPS};
pub use staked_pool::{StakedPool, PoolExchangeRate, PoolStats, StakedPoolError};
pub use slashing::{SlashingCalculator, SlashingEvent, SlashingOffense};
