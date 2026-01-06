# premium features

optional paid features enhance the experience without affecting core gameplay. free players compete on equal footing.

## philosophy

```
free tier includes:
  - full poker gameplay
  - all cryptographic security
  - basic UI
  - core functionality

premium adds:
  - convenience features
  - advanced tools
  - priority services
  - no gameplay advantage
```

## ghettobox tiers

identity service pricing:

```
free tier ($0/month):
  - {{PIN_GUESSES_FREE}} PIN attempts before lockout
  - software-mode vaults
  - {{BACKUP_INACTIVE_WARNING_DAYS}}-day inactivity warning
  - pay ${{RECOVERY_FEE_USD}} per recovery

premium ($5/month):
  - {{PIN_GUESSES_PREMIUM}} PIN attempts
  - TPM-sealed shares
  - no inactivity cleanup
  - unlimited recovery
  - priority support

pay-per-recovery:
  - free storage (unlimited)
  - ${{RECOVERY_FEE_USD}} each time you recover
  - good for occasional users
```

## client upgrades

poker client enhancements:

```
free client:
  ┌─────────────────────────────────────────┐
  │ - play poker (all variants)             │
  │ - basic table UI                         │
  │ - single table at a time                 │
  │ - basic hand history                     │
  └─────────────────────────────────────────┘

premium client ($10/month):
  ┌─────────────────────────────────────────┐
  │ - HUD with opponent statistics           │
  │   - VPIP, PFR, 3-bet %                  │
  │   - aggression factor                    │
  │   - showdown stats                       │
  │                                          │
  │ - multi-table support (up to 4)         │
  │                                          │
  │ - hand history export (CSV, JSON)       │
  │                                          │
  │ - advanced filters for table search     │
  │   - specific stakes                      │
  │   - reputation ranges                    │
  │   - time of day                          │
  │                                          │
  │ - session statistics                     │
  │   - real-time P&L                        │
  │   - hourly rate                          │
  │   - variance tracking                    │
  └─────────────────────────────────────────┘
```

## infrastructure services

network infrastructure:

```
free:
  - public relay servers (best-effort)
  - shared matchmaking
  - standard priority

premium ($2/month):
  - dedicated relay access
  - guaranteed bandwidth
  - lower latency routing
  - priority matchmaking

instant settlement (0.1% fee):
  - skip on-chain finality wait
  - instant balance updates
  - liquidity pool backing
```

## feature comparison

```
| feature              | free | premium |
|----------------------|------|---------|
| play poker           | ✓    | ✓       |
| cryptographic proofs | ✓    | ✓       |
| state channels       | ✓    | ✓       |
| reputation system    | ✓    | ✓       |
| basic UI             | ✓    | ✓       |
| HUD stats            | ✗    | ✓       |
| multi-table          | ✗    | ✓       |
| hand export          | ✗    | ✓       |
| priority relay       | ✗    | ✓       |
| unlimited recovery   | ✗    | ✓       |
```

## no pay-to-win

premium never affects fairness:

```
premium DOES NOT give:
  ✗ better cards
  ✗ higher win rates
  ✗ unfair advantages
  ✗ access to weaker opponents
  ✗ any gameplay edge

premium only gives:
  ✓ convenience
  ✓ analysis tools
  ✓ infrastructure priority
  ✓ support access
```

## subscription management

```rust
struct Subscription {
    /// user address
    user: Address,
    /// subscription tier
    tier: SubscriptionTier,
    /// expiration timestamp
    expires_at: Timestamp,
    /// auto-renew enabled
    auto_renew: bool,
}

enum SubscriptionTier {
    Free,
    GhettoboxPremium,
    ClientPremium,
    InfrastructurePremium,
    AllAccess,  // bundle discount
}

// subscription can be:
// - paid monthly (crypto)
// - paid yearly (15% discount)
// - gifted by another user
```

## bundle pricing

```
individual services:
  ghettobox premium: $5/month
  client premium: $10/month
  infrastructure: $2/month
  total: $17/month

all-access bundle: $12/month
  savings: $5/month (29% off)

annual all-access: $120/year
  savings: $24/year (17% off monthly)
```

## payment options

```
accepted payments:
  - stablecoins (USDC, DAI)
  - native token (if applicable)
  - ETH (converted at market rate)

payment flow:
  1. select subscription
  2. approve token spend
  3. subscription contract charges
  4. features unlocked immediately
```

## revenue allocation

```
premium revenue goes to:

  40% → development fund
        (ongoing improvements)

  30% → infrastructure costs
        (servers, bandwidth)

  20% → community treasury
        (grants, rewards)

  10% → marketing
        (user acquisition)
```
