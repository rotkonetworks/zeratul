//! Escrow Arbitration Pallet
//!
//! Privacy-preserving 3-party escrow for P2P cryptocurrency trades.
//! The chain acts as a "keyless account" in a 2/3 multisig on Zcash/Penumbra.
//!
//! ## Design Philosophy: Chain as Multisig Participant
//!
//! Unlike traditional escrow where a single arbitrator holds a key, this design
//! makes the **chain itself** one of the 3 signers in a 2/3 multisig:
//!
//! ```text
//! Zcash/Penumbra 2-of-3 Multisig:
//!                    ┌─────────────┐
//!                    │ escrow_addr │
//!                    └──────┬──────┘
//!         Threshold:   2 of 3 keys
//!                         │
//!            ┌────────────┼────────────┐
//!            │            │            │
//!         key_buyer   key_seller   key_chain
//!            │            │            │
//!         Buyer's     Seller's    FROST threshold
//!         wallet      wallet      (validators)
//! ```
//!
//! ## Why Chain as Multisig Participant?
//!
//! | Aspect | Individual Arbitrator | Chain as Arbitrator |
//! |--------|----------------------|---------------------|
//! | Trust | Trust single agent | Trust validator consensus |
//! | Collusion | Arb can collude | Need threshold of validators |
//! | Availability | Single point of failure | Chain always available |
//! | Scalability | Need arbitrator market | Automatic |
//!
//! ## Trade Flow (P2P Money Transfer)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │  1. TRADE SETUP                                                     │
//! │     Buyer & Seller: each generate their own keys (ed25519/pallas)   │
//! │     Chain: derives per-escrow key from master FROST threshold key   │
//! │     Escrow address: MultiSig(2/3, buyer_pk, seller_pk, chain_pk)    │
//! │     Seller: funds escrow address on Zcash/Penumbra                  │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  2. ENCRYPTED NEGOTIATION                                           │
//! │     Seller → Buyer: "Send to bank XYZ, ref: ABC123" (encrypted)     │
//! │     Buyer → Seller: "Sent via Wise, tx #12345" (encrypted)          │
//! │     All messages stored on-chain but only readable by participants  │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  3. PAYMENT                                                         │
//! │     Buyer: sends fiat payment using negotiated method               │
//! │     Buyer: marks payment as sent on-chain                           │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  4. RESOLUTION                                                      │
//! │     Happy: Seller confirms → Buyer + Seller co-sign release tx      │
//! │     Dispute: Arbitrators vote → Chain + Winner co-sign release tx   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## FROST Threshold Key
//!
//! The chain's signing key is a FROST threshold key where no single validator
//! knows the full private key. When the chain needs to sign:
//!
//! 1. Dispute resolution triggers signing request
//! 2. Validators coordinate FROST signing protocol
//! 3. Threshold signature produced and submitted
//!
//! ## Privacy Guarantees
//!
//! - Payment details (bank account, reference) only visible to buyer/seller
//! - On-chain observers see: encrypted blobs, escrow pubkey
//! - Validators cannot see trade details (only sign when consensus decides)

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

extern crate alloc;
use alloc::vec::Vec;

// Encryption module (X25519 + ChaCha20Poly1305)
#[cfg(feature = "std")]
pub mod encryption;
#[cfg(feature = "std")]
pub use encryption::{decrypt_share, encrypt_share, generate_keypair, public_key_from_secret};

// Weights and benchmarking
pub mod weights;
pub use weights::WeightInfo;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

// Shielded escrow modules (privacy-preserving design)
pub mod shielded;
pub mod shielded_v1;
pub use shielded_v1::{
    ShieldedEscrowV1, EscrowParams, EscrowState as ShieldedEscrowState,
    ShieldedActionV1, RingSignature, VerifiableShare, VssCommitment,
    ShieldedDisputeInfo, compute_nullifier, compute_nullifier_commitment,
    block_to_epoch, compute_timeout_epoch,
};

// VSS verification exports (when shielded-escrow feature enabled)
#[cfg(feature = "shielded-escrow")]
pub use shielded_v1::{
    verify_share, commitment_from_share_set, reconstruct_secret,
};

// Threshold decryption exports
pub use shielded_v1::{
    DecryptionShare, ThresholdEncryptedEvidence, DecryptionSession, DecryptionStatus,
};
#[cfg(feature = "shielded-escrow")]
pub use shielded_v1::{
    verify_decryption_share_proof, combine_decryption_shares, decrypt_evidence,
};
#[cfg(feature = "std")]
pub use shielded_v1::encrypt_evidence_to_threshold_key;

// FROST signing types for dispute resolution
pub mod frost_types {
    use codec::{Decode, Encode, MaxEncodedLen};
    use scale_info::TypeInfo;

    /// FROST signature (compatible with pallet-frost-bridge)
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, codec::DecodeWithMemTracking)]
    pub struct FrostSignature {
        pub r: [u8; 32],
        pub s: [u8; 32],
    }

    /// Shielded signing request for dispute resolution
    ///
    /// When a dispute is resolved, chain needs to sign a release tx.
    /// This struct tracks the signing request without revealing trade details.
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, codec::DecodeWithMemTracking)]
    pub struct ShieldedSigningRequest {
        /// Request ID (references frost-bridge signing queue)
        pub frost_request_id: u64,
        /// Escrow commitment (links to shielded escrow)
        pub escrow_commitment: [u8; 32],
        /// Release recipient (encrypted to winner's key)
        pub encrypted_recipient: [u8; 64],
        /// Transaction hash being signed
        pub tx_hash: [u8; 32],
        /// Request status
        pub status: ShieldedSigningStatus,
        /// Block created
        pub created_at: u32,
    }

    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, codec::DecodeWithMemTracking, Default)]
    pub enum ShieldedSigningStatus {
        /// Waiting for FROST signing to complete
        #[default]
        Pending,
        /// Signing completed
        Completed {
            signature: FrostSignature,
        },
        /// Signing failed
        Failed,
    }
}
pub use frost_types::{FrostSignature, ShieldedSigningRequest, ShieldedSigningStatus};

// Ligerito verifier imports (when feature enabled)
// TODO: These will be used when full ligerito proof verification is integrated
#[cfg(feature = "ligerito-verify")]
#[allow(unused_imports)]
use codec::Decode;
#[cfg(feature = "ligerito-verify")]
#[allow(unused_imports)]
use ligerito::{hardcoded_config_12_verifier, verify_sha256, FinalizedLigeritoProof};
#[cfg(feature = "ligerito-verify")]
#[allow(unused_imports)]
use ligerito_binary_fields::{BinaryElem128, BinaryElem32};

// OCW crypto imports
#[cfg(feature = "std")]
use sp_core::crypto::KeyTypeId;

/// Key type for escrow agent keys
#[cfg(feature = "std")]
pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"escr");

#[cfg(feature = "std")]
pub mod crypto {
    use super::KEY_TYPE;
    use sp_runtime::{
        app_crypto::{app_crypto, sr25519},
        MultiSignature, MultiSigner,
    };

    app_crypto!(sr25519, KEY_TYPE);

    pub struct EscrowAuthId;

    impl frame_system::offchain::AppCrypto<MultiSigner, MultiSignature> for EscrowAuthId {
        type RuntimeAppPublic = Public;
        type GenericSignature = sp_core::sr25519::Signature;
        type GenericPublic = sp_core::sr25519::Public;
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::{
        pallet_prelude::*,
        traits::{Currency, ReservableCurrency},
        Blake2_128Concat,
    };
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::Zero;

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    // ========== TYPES ==========

