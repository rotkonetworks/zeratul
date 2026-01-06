# signing policy

signing policy controls when the client auto-signs and when it prompts for confirmation. the goal is smooth gameplay with security for sensitive actions.

## default policy

```rust
struct SigningPolicy {
    /// auto-sign channel state updates
    auto_channel_updates: bool,  // true

    /// auto-sign poker actions (bet, fold, etc)
    auto_poker_actions: bool,  // true

    /// auto-sign shuffle proofs
    auto_shuffle_proofs: bool,  // true

    /// confirm before withdrawals
    confirm_withdrawals: bool,  // true

    /// confirm before transfers
    confirm_transfers: bool,  // true

    /// require PIN above this USD amount
    pin_threshold_usd: Option<u64>,  // None

    /// session timeout in seconds
    session_timeout_secs: u64,  // 14400 (4 hours)
}

impl Default for SigningPolicy {
    fn default() -> Self {
        SigningPolicy {
            auto_channel_updates: true,
            auto_poker_actions: true,
            auto_shuffle_proofs: true,
            confirm_withdrawals: true,
            confirm_transfers: true,
            pin_threshold_usd: None,
            session_timeout_secs: 14400,
        }
    }
}
```

## auto-sign actions

these happen without user interaction:

```
auto-signed (during gameplay):
  ✓ posting blinds
  ✓ betting / raising / calling
  ✓ folding
  ✓ check
  ✓ shuffle proofs
  ✓ card reveals
  ✓ state updates

why auto-sign:
  - these are the actions user chose
  - interrupting would ruin UX
  - popup on every bet = unusable
  - user already decided to play
```

## confirmation required

these prompt the user:

```
requires confirmation:
  ⚠ withdrawing funds from channel
  ⚠ transferring to another address
  ⚠ closing channel
  ⚠ changing account settings

confirmation dialog:
  ┌─────────────────────────────────────────────┐
  │  Confirm Withdrawal                          │
  │                                              │
  │  Amount: $250.00                             │
  │  To: 0x1234...5678                           │
  │                                              │
  │  [Cancel]            [Confirm]              │
  └─────────────────────────────────────────────┘
```

## PIN threshold

optional high-value protection:

```rust
// with threshold set to $100:
let policy = SigningPolicy {
    pin_threshold_usd: Some(100),
    ..Default::default()
};

// behavior:
// - $50 bet: auto-sign
// - $150 bet: prompt for PIN

// good for:
// - shared devices
// - high-stakes tables
// - extra peace of mind
```

## paranoid mode

maximum security configuration:

```rust
let paranoid = SigningPolicy {
    auto_channel_updates: false,
    auto_poker_actions: false,
    auto_shuffle_proofs: false,
    confirm_withdrawals: true,
    confirm_transfers: true,
    pin_threshold_usd: Some(0),  // always require PIN
    session_timeout_secs: 3600,  // 1 hour
};

// confirms every action
// very secure, very annoying
// not recommended for actual play
```

## convenience mode

minimum friction:

```rust
let convenient = SigningPolicy {
    auto_channel_updates: true,
    auto_poker_actions: true,
    auto_shuffle_proofs: true,
    confirm_withdrawals: false,  // risky!
    confirm_transfers: false,   // risky!
    pin_threshold_usd: None,
    session_timeout_secs: 28800,  // 8 hours
};

// only use on secure, personal device
// not recommended for large balances
```

## action classification

```rust
enum ActionSensitivity {
    /// safe, auto-sign
    Gameplay,
    /// careful, maybe confirm
    Financial,
    /// dangerous, always confirm
    Critical,
}

fn classify_action(action: &SignableAction) -> ActionSensitivity {
    match action {
        SignableAction::Bet { .. } => Gameplay,
        SignableAction::Fold => Gameplay,
        SignableAction::ShuffleProof { .. } => Gameplay,
        SignableAction::StateUpdate { .. } => Gameplay,

        SignableAction::Withdraw { amount, .. } => {
            if *amount > threshold { Critical } else { Financial }
        }
        SignableAction::Transfer { .. } => Critical,
        SignableAction::CloseChannel { .. } => Financial,
    }
}
```

## policy enforcement

```rust
async fn sign_action(
    session: &AuthSession,
    policy: &SigningPolicy,
    action: &SignableAction,
) -> Result<Signature, SignError> {
    let sensitivity = classify_action(action);

    match sensitivity {
        ActionSensitivity::Gameplay => {
            if policy.auto_poker_actions {
                Ok(session.sign(&action.to_bytes()))
            } else {
                prompt_confirmation(action).await?;
                Ok(session.sign(&action.to_bytes()))
            }
        }

        ActionSensitivity::Financial => {
            if let Some(threshold) = policy.pin_threshold_usd {
                if action.value_usd() > threshold {
                    prompt_pin().await?;
                }
            }
            prompt_confirmation(action).await?;
            Ok(session.sign(&action.to_bytes()))
        }

        ActionSensitivity::Critical => {
            prompt_confirmation(action).await?;
            if policy.pin_threshold_usd.is_some() {
                prompt_pin().await?;
            }
            Ok(session.sign(&action.to_bytes()))
        }
    }
}
```

## policy UI

```
settings → signing policy:

  ┌─────────────────────────────────────────────┐
  │  Signing Policy                              │
  │                                              │
  │  auto-sign gameplay:     [✓]                │
  │  confirm withdrawals:    [✓]                │
  │  confirm transfers:      [✓]                │
  │                                              │
  │  PIN for amounts over:   [$___] (optional)  │
  │                                              │
  │  session timeout:        [4 hours ▼]        │
  │                                              │
  │  [Save Changes]                              │
  └─────────────────────────────────────────────┘
```

## recommendations

```
for most users:
  - use default policy
  - auto-sign gameplay
  - confirm withdrawals

for high stakes:
  - set PIN threshold at 10% of bankroll
  - shorter session timeout
  - confirm all financial actions

for shared devices:
  - never save session
  - always require PIN for amounts
  - short timeout (1 hour)
```
