# authentication

authentication connects your identity to the poker client. ghettobox handles key management so you never see seed phrases.

## login flow

```
user experience:

  ┌─────────────────────────────────────────────────────────────┐
  │                     LOGIN SCREEN                            │
  │                                                             │
  │   email: [alice@example.com            ]                   │
  │                                                             │
  │   PIN:   [• • • • • •]                                     │
  │                                                             │
  │   [        LOGIN        ]                                  │
  │                                                             │
  │   Don't have an account? [Register]                        │
  └─────────────────────────────────────────────────────────────┘
```

## under the hood

```
login process:

  1. user enters email + PIN

  2. client derives access_key (argon2id)
     ┌───────────────────────────────────────┐
     │ "deriving key..."  [████████░░] 80%   │
     └───────────────────────────────────────┘
     takes 1-3 seconds on typical device

  3. client derives unlock_tag from access_key

  4. client contacts vault nodes
     POST /recover { user_id, unlock_tag }

  5. vaults return encrypted shares (if tag matches)

  6. client decrypts shares with access_key

  7. client combines 2+ shares to get seed

  8. client derives signing_key from seed

  9. signing_key stored in memory for session
```

## registration

```
new account:

  1. user enters email + PIN

  2. client derives access_key, unlock_tag

  3. client generates random seed (32 bytes)

  4. client splits seed: 2-of-3 shares

  5. client encrypts each share with access_key

  6. client sends to vault nodes:
     POST /register { user_id, unlock_tag, encrypted_share }

  7. vaults confirm storage

  8. client derives signing_key from seed

  9. account created, address displayed
```

## session management

```rust
struct AuthSession {
    /// derived signing key
    signing_key: SigningKey,
    /// user's address
    address: Address,
    /// when session started
    started_at: Instant,
    /// session timeout
    timeout: Duration,
}

impl AuthSession {
    fn is_valid(&self) -> bool {
        self.started_at.elapsed() < self.timeout
    }

    fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }
}
```

## session timeout

```
timeout behavior:

  default: 4 hours

  configurable:
    - 1 hour (high security)
    - 4 hours (balanced)
    - 8 hours (convenience)
    - until logout (not recommended)

  when timeout expires:
    - signing_key cleared from memory
    - active games continue (state saved)
    - must re-enter PIN to continue
```

## remember device

optional convenience:

```
"remember this device" option:
  - stores encrypted session token locally
  - auto-login without full PIN entry
  - requires device-level authentication
    (biometrics, device PIN)

security tradeoff:
  - more convenient
  - device theft is higher risk
  - recommended only on personal devices
```

## multiple devices

```
same account, multiple devices:

  device A: login with email + PIN
  device B: login with email + PIN

  both have:
    - same signing key
    - same address
    - independent sessions

  channel state:
    - synced via p2p
    - both devices see same state
    - actions from either device valid
```

## logout

```rust
fn logout(session: &mut Option<AuthSession>) {
    if let Some(s) = session.take() {
        // zero out sensitive memory
        drop(s.signing_key);
    }

    // clear any local cache
    clear_session_cache();

    // optional: notify active peers
    // they'll see you go offline
}
```

## wrong PIN handling

```
wrong PIN response:

  attempt 1: "incorrect PIN, 2 attempts remaining"
  attempt 2: "incorrect PIN, 1 attempt remaining"
  attempt 3: "share deleted, account may be locked"

client behavior:
  - show remaining attempts clearly
  - offer "forgot PIN" recovery option
  - explain consequences of lockout
```

## recovery options

```
if locked out:

  partial lockout (1 vault):
    - other 2 vaults still work
    - normal login succeeds
    - consider changing PIN

  full lockout (all vaults):
    - account unrecoverable via ghettobox
    - on-chain funds still accessible
    - manual recovery process available
```
