# dispute fees

dispute fees fund the on-chain resolution system. they create economic disincentives against bad behavior.

## fee structure

```
dispute fee: {{DISPUTE_FEE_PERCENT}}% of pot

example:
  pot size: $100
  dispute fee: $2
  winner receives: $100 (full pot)
  loser pays: $2 (from bond)

fee covers:
  - on-chain gas costs
  - protocol revenue
  - incentive alignment
```

## bond system

players post bonds when joining tables:

```rust
struct TableBond {
    /// amount per player
    amount: u64,  // ${{BOND_AMOUNT_USD}}
    /// when bond was posted
    posted_at: BlockNumber,
}

// bond lifecycle:
// 1. player joins table → bond locked
// 2. game plays out normally → bond returned
// 3. dispute occurs → bond covers fees
```

## fee calculation

```rust
fn calculate_dispute_fee(
    pot_size: u64,
    dispute_type: DisputeType,
) -> u64 {
    let base_rate = {{DISPUTE_FEE_PERCENT}} as u64;
    let fee = pot_size * base_rate / 100;

    // minimum fee for gas coverage
    let min_fee = estimate_gas_cost();

    fee.max(min_fee)
}

fn estimate_gas_cost() -> u64 {
    // roughly: dispute tx gas * current gas price
    // varies by chain conditions
    // ~$0.50 - $2.00 typically
}
```

## who pays

```
dispute scenarios:

timeout (player B didn't respond):
  - B loses by timeout
  - B's bond pays fee
  - A receives: full pot
  - B receives: bond - fee

fraud (player B submitted old state):
  - B caught cheating
  - B's bond pays fee
  - A receives: full pot
  - B receives: nothing (severe penalty)

challenge (A submitted state, B had newer):
  - A's challenge rejected
  - A pays fee (tried to cheat)
  - B receives: full pot
```

## fee distribution

```
fee breakdown:

  ┌────────────────────────────────────────────────┐
  │                 DISPUTE FEE                     │
  │                                                 │
  │  50% → gas reimbursement                       │
  │        (whoever submitted winning tx)           │
  │                                                 │
  │  50% → protocol treasury                       │
  │        (funds development, infrastructure)      │
  └────────────────────────────────────────────────┘
```

## incentive analysis

```
rational player analysis:

  griefing attempt:
    cost: ${{BOND_AMOUNT_USD}} bond + reputation damage
    benefit: none (opponent still gets their money)
    result: negative EV, don't grief

  cheat attempt:
    cost: ${{BOND_AMOUNT_USD}} bond + reputation + banned
    benefit: pot (if not caught)
    risk: high (cryptographic proofs)
    result: negative EV, don't cheat

  normal play:
    cost: none (no disputes)
    benefit: fair game
    result: optimal strategy
```

## fee comparison

```
| platform    | dispute model         | player cost |
|-------------|----------------------|-------------|
| PokerStars  | centralized arbiter  | 0% (opaque) |
| zk.poker    | on-chain, loser pays | {{DISPUTE_FEE_PERCENT}}% of pot  |
| home game   | social trust         | 0%          |

zk.poker advantages:
  - transparent rules
  - provable outcomes
  - no trusted third party
```

## bond recovery

timing for bond return:

```
cooperative close:
  - instant bond return
  - no waiting period

unilateral close:
  - wait {{DISPUTE_TIMEOUT_BLOCKS}} blocks
  - challenge period for other player
  - then bond returns

dispute resolution:
  - wait for resolution
  - winner's bond returns
  - loser's bond covers fees
```

## fee exemptions

some disputes don't charge fees:

```
no fee cases:
  - honest disagreement (both valid states)
  - network issues (not player's fault)
  - bug in client (reported responsibly)

fee applies:
  - timeout (player should be responsive)
  - old state submission (intentional cheat)
  - invalid fraud proof (wasting time)
```

## adjusting fees

fees can be adjusted by governance:

```
fee adjustment process:
  1. proposal submitted
  2. community discussion
  3. on-chain vote
  4. parameter updated

considerations:
  - gas price fluctuations
  - griefing economics
  - user experience
  - protocol sustainability
```
