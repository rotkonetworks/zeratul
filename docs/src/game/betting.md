# betting rounds

texas hold'em has four betting rounds: preflop, flop, turn, and river. each round follows specific rules.

## betting structure

```
no-limit hold'em:
  - minimum bet = big blind
  - minimum raise = previous raise or big blind
  - maximum bet = all chips (all-in)

pot-limit hold'em:
  - minimum bet = big blind
  - maximum bet = current pot size
  - raises capped at pot

fixed-limit hold'em:
  - preflop/flop: bets = 1 big blind
  - turn/river: bets = 2 big blinds
  - max 4 bets per round
```

## preflop

first betting round after hole cards dealt:

```
preflop flow:

  1. dealer posts small blind ({{SB}})
  2. other player posts big blind ({{BB}})

  3. hole cards dealt
     - 2 cards to each player
     - encrypted, only recipient can see

  4. action starts with dealer (small blind)
     options: fold, call, raise

  5. big blind responds
     options: fold, call (if no raise), check (if just called), raise

  6. action continues until both players:
     - have equal bets, and
     - have acted at least once

example:
  A posts SB: $0.50
  B posts BB: $1.00
  A raises to $3
  B calls $3 (adds $2)
  → preflop complete, both at $3
```

## flop

second betting round after 3 community cards:

```
flop flow:

  1. burn one card (not revealed)
  2. reveal 3 community cards

  3. action starts with non-dealer (BB)
     options: check, bet

  4. dealer responds
     options: check (if no bet), call, raise, fold

  5. action continues until complete

example:
  B checks
  A bets $5
  B raises to $15
  A calls $15
  → flop complete
```

## turn

third betting round after 4th community card:

```
turn flow:

  1. burn one card
  2. reveal 4th community card

  3. same betting structure as flop
  4. bets typically larger

example:
  B bets $25
  A calls $25
  → turn complete
```

## river

final betting round after 5th community card:

```
river flow:

  1. burn one card
  2. reveal 5th community card

  3. same betting structure
  4. final chance to bet

  5. if called, goes to showdown
     if fold, other player wins

example:
  B bets $50
  A raises all-in to $120
  B calls $70 more
  → river complete, showdown
```

## action types

```rust
enum BettingAction {
    /// give up hand, lose pot
    Fold,

    /// match current bet (no raise)
    Check,

    /// match current bet
    Call,

    /// increase the bet
    Bet { amount: u64 },

    /// increase after a bet
    Raise { total: u64 },

    /// bet all remaining chips
    AllIn,
}
```

## action validation

```rust
fn validate_action(
    state: &GameState,
    player: usize,
    action: &BettingAction,
) -> Result<(), ActionError> {
    // verify it's player's turn
    require(state.action_on == player)?;

    let current_bet = state.current_bet;
    let player_bet = state.player_bets[player];
    let stack = state.stacks[player];
    let to_call = current_bet - player_bet;

    match action {
        BettingAction::Fold => Ok(()),  // always valid

        BettingAction::Check => {
            require(to_call == 0)?;  // can only check if no bet to call
            Ok(())
        }

        BettingAction::Call => {
            require(to_call > 0)?;  // must have something to call
            require(stack >= to_call)?;  // must afford it
            Ok(())
        }

        BettingAction::Bet { amount } => {
            require(current_bet == 0)?;  // can only bet if no bet yet
            require(*amount >= state.big_blind)?;  // min bet
            require(stack >= *amount)?;  // must afford
            Ok(())
        }

        BettingAction::Raise { total } => {
            require(current_bet > 0)?;  // must be a bet to raise
            let raise_amount = *total - current_bet;
            require(raise_amount >= state.last_raise)?;  // min raise
            require(stack >= *total - player_bet)?;  // must afford
            Ok(())
        }

        BettingAction::AllIn => {
            // always valid if player has chips
            require(stack > 0)?;
            Ok(())
        }
    }
}
```

## state update

each action updates game state:

```rust
fn apply_betting_action(
    state: &mut GameState,
    player: usize,
    action: BettingAction,
) {
    match action {
        BettingAction::Fold => {
            state.folded[player] = true;
            state.hand_over = true;
            // other player wins
        }

        BettingAction::Check => {
            // no state change except turn
        }

        BettingAction::Call => {
            let to_call = state.current_bet - state.player_bets[player];
            state.stacks[player] -= to_call;
            state.player_bets[player] = state.current_bet;
            state.pot += to_call;
        }

        BettingAction::Bet { amount } | BettingAction::Raise { total: amount } => {
            let to_add = amount - state.player_bets[player];
            state.last_raise = amount - state.current_bet;
            state.current_bet = amount;
            state.stacks[player] -= to_add;
            state.player_bets[player] = amount;
            state.pot += to_add;
        }

        BettingAction::AllIn => {
            let all_in_amount = state.stacks[player];
            state.stacks[player] = 0;
            state.player_bets[player] += all_in_amount;
            state.pot += all_in_amount;
            if state.player_bets[player] > state.current_bet {
                state.last_raise = state.player_bets[player] - state.current_bet;
                state.current_bet = state.player_bets[player];
            }
        }
    }

    // advance action
    state.action_on = 1 - player;
    state.actions_this_round += 1;
}
```

## round completion

when is a betting round over:

```rust
fn is_round_complete(state: &GameState) -> bool {
    // someone folded
    if state.folded.iter().any(|&f| f) {
        return true;
    }

    // all-in confrontation
    if state.stacks.iter().filter(|&&s| s > 0).count() <= 1 {
        return true;
    }

    // both have acted and bets equal
    state.actions_this_round >= 2 &&
        state.player_bets[0] == state.player_bets[1]
}
```

## pot calculation

```rust
fn calculate_pot(state: &GameState) -> u64 {
    state.player_bets.iter().sum()
}

// side pots for all-in situations
fn calculate_side_pots(state: &GameState) -> Vec<Pot> {
    // see pot-distribution.md for details
}
```
