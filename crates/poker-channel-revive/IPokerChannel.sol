// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

/// @title IPokerChannel - State channel contract for zk.poker
/// @notice Manages escrow, cooperative close, and dispute resolution
/// @dev Deployed as a Rust PolkaVM contract on Polkadot Asset Hub via pallet-revive.
///      This file defines the ABI for Ethereum tooling (Foundry/cast/MetaMask).
interface IPokerChannel {
    // ========================================================================
    // Events
    // ========================================================================

    /// @notice Emitted when a new game/channel is created
    /// @param gameId Unique identifier for the game
    /// @param host Address of the table host
    /// @param bigBlind Big blind amount in planck
    event GameCreated(bytes32 indexed gameId, address indexed host, uint128 bigBlind);

    /// @notice Emitted when a player joins and deposits
    /// @param gameId Game identifier
    /// @param player Address of the joining player
    /// @param seat Seat number (0-indexed)
    event PlayerJoined(bytes32 indexed gameId, address indexed player, uint8 seat);

    /// @notice Emitted when the host starts the game
    /// @param gameId Game identifier
    event GameStarted(bytes32 indexed gameId);

    /// @notice Emitted on cooperative state update (all players signed)
    /// @param gameId Game identifier
    /// @param nonce Monotonically increasing state counter
    /// @param stateHash Hash of the off-chain game state
    event StateUpdated(bytes32 indexed gameId, uint64 nonce, bytes32 stateHash);

    /// @notice Emitted when a player opens a dispute
    /// @param gameId Game identifier
    /// @param initiator Address of the disputing player
    event DisputeOpened(bytes32 indexed gameId, address indexed initiator);

    /// @notice Emitted when the game is settled and payouts distributed
    /// @param gameId Game identifier
    event GameSettled(bytes32 indexed gameId);

    // ========================================================================
    // Game Lifecycle
    // ========================================================================

    /// @notice Create a new game channel. Host must send deposit as msg.value.
    /// @param gameId Unique game identifier (typically keccak256 of table params)
    /// @param bigBlind Big blind amount — min deposit is 10x this
    /// @param minPlayers Minimum players to start (2-9)
    /// @param maxPlayers Maximum seats at the table (2-9)
    /// @param disputeTimeout Blocks to wait before dispute can be settled
    /// @param encryptionKey Host's encryption key for mental poker
    /// @return gameId echoed back on success
    function createGame(
        bytes32 gameId,
        uint128 bigBlind,
        uint8 minPlayers,
        uint8 maxPlayers,
        uint64 disputeTimeout,
        bytes32 encryptionKey
    ) external payable returns (bytes32);

    /// @notice Join an existing game. Player must send deposit as msg.value.
    /// @param gameId Game to join
    /// @param seat Desired seat number (0-indexed, must be unoccupied)
    /// @param encryptionKey Player's encryption key for mental poker
    function joinGame(
        bytes32 gameId,
        uint8 seat,
        bytes32 encryptionKey
    ) external payable;

    /// @notice Host starts the game once enough players have joined
    /// @param gameId Game to start
    function startGame(bytes32 gameId) external;

    // ========================================================================
    // State Channel
    // ========================================================================

    /// @notice Submit a cooperatively signed state update
    /// @dev All active players must sign keccak256(gameId || nonce || stateHash)
    /// @param gameId Game identifier
    /// @param nonce Must be strictly greater than current nonce
    /// @param stateHash Hash of the off-chain engine state (engine.state_hash())
    /// @param signatures Array of 65-byte ECDSA signatures from all players
    function updateState(
        bytes32 gameId,
        uint64 nonce,
        bytes32 stateHash,
        bytes[] calldata signatures
    ) external;

    /// @notice Open a dispute with a signed state. Starts the timeout clock.
    /// @dev Can be called when a player disappears. Same sig requirements as updateState.
    /// @param gameId Game identifier
    /// @param nonce Must be greater than current nonce
    /// @param stateHash Hash of the disputed state
    /// @param signatures Array of 65-byte ECDSA signatures
    function dispute(
        bytes32 gameId,
        uint64 nonce,
        bytes32 stateHash,
        bytes[] calldata signatures
    ) external;

    /// @notice Settle the game and distribute payouts
    /// @dev From ACTIVE: cooperative close (caller should be host or have consent).
    ///      From DISPUTED: can only settle after disputeTimeout blocks have passed.
    /// @param gameId Game identifier
    /// @param payouts Array of amounts to pay each seat (index = seat number)
    function settle(
        bytes32 gameId,
        uint128[] calldata payouts
    ) external;

    // ========================================================================
    // View Functions
    // ========================================================================

    /// @notice Get game state
    /// @param gameId Game identifier
    /// @return Raw game data (96 bytes): state, host, bigBlind, min/max/current players,
    ///         stateHash, nonce, disputeBlock, disputeTimeout
    function getGame(bytes32 gameId) external view returns (bytes memory);

    /// @notice Get player info at a specific seat
    /// @param gameId Game identifier
    /// @param seat Seat number (0-indexed)
    /// @return Raw player data (68 bytes): address, deposit, encryptionKey
    function getPlayer(bytes32 gameId, uint8 seat) external view returns (bytes memory);
}
