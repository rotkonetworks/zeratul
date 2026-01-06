# business model

zk.poker is peer-to-peer. there is no house edge or rake on gameplay. revenue comes from infrastructure services and dispute resolution.

## philosophy

```
traditional online poker:
  house takes 2-5% rake from every pot
  house profits from players' losses
  house has incentive to maximize volume

zk.poker:
  players play against each other directly
  no rake = no house edge
  protocol profits from services, not gameplay
```

## revenue streams

### 1. dispute fees

when players can't agree (timeout, attempted cheat), disputes go on-chain:

```
dispute fee structure:
  ┌────────────────────────────────────────────────┐
  │ normal game: $0                                │
  │   - players cooperate                          │
  │   - channel closes cleanly                     │
  └────────────────────────────────────────────────┘

  ┌────────────────────────────────────────────────┐
  │ dispute: loser pays {{DISPUTE_FEE_PERCENT}}% of pot              │
  │   - covers gas costs                           │
  │   - dispute resolution fee                     │
  │   - winner receives full pot                   │
  └────────────────────────────────────────────────┘

bond system:
  - each player deposits ${{BOND_AMOUNT_USD}} bond when joining table
  - clean exit = bond returned
  - dispute loser's bond covers fees
  - winner is always made whole
```

### 2. ghettobox services

identity infrastructure has costs:

```
free tier:
  - {{PIN_GUESSES_FREE}} PIN guesses
  - software-mode vaults
  - {{BACKUP_INACTIVE_WARNING_DAYS}}-day inactive warning

premium ($5/month):
  - {{PIN_GUESSES_PREMIUM}} PIN guesses
  - TPM-sealed shares
  - no inactive cleanup
  - priority support

pay-per-recovery (${{RECOVERY_FEE_USD}}):
  - free storage
  - pay only when you need to recover
  - most users never pay
```

### 3. premium features

optional client upgrades:

```
free client:
  - play poker
  - basic UI
  - single table

premium client ($10/month):
  - HUD with opponent stats
  - hand history export
  - multi-table support
  - advanced filters
```

### 4. infrastructure fees

running reliable infrastructure costs money:

```
relay nodes:
  - free: use public relays (best-effort)
  - $2/month: dedicated relay access

instant settlement:
  - free: wait for on-chain finality
  - 0.1% fee: instant liquidity pool settlement
```

## incentive alignment

| actor | incentive | alignment |
|-------|-----------|-----------|
| players | fair games, no rake | protocol provides this |
| operators | revenue | disputes + services |
| griefers | waste time | pay dispute fees |
| good players | smooth UX | free gameplay |

## cost structure

```
operational costs:
  - 3 vault nodes (TPM servers)
  - relay infrastructure
  - RPC nodes / indexers
  - development

revenue needed:
  ~$X/month to break even

revenue sources:
  - disputes (scales with bad behavior)
  - premium subscriptions (scales with users)
  - recovery fees (rare events)
```

## comparison

| platform | rake | dispute model |
|----------|------|---------------|
| PokerStars | 2.5-5% | centralized |
| zk.poker | 0% | on-chain, loser pays |
| home game | 0% | trust |
