# constants

all configurable parameters in one place. these values are referenced throughout the documentation using `{{VARIABLE_NAME}}` syntax.

## cryptography

| constant | value | description |
|----------|-------|-------------|
| `SHUFFLE_CURVE` | {{SHUFFLE_CURVE}} | elliptic curve for mental poker |
| `HASH_FUNCTION` | {{HASH_FUNCTION}} | hash function for commitments |
| `SIGNATURE_SCHEME` | {{SIGNATURE_SCHEME}} | signature algorithm |
| `ENCRYPTION_CIPHER` | {{ENCRYPTION_CIPHER}} | symmetric encryption |

## PIN security

| constant | value | description |
|----------|-------|-------------|
| `PIN_KDF` | {{PIN_KDF}} | key derivation function |
| `PIN_KDF_MEMORY` | {{PIN_KDF_MEMORY}} | argon2 memory parameter |
| `PIN_KDF_ITERATIONS` | {{PIN_KDF_ITERATIONS}} | argon2 iteration count |
| `PIN_GUESSES_FREE` | {{PIN_GUESSES_FREE}} | wrong guesses before lockout (free) |
| `PIN_GUESSES_PREMIUM` | {{PIN_GUESSES_PREMIUM}} | wrong guesses before lockout (paid) |

## secret sharing

| constant | value | description |
|----------|-------|-------------|
| `VSS_THRESHOLD` | {{VSS_THRESHOLD}} | minimum shares to recover |
| `VSS_TOTAL_SHARES` | {{VSS_TOTAL_SHARES}} | total shares created |
| `VSS_FIELD` | {{VSS_FIELD}} | finite field for shamir |

## state channels

| constant | value | description |
|----------|-------|-------------|
| `DISPUTE_TIMEOUT_BLOCKS` | {{DISPUTE_TIMEOUT_BLOCKS}} | blocks before dispute resolves |
| `BOND_AMOUNT_USD` | ${{BOND_AMOUNT_USD}} | required bond per player |
| `DISPUTE_FEE_PERCENT` | {{DISPUTE_FEE_PERCENT}}% | fee charged on disputes |

## reputation

| constant | value | description |
|----------|-------|-------------|
| `MIN_REPUTATION_DEFAULT` | {{MIN_REPUTATION_DEFAULT}} | default minimum for tables |
| `TIMEOUT_REPUTATION_PENALTY` | {{TIMEOUT_REPUTATION_PENALTY}} | points lost per timeout |

## ghettobox

| constant | value | description |
|----------|-------|-------------|
| `BACKUP_INACTIVE_WARNING_DAYS` | {{BACKUP_INACTIVE_WARNING_DAYS}} | days before inactive warning |
| `BACKUP_ARCHIVE_DAYS` | {{BACKUP_ARCHIVE_DAYS}} | days before archive |
| `RECOVERY_FEE_USD` | ${{RECOVERY_FEE_USD}} | fee for pay-per-recovery |

## updating constants

to update these values, edit `book.toml`:

```toml
[preprocessor.variables.variables]
VSS_THRESHOLD = "2"
VSS_TOTAL_SHARES = "3"
# ... etc
```

then rebuild the docs:

```bash
mdbook build
```

all `{{VARIABLE_NAME}}` references will be updated automatically.