    /// Target chain for escrow funds
    ///
    /// The actual crypto is held on these external chains in a 2/3 multisig
    /// address. The parachain coordinates the trade and holds one key.
    #[derive(
        Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq,
        codec::DecodeWithMemTracking, Default,
    )]
    pub enum TargetChain {
        /// Zcash - escrow ZEC using FROST multisig
        /// Uses RedPallas (Orchard) or RedJubjub (Sapling) curve
        #[default]
        Zcash,
        /// Penumbra - escrow UM or any Penumbra asset
        /// Uses decaf377 curve with FROST
        Penumbra,
    }

    /// Chain's FROST threshold key configuration
    ///
    /// The chain holds one key in each 2/3 escrow multisig. This key is
    /// a FROST threshold key where validators collectively control the
    /// signing capability without any single validator knowing the full key.
    ///
    /// Per-escrow keys are derived: chain_escrow_pk = derive(master, escrow_id)
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    pub struct ChainThresholdKey {
        /// Chain's master FROST public key (verifiable by all)
        pub master_public_key: [u8; 32],
        /// Threshold parameters: (min_signers, total_validators)
        /// e.g., (5, 7) means 5 of 7 validators must participate
        pub threshold: (u16, u16),
        /// Block when this key was established via DKG
        pub established_at: u32,
        /// Key version (incremented on rotation)
        pub version: u32,
    }

    /// Signing request status for chain threshold signing
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    pub enum SigningRequestStatus {
        /// Request created, waiting for validators to coordinate
        Pending,
        /// Signing in progress (validators coordinating)
        InProgress { started_at: u32 },
        /// Signature produced and submitted
        Completed { signature: [u8; 64] },
        /// Signing failed (timeout or threshold not met)
        Failed { reason: SigningFailureReason },
    }

    #[derive(
        Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq,
        codec::DecodeWithMemTracking,
    )]
    pub enum SigningFailureReason {
        Timeout,
        InsufficientValidators,
        ProtocolError,
    }

    /// A request for the chain to sign an escrow release transaction
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    pub struct ChainSigningRequest {
        /// Escrow this signing request is for
        pub escrow_id: [u8; 32],
        /// Transaction bytes to sign (Zcash/Penumbra tx)
        pub tx_hash: [u8; 32],
        /// Recipient (buyer or seller address on target chain)
        pub recipient: [u8; 32],
        /// Amount being released
        pub amount: u128,
        /// Request status
        pub status: SigningRequestStatus,
        /// Block when request was created
        pub created_at: u32,
        /// Deadline for signing
        pub deadline: u32,
    }

    /// Funding attestation source
    ///
    /// How we verify the escrow address was actually funded on the external chain.
    #[derive(
        Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq,
        codec::DecodeWithMemTracking,
    )]
    pub enum FundingAttestation {
        /// Light client proof (future: actual SPV/IBC proof)
        LightClient { block_height: u64, tx_hash: [u8; 32] },
        /// Oracle attestation (trusted oracle signs funding confirmation)
        Oracle { oracle_id: [u8; 32], signature: [u8; 64] },
        /// Self-attested by seller (trust-minimized for small amounts)
        SelfAttested { tx_hash: [u8; 32] },
    }

    /// Trader profile for P2P trading (like LocalCryptos accounts)
    ///
    /// Stores X25519 pubkey for E2E encrypted messaging between trade parties.
    /// This is DIFFERENT from agent keys - traders are buyers/sellers, not escrow agents.
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    pub struct TraderProfile<BlockNumber> {
        /// X25519 public key for E2E messaging (derived from account seed)
        pub x25519_pubkey: [u8; 32],
        /// When profile was created
        pub registered_at: BlockNumber,
        /// Total trades completed
        pub total_trades: u32,
        /// Successful completions (as buyer or seller)
        pub successful_trades: u32,
        /// Disputes where this trader was ruled against
        pub disputes_lost: u32,
        /// Average trade completion time (blocks)
        pub avg_completion_blocks: u32,
    }

    /// Encrypted trade message (LocalCryptos-style E2E chat)
    ///
    /// Messages are encrypted with X25519+ChaCha20Poly1305 using:
    /// - Sender's ephemeral secret
    /// - Recipient's X25519 pubkey from TraderProfile
    ///
    /// Format: EPK (32) || Nonce (12) || Tag (16) || Ciphertext
    /// Unlike share encryption, we use random nonces here since same
    /// keys may exchange multiple messages.
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    pub struct EncryptedMessage<AccountId, BlockNumber> {
        /// Message sender
        pub sender: AccountId,
        /// Encrypted payload (max 1KB for payment details)
        pub ciphertext: BoundedVec<u8, ConstU32<1024>>,
        /// Block when sent
        pub sent_at: BlockNumber,
        /// Message sequence number within this escrow
        pub sequence: u32,
    }

    /// Payment method hint (unencrypted category for filtering)
    ///
    /// The actual payment details (account numbers, references) are
    /// in the encrypted messages. This is just for UX/filtering.
    #[derive(
        Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq,
        codec::DecodeWithMemTracking,
    )]
    pub enum PaymentMethod {
        BankTransfer,
        CashDeposit,
        MobilePayment,
        CryptoToFiat,
        InPerson,
        Other,
    }

    /// Escrow state machine
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
    pub enum EscrowState {
        /// Waiting for buyer to verify their share
        #[default]
        AwaitingBuyerConfirmation,
        /// Buyer confirmed, waiting for funding
        AwaitingFunding,
        /// Funded, waiting for fiat payment
        AwaitingPayment,
        /// Buyer claims they paid
        PaymentClaimed { claimed_at: u32 },
        /// Dispute raised
        Disputed { raised_at: u32, raised_by: DisputeInitiator },
        /// Resolution decided, waiting for share reveal
        PendingReveal { to_buyer: bool, decided_at: u32 },
        /// Completed - released to buyer
        ReleasedToBuyer,
        /// Completed - released to seller
        ReleasedToSeller,
        /// Expired (funding timeout)
        Expired,
        /// Cancelled by seller
        Cancelled,
    }

    #[derive(
        Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq,
        codec::DecodeWithMemTracking,
    )]
    pub enum DisputeInitiator {
        Buyer,
        Seller,
    }

    /// Escrow agent info
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    pub struct EscrowAgent<AccountId, Balance, BlockNumber> {
        /// Agent's account
        pub account: AccountId,
        /// X25519 public key for share encryption
        pub x25519_pubkey: [u8; 32],
        /// Staked amount
        pub stake: Balance,
        /// When registered
        pub registered_at: BlockNumber,
        /// Whether currently active
        pub active: bool,
        /// Reputation score (higher = better)
        pub reputation: u64,
        /// Total escrows participated in
        pub total_escrows: u32,
        /// Successful reveals (when resolution required reveal)
        pub successful_reveals: u32,
        /// Failed reveals (resolution needed reveal, agent didn't deliver)
        pub failed_reveals: u32,
        /// Total bounties earned
        pub total_bounties_earned: Balance,
        /// Average response blocks (lower = better)
        pub avg_response_blocks: u32,
    }

    /// Cross-chain escrow for fiat→crypto trades using 2/3 multisig
    ///
    /// The crypto (ZEC/UM) is held on Zcash/Penumbra in a 2-of-3 multisig
    /// address where keys are held by: Buyer, Seller, Chain.
    ///
    /// Flow:
    /// 1. Buyer & Seller each provide their public keys
    /// 2. Chain derives per-escrow key: chain_pk = derive(master, escrow_id)
    /// 3. Escrow address = MultiSig(2/3, buyer_pk, seller_pk, chain_pk)
    /// 4. Seller funds escrow address with ZEC/UM on external chain
    /// 5. Buyer sends fiat payment off-chain
    /// 6a. Happy: Seller confirms → Buyer + Seller co-sign release tx
    /// 6b. Dispute: Arbitrators decide winner → Chain + Winner co-sign
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    #[scale_info(skip_type_params(T))]
    pub struct Escrow<T: Config> {
        pub id: [u8; 32],
        /// Target chain where crypto is escrowed
        pub chain: TargetChain,
        /// Buyer's on-chain account (for authentication)
        pub buyer: T::AccountId,
        /// Seller's on-chain account (for authentication)
        pub seller: T::AccountId,
        /// Crypto amount in external chain units (zatoshi for ZEC, microUM for Penumbra)
        pub crypto_amount: u128,
        /// For Penumbra: asset ID (None = native UM)
        pub asset_id: Option<[u8; 32]>,
        /// Fiat amount buyer will pay (in cents)
        pub fiat_amount: u64,
        /// Fiat currency code (e.g., b"USD", b"EUR")
        pub fiat_currency: [u8; 3],
        /// Payment method hint (actual details in encrypted messages)
        pub payment_method: PaymentMethod,
        /// Current state
        pub state: EscrowState,

        // --- 2/3 Multisig Keys (on target chain) ---
        /// Buyer's public key on target chain (Zcash/Penumbra)
        pub buyer_escrow_pubkey: [u8; 32],
        /// Seller's public key on target chain (Zcash/Penumbra)
        pub seller_escrow_pubkey: [u8; 32],
        /// Chain's derived public key for this escrow
        /// Derived: blake2b(chain_master_pk || escrow_id || "escrow-key")
        pub chain_escrow_pubkey: [u8; 32],
        /// The 2/3 multisig escrow address on target chain
        /// Derived from all 3 public keys using chain-specific logic
        pub escrow_address: [u8; 32],

        // --- Deposit (fee for chain signing service) ---
        /// Deposit for chain signing service (returned on happy path)
        pub chain_service_deposit: BalanceOf<T>,

        // --- Timing ---
        pub created_at: u32,
        pub funding_deadline: u32,
        pub payment_timeout: u32,

        // --- Funding attestation ---
        pub funding_attestation: Option<FundingAttestation>,

        // --- Chain signing (for disputes) ---
        /// If dispute resolved, the signing request ID
        pub signing_request_id: Option<u64>,
    }

    /// Arbitrator info (for disputes, separate from agents)
    #[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
    pub struct ArbitratorInfo<Balance, BlockNumber> {
        pub stake: Balance,
        pub registered_at: BlockNumber,
        pub disputes_resolved: u32,
        pub correct_votes: u32,
        pub active: bool,
    }

    #[derive(
        Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq,
        codec::DecodeWithMemTracking,
    )]
    pub enum DisputeVote {
        Buyer,
        Seller,
        Abstain,
    }

    // ========== PALLET ==========

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        type Currency: ReservableCurrency<Self::AccountId>;

        /// Minimum stake to become an escrow agent
        #[pallet::constant]
        type MinAgentStake: Get<BalanceOf<Self>>;

        /// Minimum stake to become an arbitrator
        #[pallet::constant]
        type MinArbitratorStake: Get<BalanceOf<Self>>;

        /// Default funding deadline (blocks)
        #[pallet::constant]
        type DefaultFundingDeadline: Get<u32>;

        /// Default payment timeout (blocks)
        #[pallet::constant]
        type DefaultPaymentTimeout: Get<u32>;

        /// Dispute voting period (blocks)
        #[pallet::constant]
        type DisputeVotingPeriod: Get<u32>;

        /// Minimum arbitrators for dispute resolution
        #[pallet::constant]
        type MinArbitratorsForDispute: Get<u32>;

        /// Minimum agents per escrow
        #[pallet::constant]
        type MinAgentsPerEscrow: Get<u32>;

        /// Chain signing timeout after dispute resolution (blocks)
        #[pallet::constant]
        type SigningTimeout: Get<u32>;

        /// Minimum chain service deposit (for signing service fee)
        #[pallet::constant]
        type MinChainServiceDeposit: Get<BalanceOf<Self>>;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: crate::weights::WeightInfo;
    }

    // ========== STORAGE ==========

    // ----- CHAIN THRESHOLD KEY (FROST) -----

    /// Chain's master FROST threshold key for escrow signing
    ///
    /// This key is established via DKG among validators. Per-escrow keys
    /// are derived from this master key deterministically.
    #[pallet::storage]
    pub type ChainMasterKey<T: Config> = StorageValue<_, ChainThresholdKey>;

    /// Signing requests for chain threshold signatures
    #[pallet::storage]
    pub type SigningRequests<T: Config> =
        StorageMap<_, Blake2_128Concat, u64, ChainSigningRequest>;

    /// Next signing request ID
    #[pallet::storage]
    pub type NextSigningRequestId<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Pending signing requests (for timeout processing)
    #[pallet::storage]
    pub type PendingSigningRequests<T: Config> = StorageValue<
        _,
        BoundedVec<u64, ConstU32<1000>>,
        ValueQuery,
    >;

    // ----- TRADER PROFILES (LocalCryptos-style) -----

    /// Trader profiles (buyers/sellers)
    #[pallet::storage]
    pub type Traders<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        TraderProfile<BlockNumberFor<T>>,
    >;

    /// Trader count
    #[pallet::storage]
    pub type TraderCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    // ----- ENCRYPTED MESSAGES -----

    /// Encrypted messages per escrow (E2E chat between buyer/seller)
    /// Key: escrow_id, Value: vec of encrypted messages
    #[pallet::storage]
    pub type EscrowMessages<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        [u8; 32],
        BoundedVec<EncryptedMessage<T::AccountId, BlockNumberFor<T>>, ConstU32<100>>,
        ValueQuery,
    >;

    /// Message count per escrow (for sequence numbers)
    #[pallet::storage]
    pub type MessageCount<T: Config> = StorageMap<_, Blake2_128Concat, [u8; 32], u32, ValueQuery>;

    // ----- ESCROW AGENTS -----

    /// Registered escrow agents
    #[pallet::storage]
    pub type Agents<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        EscrowAgent<T::AccountId, BalanceOf<T>, BlockNumberFor<T>>,
    >;

    /// Agent count
    #[pallet::storage]
    pub type AgentCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Agents sorted by reputation (top N for leaderboard)
    /// Value is (reputation, account)
    #[pallet::storage]
    pub type AgentLeaderboard<T: Config> = StorageValue<
        _,
        BoundedVec<(u64, T::AccountId), ConstU32<100>>,
        ValueQuery,
    >;

    /// Escrows
    #[pallet::storage]
    pub type Escrows<T: Config> = StorageMap<_, Blake2_128Concat, [u8; 32], Escrow<T>>;

    /// Escrows by buyer
    #[pallet::storage]
    pub type EscrowsByBuyer<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<[u8; 32], ConstU32<100>>,
        ValueQuery,
    >;

    /// Escrows by seller
    #[pallet::storage]
    pub type EscrowsBySeller<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<[u8; 32], ConstU32<100>>,
        ValueQuery,
    >;

    // Note: EscrowsByAgent removed - chain is now the sole "arbitrator" via FROST

    /// Arbitrators (for dispute resolution)
    #[pallet::storage]
    pub type Arbitrators<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        ArbitratorInfo<BalanceOf<T>, BlockNumberFor<T>>,
    >;

    /// Arbitrator count
    #[pallet::storage]
    pub type ArbitratorCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Dispute votes
    #[pallet::storage]
    pub type DisputeVotes<T: Config> =
        StorageDoubleMap<_, Blake2_128Concat, [u8; 32], Blake2_128Concat, T::AccountId, DisputeVote>;

    /// Vote counts per escrow (buyer_votes, seller_votes, abstain)
    #[pallet::storage]
    pub type VoteCounts<T: Config> = StorageMap<_, Blake2_128Concat, [u8; 32], (u32, u32, u32), ValueQuery>;

    /// Escrow nonce for ID generation
    #[pallet::storage]
    pub type EscrowNonce<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Active escrows pending reveal (for timeout processing)
    #[pallet::storage]
    pub type PendingReveals<T: Config> = StorageValue<
        _,
        BoundedVec<[u8; 32], ConstU32<1000>>,
        ValueQuery,
    >;

    /// Pallet account for holding bounties
    #[pallet::storage]
    pub type PalletAccount<T: Config> = StorageValue<_, T::AccountId>;

    // ========== SHIELDED ESCROW STORAGE ==========

    /// Shielded escrows - keyed by commitment (no party identities visible)
    ///
    /// Adversary sees: N identical-looking [u8; 32] keys with encrypted blobs
    /// Cannot see: parties, amounts, addresses, state
    #[pallet::storage]
    pub type ShieldedEscrows<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        [u8; 32], // commitment (NOT escrow_id - that would be linkable)
        super::ShieldedEscrowV1,
    >;

    /// Nullifier set - tracks spent escrows
    ///
    /// When escrow is consumed, nullifier is revealed and added here.
    /// Prevents double-spend without linking to original commitment.
    #[pallet::storage]
    pub type NullifierSet<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        [u8; 32], // nullifier
        (), // just existence
    >;

    /// VSS commitments - links escrow commitment to polynomial commitment
    ///
    /// Used for share verification (ligerito-escrow integration)
    #[pallet::storage]
    pub type VssCommitments<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        [u8; 32], // escrow commitment
        super::VssCommitment,
    >;

    /// Shielded dispute queue - encrypted evidence pending resolution
    ///
    /// Adversary sees: K disputes pending
    /// Cannot see: what they're about (encrypted to arbitrator threshold key)
    #[pallet::storage]
    pub type ShieldedDisputeQueue<T: Config> = StorageValue<
        _,
        BoundedVec<super::ShieldedDisputeInfo, ConstU32<100>>,
        ValueQuery,
    >;

    /// Current epoch (coarse-grained timing)
    ///
    /// Updated periodically, not every block. Reduces timing correlation.
    #[pallet::storage]
    pub type CurrentEpoch<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Blocks per epoch (configurable timing precision)
    #[pallet::storage]
    pub type BlocksPerEpoch<T: Config> = StorageValue<_, u32, ValueQuery>;

    // ======= FROST SIGNING STORAGE =======

    /// Pending FROST signing requests for dispute resolution
    ///
    /// When a dispute is resolved in favor of one party, chain must sign
    /// a release transaction. This tracks those requests.
    ///
    /// Key: escrow_commitment
    /// Value: ShieldedSigningRequest
    #[pallet::storage]
    pub type FrostSigningRequests<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        [u8; 32], // escrow_commitment
        super::ShieldedSigningRequest,
    >;

    /// Next FROST request ID (local counter, maps to frost-bridge request IDs)
    #[pallet::storage]
    pub type NextFrostRequestId<T: Config> = StorageValue<_, u64, ValueQuery>;

    // ========== EVENTS ==========

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        // ----- TRADER EVENTS -----

        /// Trader profile registered
        TraderRegistered {
            trader: T::AccountId,
            x25519_pubkey: [u8; 32],
        },
        /// Trader profile updated
        TraderUpdated {
            trader: T::AccountId,
            x25519_pubkey: [u8; 32],
        },

        // ----- MESSAGING EVENTS -----

        /// Encrypted message sent in escrow chat
        MessageSent {
            escrow_id: [u8; 32],
            sender: T::AccountId,
            sequence: u32,
        },

        // ----- AGENT EVENTS -----

        /// New escrow agent registered
        AgentRegistered {
            agent: T::AccountId,
            x25519_pubkey: [u8; 32],
            stake: BalanceOf<T>,
        },
        /// Agent deregistered
        AgentDeregistered {
            agent: T::AccountId,
        },
        /// Agent reputation updated
        AgentReputationUpdated {
            agent: T::AccountId,
            old_reputation: u64,
            new_reputation: u64,
            reason: ReputationChangeReason,
        },
        /// Cross-chain escrow created (awaiting buyer confirmation)
        EscrowCreated {
            escrow_id: [u8; 32],
            chain: TargetChain,
            seller: T::AccountId,
            buyer: T::AccountId,
            crypto_amount: u128,
            fiat_amount: u64,
            escrow_pubkey: [u8; 32],
        },
        /// Buyer confirmed share_B is valid, ready for funding attestation
        BuyerConfirmed {
            escrow_id: [u8; 32],
            buyer: T::AccountId,
        },
        /// Funding attested (external chain escrow confirmed funded)
        FundingAttested {
            escrow_id: [u8; 32],
            attestation: FundingAttestation,
        },
        /// Trade completed - winner can sweep from multisig
        TradeCompleted {
            escrow_id: [u8; 32],
            winner: T::AccountId,
        },
        /// Payment claimed by buyer
        PaymentClaimed {
            escrow_id: [u8; 32],
            deadline: u32,
        },
        /// Payment confirmed by seller (happy path)
        PaymentConfirmed {
            escrow_id: [u8; 32],
        },
        /// Dispute raised
        DisputeRaised {
            escrow_id: [u8; 32],
            raised_by: DisputeInitiator,
        },
        /// Arbitrator voted
        ArbitratorVoted {
            escrow_id: [u8; 32],
            arbitrator: T::AccountId,
            vote: DisputeVote,
        },
        /// Dispute resolved, chain will sign release tx
        DisputeResolved {
            escrow_id: [u8; 32],
            to_buyer: bool,
        },
        /// Chain signing request created for dispute resolution
        ChainSigningRequested {
            escrow_id: [u8; 32],
            request_id: u64,
            recipient: [u8; 32],
        },
        /// Chain signing completed
        ChainSigningCompleted {
            escrow_id: [u8; 32],
            request_id: u64,
            signature: [u8; 64],
        },
        /// Signing timeout
        SigningTimeout {
            escrow_id: [u8; 32],
        },
        /// Escrow cancelled
        EscrowCancelled {
            escrow_id: [u8; 32],
        },
        /// Escrow expired
        EscrowExpired {
            escrow_id: [u8; 32],
        },
        /// Arbitrator registered
        ArbitratorRegistered {
            who: T::AccountId,
            stake: BalanceOf<T>,
        },
        /// Arbitrator deregistered
        ArbitratorDeregistered {
            who: T::AccountId,
        },

        // ----- SHIELDED ESCROW EVENTS -----

        /// Shielded escrow created (no party identities visible)
        ShieldedEscrowCreated {
            commitment: [u8; 32],
            vss_commitment: [u8; 32],
            epoch: u32,
        },
        /// Shielded escrow state updated
        ShieldedEscrowUpdated {
            commitment: [u8; 32],
        },
        /// Shielded escrow consumed (completed or cancelled)
        ShieldedEscrowConsumed {
            nullifier: [u8; 32],
        },
        /// Shielded dispute raised
        ShieldedDisputeRaised {
            commitment: [u8; 32],
            epoch: u32,
        },
        /// Shielded dispute resolved
        ShieldedDisputeResolved {
            commitment: [u8; 32],
            to_party_a: bool, // true = party A (buyer), false = party B (seller)
        },
        /// Epoch updated
        EpochUpdated {
            old_epoch: u32,
            new_epoch: u32,
        },
        /// VSS share verified successfully
        VssShareVerified {
            escrow_commitment: [u8; 32],
            share_index: u8,
        },

        // ----- FROST SIGNING EVENTS -----

        /// FROST signing request created for dispute resolution
        FrostSigningRequested {
            escrow_commitment: [u8; 32],
            frost_request_id: u64,
            tx_hash: [u8; 32],
        },
        /// FROST signing completed
        FrostSigningCompleted {
            escrow_commitment: [u8; 32],
            signature_r: [u8; 32],
            signature_s: [u8; 32],
        },
        /// FROST signing failed
        FrostSigningFailed {
            escrow_commitment: [u8; 32],
            frost_request_id: u64,
        },

        // ----- TIMEOUT EVENTS -----

        /// Shielded escrow expired (timeout reached)
        ShieldedEscrowExpired {
            commitment: [u8; 32],
        },
        /// Shielded dispute timed out without resolution
        ShieldedDisputeTimedOut {
            escrow_commitment: [u8; 32],
        },
    }

    #[derive(
        Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq,
        codec::DecodeWithMemTracking,
    )]
    pub enum ReputationChangeReason {
        SuccessfulReveal,
        FailedReveal,
        EscrowCompleted,
    }

    // ========== ERRORS ==========

    #[pallet::error]
    pub enum Error<T> {
        // ----- TRADER ERRORS -----
        /// Already registered as trader
        AlreadyTrader,
        /// Not registered as trader
        NotTrader,
        /// Counterparty not registered as trader
        CounterpartyNotTrader,

        // ----- MESSAGING ERRORS -----
        /// Too many messages in this escrow
        TooManyMessages,
        /// Message too large
        MessageTooLarge,
        /// Not a party to this escrow
        NotEscrowParty,

        // ----- ESCROW ERRORS -----
        /// Escrow not found
        EscrowNotFound,
        /// Not authorized for this action
        NotAuthorized,
        /// Wrong escrow state for this action
        WrongState,
        /// Not enough stake
        InsufficientStake,
        /// Too many escrows
        TooManyEscrows,
        /// Invalid opening proof
        InvalidOpeningProof,
        /// Share already revealed
        AlreadyRevealed,
        /// Invalid share (doesn't match polynomial)
        InvalidShare,
        /// Reveal deadline passed
        RevealDeadlinePassed,
        /// Buyer confirmation timeout
        BuyerConfirmationTimeout,

        // ----- AGENT ERRORS -----
        /// Already registered as agent
        AlreadyAgent,
        /// Not an agent
        NotAgent,
        /// Agent not active
        AgentNotActive,
        /// Not an assigned agent for this escrow
        NotAssignedAgent,
        /// Not enough agents selected
        NotEnoughAgents,

        // ----- ARBITRATOR ERRORS -----
        /// Already registered as arbitrator
        AlreadyArbitrator,
        /// Not an arbitrator
        NotArbitrator,
        /// Already voted
        AlreadyVoted,

        // ----- CHAIN KEY ERRORS -----
        /// No chain master key configured
        NoChainKey,
        /// Insufficient service deposit
        InsufficientDeposit,

        // ----- SHIELDED ESCROW ERRORS -----
        /// Shielded escrow not found
        ShieldedEscrowNotFound,
        /// Invalid ring signature
        InvalidRingSignature,
        /// Nullifier already spent
        NullifierAlreadySpent,
        /// Invalid nullifier (doesn't match commitment)
        InvalidNullifier,
        /// Too many shielded disputes pending
        TooManyShieldedDisputes,
        /// Invalid VSS share proof
        InvalidVssProof,
        /// Escrow already exists with this commitment
        CommitmentAlreadyExists,
        /// Invalid encrypted data size
        InvalidEncryptedDataSize,

        // ----- FROST SIGNING ERRORS -----

        /// FROST signing request not found
        FrostRequestNotFound,
        /// FROST signing already in progress
        FrostSigningInProgress,
        /// Dispute not in resolved state
        DisputeNotResolved,
        /// No pending dispute for this escrow
        NoPendingDispute,
    }

    // ========== CALLS ==========

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        // ========== TRADER PROFILE MANAGEMENT (LocalCryptos-style) ==========

        /// Register as a trader (buyer/seller)
        ///
        /// This is required to participate in P2P trades. The X25519 pubkey
        /// enables E2E encrypted messaging with trade counterparties.
        ///
        /// NOTE: This is different from agent registration. Traders are
        /// buyers/sellers, agents are escrow key holders.
        #[pallet::call_index(50)]
        #[pallet::weight(Weight::from_parts(30_000_000, 0))]
        pub fn register_trader(
            origin: OriginFor<T>,
            x25519_pubkey: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(!Traders::<T>::contains_key(&who), Error::<T>::AlreadyTrader);

            let now = frame_system::Pallet::<T>::block_number();

            let profile = TraderProfile {
                x25519_pubkey,
                registered_at: now,
                total_trades: 0,
                successful_trades: 0,
                disputes_lost: 0,
                avg_completion_blocks: 0,
            };

            Traders::<T>::insert(&who, profile);
            TraderCount::<T>::mutate(|c| *c += 1);

            Self::deposit_event(Event::TraderRegistered {
                trader: who,
                x25519_pubkey,
            });

            Ok(())
        }

        /// Update trader X25519 public key
        ///
        /// Use this if you need to rotate your messaging key.
        #[pallet::call_index(51)]
        #[pallet::weight(Weight::from_parts(20_000_000, 0))]
        pub fn update_trader_key(
            origin: OriginFor<T>,
            new_x25519_pubkey: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Traders::<T>::try_mutate(&who, |maybe_profile| -> DispatchResult {
                let profile = maybe_profile.as_mut().ok_or(Error::<T>::NotTrader)?;
                profile.x25519_pubkey = new_x25519_pubkey;
                Ok(())
            })?;

            Self::deposit_event(Event::TraderUpdated {
                trader: who,
                x25519_pubkey: new_x25519_pubkey,
            });

            Ok(())
        }

        // ========== ENCRYPTED MESSAGING (LocalCryptos-style E2E Chat) ==========

        /// Send encrypted message in escrow trade chat
        ///
        /// Only buyer and seller can send messages. Messages are encrypted
        /// client-side using X25519+ChaCha20Poly1305:
        ///
        /// 1. Sender looks up recipient's X25519 pubkey from TraderProfile
        /// 2. Sender generates ephemeral X25519 keypair
        /// 3. ECDH: shared_secret = ephemeral_sk * recipient_pk
        /// 4. KDF: key = Blake2b(personalization || recipient_pk || epk || shared_secret)
        /// 5. Encrypt: ChaCha20Poly1305(key, random_nonce, message)
        /// 6. Format: EPK (32) || Nonce (12) || Tag (16) || Ciphertext
        ///
        /// Typical messages:
        /// - Seller: "Send $100 to Bank XYZ, Account 12345, Reference: TRADE-ABC"
        /// - Buyer: "Payment sent via Wise, transaction ID: TX123456789"
        #[pallet::call_index(52)]
        #[pallet::weight(Weight::from_parts(40_000_000, 0))]
        pub fn send_message(
            origin: OriginFor<T>,
            escrow_id: [u8; 32],
            encrypted_message: BoundedVec<u8, ConstU32<1024>>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let escrow = Escrows::<T>::get(escrow_id).ok_or(Error::<T>::EscrowNotFound)?;

            // Only buyer or seller can send messages
            ensure!(
                who == escrow.buyer || who == escrow.seller,
                Error::<T>::NotEscrowParty
            );

            // Get current sequence number
            let sequence = MessageCount::<T>::get(escrow_id);

            let now = frame_system::Pallet::<T>::block_number();

            let message = EncryptedMessage {
                sender: who.clone(),
                ciphertext: encrypted_message,
                sent_at: now,
                sequence,
            };

            // Store message
            EscrowMessages::<T>::try_mutate(escrow_id, |messages| {
                messages.try_push(message).map_err(|_| Error::<T>::TooManyMessages)
            })?;

            // Increment sequence
            MessageCount::<T>::insert(escrow_id, sequence + 1);

            Self::deposit_event(Event::MessageSent {
                escrow_id,
                sender: who,
                sequence,
            });

            Ok(())
        }

        // ========== AGENT MANAGEMENT ==========

        /// Register as an escrow agent
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(50_000_000, 0))]
        pub fn register_agent(
            origin: OriginFor<T>,
            x25519_pubkey: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(!Agents::<T>::contains_key(&who), Error::<T>::AlreadyAgent);

            let stake = T::MinAgentStake::get();
            T::Currency::reserve(&who, stake)?;

            let now = frame_system::Pallet::<T>::block_number();

            let agent = EscrowAgent {
                account: who.clone(),
                x25519_pubkey,
                stake,
                registered_at: now,
                active: true,
                reputation: 0,
                total_escrows: 0,
                successful_reveals: 0,
                failed_reveals: 0,
                total_bounties_earned: Zero::zero(),
                avg_response_blocks: 0,
            };

            Agents::<T>::insert(&who, agent);
            AgentCount::<T>::mutate(|c| *c += 1);

            Self::update_leaderboard(&who, 0);

            Self::deposit_event(Event::AgentRegistered {
                agent: who,
                x25519_pubkey,
                stake,
            });

            Ok(())
        }

        /// Deregister as an escrow agent
        ///
        /// Note: Agents are now optional - the chain acts as the primary arbitrator
        /// via FROST threshold signing. Agents can still be used for additional
        /// reputation/marketplace features.
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(50_000_000, 0))]
        pub fn deregister_agent(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let agent = Agents::<T>::get(&who).ok_or(Error::<T>::NotAgent)?;

            // Just check agent is not currently active in any critical role
            ensure!(!agent.active || agent.total_escrows == agent.successful_reveals + agent.failed_reveals,
                Error::<T>::WrongState);

            T::Currency::unreserve(&who, agent.stake);
            Agents::<T>::remove(&who);
            AgentCount::<T>::mutate(|c| *c = c.saturating_sub(1));

            Self::remove_from_leaderboard(&who);

            Self::deposit_event(Event::AgentDeregistered { agent: who });

            Ok(())
        }

        /// Update agent's X25519 public key
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(30_000_000, 0))]
        pub fn update_agent_key(
            origin: OriginFor<T>,
            new_x25519_pubkey: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Agents::<T>::try_mutate(&who, |maybe_agent| {
                let agent = maybe_agent.as_mut().ok_or(Error::<T>::NotAgent)?;
                agent.x25519_pubkey = new_x25519_pubkey;
                Ok(())
            })
        }

        // ========== ESCROW LIFECYCLE ==========

        /// Create cross-chain escrow for fiat→crypto trade
        ///
        /// Creates a 2-of-3 multisig escrow where:
        /// - Buyer provides their external chain pubkey
        /// - Seller provides their external chain pubkey
        /// - Chain derives per-escrow pubkey from master FROST key
        ///
        /// The escrow address is computed from all 3 keys.
        /// Seller then funds this address on the external chain.
        #[pallet::call_index(10)]
        #[pallet::weight(Weight::from_parts(100_000_000, 0))]
        pub fn create_escrow(
            origin: OriginFor<T>,
            // Target chain and amount
            chain: TargetChain,
            buyer: T::AccountId,
            crypto_amount: u128,
            asset_id: Option<[u8; 32]>, // For Penumbra multi-asset
            // Fiat side
            fiat_amount: u64,
            fiat_currency: [u8; 3],
            payment_method: PaymentMethod,
            // 2/3 Multisig keys
            buyer_escrow_pubkey: [u8; 32],
            seller_escrow_pubkey: [u8; 32],
            // Service deposit for chain signing
            chain_service_deposit: BalanceOf<T>,
        ) -> DispatchResult {
            let seller = ensure_signed(origin)?;

            // Verify both parties are registered traders
            ensure!(Traders::<T>::contains_key(&seller), Error::<T>::NotTrader);
            ensure!(Traders::<T>::contains_key(&buyer), Error::<T>::CounterpartyNotTrader);

            // Require minimum service deposit
            ensure!(
                chain_service_deposit >= T::MinChainServiceDeposit::get(),
                Error::<T>::InsufficientDeposit
            );

            // Reserve service deposit (returned on happy path, pays for chain signing on dispute)
            T::Currency::reserve(&seller, chain_service_deposit)?;

            // Get chain master key
            let master_key = ChainMasterKey::<T>::get().ok_or(Error::<T>::NoChainKey)?;

            let nonce = EscrowNonce::<T>::get();
            EscrowNonce::<T>::put(nonce + 1);

            let escrow_id = Self::compute_escrow_id(&seller, &buyer, nonce, &seller_escrow_pubkey);

            // Derive chain's per-escrow key from master key
            let chain_escrow_pubkey = Self::derive_escrow_key(&master_key.master_public_key, &escrow_id);

            // Compute 2/3 multisig address from all 3 keys
            let escrow_address = Self::compute_multisig_address(
                &chain,
                &buyer_escrow_pubkey,
                &seller_escrow_pubkey,
                &chain_escrow_pubkey,
            );

            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(0);

            let escrow = Escrow {
                id: escrow_id,
                chain: chain.clone(),
                buyer: buyer.clone(),
                seller: seller.clone(),
                crypto_amount,
                asset_id,
                fiat_amount,
                fiat_currency,
                payment_method,
                state: EscrowState::AwaitingBuyerConfirmation,
                buyer_escrow_pubkey,
                seller_escrow_pubkey,
                chain_escrow_pubkey,
                escrow_address,
                chain_service_deposit,
                created_at: now,
                funding_deadline: now + T::DefaultFundingDeadline::get(),
                payment_timeout: T::DefaultPaymentTimeout::get(),
                funding_attestation: None,
                signing_request_id: None,
            };

            Escrows::<T>::insert(escrow_id, escrow);

            EscrowsBySeller::<T>::try_mutate(&seller, |list| {
                list.try_push(escrow_id).map_err(|_| Error::<T>::TooManyEscrows)
            })?;

            EscrowsByBuyer::<T>::try_mutate(&buyer, |list| {
                list.try_push(escrow_id).map_err(|_| Error::<T>::TooManyEscrows)
            })?;

            Self::deposit_event(Event::EscrowCreated {
                escrow_id,
                chain,
                seller,
                buyer,
                crypto_amount,
                fiat_amount,
                escrow_pubkey: escrow_address,
            });

            Ok(())
        }

        /// Buyer confirms share_B is valid
        ///
        /// Buyer has received share_B off-chain and verified it opens
        /// correctly against poly_commitment at index 2. This confirms
        /// the VSS setup is correct and trade can proceed.
        ///
        /// Next step: wait for funding attestation or proceed to payment.
        #[pallet::call_index(11)]
        #[pallet::weight(Weight::from_parts(30_000_000, 0))]
        pub fn confirm_share(
            origin: OriginFor<T>,
            escrow_id: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Escrows::<T>::try_mutate(escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;
                ensure!(who == escrow.buyer, Error::<T>::NotAuthorized);
                ensure!(
                    escrow.state == EscrowState::AwaitingBuyerConfirmation,
                    Error::<T>::WrongState
                );

                // Move to AwaitingFunding - waiting for external chain funding attestation
                escrow.state = EscrowState::AwaitingFunding;

                Self::deposit_event(Event::BuyerConfirmed {
                    escrow_id,
                    buyer: who,
                });

                Ok(())
            })
        }

        /// Submit funding attestation for external chain escrow
        ///
        /// After buyer confirms share, seller (or oracle) attests that the
        /// escrow address on Zcash/Penumbra has been funded.
        #[pallet::call_index(12)]
        #[pallet::weight(Weight::from_parts(30_000_000, 0))]
        pub fn attest_funding(
            origin: OriginFor<T>,
            escrow_id: [u8; 32],
            attestation: FundingAttestation,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Escrows::<T>::try_mutate(escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;
                // Seller or designated oracle can attest
                ensure!(who == escrow.seller, Error::<T>::NotAuthorized);
                ensure!(escrow.state == EscrowState::AwaitingFunding, Error::<T>::WrongState);

                escrow.funding_attestation = Some(attestation.clone());
                escrow.state = EscrowState::AwaitingPayment;

                Self::deposit_event(Event::FundingAttested {
                    escrow_id,
                    attestation,
                });

                Ok(())
            })
        }

        /// Buyer marks fiat payment as sent
        ///
        /// After this, the seller should verify the fiat payment arrived
        /// and call confirm_payment to release crypto to buyer.
        #[pallet::call_index(13)]
        #[pallet::weight(Weight::from_parts(20_000_000, 0))]
        pub fn mark_paid(origin: OriginFor<T>, escrow_id: [u8; 32]) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Escrows::<T>::try_mutate(escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;
                ensure!(who == escrow.buyer, Error::<T>::NotAuthorized);
                ensure!(escrow.state == EscrowState::AwaitingPayment, Error::<T>::WrongState);

                let now: u32 = frame_system::Pallet::<T>::block_number()
                    .try_into()
                    .unwrap_or(0);
                let deadline = now + escrow.payment_timeout;

                escrow.state = EscrowState::PaymentClaimed { claimed_at: now };

                Self::deposit_event(Event::PaymentClaimed { escrow_id, deadline });

                Ok(())
            })
        }

        /// Seller confirms fiat payment received - happy path completion
        ///
        /// For cross-chain escrow:
        /// 1. Seller confirms fiat received
        /// 2. Seller sends share_A to buyer OFF-CHAIN
        /// 3. Buyer reconstructs secret: interpolate(share_A, share_B) = P(0)
        /// 4. Buyer derives private key from P(0), sweeps Zcash/Penumbra escrow
        ///
        /// The bounty is returned to seller (no dispute needed).
        #[pallet::call_index(14)]
        #[pallet::weight(Weight::from_parts(50_000_000, 0))]
        pub fn confirm_payment(origin: OriginFor<T>, escrow_id: [u8; 32]) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Escrows::<T>::try_mutate(escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;
                ensure!(who == escrow.seller, Error::<T>::NotAuthorized);
                ensure!(
                    matches!(escrow.state, EscrowState::PaymentClaimed { .. }),
                    Error::<T>::WrongState
                );

                let buyer = escrow.buyer.clone();
                let seller = escrow.seller.clone();
                let created_at = escrow.created_at;

                escrow.state = EscrowState::ReleasedToBuyer;

                // Return service deposit to seller (no chain signing needed for happy path)
                T::Currency::unreserve(&seller, escrow.chain_service_deposit);

                // Update trader stats for both parties
                let now: u32 = frame_system::Pallet::<T>::block_number()
                    .try_into()
                    .unwrap_or(0);
                Self::update_trader_successful_trade(&buyer, created_at, now);
                Self::update_trader_successful_trade(&seller, created_at, now);

                Self::deposit_event(Event::TradeCompleted {
                    escrow_id,
                    winner: buyer,
                });

                Ok(())
            })
        }

        /// Raise a dispute
        #[pallet::call_index(15)]
        #[pallet::weight(Weight::from_parts(25_000_000, 0))]
        pub fn raise_dispute(origin: OriginFor<T>, escrow_id: [u8; 32]) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Escrows::<T>::try_mutate(escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;

                let raised_by = if who == escrow.buyer {
                    DisputeInitiator::Buyer
                } else if who == escrow.seller {
                    DisputeInitiator::Seller
                } else {
                    return Err(Error::<T>::NotAuthorized.into());
                };

                ensure!(
                    matches!(escrow.state, EscrowState::PaymentClaimed { .. }),
                    Error::<T>::WrongState
                );

                let now: u32 = frame_system::Pallet::<T>::block_number()
                    .try_into()
                    .unwrap_or(0);

                escrow.state = EscrowState::Disputed {
                    raised_at: now,
                    raised_by: raised_by.clone(),
                };

                Self::deposit_event(Event::DisputeRaised { escrow_id, raised_by });

                Ok(())
            })
        }

        /// Arbitrator votes on dispute
        #[pallet::call_index(16)]
        #[pallet::weight(Weight::from_parts(30_000_000, 0))]
        pub fn vote_dispute(
            origin: OriginFor<T>,
            escrow_id: [u8; 32],
            vote: DisputeVote,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(Arbitrators::<T>::contains_key(&who), Error::<T>::NotArbitrator);

            let escrow = Escrows::<T>::get(escrow_id).ok_or(Error::<T>::EscrowNotFound)?;
            ensure!(
                matches!(escrow.state, EscrowState::Disputed { .. }),
                Error::<T>::WrongState
            );
            ensure!(
                !DisputeVotes::<T>::contains_key(escrow_id, &who),
                Error::<T>::AlreadyVoted
            );

            DisputeVotes::<T>::insert(escrow_id, &who, vote.clone());

            VoteCounts::<T>::mutate(escrow_id, |(buyer, seller, abstain)| match vote {
                DisputeVote::Buyer => *buyer += 1,
                DisputeVote::Seller => *seller += 1,
                DisputeVote::Abstain => *abstain += 1,
            });

            Self::deposit_event(Event::ArbitratorVoted {
                escrow_id,
                arbitrator: who,
                vote,
            });

            Self::try_resolve_dispute(escrow_id)?;

            Ok(())
        }

        // ========== CHAIN SIGNING (for dispute resolution) ==========

        /// Submit chain signing completion (called by off-chain worker or oracle)
        ///
        /// After dispute resolution, validators coordinate FROST signing.
        /// This submits the completed signature.
        #[pallet::call_index(20)]
        #[pallet::weight(Weight::from_parts(100_000_000, 0))]
        pub fn submit_chain_signature(
            origin: OriginFor<T>,
            request_id: u64,
            signature: [u8; 64],
        ) -> DispatchResult {
            // TODO: Verify origin is authorized (OCW or threshold of validators)
            let _who = ensure_signed(origin)?;

            let mut request = SigningRequests::<T>::get(request_id)
                .ok_or(Error::<T>::EscrowNotFound)?;

            ensure!(
                matches!(request.status, SigningRequestStatus::Pending | SigningRequestStatus::InProgress { .. }),
                Error::<T>::WrongState
            );

            // Update signing request
            request.status = SigningRequestStatus::Completed { signature };
            SigningRequests::<T>::insert(request_id, request.clone());

            // Update escrow state
            Escrows::<T>::try_mutate(request.escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;

                let (to_buyer, _) = match escrow.state {
                    EscrowState::PendingReveal { to_buyer, decided_at } => (to_buyer, decided_at),
                    _ => return Err(Error::<T>::WrongState.into()),
                };

                let buyer = escrow.buyer.clone();
                let seller = escrow.seller.clone();
                let created_at = escrow.created_at;

                let winner = if to_buyer {
                    escrow.state = EscrowState::ReleasedToBuyer;
                    buyer.clone()
                } else {
                    escrow.state = EscrowState::ReleasedToSeller;
                    seller.clone()
                };

                // Service deposit goes to chain (for signing work)
                // In production: distribute to validators who participated in signing
                T::Currency::unreserve(&seller, escrow.chain_service_deposit);

                let now: u32 = frame_system::Pallet::<T>::block_number()
                    .try_into()
                    .unwrap_or(0);

                // Update trader stats
                if to_buyer {
                    Self::update_trader_dispute_lost(&seller);
                    Self::update_trader_successful_trade(&buyer, created_at, now);
                } else {
                    Self::update_trader_dispute_lost(&buyer);
                    Self::update_trader_successful_trade(&seller, created_at, now);
                }

                // Remove from pending
                PendingSigningRequests::<T>::mutate(|list| {
                    list.retain(|id| *id != request_id);
                });

                Self::deposit_event(Event::ChainSigningCompleted {
                    escrow_id: request.escrow_id,
                    request_id,
                    signature,
                });

                Self::deposit_event(Event::TradeCompleted {
                    escrow_id: request.escrow_id,
                    winner,
                });

                Ok(())
            })
        }

        // ========== ARBITRATOR MANAGEMENT ==========

        /// Register as arbitrator
        #[pallet::call_index(30)]
        #[pallet::weight(Weight::from_parts(40_000_000, 0))]
        pub fn register_arbitrator(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(
                !Arbitrators::<T>::contains_key(&who),
                Error::<T>::AlreadyArbitrator
            );

            let stake = T::MinArbitratorStake::get();
            T::Currency::reserve(&who, stake)?;

            Arbitrators::<T>::insert(
                &who,
                ArbitratorInfo {
                    stake,
                    registered_at: frame_system::Pallet::<T>::block_number(),
                    disputes_resolved: 0,
                    correct_votes: 0,
                    active: true,
                },
            );

            ArbitratorCount::<T>::mutate(|c| *c += 1);

            Self::deposit_event(Event::ArbitratorRegistered { who, stake });

            Ok(())
        }

        /// Deregister as arbitrator
        #[pallet::call_index(31)]
        #[pallet::weight(Weight::from_parts(40_000_000, 0))]
        pub fn deregister_arbitrator(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let info = Arbitrators::<T>::get(&who).ok_or(Error::<T>::NotArbitrator)?;

            T::Currency::unreserve(&who, info.stake);
            Arbitrators::<T>::remove(&who);
            ArbitratorCount::<T>::mutate(|c| *c = c.saturating_sub(1));

            Self::deposit_event(Event::ArbitratorDeregistered { who });

            Ok(())
        }

        /// Cancel escrow (seller only, before buyer accepts)
        ///
        /// Seller can cancel if buyer hasn't accepted yet.
        /// All locked funds are returned to seller.
        #[pallet::call_index(40)]
        #[pallet::weight(Weight::from_parts(30_000_000, 0))]
        pub fn cancel_escrow(origin: OriginFor<T>, escrow_id: [u8; 32]) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Escrows::<T>::try_mutate(escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;
                ensure!(who == escrow.seller, Error::<T>::NotAuthorized);
                // Can only cancel before buyer accepts
                ensure!(
                    escrow.state == EscrowState::AwaitingBuyerConfirmation,
                    Error::<T>::WrongState
                );

                escrow.state = EscrowState::Cancelled;

                // Return service deposit to seller
                T::Currency::unreserve(&escrow.seller, escrow.chain_service_deposit);

                Self::deposit_event(Event::EscrowCancelled { escrow_id });

                Ok(())
            })
        }

        // ========== SHIELDED ESCROW EXTRINSICS ==========
        //
        // These extrinsics hide all trade details from chain observers.
        // No AccountIds, amounts, or addresses are stored in cleartext.

        /// Create shielded escrow
        ///
        /// Submitter identity is NOT linked to escrow on-chain.
        /// Commitment hides all trade parameters.
        /// Encrypted data can only be read by parties who know the shared secret.
        ///
        /// No party identities visible - anyone can submit a shielded escrow.
        #[pallet::call_index(60)]
        #[pallet::weight(Weight::from_parts(80_000_000, 0))]
        pub fn shielded_create(
            origin: OriginFor<T>,
            escrow: super::ShieldedEscrowV1,
            vss_commitment: super::VssCommitment,
        ) -> DispatchResult {
            // Origin check - we accept any signed origin but don't link it to escrow
            let _who = ensure_signed(origin)?;

            // Verify escrow doesn't already exist
            ensure!(
                !ShieldedEscrows::<T>::contains_key(&escrow.commitment),
                Error::<T>::CommitmentAlreadyExists
            );

            // Verify encrypted data size
            ensure!(
                escrow.encrypted_data.len() == 512,
                Error::<T>::InvalidEncryptedDataSize
            );

            // Get current epoch
            let epoch = CurrentEpoch::<T>::get();

            // Store shielded escrow
            ShieldedEscrows::<T>::insert(&escrow.commitment, escrow.clone());

            // Store VSS commitment (for share verification)
            VssCommitments::<T>::insert(&escrow.commitment, vss_commitment.clone());

            Self::deposit_event(Event::ShieldedEscrowCreated {
                commitment: escrow.commitment,
                vss_commitment: vss_commitment.root,
                epoch,
            });

            Ok(())
        }

        /// Update shielded escrow state
        ///
        /// Updates encrypted data (state change is hidden inside).
        /// Authorization proves caller is a party without revealing which one.
        #[pallet::call_index(61)]
        #[pallet::weight(Weight::from_parts(60_000_000, 0))]
        pub fn shielded_update(
            origin: OriginFor<T>,
            commitment: [u8; 32],
            new_encrypted_data: [u8; 512],
            authorization: [u8; 64],
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            ShieldedEscrows::<T>::try_mutate(&commitment, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::ShieldedEscrowNotFound)?;

                // Verify authorization (simplified - full impl needs proper sig check)
                // The authorization is a Schnorr signature over H(commitment || new_encrypted_data)
                // using a key derived from escrow_secret
                ensure!(authorization != [0u8; 64], Error::<T>::NotAuthorized);

                // Update encrypted data
                escrow.encrypted_data = new_encrypted_data;

                Self::deposit_event(Event::ShieldedEscrowUpdated { commitment });

                Ok(())
            })
        }

        /// Consume shielded escrow (complete or cancel)
        ///
        /// Reveals nullifier to mark escrow as spent.
        /// Chain verifies H(nullifier) matches stored nullifier_commitment.
        /// Cannot link nullifier to original commitment (unlinkability).
        #[pallet::call_index(62)]
        #[pallet::weight(Weight::from_parts(70_000_000, 0))]
        pub fn shielded_consume(
            origin: OriginFor<T>,
            commitment: [u8; 32],
            nullifier: [u8; 32],
            final_encrypted_data: [u8; 512],
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            // Check nullifier not already spent
            ensure!(
                !NullifierSet::<T>::contains_key(&nullifier),
                Error::<T>::NullifierAlreadySpent
            );

            // Get escrow and verify nullifier matches
            let escrow = ShieldedEscrows::<T>::get(&commitment)
                .ok_or(Error::<T>::ShieldedEscrowNotFound)?;

            // Verify: H(nullifier) == nullifier_commitment
            let computed_commitment = super::compute_nullifier_commitment(&nullifier);
            ensure!(
                computed_commitment == escrow.nullifier_commitment,
                Error::<T>::InvalidNullifier
            );

            // Mark nullifier as spent
            NullifierSet::<T>::insert(&nullifier, ());

            // Update escrow with final state
            ShieldedEscrows::<T>::mutate(&commitment, |maybe_escrow| {
                if let Some(e) = maybe_escrow {
                    e.encrypted_data = final_encrypted_data;
                }
            });

            Self::deposit_event(Event::ShieldedEscrowConsumed { nullifier });

            Ok(())
        }

        /// Raise shielded dispute
        ///
        /// Evidence is encrypted to arbitrator threshold key.
        /// Ring signature proves submitter is buyer or seller without revealing which.
        /// Arbitrators can decrypt evidence but cannot link to parachain identities.
        #[pallet::call_index(63)]
        #[pallet::weight(Weight::from_parts(100_000_000, 0))]
        pub fn shielded_dispute(
            origin: OriginFor<T>,
            commitment: [u8; 32],
            encrypted_evidence: BoundedVec<u8, ConstU32<2048>>,
            ring_signature: super::RingSignature,
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            // Verify escrow exists
            ensure!(
                ShieldedEscrows::<T>::contains_key(&commitment),
                Error::<T>::ShieldedEscrowNotFound
            );

            // Verify ring signature (proves submitter is buyer or seller)
            let message = [&commitment[..], &encrypted_evidence[..]].concat();
            ensure!(
                ring_signature.verify(&message),
                Error::<T>::InvalidRingSignature
            );

            let epoch = CurrentEpoch::<T>::get();

            // Create dispute info
            let dispute_info = super::ShieldedDisputeInfo {
                escrow_commitment: commitment,
                encrypted_evidence,
                authorization: ring_signature,
                raised_epoch: epoch,
                deadline_epoch: epoch + 10, // 10 epochs for resolution
            };

            // Add to dispute queue
            ShieldedDisputeQueue::<T>::try_mutate(|queue| {
                queue.try_push(dispute_info).map_err(|_| Error::<T>::TooManyShieldedDisputes)
            })?;

            Self::deposit_event(Event::ShieldedDisputeRaised { commitment, epoch });

            Ok(())
        }

        /// Verify VSS share against commitment
        ///
        /// Allows any party to verify that a share is valid (matches the VSS commitment).
        /// This is critical for detecting dealer cheating - if the dealer gave you a
        /// share that doesn't verify, you should reject the escrow setup.
        ///
        /// This is a read-only verification - it doesn't modify state, just confirms
        /// the share is valid. In practice, verification happens client-side, but
        /// this extrinsic allows on-chain verification if needed for dispute resolution.
        #[pallet::call_index(64)]
        #[pallet::weight(Weight::from_parts(50_000_000, 0))]
        pub fn verify_vss_share(
            origin: OriginFor<T>,
            escrow_commitment: [u8; 32],
            share: super::VerifiableShare,
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            // Get VSS commitment for this escrow
            let vss_commitment = VssCommitments::<T>::get(&escrow_commitment)
                .ok_or(Error::<T>::ShieldedEscrowNotFound)?;

            // Verify share against commitment
            #[cfg(feature = "shielded-escrow")]
            {
                ensure!(
                    super::verify_share(&share, &vss_commitment),
                    Error::<T>::InvalidVssProof
                );
            }

            #[cfg(not(feature = "shielded-escrow"))]
            {
                // Fallback verification for no_std without shielded-escrow feature
                // Uses sp_io::hashing::sha2_256
                use sp_io::hashing::sha2_256;

                // Check index bounds
                ensure!(
                    share.index < vss_commitment.num_shares,
                    Error::<T>::InvalidVssProof
                );

                // Compute leaf hash
                let leaf_hash = sha2_256(&share.value);

                // Verify Merkle proof
                let mut current = leaf_hash;
                let mut idx = share.index as usize;

                for sibling in share.merkle_proof.iter() {
                    let mut data = [0u8; 64];
                    if idx % 2 == 0 {
                        data[0..32].copy_from_slice(&current);
                        data[32..64].copy_from_slice(sibling);
                    } else {
                        data[0..32].copy_from_slice(sibling);
                        data[32..64].copy_from_slice(&current);
                    }
                    current = sha2_256(&data);
                    idx /= 2;
                }

                ensure!(
                    current == vss_commitment.root,
                    Error::<T>::InvalidVssProof
                );
            }

            // Emit event for successful verification
            Self::deposit_event(Event::VssShareVerified {
                escrow_commitment,
                share_index: share.index,
            });

            Ok(())
        }

        // ======= FROST SIGNING FOR DISPUTE RESOLUTION =======

        /// Resolve dispute and initiate FROST signing for release
        ///
        /// Called by governance/arbitrator collective after reviewing evidence.
        /// Creates a signing request for the release transaction.
        ///
        /// # Privacy
        /// - Chain doesn't learn who wins (encrypted_recipient)
        /// - Chain doesn't learn amount (tx_data is opaque)
        /// - Only knows: this escrow resolved, someone gets funds
        #[pallet::call_index(65)]
        #[pallet::weight(Weight::from_parts(100_000_000, 0))]
        pub fn resolve_dispute_with_frost(
            origin: OriginFor<T>,
            escrow_commitment: [u8; 32],
            // Transaction data for FROST signing (release tx)
            tx_data: [u8; 256],
            // Recipient address encrypted to winner's key
            encrypted_recipient: [u8; 64],
        ) -> DispatchResult {
            // Only root/governance can resolve disputes
            ensure_root(origin)?;

            // Verify escrow exists
            ensure!(
                ShieldedEscrows::<T>::contains_key(&escrow_commitment),
                Error::<T>::ShieldedEscrowNotFound
            );

            // Verify no signing already in progress
            ensure!(
                !FrostSigningRequests::<T>::contains_key(&escrow_commitment),
                Error::<T>::FrostSigningInProgress
            );

            // Compute tx hash for signing
            let tx_hash = sp_io::hashing::sha2_256(&tx_data);

            // Get next request ID
            let frost_request_id = NextFrostRequestId::<T>::get();
            NextFrostRequestId::<T>::put(frost_request_id + 1);

            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(0);

            // Create signing request
            let signing_request = super::ShieldedSigningRequest {
                frost_request_id,
                escrow_commitment,
                encrypted_recipient,
                tx_hash,
                status: super::ShieldedSigningStatus::Pending,
                created_at: now,
            };

            FrostSigningRequests::<T>::insert(&escrow_commitment, signing_request);

            // Emit event
            Self::deposit_event(Event::FrostSigningRequested {
                escrow_commitment,
                frost_request_id,
                tx_hash,
            });

            // Note: The actual FROST signing happens via frost-bridge pallet
            // OCW will pick up pending requests and coordinate signing

            Ok(())
        }

        /// Submit completed FROST signature
        ///
        /// Called by OCW or validator when FROST signing completes.
        /// Updates the signing request status and emits completion event.
        #[pallet::call_index(66)]
        #[pallet::weight(Weight::from_parts(50_000_000, 0))]
        pub fn submit_frost_signature(
            origin: OriginFor<T>,
            escrow_commitment: [u8; 32],
            signature: super::FrostSignature,
        ) -> DispatchResult {
            // Allow signed origin (from OCW or validators)
            let _who = ensure_signed(origin)?;

            // Get and update signing request
            let mut request = FrostSigningRequests::<T>::get(&escrow_commitment)
                .ok_or(Error::<T>::FrostRequestNotFound)?;

            // Update status
            request.status = super::ShieldedSigningStatus::Completed {
                signature: signature.clone(),
            };

            FrostSigningRequests::<T>::insert(&escrow_commitment, request);

            // Mark escrow as consumed (add to nullifier set)
            // The nullifier is revealed when the winner claims the funds
            // For now, we just emit the completion event

            Self::deposit_event(Event::FrostSigningCompleted {
                escrow_commitment,
                signature_r: signature.r,
                signature_s: signature.s,
            });

            // Emit dispute resolved event
            Self::deposit_event(Event::ShieldedDisputeResolved {
                commitment: escrow_commitment,
                to_party_a: true, // Encrypted - actual recipient unknown to chain
            });

            Ok(())
        }

        /// Mark FROST signing as failed
        ///
        /// Called when signing times out or fails.
        #[pallet::call_index(67)]
        #[pallet::weight(Weight::from_parts(30_000_000, 0))]
        pub fn mark_frost_signing_failed(
            origin: OriginFor<T>,
            escrow_commitment: [u8; 32],
        ) -> DispatchResult {
            // Only root can mark as failed
            ensure_root(origin)?;

            let mut request = FrostSigningRequests::<T>::get(&escrow_commitment)
                .ok_or(Error::<T>::FrostRequestNotFound)?;

            let frost_request_id = request.frost_request_id;

            request.status = super::ShieldedSigningStatus::Failed;
            FrostSigningRequests::<T>::insert(&escrow_commitment, request);

            Self::deposit_event(Event::FrostSigningFailed {
                escrow_commitment,
                frost_request_id,
            });

            Ok(())
        }
    }

    // ========== HOOKS ==========

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Offchain worker: coordinate FROST signing with frost-bridge pallet
        fn offchain_worker(_block_number: BlockNumberFor<T>) {
            // Process pending shielded signing requests
            Self::ocw_process_shielded_signing();

            // Process threshold decryption for disputes
            Self::ocw_process_decryption_sessions();
        }

        fn on_finalize(n: BlockNumberFor<T>) {
            let now: u32 = n.try_into().unwrap_or(0);
            Self::process_signing_timeouts(now);

            // Process shielded escrow timeouts
            Self::process_shielded_timeouts(now);

            // Process dispute resolution timeouts
            Self::process_dispute_timeouts(now);

            // Update epoch for coarse-grained timing (privacy feature)
            Self::maybe_update_epoch(now);
        }
    }

    // ========== INTERNAL ==========

    impl<T: Config> Pallet<T> {
        fn compute_escrow_id(
            seller: &T::AccountId,
            buyer: &T::AccountId,
            nonce: u64,
            escrow_pubkey: &[u8; 32],
        ) -> [u8; 32] {
            use sp_io::hashing::blake2_256;
            let mut data = Vec::new();
            data.extend_from_slice(&seller.encode());
            data.extend_from_slice(&buyer.encode());
            data.extend_from_slice(&nonce.to_le_bytes());
            data.extend_from_slice(escrow_pubkey);
            blake2_256(&data)
        }

        /// Derive per-escrow chain key from master FROST key
        fn derive_escrow_key(master_pk: &[u8; 32], escrow_id: &[u8; 32]) -> [u8; 32] {
            use sp_io::hashing::blake2_256;
            let mut data = Vec::new();
            data.extend_from_slice(master_pk);
            data.extend_from_slice(escrow_id);
            data.extend_from_slice(b"escrow-key");
            blake2_256(&data)
        }

        /// Compute 2/3 multisig address from buyer, seller, and chain keys
        fn compute_multisig_address(
            chain: &TargetChain,
            buyer_pk: &[u8; 32],
            seller_pk: &[u8; 32],
            chain_pk: &[u8; 32],
        ) -> [u8; 32] {
            use sp_io::hashing::blake2_256;
            // In real impl: chain-specific multisig address derivation
            // For Zcash: t-address from combined pubkeys
            // For Penumbra: address from combined spend auth keys
            let mut data = Vec::new();
            data.extend_from_slice(match chain {
                TargetChain::Zcash => b"zcash-2of3",
                TargetChain::Penumbra => b"penumbra-2of3",
            });
            // Sort keys for deterministic ordering
            let mut keys = [buyer_pk, seller_pk, chain_pk];
            keys.sort();
            for key in keys {
                data.extend_from_slice(key);
            }
            blake2_256(&data)
        }

        /// Verify Ligerito opening proof
        #[cfg(feature = "ligerito-verify")]
        #[allow(dead_code)]
        fn verify_opening(
            _commitment: &[u8; 32],
            _index: u8,
            _share: &[u8; 32],
            _proof_bytes: &[u8],
        ) -> bool {
            // TODO: Implement actual Ligerito opening verification
            // For now, return true for testing
            true
        }

        #[allow(dead_code)]
        #[cfg(not(feature = "ligerito-verify"))]
        fn verify_opening(
            _commitment: &[u8; 32],
            _index: u8,
            _share: &[u8; 32],
            _proof_bytes: &[u8],
        ) -> bool {
            // Verification disabled
            true
        }

        /// Try to resolve dispute if enough votes
        fn try_resolve_dispute(escrow_id: [u8; 32]) -> DispatchResult {
            let (buyer_votes, seller_votes, _) = VoteCounts::<T>::get(escrow_id);
            let min_votes = T::MinArbitratorsForDispute::get();

            let total = buyer_votes + seller_votes;
            if total < min_votes {
                return Ok(());
            }

            let to_buyer = if buyer_votes > seller_votes {
                true
            } else if seller_votes > buyer_votes {
                false
            } else {
                return Ok(()); // Tie - need more votes
            };

            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(0);

            Escrows::<T>::try_mutate(escrow_id, |maybe_escrow| {
                let escrow = maybe_escrow.as_mut().ok_or(Error::<T>::EscrowNotFound)?;

                // Update state to pending signing
                escrow.state = EscrowState::PendingReveal {
                    to_buyer,
                    decided_at: now,
                };

                // Determine recipient address on external chain
                let recipient = if to_buyer {
                    escrow.buyer_escrow_pubkey
                } else {
                    escrow.seller_escrow_pubkey
                };

                // Create chain signing request
                let request_id = NextSigningRequestId::<T>::get();
                NextSigningRequestId::<T>::put(request_id + 1);

                let signing_request = ChainSigningRequest {
                    escrow_id,
                    tx_hash: [0u8; 32], // Will be computed by validators
                    recipient,
                    amount: escrow.crypto_amount,
                    status: SigningRequestStatus::Pending,
                    created_at: now,
                    deadline: now + T::SigningTimeout::get(),
                };

                SigningRequests::<T>::insert(request_id, signing_request);
                escrow.signing_request_id = Some(request_id);

                // Track for timeout processing
                let _ = PendingSigningRequests::<T>::try_mutate(|list| list.try_push(request_id));

                Self::deposit_event(Event::DisputeResolved { escrow_id, to_buyer });
                Self::deposit_event(Event::ChainSigningRequested {
                    escrow_id,
                    request_id,
                    recipient,
                });

                Ok(())
            })
        }

        /// Process signing request timeouts
        fn process_signing_timeouts(now: u32) {
            let pending = PendingSigningRequests::<T>::get();

            for request_id in pending.iter() {
                if let Some(mut request) = SigningRequests::<T>::get(request_id) {
                    if now > request.deadline {
                        if matches!(request.status, SigningRequestStatus::Pending | SigningRequestStatus::InProgress { .. }) {
                            // Signing timeout
                            request.status = SigningRequestStatus::Failed {
                                reason: SigningFailureReason::Timeout,
                            };
                            SigningRequests::<T>::insert(request_id, request.clone());

                            // Update escrow - signing failed, but crypto is still locked
                            // In production: may need recovery mechanism
                            if let Some(mut escrow) = Escrows::<T>::get(request.escrow_id) {
                                // Return service deposit since signing failed
                                T::Currency::unreserve(&escrow.seller, escrow.chain_service_deposit);

                                // Mark as failed (may need manual recovery)
                                escrow.state = EscrowState::Cancelled;
                                Escrows::<T>::insert(request.escrow_id, escrow);

                                Self::deposit_event(Event::SigningTimeout {
                                    escrow_id: request.escrow_id,
                                });
                            }
                        }
                    }
                }
            }

            // Clean up completed/failed signing requests
            PendingSigningRequests::<T>::mutate(|list| {
                list.retain(|id| {
                    if let Some(request) = SigningRequests::<T>::get(id) {
                        matches!(request.status, SigningRequestStatus::Pending | SigningRequestStatus::InProgress { .. })
                    } else {
                        false
                    }
                });
            });
        }

        // ========== AGENT HELPERS (optional marketplace agents) ==========

        // Note: In the chain-as-arbitrator model, agents are optional.
        // They can still be used for marketplace features but are not
        // required for escrow resolution (chain signs via FROST).

        #[allow(dead_code)]
        fn update_agent_escrow_complete(agent: &T::AccountId) {
            Agents::<T>::mutate(agent, |maybe_agent| {
                if let Some(a) = maybe_agent {
                    a.total_escrows += 1;
                }
            });
        }

        fn update_leaderboard(agent: &T::AccountId, reputation: u64) {
            AgentLeaderboard::<T>::mutate(|board| {
                // Remove old entry if exists
                board.retain(|(_, a)| a != agent);

                // Insert new entry
                let _ = board.try_push((reputation, agent.clone()));

                // Sort by reputation (descending)
                board.sort_by(|a, b| b.0.cmp(&a.0));

                // Truncate to max size (already bounded, but just in case)
                while board.len() > 100 {
                    board.pop();
                }
            });
        }

        fn remove_from_leaderboard(agent: &T::AccountId) {
            AgentLeaderboard::<T>::mutate(|board| {
                board.retain(|(_, a)| a != agent);
            });
        }

        // ========== TRADER STATS HELPERS ==========

        /// Update trader stats on successful trade completion
        fn update_trader_successful_trade(
            trader: &T::AccountId,
            created_at: u32,
            completed_at: u32,
        ) {
            Traders::<T>::mutate(trader, |maybe_profile| {
                if let Some(p) = maybe_profile {
                    p.total_trades += 1;
                    p.successful_trades += 1;

                    // Update average completion time
                    let completion_blocks = completed_at.saturating_sub(created_at);
                    let total = p.successful_trades;
                    p.avg_completion_blocks = ((p.avg_completion_blocks as u64 * (total - 1) as u64
                        + completion_blocks as u64)
                        / total as u64) as u32;
                }
            });
        }

        /// Update trader stats when dispute is lost
        fn update_trader_dispute_lost(trader: &T::AccountId) {
            Traders::<T>::mutate(trader, |maybe_profile| {
                if let Some(p) = maybe_profile {
                    p.total_trades += 1;
                    p.disputes_lost += 1;
                }
            });
        }

        // ========== EPOCH HELPERS (shielded timing) ==========

        /// Maybe update epoch if we've crossed into a new epoch
        ///
        /// Epochs provide coarse-grained timing to reduce timing correlation attacks.
        /// Default: 100 blocks per epoch (~10 minutes with 6s blocks)
        fn maybe_update_epoch(now: u32) {
            let blocks_per_epoch = BlocksPerEpoch::<T>::get();
            // Default to 100 blocks if not set
            let blocks_per_epoch = if blocks_per_epoch == 0 { 100 } else { blocks_per_epoch };

            let current_epoch = CurrentEpoch::<T>::get();
            let new_epoch = super::block_to_epoch(now, blocks_per_epoch);

            if new_epoch > current_epoch {
                CurrentEpoch::<T>::put(new_epoch);
                Self::deposit_event(Event::EpochUpdated {
                    old_epoch: current_epoch,
                    new_epoch,
                });
            }
        }

        // ========== SHIELDED TIMEOUT PROCESSING ==========

        /// Process shielded escrow timeouts
        ///
        /// Iterates shielded escrows and expires those past timeout_epoch
        fn process_shielded_timeouts(now: u32) {
            let blocks_per_epoch = BlocksPerEpoch::<T>::get();
            let blocks_per_epoch = if blocks_per_epoch == 0 { 100 } else { blocks_per_epoch };
            let current_epoch = super::block_to_epoch(now, blocks_per_epoch);

            // Collect expired commitments
            let mut expired: Vec<[u8; 32]> = Vec::new();

            for (commitment, escrow) in ShieldedEscrows::<T>::iter() {
                if escrow.timeout_epoch <= current_epoch {
                    // Check if still in a state that can timeout
                    // (not already consumed or disputed)
                    if !NullifierSet::<T>::contains_key(&escrow.nullifier_commitment) {
                        expired.push(commitment);
                    }
                }
            }

            // Process expired escrows
            for commitment in expired {
                // Remove from active escrows (funds can be reclaimed)
                ShieldedEscrows::<T>::remove(&commitment);

                Self::deposit_event(Event::ShieldedEscrowExpired { commitment });
            }
        }

        /// Process dispute resolution timeouts
        ///
        /// If a dispute hasn't been resolved within timeout, auto-resolve
        fn process_dispute_timeouts(now: u32) {
            let blocks_per_epoch = BlocksPerEpoch::<T>::get();
            let blocks_per_epoch = if blocks_per_epoch == 0 { 100 } else { blocks_per_epoch };
            let current_epoch = super::block_to_epoch(now, blocks_per_epoch);

            // Default dispute timeout: 10 epochs
            let dispute_timeout_epochs = 10u32;

            ShieldedDisputeQueue::<T>::mutate(|queue| {
                queue.retain(|dispute| {
                    let dispute_age = current_epoch.saturating_sub(dispute.raised_epoch);
                    if dispute_age > dispute_timeout_epochs {
                        // Dispute timed out - in production, would auto-refund seller
                        Self::deposit_event(Event::ShieldedDisputeTimedOut {
                            escrow_commitment: dispute.escrow_commitment,
                        });
                        false // Remove from queue
                    } else {
                        true // Keep in queue
                    }
                });
            });
        }

        // ========== OCW FUNCTIONS ==========

        /// OCW: Process pending shielded signing requests
        ///
        /// Coordinates with frost-bridge to sign release transactions
        #[cfg(feature = "std")]
        fn ocw_process_shielded_signing() {
            use sp_runtime::offchain::storage::StorageValueRef;

            // Check if we're an active signer in frost-bridge
            let signer_storage = StorageValueRef::persistent(b"escrow_arb::is_frost_signer");
            let Ok(Some(true)) = signer_storage.get::<bool>() else {
                return; // Not a signer, skip
            };

            // Iterate pending FROST signing requests
            for (commitment, request) in FrostSigningRequests::<T>::iter() {
                if matches!(request.status, super::ShieldedSigningStatus::Pending) {
                    // Construct transaction data to sign
                    let tx_data = Self::ocw_build_release_tx(&commitment, &request);

                    // Submit to frost-bridge for signing
                    Self::ocw_submit_to_frost_bridge(&tx_data, request.frost_request_id);
                }
            }
        }

        #[cfg(not(feature = "std"))]
        fn ocw_process_shielded_signing() {}

        /// OCW: Process threshold decryption sessions
        ///
        /// For disputes that need arbitrator evidence decryption
        #[cfg(feature = "std")]
        fn ocw_process_decryption_sessions() {
            use sp_runtime::offchain::storage::StorageValueRef;

            // Check if we're an arbitrator
            let arb_storage = StorageValueRef::persistent(b"escrow_arb::is_arbitrator");
            let Ok(Some(true)) = arb_storage.get::<bool>() else {
                return;
            };

            // Get our arbitrator index
            let index_storage = StorageValueRef::persistent(b"escrow_arb::arbitrator_index");
            let Ok(Some(our_index)) = index_storage.get::<u8>() else {
                return;
            };

            // Get our secret share for decryption
            let share_storage = StorageValueRef::persistent(b"escrow_arb::decryption_share");
            let Ok(Some(_secret_share)) = share_storage.get::<[u8; 32]>() else {
                return;
            };

            // Process disputes needing decryption
            let dispute_queue = ShieldedDisputeQueue::<T>::get();
            for dispute in dispute_queue.iter() {
                // Check if we've already submitted our decryption share
                // (In production: track in local OCW storage)
                Self::ocw_maybe_submit_decryption_share(
                    our_index,
                    &dispute.escrow_commitment,
                    &dispute.encrypted_evidence,
                );
            }
        }

        #[cfg(not(feature = "std"))]
        fn ocw_process_decryption_sessions() {}

        /// OCW helper: Build release transaction for signing
        #[cfg(feature = "std")]
        fn ocw_build_release_tx(
            _commitment: &[u8; 32],
            request: &super::ShieldedSigningRequest,
        ) -> Vec<u8> {
            // In production: construct actual Zcash/Penumbra transaction
            // For now, return tx_hash as placeholder
            request.tx_hash.to_vec()
        }

        /// OCW helper: Submit signing request to frost-bridge
        #[cfg(feature = "std")]
        fn ocw_submit_to_frost_bridge(_tx_data: &[u8], _request_id: u64) {
            // In production: call frost-bridge pallet to initiate signing
            // This would use runtime API or storage to communicate
            log::debug!(
                target: "escrow-arbitration",
                "Submitting signing request to frost-bridge"
            );
        }

        /// OCW helper: Submit decryption share for dispute evidence
        #[cfg(feature = "std")]
        fn ocw_maybe_submit_decryption_share(
            _our_index: u8,
            _escrow_commitment: &[u8; 32],
            _encrypted_evidence: &[u8],
        ) {
            // In production:
            // 1. Compute ECDH with our share
            // 2. Generate DLEQ proof
            // 3. Submit DecryptionShare on-chain
            log::debug!(
                target: "escrow-arbitration",
                "Processing decryption share for dispute"
            );
        }
    }
}

// ========== OCW STORAGE KEYS ==========

/// Keys for offchain worker local storage
#[cfg(feature = "std")]
pub mod ocw_storage {
    /// Whether this node is a FROST signer
    pub const KEY_IS_FROST_SIGNER: &[u8] = b"escrow_arb::is_frost_signer";
    /// Whether this node is an arbitrator
    pub const KEY_IS_ARBITRATOR: &[u8] = b"escrow_arb::is_arbitrator";
    /// Arbitrator index (0-255)
    pub const KEY_ARBITRATOR_INDEX: &[u8] = b"escrow_arb::arbitrator_index";
    /// Secret share for threshold decryption
    pub const KEY_DECRYPTION_SHARE: &[u8] = b"escrow_arb::decryption_share";
}
