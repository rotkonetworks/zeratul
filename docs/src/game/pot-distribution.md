# pot distribution

pot distribution handles complex scenarios: side pots, split pots, and odd chips.

## simple pot

most common: one winner takes all:

```
example:
  player A: $100 in pot
  player B: $100 in pot
  total pot: $200

  A wins → A receives $200
  B wins → B receives $200
```

## split pot

identical hands split equally:

```
example:
  board: A♠ K♠ Q♠ J♠ 10♠ (royal flush on board)
  A hole: 2♣ 3♣
  B hole: 4♦ 5♦

  both have same hand (board plays)
  pot: $200
  A receives: $100
  B receives: $100
```

## odd chip rule

when pot doesn't divide evenly:

```
example:
  pot: $201 (split two ways)
  $201 / 2 = $100.50

  but we use integer chips:
  A receives: $101 (first position)
  B receives: $100

odd chip goes to:
  - first position clockwise from button
  - or first alphabetically (by address)
  - consistent rule prevents disputes
```

```rust
fn distribute_split_pot(
    pot: u64,
    winners: &[PlayerId],
    positions: &[usize],
) -> HashMap<PlayerId, u64> {
    let share = pot / winners.len() as u64;
    let remainder = pot % winners.len() as u64;

    let mut payouts = HashMap::new();

    // sort winners by position
    let mut sorted: Vec<_> = winners.iter()
        .map(|&p| (positions[p], p))
        .collect();
    sorted.sort_by_key(|(pos, _)| *pos);

    for (i, (_, player)) in sorted.iter().enumerate() {
        let extra = if i < remainder as usize { 1 } else { 0 };
        payouts.insert(*player, share + extra);
    }

    payouts
}
```

## side pots

when players have different stack sizes:

```
example setup:
  A has: $50 (goes all-in)
  B has: $150

betting:
  A bets $50 (all-in)
  B calls $50

pots:
  main pot: $100 (A: $50, B: $50)
  A eligible for: main pot only

if A wins: A gets $100
if B wins: B gets $100 (same result heads-up)
```

## complex side pots

three-way example (for future multi-table):

```
setup:
  A stack: $30
  B stack: $70
  C stack: $150

all players go all-in:

main pot: $90 (A, B, C each contributed $30)
  eligible: A, B, C

side pot 1: $80 (B and C each contributed $40 more)
  eligible: B, C only

if A wins main:
  A receives: $90
  B vs C for side pot 1

if B wins everything:
  B receives: $90 + $80 = $170

if C wins everything:
  C receives: $90 + $80 = $170
```

```rust
struct SidePot {
    /// amount in this pot
    amount: u64,
    /// players eligible for this pot
    eligible: Vec<PlayerId>,
}

fn calculate_side_pots(
    player_contributions: &[(PlayerId, u64)],
) -> Vec<SidePot> {
    // sort by contribution
    let mut sorted = player_contributions.to_vec();
    sorted.sort_by_key(|(_, contrib)| *contrib);

    let mut pots = Vec::new();
    let mut prev_level = 0;

    for (_, contribution) in &sorted {
        if *contribution > prev_level {
            let increment = contribution - prev_level;
            let eligible: Vec<_> = sorted.iter()
                .filter(|(_, c)| *c >= *contribution)
                .map(|(p, _)| *p)
                .collect();

            pots.push(SidePot {
                amount: increment * eligible.len() as u64,
                eligible,
            });

            prev_level = *contribution;
        }
    }

    pots
}
```

## pot awarding

determine winners for each pot:

```rust
fn award_pots(
    pots: &[SidePot],
    hand_rankings: &HashMap<PlayerId, EvaluatedHand>,
) -> HashMap<PlayerId, u64> {
    let mut winnings: HashMap<PlayerId, u64> = HashMap::new();

    for pot in pots {
        // find best hand among eligible players
        let best_hand = pot.eligible.iter()
            .filter_map(|p| hand_rankings.get(p))
            .max();

        // find all players with that hand
        let winners: Vec<_> = pot.eligible.iter()
            .filter(|p| hand_rankings.get(*p) == best_hand)
            .cloned()
            .collect();

        // distribute pot among winners
        let share = pot.amount / winners.len() as u64;
        let remainder = pot.amount % winners.len() as u64;

        for (i, winner) in winners.iter().enumerate() {
            let extra = if i < remainder as usize { 1 } else { 0 };
            *winnings.entry(*winner).or_insert(0) += share + extra;
        }
    }

    winnings
}
```

## verification

pot math must be verifiable:

```rust
fn verify_pot_distribution(
    initial_stacks: &[u64],
    contributions: &[u64],
    final_stacks: &[u64],
) -> Result<(), VerifyError> {
    // total chips must be conserved
    let initial_total: u64 = initial_stacks.iter().sum();
    let final_total: u64 = final_stacks.iter().sum();

    if initial_total != final_total {
        return Err(VerifyError::ChipsNotConserved);
    }

    // contributions must match stack changes
    for i in 0..initial_stacks.len() {
        let expected_final = initial_stacks[i] - contributions[i] + winnings[i];
        if final_stacks[i] != expected_final {
            return Err(VerifyError::StackMismatch);
        }
    }

    Ok(())
}
```

## rake-free

```
zk.poker is peer-to-peer:
  - no house edge
  - no rake taken
  - winner receives 100% of pot

traditional poker sites:
  - 2.5% - 5% rake
  - reduces player EV
  - house profits from volume

zk.poker:
  - 0% rake
  - players keep all winnings
  - revenue from other sources
```

## state update

channel state after pot distribution:

```rust
fn finalize_hand(
    channel_state: &mut ChannelState,
    winnings: &HashMap<PlayerId, u64>,
) {
    // apply winnings to balances
    for (player, amount) in winnings {
        let idx = player_index(player);
        channel_state.balances[idx] += amount;
    }

    // clear game state
    channel_state.game_state_hash = None;

    // both players sign new state
    // increment version
    channel_state.version += 1;
}
```
