# account recovery

ghettobox allows account recovery with just email + PIN. no seed phrases to lose.

## recovery flow

```
user needs:
  - email address (unchanged)
  - PIN (remembered)

user doesn't need:
  - backup phrases
  - recovery codes
  - old device

recovery steps:
  ┌─────────────────────────────────────────────────────────────┐
  │ 1. enter email + PIN on new device                          │
  └─────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ 2. client derives access_key = argon2id(PIN, email)        │
  │    client derives unlock_tag = hash(access_key)[0:16]      │
  └─────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ 3. contact {{VSS_THRESHOLD}}+ vault nodes with unlock_tag              │
  │    vaults verify tag, return encrypted shares               │
  └─────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ 4. decrypt shares with access_key                           │
  │    combine {{VSS_THRESHOLD}} shares to recover seed                    │
  └─────────────────────────────────────────────────────────────┘
                          │
                          ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ 5. derive signing_key = hkdf(seed, "ghettobox:ed25519:v1") │
  │    account recovered, same address as before                │
  └─────────────────────────────────────────────────────────────┘
```

## wrong PIN handling

```
attempt 1: wrong unlock_tag
  - vault increments failure counter
  - returns "wrong PIN" error
  - remaining attempts: {{PIN_GUESSES_FREE}} - 1

attempt 2: wrong unlock_tag
  - vault increments failure counter
  - returns "wrong PIN" error
  - remaining attempts: {{PIN_GUESSES_FREE}} - 2

attempt 3: wrong unlock_tag
  - vault deletes share
  - returns "share deleted" error
  - account locked at this vault

recovery still possible:
  - if {{VSS_THRESHOLD}} other vaults have shares
  - premium users have more attempts
```

## lockout recovery

if too many wrong attempts:

```
partial lockout (some shares deleted):
  - {{VSS_THRESHOLD}} shares still available elsewhere
  - recover normally with remaining vaults
  - consider distributing to new vault

complete lockout (all shares deleted):
  - account unrecoverable
  - funds in on-chain contracts still accessible
  - can prove ownership via on-chain history
  - manual recovery process (support ticket)
```

## PIN change

changing PIN creates new encrypted shares:

```
1. recover account with old PIN
   - get current seed in memory

2. derive new access_key with new PIN
   - new_access_key = argon2id(new_PIN, email)

3. re-encrypt shares with new key
   - same shares, new encryption

4. register new shares with vaults
   - vaults overwrite old data
   - signature proves ownership

5. old PIN no longer works
   - old access_key can't decrypt
   - old unlock_tag doesn't match
```

## email change

changing email is complex:

```
problem:
  - email is part of salt for KDF
  - changing email changes access_key
  - old encrypted shares become unrecoverable

solution:
  1. recover with old email + PIN
  2. generate entirely new identity
  3. new email + PIN → new access_key
  4. new shares distributed to vaults
  5. migrate on-chain assets to new address

warning:
  - old address abandoned (or keep access)
  - channel counterparties must be notified
  - reputation doesn't transfer
```

## device migration

moving to new device:

```
option 1: recover from vaults (recommended)
  - enter email + PIN on new device
  - full recovery process
  - old device still works (until session expires)

option 2: session transfer
  - export session token from old device
  - import on new device
  - no vault contact needed
  - session key must be protected in transit

option 3: simultaneous sessions
  - both devices recover independently
  - same signing key on both
  - useful for desktop + mobile
```

## inheritance planning

if user dies or becomes incapacitated:

```
current limitation:
  - no way to recover without PIN
  - intentional security property

future enhancement:
  - social recovery with trusted contacts
  - time-locked recovery after inactivity
  - legal process for courts to compel recovery

recommendation:
  - store PIN in physical safe
  - estate planning for crypto assets
  - use multi-sig for large holdings
```

## backup verification

users should verify recovery works:

```
verification process:
  1. logout of current session
  2. recover account on same device
  3. verify same address derived

recommended frequency:
  - after initial registration
  - after PIN change
  - every 6 months (reminder)
```

## recovery fees

```
free tier:
  - storage free for {{BACKUP_INACTIVE_WARNING_DAYS}} days
  - after inactivity: warning email
  - after {{BACKUP_ARCHIVE_DAYS}} days: archived
  - recovery from archive: ${{RECOVERY_FEE_USD}}

premium tier ($5/month):
  - unlimited storage
  - no inactivity cleanup
  - priority vault access
  - {{PIN_GUESSES_PREMIUM}} wrong PIN attempts

pay-per-recovery:
  - storage free
  - ${{RECOVERY_FEE_USD}} per recovery
  - good for infrequent users
```
