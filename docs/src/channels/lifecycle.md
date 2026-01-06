# channel lifecycle

state channels enable off-chain poker with on-chain settlement. funds are locked on-chain, gameplay happens off-chain, and final balances settle on-chain.

## lifecycle overview

```
┌─────────────────────────────────────────────────────────────┐
│                      CHANNEL LIFECYCLE                       │
└─────────────────────────────────────────────────────────────┘

  1. OPEN                    2. ACTIVE                 3. CLOSE
  ┌───────────┐             ┌───────────┐             ┌───────────┐
  │ on-chain  │             │ off-chain │             │ on-chain  │
  │           │             │           │             │           │
  │ deposit   │────────────▶│ gameplay  │────────────▶│ withdraw  │
  │ funds     │             │ state     │             │ funds     │
  │           │             │ updates   │             │           │
  └───────────┘             └───────────┘             └───────────┘
       │                         │                         │
       │                         │                         │
       ▼                         ▼                         ▼
  funds locked              signatures              funds released
  in contract               exchanged               to players
```

## channel opening

players deposit funds to open a channel:

```rust
struct ChannelOpen {
    /// unique channel identifier
    channel_id: [u8; 32],
    /// participating players
    players: Vec<Address>,
    /// initial deposits per player
    deposits: Vec<u64>,
    /// block number when opened
    opened_at: u64,
}

// on-chain transaction
fn open_channel(
    player_a: Address,
    player_b: Address,
    deposit_a: u64,
    deposit_b: u64,
) -> ChannelId {
    // verify signatures from both players
    // lock deposits in channel contract
    // emit ChannelOpened event
    // return channel_id
}
```

## initial state

first state after channel opens:

```rust
struct ChannelState {
    /// channel identifier
    channel_id: [u8; 32],
    /// version number (starts at 0)
    version: u64,
    /// current balances
    balances: Vec<u64>,
    /// game state (if mid-game)
    game_state: Option<GameState>,
    /// signatures from all players
    signatures: Vec<Signature>,
}

// version 0: initial state
let initial = ChannelState {
    channel_id,
    version: 0,
    balances: vec![deposit_a, deposit_b],
    game_state: None,
    signatures: vec![],
};
```

## state transitions

each action creates a new state version:

```
v0: channel opened
    balances: [1000, 1000]

v1: game started, blinds posted
    balances: [995, 990]  // small blind 5, big blind 10
    game_state: Some(...)

v2: player A raises to 50
    balances: [950, 990]
    game_state: Some(...)

v3: player B calls 50
    balances: [950, 950]
    game_state: Some(...)

...

v42: hand complete, pot awarded
    balances: [1200, 800]
    game_state: None
```

## channel closing

### cooperative close

both players agree on final balances:

```rust
fn cooperative_close(
    channel_id: ChannelId,
    final_state: ChannelState,
) -> Result<(), CloseError> {
    // verify signatures from all players
    require_signatures(&final_state)?;

    // verify state is valid close state
    require_no_active_game(&final_state)?;

    // distribute funds
    for (player, balance) in final_state.players.iter()
        .zip(final_state.balances.iter())
    {
        transfer(channel_contract, player, *balance);
    }

    // emit ChannelClosed event
    emit(ChannelClosed {
        channel_id,
        close_type: CloseType::Cooperative,
        final_balances: final_state.balances,
    });

    Ok(())
}
```

### unilateral close

one player submits latest state:

```rust
fn initiate_close(
    channel_id: ChannelId,
    state: ChannelState,
) -> Result<(), CloseError> {
    // verify submitter is channel participant
    require_participant(msg.sender)?;

    // verify state signatures
    require_signatures(&state)?;

    // start challenge period
    challenges[channel_id] = Challenge {
        state,
        submitted_at: block.number,
        deadline: block.number + {{DISPUTE_TIMEOUT_BLOCKS}},
    };

    emit(CloseInitiated { channel_id, deadline });

    Ok(())
}
```

## challenge period

after unilateral close initiation:

```
challenge window: {{DISPUTE_TIMEOUT_BLOCKS}} blocks

during this time:
  - other player can submit newer state
  - newer = higher version number
  - latest valid state wins

after deadline:
  - no more challenges accepted
  - funds distributed per submitted state
```

## channel states

```
┌─────────────────────────────────────────────────────────────┐
│                      STATE DIAGRAM                          │
└─────────────────────────────────────────────────────────────┘

          open()
  NONE ──────────▶ OPEN
                     │
                     │ play games
                     ▼
                   ACTIVE
                     │
      ┌──────────────┼──────────────┐
      │              │              │
      ▼              ▼              ▼
  cooperative    initiate()    timeout()
      │              │              │
      │              ▼              │
      │          CLOSING           │
      │              │              │
      │              │ deadline     │
      │              ▼              │
      └────────▶ CLOSED ◀──────────┘
```

## multi-game channels

channels persist across multiple games:

```
game 1: balances change within channel
game 2: balances change within channel
...
game N: balances change within channel

single close: final balances after N games
  - only 2 on-chain transactions total
  - gas cost amortized across all games
```

## deposits and withdrawals

mid-channel balance changes:

```
top-up (add funds):
  1. on-chain deposit to channel
  2. update channel state off-chain
  3. both players sign new state

partial withdrawal:
  1. both players agree on withdrawal
  2. update channel state off-chain
  3. on-chain withdrawal transaction
```
