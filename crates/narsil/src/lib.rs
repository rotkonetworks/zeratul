//! narsil - private syndicate consensus on penumbra
//!
//! a narsil syndicate is a decaf377 spending key split via OSST.
//! it looks like a normal penumbra account to the chain, but requires
//! threshold approval for any action. members coordinate off-chain via relays,
//! only the final signed transaction hits penumbra.
//!
//! inspired by [henry de valence's narsil talk](https://www.youtube.com/watch?v=VWdHaKGrjq0&t=16m)
//!
//! # privacy property
//!
//! when a syndicate signs a transaction, penumbra validators cannot determine
//! which members participated. internal voting patterns, dissent, and power
//! dynamics remain hidden - the chain only learns that a valid t-of-n
//! subset authorized the action.
//!
//! relay-based coordination prevents metadata leakage that would occur with
//! direct P2P connections. members post to and fetch from public relays using
//! pseudonymous mailboxes derived from their viewing keys.
//!
//! # 100-share model
//!
//! each syndicate has exactly 100 OSST key shares. one share equals:
//! - one cryptographic signing contribution
//! - one governance vote
//! - one percent of distributions
//!
//! members may own multiple shares. threshold is share-based (e.g., 67 shares
//! needed means any subset holding 67+ shares can sign).
//!
//! # architecture
//!
//! ```text
//! ┌────────────────────────── PENUMBRA ─────────────────────────┐
//! │                                                              │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
//! │  │ alice's     │  │ bob's       │  │ syndicate account   │  │
//! │  │ personal    │  │ personal    │  │ (OSST group key)    │  │
//! │  │ account     │  │ account     │  │                     │  │
//! │  └─────────────┘  └─────────────┘  └──────────┬──────────┘  │
//! │                                               │              │
//! └───────────────────────────────────────────────┼──────────────┘
//!                                                 │
//!           ┌─────────────────────────────────────┘
//!           │  threshold-signed transactions
//!           │
//! ┌─────────┴───────────────────────────────────────────────────┐
//! │                    NARSIL RELAY LAYER                       │
//! │                                                             │
//! │  ┌─────────┐  ┌─────────┐  ┌─────────┐                     │
//! │  │ alice   │  │ bob     │  │ carol   │                     │
//! │  │ 30 shr  │  │ 30 shr  │  │ 40 shr  │                     │
//! │  └────┬────┘  └────┬────┘  └────┬────┘                     │
//! │       │            │            │                           │
//! │       └────────────┴─────┬──────┘                           │
//! │                          │ pseudonymous mailboxes           │
//! │                    ┌─────┴─────┐                            │
//! │                    │  RELAYS   │ (ipfs, dht, s3)            │
//! │                    └─────┬─────┘                            │
//! │        ┌─────────────────┴─────────────────┐                │
//! │        │ propose action (spend/swap/etc)   │                │
//! │        │ collect votes + osst contributions│                │
//! │        │ any member aggregates, broadcasts │                │
//! │        └───────────────────────────────────┘                │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # example
//!
//! ```ignore
//! use narsil::{Syndicate, Member, Round};
//!
//! // form syndicate with 3-of-5 threshold
//! let syndicate = Syndicate::new(threshold: 3, members: 5);
//!
//! // proposer creates round with state transition
//! let round = Round::propose(height, proposer_idx, payload);
//!
//! // members verify and contribute osst proofs
//! let contribution = round.contribute(&my_share, &mut rng);
//!
//! // once t contributions collected, finalize
//! let block = round.finalize(&contributions, &group_key)?;
//! // block.proof is the osst threshold signature
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod aggregator;
pub mod bft;
pub mod ceremony;
pub mod client;
pub mod coordinator;
pub mod crypto;
pub mod formation;
pub mod governance;
pub mod mailbox;
pub mod net;
pub mod networks;
pub mod penumbra;
pub mod proposal;
pub mod relay;
pub mod replay;
pub mod scanner;
pub mod reshare;
pub mod shares;
pub mod state;
pub mod syndicate;
pub mod traits;
pub mod vss;
pub mod wallet;
pub mod wire;
pub mod worker;

// re-export osst types
pub use osst::{
    Contribution, OsstCurve, OsstError, OsstPoint, OsstScalar, SecretShare,
    compute_lagrange_coefficients, hash_to_challenge, verify,
};

#[cfg(feature = "ristretto255")]
pub use osst::{Ristretto255, RistrettoContribution, RistrettoSecretShare};

#[cfg(feature = "pallas")]
pub use osst::{PallasCurve, PallasContribution, PallasSecretShare};

#[cfg(feature = "secp256k1")]
pub use osst::Secp256k1Curve;

#[cfg(feature = "decaf377")]
pub use osst::Decaf377Curve;

