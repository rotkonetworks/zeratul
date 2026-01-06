# api reference

API documentation for integrating with zk.poker components.

## ghettobox client

```rust
use ghettobox::{Client, Account};

// initialize client with vault endpoints
let client = Client::new(vec![
    "https://vault1.zkpoker.com",
    "https://vault2.zkpoker.com",
    "https://vault3.zkpoker.com",
]).await?;

// create new account
let result = client.create_account(
    "alice@example.com",
    b"123456",  // PIN
)?;
println!("address: {}", result.account.address_hex());

// recover existing account
let account = client.recover(
    "alice@example.com",
    b"123456",
).await?;

// sign message
let signature = account.sign(b"hello world");

// verify signature
let valid = account.verify(b"hello world", &signature);
```

## vault API

HTTP endpoints for vault nodes:

```
POST /register
  request:
    {
      "user_id": "sha256(email)",
      "unlock_tag": "hex(16 bytes)",
      "encrypted_share": "hex(64 bytes)"
    }
  response:
    {
      "ok": true,
      "signature": "vault signature confirming storage"
    }


POST /recover
  request:
    {
      "user_id": "sha256(email)",
      "unlock_tag": "hex(16 bytes)"
    }
  response (success):
    {
      "ok": true,
      "encrypted_share": "hex(64 bytes)"
    }
  response (wrong PIN):
    {
      "ok": false,
      "error": "wrong_pin",
      "remaining_attempts": 2
    }


GET /health
  response:
    {
      "status": "ok",
      "version": "1.0.0"
    }
```

## poker client API

```rust
use poker_client::{PokerClient, TableConfig, GameAction};

// create client
let client = PokerClient::new(account).await?;

// join or create table
let table = client.join_table(TableConfig {
    game_type: GameType::NLHoldem,
    stakes: Stakes { small_blind: 50, big_blind: 100 },
    min_buyin: 5000,
    max_buyin: 10000,
}).await?;

// perform action
table.action(GameAction::Raise { amount: 300 }).await?;

// fold
table.action(GameAction::Fold).await?;

// get current state
let state = table.state();
println!("pot: {}", state.pot);
println!("my stack: {}", state.my_stack);
println!("my cards: {:?}", state.hole_cards);
```

## state channel contract

solidity interface:

```solidity
interface IPokerChannel {
    // open new channel
    function open(
        address playerA,
        address playerB,
        uint256 depositA,
        uint256 depositB
    ) external returns (bytes32 channelId);

    // cooperative close
    function cooperativeClose(
        bytes32 channelId,
        uint256[] calldata finalBalances,
        bytes[] calldata signatures
    ) external;

    // initiate dispute
    function initiateDispute(
        bytes32 channelId,
        ChannelState calldata state,
        bytes[] calldata signatures
    ) external;

    // challenge with newer state
    function challenge(
        bytes32 channelId,
        ChannelState calldata newerState,
        bytes[] calldata signatures
    ) external;

    // resolve dispute after timeout
    function resolve(bytes32 channelId) external;

    // events
    event ChannelOpened(bytes32 indexed channelId, address[] players);
    event ChannelClosed(bytes32 indexed channelId, CloseType closeType);
    event DisputeInitiated(bytes32 indexed channelId, uint256 deadline);
    event DisputeChallenged(bytes32 indexed channelId, uint256 newVersion);
}

struct ChannelState {
    bytes32 channelId;
    uint64 version;
    uint256[] balances;
    bytes32 gameStateHash;
}

enum CloseType {
    Cooperative,
    Timeout,
    Dispute
}
```

## p2p messages

```rust
/// message types between peers
#[derive(Serialize, Deserialize)]
enum PeerMessage {
    /// game action
    GameAction(GameAction),

    /// signed state update
    StateUpdate {
        state: ChannelState,
        signature: Signature,
    },

    /// shuffle proof
    Shuffle {
        deck: EncryptedDeck,
        proof: ShuffleProof,
    },

    /// card reveal
    Reveal {
        position: u8,
        share: DecryptionShare,
        proof: DecryptionProof,
    },

    /// close channel request
    CloseRequest {
        final_balances: Vec<u64>,
        signature: Signature,
    },

    /// close channel accept
    CloseAccept {
        signature: Signature,
    },

    /// ping for latency
    Ping(u64),

    /// pong response
    Pong(u64),
}
```

## matchmaking API

```
GET /tables
  query params:
    ?game_type=nlhe
    &min_stakes=100
    &max_stakes=1000
    &min_reputation=80
  response:
    {
      "tables": [
        {
          "table_id": "abc123",
          "config": { ... },
          "creator_ticket": "node:5oj...",
          "status": "open"
        }
      ]
    }


POST /tables
  request:
    {
      "config": {
        "game_type": "nlhe",
        "stakes": { "small_blind": 50, "big_blind": 100 },
        "min_buyin": 5000,
        "max_buyin": 10000,
        "min_reputation": 80
      },
      "creator_ticket": "node:5oj..."
    }
  response:
    {
      "table_id": "abc123"
    }


DELETE /tables/:table_id
  response:
    { "ok": true }
```

## reputation query

```rust
// query on-chain reputation
async fn get_reputation(
    chain: &ChainClient,
    address: Address,
) -> Result<Reputation, QueryError> {
    let history = chain.get_channel_closes(address).await?;
    Ok(Reputation::from_history(&history))
}

struct Reputation {
    games_played: u64,
    games_completed: u64,
    disputes_lost: u64,
    timeouts: u64,
}

impl Reputation {
    fn score(&self) -> u8 {
        let base = 100.0;
        let penalty = (self.disputes_lost * 10 + self.timeouts * 5) as f64;
        (base - penalty).max(0.0).min(100.0) as u8
    }
}
```

## webhook events

optional event notifications:

```
POST /webhooks/game-complete
  {
    "event": "game_complete",
    "channel_id": "0x...",
    "hand_number": 42,
    "winner": "0x...",
    "pot": 1500,
    "timestamp": 1234567890
  }

POST /webhooks/channel-closed
  {
    "event": "channel_closed",
    "channel_id": "0x...",
    "close_type": "cooperative",
    "final_balances": [5000, 5000],
    "timestamp": 1234567890
  }
```
