# table setup

before playing, players must find each other, open a channel, and agree on table parameters.

## finding players

```
discovery methods:

1. matchmaking service
   - player posts "looking for game"
   - includes: stakes, game type, reputation requirements
   - service matches compatible players

2. direct invite
   - player shares invite link/code
   - friend connects directly
   - private game

3. table browser
   - browse open tables
   - see stakes, players, reputation
   - join available seat
```

## table parameters

```rust
struct TableConfig {
    /// game variant
    game_type: GameType,

    /// stakes (small blind / big blind)
    stakes: Stakes,

    /// minimum buy-in
    min_buyin: u64,

    /// maximum buy-in
    max_buyin: u64,

    /// time per action (seconds)
    action_timeout: u32,

    /// reputation requirements
    min_reputation: u8,

    /// maximum players (2 for heads-up)
    max_players: u8,
}

enum GameType {
    /// texas hold'em, no limit
    NLHoldem,
    /// texas hold'em, pot limit
    PLHoldem,
    /// texas hold'em, fixed limit
    FLHoldem,
}

struct Stakes {
    small_blind: u64,
    big_blind: u64,
}
```

## table creation flow

```
player A creates table:

  1. set table config
     ┌─────────────────────────────────────────┐
     │ game: NL Hold'em                         │
     │ stakes: $0.50 / $1.00                    │
     │ buy-in: $50 - $100                       │
     │ timeout: 30 seconds                      │
     │ min reputation: 80                       │
     └─────────────────────────────────────────┘

  2. post to matchmaking / share invite

  3. wait for player B to join

  4. both open state channel
     - deposit buy-in amount
     - agree on table config

  5. begin game
```

## joining a table

```
player B joins:

  1. discover table (browser/invite/matchmaking)

  2. verify table config acceptable
     - stakes within budget
     - reputation met
     - game type desired

  3. open channel with player A
     - deposit buy-in
     - receive table config confirmation

  4. ready to play
```

## channel opening

detailed channel setup:

```rust
struct TableJoinRequest {
    /// table identifier
    table_id: TableId,
    /// player's address
    player: Address,
    /// desired buy-in amount
    buyin: u64,
    /// signature proving ownership
    signature: Signature,
}

fn join_table(request: TableJoinRequest) -> Result<ChannelId, JoinError> {
    let table = tables.get(&request.table_id)?;

    // verify buy-in within limits
    require(request.buyin >= table.config.min_buyin)?;
    require(request.buyin <= table.config.max_buyin)?;

    // verify reputation
    let rep = reputation.get(&request.player)?;
    require(rep.score() >= table.config.min_reputation)?;

    // open state channel
    let channel_id = open_channel(
        table.creator,
        request.player,
        table.creator_buyin,
        request.buyin,
    )?;

    Ok(channel_id)
}
```

## seat assignment

heads-up (2 player) seating:

```
position assignment:
  - dealer button alternates each hand
  - small blind = button (heads-up rule)
  - big blind = other player

example:
  hand 1: A = dealer/SB, B = BB
  hand 2: B = dealer/SB, A = BB
  hand 3: A = dealer/SB, B = BB
  ...

first dealer:
  - determined by card draw
  - or random beacon
  - or predefined (table creator)
```

## initial state

after channel opens:

```rust
struct InitialGameState {
    /// table configuration
    config: TableConfig,
    /// player balances (buy-ins)
    stacks: [u64; 2],
    /// current dealer position (0 or 1)
    dealer: u8,
    /// hand number (starts at 1)
    hand_number: u64,
}

fn create_initial_state(
    channel: &Channel,
    config: &TableConfig,
) -> InitialGameState {
    InitialGameState {
        config: config.clone(),
        stacks: channel.balances.clone().try_into().unwrap(),
        dealer: 0,  // first player is initial dealer
        hand_number: 1,
    }
}
```

## pre-game handshake

verify both clients ready:

```
handshake protocol:

  A → B: ClientReady { version, capabilities }
  B → A: ClientReady { version, capabilities }

  verify:
    - protocol versions compatible
    - both support required features
    - encryption keys exchanged

  A → B: StartGame { config_hash }
  B → A: StartGame { config_hash }

  verify:
    - both agree on config
    - ready to shuffle
```

## rebuy / top-up

adding chips during game:

```rust
enum RebuyType {
    /// add chips between hands
    TopUp { amount: u64 },
    /// re-enter after bust (if allowed)
    Rebuy { amount: u64 },
}

fn request_rebuy(
    channel: &Channel,
    rebuy: RebuyType,
) -> Result<(), RebuyError> {
    // can only rebuy between hands
    require(!channel.hand_in_progress)?;

    // verify doesn't exceed max buy-in
    let new_stack = current_stack + rebuy.amount();
    require(new_stack <= channel.config.max_buyin)?;

    // requires on-chain deposit
    // then off-chain state update
}
```

## leaving the table

clean exit protocol:

```
leaving options:

1. complete current hand
   - finish any active hand
   - cooperative close channel
   - both receive final balances

2. immediate leave
   - forfeit current hand (if any)
   - still cooperative close
   - faster but may lose hand

3. disconnect (unintended)
   - other player can close unilaterally
   - timeout dispute if needed
```
