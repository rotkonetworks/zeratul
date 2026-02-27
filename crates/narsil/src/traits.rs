//! core traits for network-agnostic syndicate operations
//!
//! narsil is designed to work with any blockchain or task execution system.
//! implement these traits to integrate with your target network.
//!
//! # architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        NARSIL CORE                              │
//! │  (proposal, voting, osst aggregation, state management)         │
//! └─────────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                      TRAIT LAYER                                │
//! │  NetworkAdapter, ActionBuilder, SignatureScheme, StateBackend  │
//! └─────────────────────────────────────────────────────────────────┘
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          ▼                   ▼                   ▼
//!    ┌──────────┐       ┌──────────┐       ┌──────────┐
//!    │ Penumbra │       │ Ethereum │       │  Custom  │
//!    │ Adapter  │       │ Adapter  │       │ Backend  │
//!    └──────────┘       └──────────┘       └──────────┘
//! ```
//!
//! # example implementation
//!
//! ```ignore
//! struct MyChainAdapter;
//!
//! impl NetworkAdapter for MyChainAdapter {
//!     type Address = [u8; 20];
//!     type Transaction = MyTx;
//!     type Error = MyError;
//!
//!     fn submit(&self, tx: Self::Transaction) -> Result<TxHash, Self::Error> {
//!         // submit to your chain
//!     }
//! }
//! ```

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::wire::Hash32;

/// transaction hash (chain-agnostic)
pub type TxHash = [u8; 32];

/// network adapter trait
///
/// implement this to connect narsil to your target blockchain or execution layer
pub trait NetworkAdapter {
    /// address type for this network
    type Address: Clone + AsRef<[u8]>;

    /// transaction type for this network
    type Transaction: Clone;

    /// receipt/confirmation type
    type Receipt;

    /// error type
    type Error: core::fmt::Debug;

    /// network identifier (e.g., "penumbra-mainnet", "ethereum-sepolia")
    fn network_id(&self) -> &str;

    /// check if network is reachable
    fn is_connected(&self) -> bool;

    /// submit signed transaction to network
    fn submit(&self, tx: &Self::Transaction) -> Result<TxHash, Self::Error>;

    /// check transaction status
    fn tx_status(&self, hash: &TxHash) -> Result<TxStatus, Self::Error>;

    /// get current block/height
    fn current_height(&self) -> Result<u64, Self::Error>;

    /// estimate fee for transaction
    fn estimate_fee(&self, tx: &Self::Transaction) -> Result<u64, Self::Error>;
}

/// transaction status
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxStatus {
    /// not found
    Unknown,
    /// in mempool
    Pending,
    /// confirmed
    Confirmed { height: u64 },
    /// failed
    Failed,
}

/// action builder trait
///
/// implement this to define what actions your syndicate can take
pub trait ActionBuilder {
    /// the action type this builder produces
    type Action: Clone + core::fmt::Debug;

    /// error type for building actions
    type Error: core::fmt::Debug;

    /// action kind identifier
    fn action_kind(&self) -> &str;

    /// build action from serialized data
    fn build_from_bytes(&self, data: &[u8]) -> Result<Self::Action, Self::Error>;

    /// serialize action to bytes
    fn to_bytes(&self, action: &Self::Action) -> Vec<u8>;

    /// validate action before signing
    fn validate(&self, action: &Self::Action) -> Result<(), Self::Error>;

    /// human-readable description
    fn describe(&self, action: &Self::Action) -> String;
}

/// signature scheme trait
///
/// abstracts over different threshold signature schemes (osst, frost, etc.)
pub trait SignatureScheme {
    /// public key type
    type PublicKey: Clone + AsRef<[u8]>;

    /// secret share type
    type SecretShare: Clone;

    /// signature type
    type Signature: Clone + AsRef<[u8]>;

    /// contribution type (partial signature)
    type Contribution: Clone;

    /// error type
    type Error: core::fmt::Debug;

    /// scheme identifier
    fn scheme_id(&self) -> &str;

    /// get group public key
    fn group_key(&self) -> &Self::PublicKey;

