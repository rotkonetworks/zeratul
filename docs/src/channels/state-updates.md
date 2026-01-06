# state updates

state updates are signed messages that advance the channel state. they form the core of off-chain gameplay.

## state structure

```rust
struct ChannelState {
    /// channel identifier
    channel_id: [u8; 32],

    /// monotonically increasing version
    version: u64,

    /// balance per player
    balances: Vec<u64>,

    /// hash of game state (if mid-game)
    game_state_hash: Option<[u8; 32]>,

    /// signatures from all parties
    signatures: Vec<Signature>,
}

/// hash for signing
impl ChannelState {
    fn signing_hash(&self) -> [u8; 32] {
        blake3::hash(&[
            &self.channel_id[..],
            &self.version.to_le_bytes(),
            &encode_balances(&self.balances),
            &self.game_state_hash.unwrap_or([0u8; 32]),
        ].concat())
    }
}
```

## update protocol

```
player A wants to make action:

  1. A creates state_v(n+1)
     ┌─────────────────────────────────────────┐
     │ version: n + 1                          │
     │ balances: [updated based on action]     │
     │ game_state_hash: hash(new_game_state)   │
     │ signatures: [sig_A]                     │
     └─────────────────────────────────────────┘

  2. A sends to B via p2p
     ─────────────────────────────────────────▶

  3. B verifies:
     - version = prev_version + 1
     - balances are valid for action
     - game state transition is legal
     - A's signature is valid

  4. B counter-signs
     ┌─────────────────────────────────────────┐
     │ signatures: [sig_A, sig_B]              │
     └─────────────────────────────────────────┘

  5. B sends back to A
     ◀─────────────────────────────────────────

  6. both now have state_v(n+1)
     - fully signed
     - can be submitted on-chain if needed
```

## action types

```rust
enum GameAction {
    /// post blind bet
    PostBlind { amount: u64 },

    /// fold hand
    Fold,

    /// match current bet
    Call,

    /// increase bet
    Raise { amount: u64 },

    /// check (no bet, pass action)
    Check,

    /// go all-in
    AllIn,

    /// reveal card decryption share
    RevealShare { position: u8, share: DecryptionShare },

    /// submit shuffle proof
    Shuffle { deck: EncryptedDeck, proof: ShuffleProof },
}
```

## balance updates

each action affects balances:

```rust
fn apply_action(
    state: &ChannelState,
    action: GameAction,
    player: usize,
) -> Result<ChannelState, ActionError> {
    let mut new_balances = state.balances.clone();

    match action {
        GameAction::PostBlind { amount } => {
            require(new_balances[player] >= amount)?;
            new_balances[player] -= amount;
            // amount goes to pot (tracked in game_state)
        }

        GameAction::Fold => {
            // no balance change, hand over
        }

        GameAction::Call => {
            let to_call = game_state.current_bet - game_state.player_bet[player];
            require(new_balances[player] >= to_call)?;
            new_balances[player] -= to_call;
        }

        GameAction::Raise { amount } => {
            let total = game_state.current_bet + amount - game_state.player_bet[player];
            require(new_balances[player] >= total)?;
            new_balances[player] -= total;
        }

        // ... other actions
    }

    Ok(ChannelState {
        channel_id: state.channel_id,
        version: state.version + 1,
        balances: new_balances,
        game_state_hash: Some(hash_game_state(&game_state)),
        signatures: vec![],
    })
}
```

## state validity

on-chain contract validates state:

```rust
fn validate_state(state: &ChannelState) -> Result<(), ValidationError> {
    // 1. verify channel exists
    let channel = channels.get(state.channel_id)?;

    // 2. verify signatures from all players
    for (i, player) in channel.players.iter().enumerate() {
        let sig = state.signatures.get(i)?;
        verify_signature(player, &state.signing_hash(), sig)?;
    }

    // 3. verify balances don't exceed deposits
    let total: u64 = state.balances.iter().sum();
    require(total <= channel.total_deposited)?;

    // 4. verify version is higher than previous
    if let Some(prev) = challenges.get(state.channel_id) {
        require(state.version > prev.state.version)?;
    }

    Ok(())
}
```

## state storage

clients store state locally:

```rust
struct LocalStorage {
    /// latest fully-signed state
    latest_state: ChannelState,

    /// state history (for disputes)
    history: Vec<ChannelState>,

    /// pending state (waiting for counter-sig)
    pending: Option<ChannelState>,

    /// game state details
    game_state: Option<GameState>,
}
```

## partial signatures

handling incomplete signing:

```
scenario: A sends state, B hasn't signed yet

A has:
  state_v5: [sig_A]  (pending)
  state_v4: [sig_A, sig_B]  (latest confirmed)

B has:
  state_v5: received, verifying
  state_v4: [sig_A, sig_B]  (latest confirmed)

if B rejects or times out:
  - A reverts to state_v4
  - dispute uses state_v4

if B accepts:
  - B signs and sends back
  - both advance to state_v5
```

## optimistic updates

for low-latency gameplay:

```
optimistic mode:
  1. A sends action + new state
  2. A immediately applies locally (optimistic)
  3. UI updates without waiting
  4. if B rejects, rollback

works because:
  - most actions are valid
  - rejection is rare
  - provides better UX
```

## batch updates

multiple actions in one round:

```
batch example (dealing):
  - shuffle proof from A
  - shuffle proof from B
  - reveal share for hole card 1
  - reveal share for hole card 2
  - reveal share for hole card 3
  - reveal share for hole card 4

single state update:
  version: n + 1
  contains: all cryptographic proofs
  signed once: by both players
```
