# secret sharing

VSS (verifiable secret sharing) splits a secret into shares where any {{VSS_THRESHOLD}} shares can reconstruct the original, but fewer shares reveal nothing.

## shamir's secret sharing

based on polynomial interpolation:

```
setup:
  - secret: s (a random 32-byte seed)
  - threshold: t = {{VSS_THRESHOLD}}
  - shares: n = {{VSS_TOTAL_SHARES}}

splitting:
  1. generate random polynomial of degree t-1:
     P(x) = s + a₁x + a₂x² + ... + aₜ₋₁xᵗ⁻¹

  2. evaluate at n distinct points:
     share₁ = P(1)
     share₂ = P(2)
     share₃ = P(3)

  3. distribute shares to vault nodes

reconstruction:
  - any t shares determine polynomial uniquely
  - P(0) = s (the secret)
  - lagrange interpolation recovers it
```

## implementation

```rust
use vsss_rs::{Shamir, Share};

/// split secret into shares
fn split_secret(
    secret: &[u8; 32],
    threshold: usize,
    total: usize,
) -> Vec<Share> {
    // use prime field for security
    let shamir = Shamir::<{{VSS_FIELD}}, u8>::new(threshold, total);
    shamir.split_secret(secret).unwrap()
}

/// reconstruct from threshold shares
fn combine_shares(shares: &[Share]) -> [u8; 32] {
    let shamir = Shamir::<{{VSS_FIELD}}, u8>::new(
        shares.len(),
        shares.len()
    );
    shamir.combine_shares(shares).unwrap()
}
```

## verifiable secret sharing

VSS adds verification that shares are consistent:

```
problem with plain shamir:
  - dealer could give inconsistent shares
  - reconstruction might fail or produce wrong result
  - no way to detect until too late

VSS solution:
  - dealer publishes commitments to polynomial coefficients
  - each share can be verified against commitments
  - inconsistent shares detected immediately
```

```rust
struct VssShare {
    /// the share value
    share: Share,
    /// commitment to verify against
    commitment: [RistrettoPoint; {{VSS_THRESHOLD}}],
}

fn verify_share(vss_share: &VssShare) -> bool {
    // verify share is on committed polynomial
    let expected = evaluate_commitment(
        &vss_share.commitment,
        vss_share.share.index
    );
    expected == vss_share.share.value_as_point()
}
```

## ghettobox VSS scheme

```
registration:
  1. client generates random seed (32 bytes)
  2. client splits seed: {{VSS_THRESHOLD}}-of-{{VSS_TOTAL_SHARES}}
  3. client encrypts each share with access_key
  4. client sends encrypted shares to vault nodes
  5. client derives signing key from seed

recovery:
  1. client derives access_key from PIN
  2. client requests shares from {{VSS_THRESHOLD}} vaults
  3. vaults verify unlock_tag, return encrypted shares
  4. client decrypts shares
  5. client combines shares to recover seed
  6. client derives signing key from seed
```

## share distribution

```
vault assignment:
  share₁ → vault 1 (rotko)
  share₂ → vault 2 (partner)
  share₃ → vault 3 (third party)

redundancy:
  - any 1 vault can be offline
  - any 1 vault can be compromised
  - recovery still works with {{VSS_THRESHOLD}} honest vaults
```

## encryption layer

shares are encrypted before storage:

```rust
fn encrypt_share(
    share: &Share,
    access_key: &[u8; 32],
) -> [u8; 64] {
    // derive encryption key from access key
    let enc_key = hkdf_expand(access_key, b"share-encryption");

    // encrypt with ChaCha20-Poly1305
    let nonce = [0u8; 12];  // deterministic (same key per share)
    let ciphertext = chacha20poly1305::encrypt(
        &enc_key,
        &nonce,
        share.as_bytes()
    );

    ciphertext
}
```

## security analysis

```
threshold security:
  - t-1 shares reveal zero information about secret
  - information-theoretic security (not computational)
  - even unlimited computation can't break it

with {{VSS_THRESHOLD}} = 2, {{VSS_TOTAL_SHARES}} = 3:
  - 1 compromised vault: no information leaked
  - 2 compromised vaults: secret exposed (if PIN known)
  - need PIN for all scenarios

attack requirements:
  - compromise {{VSS_THRESHOLD}} independent organizations
  - know user's PIN
  - both conditions simultaneously
```

## comparison to other schemes

```
| scheme                | shares | recovery  | trust model      |
|-----------------------|--------|-----------|------------------|
| single key            | 1      | all-or-nothing | single point |
| multi-sig             | n-of-m | on-chain  | threshold        |
| shamir (plain)        | t-of-n | offline   | honest dealer    |
| VSS                   | t-of-n | offline   | verifiable       |
| MPC                   | t-of-n | online    | no trusted party |

ghettobox uses VSS:
  - verifiable shares
  - offline reconstruction
  - user-controlled threshold
```

## proactive secret sharing

future enhancement: refresh shares without changing secret:

```
share refresh protocol:
  1. each vault generates random update
  2. updates sum to zero (secret unchanged)
  3. each vault applies its update locally
  4. old shares become invalid

benefits:
  - limits exposure window
  - compromised share becomes useless after refresh
  - no user interaction required
```