    /// create contribution for message
    fn contribute(
        &self,
        share: &Self::SecretShare,
        message: &[u8],
    ) -> Result<Self::Contribution, Self::Error>;

    /// aggregate contributions into final signature
    fn aggregate(
        &self,
        contributions: &[Self::Contribution],
        message: &[u8],
    ) -> Result<Self::Signature, Self::Error>;

    /// verify signature
    fn verify(
        &self,
        signature: &Self::Signature,
        message: &[u8],
    ) -> Result<bool, Self::Error>;
}

/// transaction builder trait
///
/// builds network-specific transactions from syndicate actions
pub trait TransactionBuilder<N: NetworkAdapter, A: ActionBuilder> {
    /// error type
    type Error: core::fmt::Debug;

    /// build unsigned transaction from action
    fn build_unsigned(
        &self,
        action: &A::Action,
        network: &N,
    ) -> Result<N::Transaction, Self::Error>;

    /// attach signature to transaction
    fn attach_signature<S: SignatureScheme>(
        &self,
        tx: N::Transaction,
        signature: &S::Signature,
    ) -> Result<N::Transaction, Self::Error>;
}

/// state backend trait
///
/// abstracts syndicate state storage (memory, sqlite, rocksdb, etc.)
pub trait StateBackend {
    /// error type
    type Error: core::fmt::Debug;

    /// load state for syndicate
    fn load(&self, syndicate_id: &Hash32) -> Result<Option<Vec<u8>>, Self::Error>;

    /// save state for syndicate
    fn save(&self, syndicate_id: &Hash32, state: &[u8]) -> Result<(), Self::Error>;

    /// delete state
    fn delete(&self, syndicate_id: &Hash32) -> Result<(), Self::Error>;

    /// list all syndicate ids
    fn list(&self) -> Result<Vec<Hash32>, Self::Error>;
}

/// relay backend trait
///
/// abstracts message relay (ipfs, dht, http, etc.)
#[cfg(feature = "std")]
pub trait RelayBackend: Send + Sync {
    /// error type
    type Error: core::fmt::Debug + Send;

    /// relay identifier
    fn relay_id(&self) -> &str;

    /// post message to mailbox
    fn post(&self, mailbox: &[u8; 32], message: &[u8]) -> Result<Hash32, Self::Error>;

    /// fetch messages from mailbox
    fn fetch(&self, mailbox: &[u8; 32], after: Option<Hash32>) -> Result<Vec<(Hash32, Vec<u8>)>, Self::Error>;

    /// broadcast to topic
    fn broadcast(&self, topic: &[u8; 32], message: &[u8]) -> Result<Hash32, Self::Error>;

    /// subscribe to topic (returns iterator)
    fn subscribe(&self, topic: &[u8; 32]) -> Result<Box<dyn Iterator<Item = Vec<u8>> + Send>, Self::Error>;
}

/// key derivation trait
///
/// abstracts key derivation for different account models
pub trait KeyDerivation {
    /// derived key type
    type DerivedKey: Clone;

    /// error type
    type Error: core::fmt::Debug;

    /// derive syndicate keys from seed
    fn derive_syndicate_keys(
        &self,
        seed: &[u8],
        syndicate_id: &Hash32,
    ) -> Result<SyndicateKeys<Self::DerivedKey>, Self::Error>;
}

/// syndicate keys bundle
#[derive(Clone, Debug)]
pub struct SyndicateKeys<K> {
    /// spending/signing key
    pub signing_key: K,
    /// viewing/decryption key
    pub viewing_key: K,
    /// nullifier key (for privacy chains)
    pub nullifier_key: Option<K>,
}

/// action registry
///
/// registers and dispatches action builders
pub struct ActionRegistry {
    builders: Vec<(String, Box<dyn DynActionBuilder>)>,
}

impl ActionRegistry {
    /// create empty registry
    pub fn new() -> Self {
        Self { builders: Vec::new() }
    }

