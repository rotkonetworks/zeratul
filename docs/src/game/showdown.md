# showdown

showdown occurs when betting is complete and multiple players remain. players reveal hands to determine the winner.

## showdown trigger

```
showdown happens when:
  - river betting complete
  - 2+ players haven't folded
  - all bets matched (or all-in)

showdown doesn't happen when:
  - all but one player folded
  - hand ends early (fold wins)
```

## reveal order

```
standard reveal order:
  1. last aggressor shows first
     (player who made final bet/raise)

  2. other players can:
     - show to try to win
     - muck (hide cards, forfeit)

heads-up simplification:
  - both players must reveal
  - simultaneous or in position order
  - no mucking if called
```

## reveal protocol

cryptographic card reveal:

```
reveal process for player A's hole cards:

  1. A broadcasts decryption shares
     - share for position 0 (first hole card)
     - share for position 1 (second hole card)
     - ZK proofs of validity

  2. B verifies proofs

  3. B provides own decryption shares
     (needed because of joint encryption)

  4. both can now compute:
     card_0 = decrypt(enc_0, share_A_0, share_B_0)
     card_1 = decrypt(enc_1, share_A_1, share_B_1)

  5. verify cards are valid (one of 52)
```

```rust
struct ShowdownReveal {
    /// player revealing
    player: PlayerId,
    /// decryption shares for hole cards
    shares: Vec<DecryptionShare>,
    /// proofs that shares are correct
    proofs: Vec<DecryptionProof>,
}

fn process_reveal(
    deck: &EncryptedDeck,
    reveal: &ShowdownReveal,
    positions: &[usize],  // hole card positions
) -> Result<Vec<Card>, RevealError> {
    // verify proofs
    for (share, proof) in reveal.shares.iter().zip(reveal.proofs.iter()) {
        verify_decryption_proof(share, proof)?;
    }

    // combine with other player's shares to get cards
    let cards = positions.iter()
        .zip(reveal.shares.iter())
        .map(|(&pos, share)| {
            let other_share = get_other_player_share(pos)?;
            let card_point = decrypt_card(&deck[pos], share, other_share);
            Card::from_point(card_point)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(cards)
}
```

## hand comparison

after reveals, compare hands:

```rust
fn determine_winner(
    community: &[Card; 5],
    player_a_hole: &[Card; 2],
    player_b_hole: &[Card; 2],
) -> ShowdownResult {
    // evaluate best 5-card hands
    let hand_a = evaluate_best_hand(community, player_a_hole);
    let hand_b = evaluate_best_hand(community, player_b_hole);

    match hand_a.cmp(&hand_b) {
        Ordering::Greater => ShowdownResult::WinnerA,
        Ordering::Less => ShowdownResult::WinnerB,
        Ordering::Equal => ShowdownResult::Split,
    }
}

fn evaluate_best_hand(
    community: &[Card; 5],
    hole: &[Card; 2],
) -> EvaluatedHand {
    // combine 7 cards
    let all_cards = [
        community[0], community[1], community[2],
        community[3], community[4],
        hole[0], hole[1],
    ];

    // find best 5-card combination
    evaluate_7_cards(&all_cards)
}
```

## showdown state

```rust
struct ShowdownState {
    /// community cards (revealed earlier)
    community: [Card; 5],

    /// revealed hole cards per player
    revealed: HashMap<PlayerId, [Card; 2]>,

    /// evaluated hands
    evaluations: HashMap<PlayerId, EvaluatedHand>,

    /// winner(s)
    winners: Vec<PlayerId>,

    /// pot amounts won
    winnings: HashMap<PlayerId, u64>,
}
```

## verification

all players verify showdown:

```rust
fn verify_showdown(
    state: &ShowdownState,
    claims: &ShowdownClaims,
) -> Result<(), ShowdownError> {
    // verify all hole cards are valid
    for (player, cards) in &state.revealed {
        for card in cards {
            if !card.is_valid() {
                return Err(ShowdownError::InvalidCard);
            }
        }
    }

    // re-evaluate hands locally
    for (player, claimed_eval) in &claims.evaluations {
        let hole = state.revealed.get(player)?;
        let actual_eval = evaluate_best_hand(&state.community, hole);
        if actual_eval != *claimed_eval {
            return Err(ShowdownError::WrongEvaluation);
        }
    }

    // verify winner determination
    let actual_winners = determine_winners(&state.evaluations);
    if actual_winners != claims.winners {
        return Err(ShowdownError::WrongWinner);
    }

    Ok(())
}
```

## muck rules

when players can hide cards:

```
can muck:
  - if you fold before showdown
  - if you lose and no one calls your hand
  - in multi-way pots, if beaten before your turn

cannot muck:
  - if you win (must show to claim pot)
  - if called heads-up (must show)
  - if dispute requires verification
```

## all-in showdown

special case: all players all-in before river:

```
all-in showdown:
  1. no more betting possible
  2. run out remaining community cards
  3. both players reveal immediately
  4. winner determined by final board

reveals happen:
  - after all cards are dealt
  - no information advantage
  - pure evaluation
```

## showdown state update

final channel state after showdown:

```rust
fn apply_showdown_result(
    channel_state: &mut ChannelState,
    game_state: &GameState,
    result: &ShowdownResult,
) {
    // distribute pot to winner(s)
    match result {
        ShowdownResult::WinnerA => {
            channel_state.balances[0] += game_state.pot;
        }
        ShowdownResult::WinnerB => {
            channel_state.balances[1] += game_state.pot;
        }
        ShowdownResult::Split => {
            let half = game_state.pot / 2;
            channel_state.balances[0] += half;
            channel_state.balances[1] += game_state.pot - half;
            // odd chip to first position
        }
    }

    // clear game state
    channel_state.game_state_hash = None;

    // increment version
    channel_state.version += 1;
}
```
