# pin stretching

PINs are inherently low-entropy (4-6 digits). pin stretching converts weak PINs into strong cryptographic keys using expensive key derivation.

## the problem

```
PIN entropy:
  4 digits = 10,000 combinations
  6 digits = 1,000,000 combinations

brute force without stretching:
  4-digit PIN: ~0.1 seconds
  6-digit PIN: ~10 seconds

this is unacceptable for security
```

## the solution: argon2id

argon2id is a memory-hard KDF that makes brute force expensive:

```
argon2id parameters:
  - memory: {{PIN_KDF_MEMORY}} (memory cost)
  - iterations: {{PIN_KDF_ITERATIONS}} (time cost)
  - parallelism: 1 (single thread)

brute force with these parameters:
  4-digit PIN: ~3 hours
  6-digit PIN: ~300 hours

online attacks (against vault):
  - rate limited to {{PIN_GUESSES_FREE}} attempts
  - 4-digit PIN: 3 guesses, 0.03% success
  - 6-digit PIN: 3 guesses, 0.0003% success
```

## derivation process

```rust
use argon2::{Argon2, Algorithm, Version, Params};

fn derive_access_key(
    pin: &[u8],
    email: &str,
) -> [u8; 32] {
    // salt = hash of email (deterministic)
    let salt = blake3::hash(email.as_bytes());

    // argon2id parameters
    let params = Params::new(
        {{PIN_KDF_MEMORY}} / 1024,   // memory in KiB
        {{PIN_KDF_ITERATIONS}},       // iterations
        1,                            // parallelism
        Some(32),                     // output length
    ).unwrap();

    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        params,
    );

    // derive key
    let mut key = [0u8; 32];
    argon2.hash_password_into(pin, salt.as_bytes(), &mut key)
        .unwrap();

    key
}
```

## unlock tag derivation

unlock_tag lets vaults verify PIN without knowing it:

```rust
fn derive_unlock_tag(access_key: &[u8; 32]) -> [u8; 16] {
    // deterministic hash of access key
    let hash = blake3::hash(access_key);
    hash.as_bytes()[0..16].try_into().unwrap()
}
```

```
security properties:
  - unlock_tag reveals nothing about PIN
  - can't reverse access_key from unlock_tag
  - vault can verify correct PIN in constant time
  - timing attacks prevented
```

## client-side computation

PIN stretching happens entirely on user's device:

```
computation time (typical):
  mobile device: 2-5 seconds
  desktop: 0.5-2 seconds
  server: 0.2-0.5 seconds

user experience:
  - show "deriving key..." indicator
  - computation during email verification
  - imperceptible for most users
```

## parameter tuning

```
tradeoffs:
  ┌────────────────┬───────────────┬───────────────┐
  │ parameter      │ increase      │ decrease      │
  ├────────────────┼───────────────┼───────────────┤
  │ memory         │ harder brute  │ faster login  │
  │                │ force, slower │ less secure   │
  ├────────────────┼───────────────┼───────────────┤
  │ iterations     │ harder brute  │ faster login  │
  │                │ force, slower │ less secure   │
  └────────────────┴───────────────┴───────────────┘

current values optimize for:
  - mobile device performance
  - strong offline attack resistance
  - reasonable login time
```

## upgrade path

when hardware improves, parameters can increase:

```rust
enum KdfVersion {
    V1 {
        memory: 256_000_000,  // 256MB
        iterations: 3,
    },
    V2 {
        memory: 512_000_000,  // 512MB (future)
        iterations: 4,
    },
}

// vault stores version with share
// client uses correct parameters for recovery
```

## comparison with alternatives

```
| KDF      | memory-hard | GPU resistant | recommendation |
|----------|-------------|---------------|----------------|
| argon2id | yes         | yes           | use this       |
| scrypt   | yes         | partial       | acceptable     |
| bcrypt   | no          | partial       | legacy only    |
| PBKDF2   | no          | no            | never use      |

argon2id won the Password Hashing Competition (2015)
it's the current best practice
```

## attack scenarios

```
scenario 1: stolen vault database
  attacker has: encrypted shares + unlock_tags
  attacker needs: PIN (not stored anywhere)

  brute force cost:
    4-digit PIN × 256MB × 3 iterations = expensive
    at 1 PIN/second: 2.7 hours for 4 digits
    still need {{VSS_THRESHOLD}} vault databases

scenario 2: online attack
  attacker tries PINs against vault API

  protection:
    - rate limited to {{PIN_GUESSES_FREE}} attempts
    - share deleted after limit
    - need to attack {{VSS_THRESHOLD}} vaults simultaneously

scenario 3: shoulder surfing
  attacker observes PIN entry

  protection:
    - still need email access
    - user should change PIN immediately
    - consider biometric unlock for mobile
```
