# cooperative close

cooperative close is the happy path: both players agree on final balances and close the channel without disputes.

## why cooperative close

```
benefits vs dispute:
  ┌────────────────────┬──────────────┬────────────────┐
  │                    │ cooperative  │ dispute        │
  ├────────────────────┼──────────────┼────────────────┤
  │ gas cost           │ minimal      │ higher         │
  │ time to settle     │ immediate    │ {{DISPUTE_TIMEOUT_BLOCKS}} blocks   │
  │ fees               │ none         │ {{DISPUTE_FEE_PERCENT}}%            │
  │ reputation impact  │ positive     │ negative       │
  └────────────────────┴──────────────┴────────────────┘

rational players always prefer cooperative close
```

## close flow

```
both players ready to close:

  player A                                    player B
     │                                           │
     │  1. "let's close with these balances"     │
     │ ─────────────────────────────────────────▶│
     │                                           │
     │                                           │ 2. verify balances
     │                                           │    match latest state
     │                                           │
     │  3. "agreed" + signature                  │
     │ ◀─────────────────────────────────────────│
     │                                           │
     │ 4. submit close tx                        │
     │    with both signatures                   │
     │                                           │
     ▼                                           ▼
  ┌─────────────────────────────────────────────────┐
  │  ON-CHAIN: verify sigs, distribute funds       │
  │  A receives: balance_A                          │
  │  B receives: balance_B                          │
  └─────────────────────────────────────────────────┘
```

## close request message

```rust
struct CloseRequest {
    /// channel to close
    channel_id: [u8; 32],
    /// proposed final balances
    final_balances: Vec<u64>,
    /// requester's signature
    signature: Signature,
}

struct CloseAccept {
    /// channel to close
    channel_id: [u8; 32],
    /// agreed final balances
    final_balances: Vec<u64>,
    /// accepter's signature
    signature: Signature,
}
```

## on-chain transaction

```rust
fn cooperative_close(
    channel_id: ChannelId,
    final_balances: Vec<u64>,
    signatures: Vec<Signature>,
) -> Result<(), CloseError> {
    let channel = channels.get(&channel_id)?;

    // verify all players signed
    for (i, player) in channel.players.iter().enumerate() {
        let msg = encode_close_message(channel_id, &final_balances);
        verify_signature(player, &msg, &signatures[i])?;
    }

    // verify balances sum to deposits
    let total: u64 = final_balances.iter().sum();
    require(total == channel.total_deposited)?;

    // distribute funds
    for (player, balance) in channel.players.iter()
        .zip(final_balances.iter())
    {
        transfer(player, *balance);
    }

    // mark channel as closed
    channel.status = ChannelStatus::Closed;

    // record in reputation
    emit(ChannelClosed {
        channel_id,
        close_type: CloseType::Cooperative,
        final_balances,
    });

    Ok(())
}
```

## partial close

withdraw some funds while keeping channel open:

```rust
struct PartialWithdraw {
    /// channel to withdraw from
    channel_id: [u8; 32],
    /// new (lower) balances
    new_balances: Vec<u64>,
    /// amounts to withdraw
    withdrawals: Vec<u64>,
    /// signatures from all players
    signatures: Vec<Signature>,
}

fn partial_withdraw(request: PartialWithdraw) -> Result<(), WithdrawError> {
    // verify signatures
    // verify new_balances + withdrawals = old_balances
    // send withdrawals to players
    // update channel balances
    // channel remains open
}
```

## grace period

before closing, allow final actions:

```
close protocol:
  1. player A requests close
  2. both stop sending new game actions
  3. complete any pending reveals
  4. wait for pending state confirmations
  5. both sign close transaction
  6. submit on-chain

timing:
  - request to close: instant
  - pending completions: ~5 seconds
  - on-chain confirmation: ~12 seconds (1 block)
```

## reconnection handling

what if connection drops during close:

```
scenario: A requests close, B disconnects

A's options:
  1. wait for B to reconnect (preferred)
  2. initiate unilateral close (fallback)

reconnection window:
  - wait reasonable time (e.g., 5 minutes)
  - send close request again
  - if still no response, go unilateral
```

## close reasons

```rust
enum CloseReason {
    /// game session complete
    SessionEnd,
    /// player wants to cash out
    Cashout,
    /// player leaving table
    LeaveTable,
    /// both players going offline
    MutualExit,
    /// switching to different channel
    ChannelMigration,
}

// reason is optional metadata
// doesn't affect close mechanics
// useful for analytics
```

## batch close

close multiple channels efficiently:

```rust
/// close multiple channels in one transaction
fn batch_close(
    closes: Vec<(ChannelId, Vec<u64>, Vec<Signature>)>,
) -> Result<(), CloseError> {
    for (channel_id, balances, sigs) in closes {
        cooperative_close(channel_id, balances, sigs)?;
    }
    Ok(())
}

// saves gas when closing many channels
// useful for tournament end
```

## close verification

client verifies before signing:

```rust
fn verify_close_request(
    request: &CloseRequest,
    local_state: &ChannelState,
) -> Result<(), VerifyError> {
    // balances must match our latest state
    if request.final_balances != local_state.balances {
        return Err(VerifyError::BalanceMismatch);
    }

    // no active game (all hands complete)
    if local_state.game_state.is_some() {
        return Err(VerifyError::GameInProgress);
    }

    // signature is valid
    verify_signature(
        &request.requester,
        &encode_close_message(&request),
        &request.signature,
    )?;

    Ok(())
}
```