    /// register action builder
    pub fn register<A: ActionBuilder + 'static>(&mut self, builder: A) {
        self.builders.push((
            builder.action_kind().into(),
            Box::new(ActionBuilderWrapper(builder)),
        ));
    }

    /// find builder by kind
    pub fn find(&self, kind: &str) -> Option<&dyn DynActionBuilder> {
        self.builders
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, b)| b.as_ref())
    }

    /// list registered action kinds
    pub fn kinds(&self) -> Vec<&str> {
        self.builders.iter().map(|(k, _)| k.as_str()).collect()
    }
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// dynamic action builder (type-erased)
pub trait DynActionBuilder {
    /// action kind
    fn kind(&self) -> &str;

    /// build from bytes
    fn build(&self, data: &[u8]) -> Result<Vec<u8>, String>;

    /// validate bytes
    fn validate(&self, data: &[u8]) -> Result<(), String>;

    /// describe action
    fn describe(&self, data: &[u8]) -> String;
}

struct ActionBuilderWrapper<A: ActionBuilder>(A);

impl<A: ActionBuilder> DynActionBuilder for ActionBuilderWrapper<A> {
    fn kind(&self) -> &str {
        self.0.action_kind()
    }

    fn build(&self, data: &[u8]) -> Result<Vec<u8>, String> {
        let action = self.0.build_from_bytes(data)
            .map_err(|e| format!("{:?}", e))?;
        Ok(self.0.to_bytes(&action))
    }

    fn validate(&self, data: &[u8]) -> Result<(), String> {
        let action = self.0.build_from_bytes(data)
            .map_err(|e| format!("{:?}", e))?;
        self.0.validate(&action).map_err(|e| format!("{:?}", e))
    }

    fn describe(&self, data: &[u8]) -> String {
        match self.0.build_from_bytes(data) {
            Ok(action) => self.0.describe(&action),
            Err(e) => format!("invalid action: {:?}", e),
        }
    }
}

/// generic syndicate runtime
///
/// wires together all the components for a specific network
pub struct SyndicateRuntime<N, S, B>
where
    N: NetworkAdapter,
    S: SignatureScheme,
    B: StateBackend,
{
    /// network adapter
    pub network: N,
    /// signature scheme
    pub signature: S,
    /// state backend
    pub state: B,
    /// action registry
    pub actions: ActionRegistry,
}

