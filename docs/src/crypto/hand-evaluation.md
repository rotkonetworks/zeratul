# hand evaluation

determining the winner requires evaluating poker hands. evaluation happens client-side with results verifiable by all players.

## hand rankings

standard poker hand rankings (highest to lowest):

```
1. royal flush      A K Q J 10 (same suit)
2. straight flush   5 consecutive (same suit)
3. four of a kind   4 cards same rank
4. full house       3 of a kind + pair
5. flush            5 cards same suit
6. straight         5 consecutive ranks
7. three of a kind  3 cards same rank
8. two pair         2 different pairs
9. one pair         2 cards same rank
10. high card       nothing else
```

## hand representation

```rust
/// card representation
#[derive(Clone, Copy, PartialEq, Eq)]
struct Card {
    rank: u8,  // 2-14 (2-10, J=11, Q=12, K=13, A=14)
    suit: u8,  // 0-3 (clubs, diamonds, hearts, spades)
}

/// evaluated hand
struct EvaluatedHand {
    /// hand category (1-10, 10 = royal flush)
    category: u8,
    /// tiebreaker values (category-specific)
    kickers: [u8; 5],
}

impl Ord for EvaluatedHand {
    fn cmp(&self, other: &Self) -> Ordering {
        self.category.cmp(&other.category)
            .then_with(|| self.kickers.cmp(&other.kickers))
    }
}
```

## evaluation algorithm

find best 5-card hand from 7 cards (2 hole + 5 community):

```rust
fn evaluate_hand(cards: &[Card; 7]) -> EvaluatedHand {
    // generate all 21 combinations of 5 cards
    let combinations = cards.iter()
        .combinations(5)
        .collect::<Vec<_>>();

    // evaluate each combination
    combinations.iter()
        .map(|combo| evaluate_five(combo))
        .max()
        .unwrap()
}

fn evaluate_five(cards: &[Card; 5]) -> EvaluatedHand {
    let flush = is_flush(cards);
    let straight = is_straight(cards);

    match (flush, straight) {
        (true, Some(high)) if high == 14 => royal_flush(),
        (true, Some(high)) => straight_flush(high),
        _ => {
            let counts = rank_counts(cards);
            match counts.as_slice() {
                [4, 1] => four_of_a_kind(cards),
                [3, 2] => full_house(cards),
                _ if flush => flush_hand(cards),
                _ if straight.is_some() => straight_hand(straight.unwrap()),
                [3, 1, 1] => three_of_a_kind(cards),
                [2, 2, 1] => two_pair(cards),
                [2, 1, 1, 1] => one_pair(cards),
                _ => high_card(cards),
            }
        }
    }
}
```

## optimized lookup table

for performance, use precomputed lookup tables:

```rust
/// 7-card evaluation using perfect hash
fn evaluate_7cards_fast(cards: &[Card; 7]) -> u16 {
    // encode cards as bitmasks
    let suits: [u16; 4] = encode_suits(cards);
    let ranks: u64 = encode_ranks(cards);

    // check for flushes first
    for &suit_mask in &suits {
        if suit_mask.count_ones() >= 5 {
            return FLUSH_TABLE[suit_mask as usize];
        }
    }

    // use rank lookup table
    RANK_TABLE[perfect_hash(ranks)]
}

// lookup tables: ~150KB total
// evaluation time: ~50ns per hand
```

## showdown protocol

```
1. all remaining players reveal hole cards
   - winner must reveal to claim pot
   - losers can muck (keep hidden)

2. client evaluates all hands locally

3. comparison determines winner(s)
   - ties split pot equally
   - odd chip goes to first position

4. state update reflects new balances
   - signed by all players
   - becomes channel state
```

## verification

any player can verify hand evaluation:

```rust
fn verify_showdown(
    community: &[Card; 5],
    revealed_hands: &[(PlayerId, [Card; 2])],
    claimed_winner: PlayerId,
) -> Result<(), ShowdownError> {
    // evaluate all hands
    let evaluations: Vec<_> = revealed_hands.iter()
        .map(|(id, hole)| {
            let seven = combine(community, hole);
            (*id, evaluate_hand(&seven))
        })
        .collect();

    // find actual winner
    let actual_winner = evaluations.iter()
        .max_by_key(|(_, eval)| eval)
        .unwrap().0;

    if actual_winner != claimed_winner {
        return Err(ShowdownError::WrongWinner);
    }

    Ok(())
}
```

## side pots

when players have different stack sizes:

```
example:
  player A: $50 all-in
  player B: $100 all-in
  player C: calls $100

main pot: $150 (A, B, C eligible)
side pot: $100 (B, C eligible)

if A wins: A gets main pot, B/C contest side pot
if B wins: B gets both pots
```

```rust
struct Pot {
    amount: u64,
    eligible: Vec<PlayerId>,
}

fn calculate_pots(bets: &[(PlayerId, u64)]) -> Vec<Pot> {
    // sort by bet size
    let mut sorted = bets.to_vec();
    sorted.sort_by_key(|(_, bet)| *bet);

    let mut pots = Vec::new();
    let mut prev_level = 0;

    for (_, bet) in &sorted {
        if *bet > prev_level {
            let increment = bet - prev_level;
            let eligible: Vec<_> = sorted.iter()
                .filter(|(_, b)| *b >= *bet)
                .map(|(id, _)| *id)
                .collect();

            pots.push(Pot {
                amount: increment * eligible.len() as u64,
                eligible,
            });
            prev_level = *bet;
        }
    }

    pots
}
```

## tie breaking

```
same category: compare kickers in order

example (two pair):
  hand A: K K 7 7 3
  hand B: K K 7 7 5

  category: equal (two pair)
  first pair: equal (kings)
  second pair: equal (sevens)
  kicker: B wins (5 > 3)

split pot:
  if all kickers equal, split pot
  odd chip to earliest position
```
