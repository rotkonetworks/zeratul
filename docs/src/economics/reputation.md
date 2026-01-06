# reputation system

all game outcomes are recorded on-chain, creating a permanent, verifiable reputation for each address.

## on-chain data

every channel close records:

```rust
struct ChannelClose {
    /// channel id
    channel_id: [u8; 32],
    /// participating players
    players: Vec<Address>,
    /// how it closed
    close_type: CloseType,
    /// final balances
    final_balances: Vec<u64>,
    /// block number
    block: u64,
}

enum CloseType {
    /// both players signed
    Cooperative,
    /// timeout - player didn't respond
    Timeout { who: Address },
    /// dispute - invalid state submitted
    Dispute { cheater: Address },
}
```

## derived reputation

anyone can compute reputation from on-chain history:

```rust
struct Reputation {
    /// total games played
    games_played: u64,
    /// games that closed cooperatively
    games_completed: u64,
    /// disputes where this address was at fault
    disputes_lost: u64,
    /// timeouts caused by this address
    timeouts: u64,
    /// timestamp of most recent timeout
    last_timeout: Option<u64>,
}

impl Reputation {
    fn completion_rate(&self) -> f64 {
        self.games_completed as f64 / self.games_played as f64
    }

    fn dispute_rate(&self) -> f64 {
        self.disputes_lost as f64 / self.games_played as f64
    }

    fn timeout_rate(&self) -> f64 {
        self.timeouts as f64 / self.games_played as f64
    }

    fn score(&self) -> u8 {
        // 100 = perfect, 0 = terrible
        let base = 100.0;
        let penalty = (self.disputes_lost * 10 + self.timeouts * 5) as f64;
        (base - penalty).max(0.0).min(100.0) as u8
    }
}
```

## table filtering

tables can require minimum reputation:

```rust
struct TableSettings {
    /// minimum reputation score (0-100)
    min_reputation: u8,  // default: {{MIN_REPUTATION_DEFAULT}}
    /// maximum timeout rate
    max_timeout_rate: f64,  // default: 5%
    /// minimum games played
    min_games: u64,  // default: 0 (new players welcome)
}
```

example configurations:

| table type | min_rep | min_games | notes |
|------------|---------|-----------|-------|
| beginner | 0 | 0 | anyone can join |
| standard | {{MIN_REPUTATION_DEFAULT}} | 10 | some history required |
| high stakes | 95 | 100 | serious players only |
| private | 98 | 50 | invitation + reputation |

## reputation decay

old incidents matter less:

```
decay formula:
  effective_penalty = base_penalty * decay_factor
  decay_factor = 0.5 ^ (months_since_incident / 6)

example:
  timeout 6 months ago: 50% weight
  timeout 12 months ago: 25% weight
  timeout 24 months ago: 6.25% weight
```

this allows players to recover from mistakes over time.

## sybil resistance

creating new accounts doesn't help:

```
new account:
  - 0 games played
  - can only join "beginner" tables
  - must build reputation from scratch
  - premium tables require history

reputation is earned, not bought
```

## griefing economics

griefing has diminishing returns:

```
griefer strategy:
  1. join table
  2. timeout intentionally
  3. ??? profit ???

reality:
  - loses ${{BOND_AMOUNT_USD}} bond per timeout
  - reputation drops
  - can't join good tables
  - stuck playing with other griefers
  - no economic benefit

cost of griefing 10 tables: ${{BOND_AMOUNT_USD}} × 10 = $50
benefit: none
```

## querying reputation

```rust
// query chain for address history
let history = chain.get_channel_closes(address).await?;

// compute reputation
let rep = Reputation::from_history(&history);

println!("score: {}/100", rep.score());
println!("completion: {:.1}%", rep.completion_rate() * 100.0);
println!("disputes: {}", rep.disputes_lost);
println!("timeouts: {}", rep.timeouts);
```

## social layer

reputation enables social features:

- **friends list**: track players you trust
- **block list**: avoid known griefers
- **table ratings**: tables get reputation too
- **leaderboards**: highest completion rates
