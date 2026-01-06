# error codes

comprehensive list of errors that can occur in zk.poker.

## authentication errors

```rust
enum AuthError {
    /// PIN verification failed
    WrongPin {
        remaining_attempts: u8,
    },

    /// account locked due to too many wrong PINs
    AccountLocked,

    /// vault node unreachable
    VaultUnavailable {
        vault_index: u8,
    },

    /// not enough vault shares retrieved
    InsufficientShares {
        retrieved: u8,
        required: u8,
    },

    /// share decryption failed (corrupted?)
    ShareDecryptionFailed,

    /// email not found in any vault
    AccountNotFound,

    /// email already registered
    AccountExists,

    /// session expired
    SessionExpired,
}
```

## channel errors

```rust
enum ChannelError {
    /// channel not found on-chain
    ChannelNotFound {
        channel_id: [u8; 32],
    },

    /// insufficient balance for operation
    InsufficientBalance {
        required: u64,
        available: u64,
    },

    /// invalid state version (must be monotonic)
    InvalidVersion {
        expected: u64,
        received: u64,
    },

    /// signature verification failed
    InvalidSignature {
        signer: Address,
    },

    /// channel already closed
    ChannelClosed,

    /// channel in dispute, can't update
    ChannelDisputed,

    /// counterparty not responding
    CounterpartyTimeout,

    /// state hash mismatch
    StateHashMismatch,
}
```

## game errors

```rust
enum GameError {
    /// action not allowed in current state
    InvalidAction {
        action: String,
        reason: String,
    },

    /// not player's turn
    NotYourTurn {
        current_player: usize,
    },

    /// bet amount invalid
    InvalidBetAmount {
        min: u64,
        max: u64,
        attempted: u64,
    },

    /// insufficient chips for action
    InsufficientChips {
        required: u64,
        available: u64,
    },

    /// shuffle proof verification failed
    InvalidShuffleProof,

    /// card reveal verification failed
    InvalidCardReveal {
        position: u8,
    },

    /// invalid card (not one of 52)
    InvalidCard,

    /// hand evaluation mismatch
    HandEvaluationMismatch,

    /// game not in progress
    NoActiveGame,

    /// game already in progress
    GameInProgress,
}
```

## network errors

```rust
enum NetworkError {
    /// peer connection failed
    ConnectionFailed {
        peer: NodeId,
        reason: String,
    },

    /// peer disconnected unexpectedly
    PeerDisconnected {
        peer: NodeId,
    },

    /// message send failed
    SendFailed {
        reason: String,
    },

    /// message receive timeout
    ReceiveTimeout,

    /// invalid message format
    InvalidMessage {
        reason: String,
    },

    /// relay connection failed
    RelayFailed {
        relay: String,
    },

    /// no route to peer
    NoRoute {
        peer: NodeId,
    },
}
```

## cryptographic errors

```rust
enum CryptoError {
    /// ZK proof verification failed
    ProofVerificationFailed {
        proof_type: String,
    },

    /// signature verification failed
    SignatureInvalid,

    /// point not on curve
    InvalidCurvePoint,

    /// hash mismatch
    HashMismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },

    /// decryption failed
    DecryptionFailed,

    /// key derivation failed
    KeyDerivationFailed,
}
```

## dispute errors

```rust
enum DisputeError {
    /// challenge period expired
    ChallengePeriodExpired,

    /// challenge period not yet expired
    ChallengePeriodActive {
        blocks_remaining: u64,
    },

    /// submitted state not newer
    StateNotNewer {
        submitted_version: u64,
        current_version: u64,
    },

    /// fraud proof invalid
    InvalidFraudProof {
        reason: String,
    },

    /// not a channel participant
    NotParticipant,

    /// dispute already in progress
    DisputeInProgress,

    /// no dispute to resolve
    NoActiveDispute,
}
```

## error handling

```rust
/// display user-friendly error message
fn display_error(error: &Error) -> String {
    match error {
        Error::Auth(AuthError::WrongPin { remaining_attempts }) => {
            format!(
                "incorrect PIN. {} attempts remaining before account lockout.",
                remaining_attempts
            )
        }

        Error::Auth(AuthError::AccountLocked) => {
            "account locked due to too many wrong PIN attempts. \
             contact support for recovery options.".to_string()
        }

        Error::Channel(ChannelError::InsufficientBalance { required, available }) => {
            format!(
                "insufficient balance. need ${:.2}, have ${:.2}.",
                *required as f64 / 100.0,
                *available as f64 / 100.0
            )
        }

        Error::Game(GameError::NotYourTurn { .. }) => {
            "it's not your turn. wait for opponent's action.".to_string()
        }

        Error::Network(NetworkError::PeerDisconnected { .. }) => {
            "opponent disconnected. waiting for reconnection...".to_string()
        }

        // ... other cases
        _ => format!("error: {:?}", error),
    }
}
```

## error recovery

```
recovery strategies:

  wrong PIN:
    - try again (carefully)
    - use "forgot PIN" if available
    - contact support before lockout

  vault unavailable:
    - retry after delay
    - try other vaults
    - only need 2 of 3

  peer disconnected:
    - automatic reconnection attempts
    - game state preserved
    - can initiate dispute if prolonged

  insufficient balance:
    - top up channel
    - reduce bet size
    - close channel and reopen

  invalid proof:
    - indicates cheating or bug
    - collect evidence
    - initiate dispute
```

## logging

```rust
/// log error with context
fn log_error(error: &Error, context: &ErrorContext) {
    tracing::error!(
        error = ?error,
        channel_id = ?context.channel_id,
        action = %context.action,
        peer = ?context.peer,
        "operation failed"
    );
}

struct ErrorContext {
    channel_id: Option<[u8; 32]>,
    action: String,
    peer: Option<NodeId>,
    timestamp: Timestamp,
}
```