impl<N, S, B> SyndicateRuntime<N, S, B>
where
    N: NetworkAdapter,
    S: SignatureScheme,
    B: StateBackend,
{
    /// create new runtime
    pub fn new(network: N, signature: S, state: B) -> Self {
        Self {
            network,
            signature,
            state,
            actions: ActionRegistry::new(),
        }
    }

    /// register action builder
    pub fn register_action<A: ActionBuilder + 'static>(&mut self, builder: A) {
        self.actions.register(builder);
    }

    /// check if runtime is ready
    pub fn is_ready(&self) -> bool {
        self.network.is_connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // mock implementations for testing

    struct MockNetwork;

    impl NetworkAdapter for MockNetwork {
        type Address = [u8; 20];
        type Transaction = Vec<u8>;
        type Receipt = ();
        type Error = &'static str;

        fn network_id(&self) -> &str { "mock-network" }
        fn is_connected(&self) -> bool { true }
        fn submit(&self, _tx: &Self::Transaction) -> Result<TxHash, Self::Error> {
            Ok([0u8; 32])
        }
        fn tx_status(&self, _hash: &TxHash) -> Result<TxStatus, Self::Error> {
            Ok(TxStatus::Confirmed { height: 100 })
        }
        fn current_height(&self) -> Result<u64, Self::Error> { Ok(100) }
        fn estimate_fee(&self, _tx: &Self::Transaction) -> Result<u64, Self::Error> { Ok(1000) }
    }

    struct MockSignature;

    impl SignatureScheme for MockSignature {
        type PublicKey = [u8; 32];
        type SecretShare = [u8; 32];
        type Signature = [u8; 64];
        type Contribution = [u8; 64];
        type Error = &'static str;

        fn scheme_id(&self) -> &str { "mock-sig" }
        fn group_key(&self) -> &Self::PublicKey { &[0u8; 32] }
        fn contribute(&self, _share: &Self::SecretShare, _msg: &[u8]) -> Result<Self::Contribution, Self::Error> {
            Ok([0u8; 64])
        }
        fn aggregate(&self, _contribs: &[Self::Contribution], _msg: &[u8]) -> Result<Self::Signature, Self::Error> {
            Ok([0u8; 64])
        }
        fn verify(&self, _sig: &Self::Signature, _msg: &[u8]) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    struct MockState;

    impl StateBackend for MockState {
        type Error = &'static str;

        fn load(&self, _id: &Hash32) -> Result<Option<Vec<u8>>, Self::Error> { Ok(None) }
        fn save(&self, _id: &Hash32, _state: &[u8]) -> Result<(), Self::Error> { Ok(()) }
        fn delete(&self, _id: &Hash32) -> Result<(), Self::Error> { Ok(()) }
        fn list(&self) -> Result<Vec<Hash32>, Self::Error> { Ok(vec![]) }
    }

    struct TransferAction;

    impl ActionBuilder for TransferAction {
        type Action = (Vec<u8>, u64); // (recipient, amount)
        type Error = &'static str;

        fn action_kind(&self) -> &str { "transfer" }

        fn build_from_bytes(&self, data: &[u8]) -> Result<Self::Action, Self::Error> {
            if data.len() < 40 {
                return Err("invalid data");
            }
            let recipient = data[..32].to_vec();
            let amount = u64::from_le_bytes(data[32..40].try_into().unwrap());
            Ok((recipient, amount))
        }

        fn to_bytes(&self, action: &Self::Action) -> Vec<u8> {
            let mut buf = action.0.clone();
            buf.extend_from_slice(&action.1.to_le_bytes());
            buf
        }

        fn validate(&self, action: &Self::Action) -> Result<(), Self::Error> {
            if action.1 == 0 {
                return Err("amount must be > 0");
            }
            Ok(())
        }

        fn describe(&self, action: &Self::Action) -> String {
            format!("transfer {} to {:?}", action.1, &action.0[..4])
        }
    }

    #[test]
    fn test_runtime_creation() {
        let runtime = SyndicateRuntime::new(
            MockNetwork,
            MockSignature,
            MockState,
        );

        assert!(runtime.is_ready());
        assert_eq!(runtime.network.network_id(), "mock-network");
        assert_eq!(runtime.signature.scheme_id(), "mock-sig");
    }

    #[test]
    fn test_action_registry() {
        let mut registry = ActionRegistry::new();
        registry.register(TransferAction);

        assert_eq!(registry.kinds(), vec!["transfer"]);

        let builder = registry.find("transfer").unwrap();
        assert_eq!(builder.kind(), "transfer");

        // build valid action
        let mut data = vec![0u8; 32]; // recipient
        data.extend_from_slice(&100u64.to_le_bytes()); // amount
        let result = builder.build(&data);
        assert!(result.is_ok());

        // describe
        let desc = builder.describe(&data);
        assert!(desc.contains("transfer 100"));
    }

    #[test]
    fn test_action_validation() {
        let mut registry = ActionRegistry::new();
        registry.register(TransferAction);

        let builder = registry.find("transfer").unwrap();

        // valid action
        let mut data = vec![0u8; 32];
        data.extend_from_slice(&100u64.to_le_bytes());
        assert!(builder.validate(&data).is_ok());

        // invalid action (amount = 0)
        let mut data = vec![0u8; 32];
        data.extend_from_slice(&0u64.to_le_bytes());
        assert!(builder.validate(&data).is_err());
    }

    #[test]
    fn test_tx_status() {
        assert_eq!(TxStatus::Unknown, TxStatus::Unknown);
        assert_ne!(TxStatus::Pending, TxStatus::Failed);

        let status = TxStatus::Confirmed { height: 100 };
        if let TxStatus::Confirmed { height } = status {
            assert_eq!(height, 100);
        }
    }
}
