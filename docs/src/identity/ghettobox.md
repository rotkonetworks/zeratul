# ghettobox

ghettobox provides web3 identity without browser extensions. users login with email + PIN, and their signing keys are derived from distributed vault shares.

## why ghettobox

| traditional web3 | ghettobox |
|------------------|-----------|
| install metamask | just enter email + PIN |
| backup 24 words | PIN-protected vault backup |
| lose seed = lose funds | recover with email + PIN |
| approve every tx | auto-sign during gameplay |
| popup spam | seamless UX |

## how it works

```
registration:
  ┌─────────────────────────────────────────────────────────┐
  │ user enters: email + PIN                                │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ client derives:                                         │
  │   access_key = argon2id(PIN, email, {{PIN_KDF_MEMORY}}, {{PIN_KDF_ITERATIONS}})            │
  │   unlock_tag = hash(access_key)[0:16]                   │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ client generates:                                       │
  │   seed = random 32 bytes                                │
  │   shares = vss_split(seed, {{VSS_THRESHOLD}}, {{VSS_TOTAL_SHARES}})                      │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ for each vault node:                                    │
  │   encrypted_share = encrypt(share, access_key)         │
  │   send (user_id, unlock_tag, encrypted_share)          │
  │   vault seals to TPM                                    │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ client derives:                                         │
  │   signing_key = hkdf(seed, "ghettobox:ed25519:v1")     │
  │   address = signing_key.public()                        │
  └─────────────────────────────────────────────────────────┘
```

## login flow

```
  ┌─────────────────────────────────────────────────────────┐
  │ user enters: email + PIN                                │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ client derives: access_key, unlock_tag                  │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ request shares from {{VSS_THRESHOLD}}+ vault nodes:                      │
  │   POST /recover { user_id, unlock_tag }                 │
  │   vault verifies tag, returns decrypted share           │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ client combines {{VSS_THRESHOLD}} shares:                                │
  │   seed = vss_combine(shares)                            │
  │   signing_key = hkdf(seed, ...)                         │
  └─────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────┐
  │ signing_key in memory for session                       │
  │ auto-signs game actions without prompts                 │
  └─────────────────────────────────────────────────────────┘
```

## security model

**what vault operators know:**
- encrypted share blob
- unlock_tag hash (for verification)
- last activity timestamp

**what vault operators DON'T know:**
- user's PIN
- the actual unlock key
- the seed or signing key

**attack resistance:**

| attack | protection |
|--------|------------|
| vault breach | shares encrypted with user's key |
| PIN brute force | argon2id: {{PIN_KDF_MEMORY}}, {{PIN_KDF_ITERATIONS}} iterations |
| online guessing | {{PIN_GUESSES_FREE}} wrong guesses = share deleted |
| collusion | need {{VSS_THRESHOLD}}/{{VSS_TOTAL_SHARES}} vaults + PIN |

## vault distribution

for maximum security, vaults should be operated by independent parties:

```
recommended setup:
  vault 1: rotko (default)
  vault 2: partner organization
  vault 3: user's own server (optional)

trust model:
  - any 1 vault can go offline
  - any 1 vault can be compromised
  - need {{VSS_THRESHOLD}} honest vaults + correct PIN
```

## code example

```rust
use ghettobox::Client;

// register new account
let client = Client::new(vault_nodes).await?;
let result = client.create_account("alice@example.com", b"1234")?;
println!("address: {}", result.account.address_hex());

// login to existing account
let account = client.recover("alice@example.com", b"1234").await?;

// sign a message
let signature = account.sign(b"hello world");
```