// re-export main types from submodules
pub use bft::{Round, FinalizedBlock, RoundError};
pub use crypto::{
    SyndicateCrypto, EncryptedMessage, SignedMessage,
    MemberCrypto, DirectMessage, generate_nonce,
};
pub use formation::{
    Formation, FormationMode, SharePolicy, Commitment, FormationError,
    shares_for_investment, nav_per_share, buyout_value,
};
pub use governance::{
    ShareRegistry, GovernanceRules, Proposal as GovernanceProposal, ProposalState as GovProposalState, ActionType,
    Distribution, GovernanceError, VoteSummary, MAX_SHARES,
};
pub use proposal::{
    Vote, ProposalKind, ProposalPayload, SyndicateProposal, ProposalState,
    ParameterKind, MembershipChangeKind, ShareSource, BuyoutTerms,
};
pub use state::{
    StateRoot, StateTransition, NullifierSet,
    SyndicateStateManager, StateError, VoteTally,
};
pub use syndicate::{Syndicate, Member, SyndicateConfig};

// wire format types for relay-based coordination
pub use wire::{
    Envelope, MessagePayload, SignedProposal, SignedVote, SignedContribution,
    SyndicateState, ShareOwnership, MemberInfo, GovernanceRules as WireGovernanceRules,
    Proposal as WireProposal, ProposalKind as WireProposalKind, Vote as WireVote, VoteType,
    Contribution as WireContribution, SyncRequest, SyncResponse, RecordedVote,
    ShareId, ProposalId, Hash32, Signature64,
};

// 100-share model (pubkey-addressed for relay coordination)
pub use shares::{
    ShareRegistry as PubkeyShareRegistry, ShareError, BatchedContribution,
    TOTAL_SHARES, MAX_SHARES_PER_MEMBER,
};

// mailbox addressing for relay coordination
pub use mailbox::{
    MailboxId, BroadcastTopic, MemberAddress, SyndicateRouter,
};

// replay protection
pub use replay::{
    ReplayCheck, ReplayValidator, EnvelopeBuilder, compute_state_hash,
};

// public relay protocol
pub use relay::{
    RelayEndpoint, RelayMessage, RelayRequest, RelayResponse, RelayError,
    RelayConfig,
};
#[cfg(feature = "std")]
pub use relay::MultiRelayClient;

// chain scanner for shielded wallets
pub use scanner::{
    CompactBlock, CompactOutput, TreeUpdate, DecryptedNote,
    FullViewingKey, Scanner, ScannerConfig, ScanResult,
    WitnessBuilder, SyncState, SyncStatus,
};

// verifiable secret sharing for backup
pub use vss::{
    VerifiableSharePackage, VssHeader, BackupShare, ShareDistributor, VssError,
};

// private wallet for shielded chains (penumbra, zcash)
pub use wallet::{
    SyndicateWallet, ShieldedNote, NoteWitness, WalletStatus, ShieldedChain,
    WalletRotation, RotationPhase, RotationReason, WalletManager,
};

// client for syndicate participation
pub use client::{
    MemberStorage, ProposalBuilder, SyndicateClient, ClientError,
};

// osst contribution aggregation
pub use aggregator::{
    ContributionCollector, CollectedContribution, BatchedAggregator,
    AggregationResult, AggregationError,
};

// coordinator selection (most shares = relay duty)
pub use coordinator::{
    CoordinatorSelector, CoordinatorDuties, CoordinatorRole, CoordinatorPhase,
    RoundCoordinator, RankedMember, SelectionStrategy,
};

// formation ceremony
pub use ceremony::{
    FormationCeremony, CeremonyPhase, JoiningMember, DkgCommitment, DkgShare,
    FormationResult, CeremonyError,
};

// resharing for membership changes
pub use reshare::{
    ReshareSession, ResharePhase, ReshareProposal, ReshareReason,
    ReshareResult, ReshareError, ReshareCommitment, OldMember, NewMember,
};

// network-agnostic traits for integration
pub use traits::{
    TxHash, TxStatus, NetworkAdapter, ActionBuilder, SignatureScheme,
    TransactionBuilder, StateBackend, KeyDerivation, SyndicateKeys,
    ActionRegistry, DynActionBuilder, SyndicateRuntime,
};
#[cfg(feature = "std")]
pub use traits::RelayBackend;

// network adapters
pub use networks::{
    PolkadotAdapter, PenumbraAdapter, ZcashAdapter, CosmosAdapter,
};
pub use networks::polkadot::{PolkadotAddress, PolkadotTransaction, AssetHubAction, AssetHubActionBuilder};
pub use networks::penumbra::{PenumbraAddress, PenumbraTransaction, PenumbraAction, PenumbraActionBuilder};
pub use networks::zcash::{ZcashAddress, ZcashTransaction, ZcashAction, ZcashActionBuilder};
pub use networks::cosmos::{CosmosAddress, CosmosTransaction, CosmosAction, CosmosActionBuilder, Coin};

// offchain worker (network isolation boundary)
pub use worker::{
    ChainQuery, ChainResponse, ChainEvent, EventFilter,
    SubmitRequest, SubmitResponse, SubmitBuilder,
    TxStatusResponse, WorkerError,
};
#[cfg(feature = "std")]
pub use worker::{OffchainWorker, WorkerHandle, MockWorker};
