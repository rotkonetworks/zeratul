# zoda-vss

verifiable secret sharing via reed-solomon coding.

based on observations from guillermo angeris: for messages larger than ~128 bits, you can do verifiable shamir secret sharing with very little additional overhead.

note: this is VSS (verifiable secret sharing), not to be confused with commonware's ZODA which is a data availability coding scheme.

## security warning

this crate has not been audited. use at your own risk.

## features

- **verifiable**: parties receiving shares can check against header and know they will decode the same secret
- **low overhead**: for 256-bit secrets, header is just 34 bytes (2 bytes params + 32 bytes commitment)
- **threshold**: standard t-of-n threshold reconstruction
- **no_std**: works in constrained environments

## usage

```rust
use zoda_vss::{Dealer, Player};

// dealer creates 3-of-5 shares for a 32-byte secret
let secret = [0x42u8; 32];
let dealer = Dealer::new(3, 5);
let (header, shares) = dealer.share(&secret, &mut rng);

// players verify their shares
for share in &shares {
    assert!(share.verify(&header));
}

// any 3 shares can reconstruct
let reconstructed = Player::reconstruct(&header, &shares[0..3])?;
assert_eq!(reconstructed, secret);
```

## theory

reed-solomon encoding over GF(2^8):

1. secret bytes become constant terms of degree-(t-1) polynomials
2. random coefficients fill higher-degree terms
3. shares are polynomial evaluations at points 1..=n
4. header commits to polynomial coefficients via SHA-256
5. reconstruction uses lagrange interpolation at x=0

key insight: verification is lightweight because the header commitment allows any party to check that their share is consistent with the encoded polynomial, ensuring all honest parties reconstruct the same secret.

## comparison with polynomial commitments

| aspect | polynomial commitments | zoda |
|--------|----------------------|------|
| header size | O(threshold) group elements | O(1) hash |
| verification | group operations | hash comparison |
| homomorphic | yes | no |
| dealer construction | collective possible | dealer only |

use zoda when:
- bandwidth matters more than homomorphic properties
- dealer is trusted to construct correctly
- lightweight verification is preferred

use polynomial commitments when:
- need to aggregate/combine commitments
- collective secret construction required
- stronger algebraic properties needed

## license

MIT OR Apache-2.0
