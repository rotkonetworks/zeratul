# shuffle protocol

the shuffle protocol ensures no single player can manipulate the deck. every player participates in shuffling and proves their shuffle was valid without revealing card positions.

## overview

```
multi-party shuffle:

  initial deck          player A           player B
  ┌─────────────┐      shuffles &         shuffles &
  │ 52 cards    │───▶  encrypts    ───▶   encrypts    ───▶  final deck
  │ known order │      proves             proves            (nobody knows
  └─────────────┘      validity           validity           card positions)
```

## elgamal encryption

each card is represented as a point on {{SHUFFLE_CURVE}}:

```rust
/// card as curve point
struct EncryptedCard {
    /// C1 = r * G
    c1: RistrettoPoint,
    /// C2 = M + r * pk
    c2: RistrettoPoint,
}

/// encrypt card to player's public key
fn encrypt(card: RistrettoPoint, pk: RistrettoPoint) -> EncryptedCard {
    let r = Scalar::random(&mut rng);
    EncryptedCard {
        c1: r * RISTRETTO_BASEPOINT,
        c2: card + r * pk,
    }
}

/// decrypt with private key
fn decrypt(enc: EncryptedCard, sk: Scalar) -> RistrettoPoint {
    enc.c2 - sk * enc.c1
}
```

## card encoding

standard 52-card deck encoded as curve points:

```
encoding scheme:
  card_point = hash_to_curve(card_index)

  index 0  = 2♣  → hash_to_curve(0)
  index 1  = 3♣  → hash_to_curve(1)
  ...
  index 12 = A♣  → hash_to_curve(12)
  index 13 = 2♦  → hash_to_curve(13)
  ...
  index 51 = A♠  → hash_to_curve(51)

all players agree on this mapping before game starts
```

## shuffle-and-rerandomize

each player's shuffle operation:

```rust
fn shuffle_and_rerandomize(
    deck: &[EncryptedCard],
    pk: RistrettoPoint,
) -> (Vec<EncryptedCard>, ShuffleProof) {
    // 1. generate random permutation
    let perm = random_permutation(52);

    // 2. apply permutation
    let permuted: Vec<_> = perm.iter()
        .map(|&i| deck[i])
        .collect();

    // 3. rerandomize each card (add fresh encryption)
    let rerandomized: Vec<_> = permuted.iter()
        .map(|card| {
            let r = Scalar::random(&mut rng);
            EncryptedCard {
                c1: card.c1 + r * RISTRETTO_BASEPOINT,
                c2: card.c2 + r * pk,
            }
        })
        .collect();

    // 4. generate ZK proof
    let proof = generate_shuffle_proof(deck, &rerandomized, &perm);

    (rerandomized, proof)
}
```

## ZK shuffle proof

prove shuffle validity without revealing permutation:

```
what the proof shows:
  1. output is a permutation of input (same cards, different order)
  2. each card was properly rerandomized
  3. prover knows the permutation

what the proof hides:
  - the actual permutation used
  - which input card maps to which output

proof system: bulletproofs-based argument
proof size: O(n log n) for n cards
verification: O(n) operations
```

## shuffle rounds

full shuffle protocol for 2 players:

```
round 0: initial deck (public)
  D₀ = [card₀, card₁, ..., card₅₁]

round 1: player A shuffles
  D₁ = shuffle(D₀, pk_joint)
  A broadcasts (D₁, proof₁)
  all verify proof₁

round 2: player B shuffles
  D₂ = shuffle(D₁, pk_joint)
  B broadcasts (D₂, proof₂)
  all verify proof₂

final: D₂ is the shuffled deck
  - neither player knows card positions
  - both players contributed randomness
  - cheating is detectable
```

## joint public key

deck encrypted to joint key that requires all players:

```rust
/// each player generates keypair
let (sk_a, pk_a) = generate_keypair();
let (sk_b, pk_b) = generate_keypair();

/// joint public key (sum of individual keys)
let pk_joint = pk_a + pk_b;

/// to decrypt, need both private keys
fn joint_decrypt(
    enc: EncryptedCard,
    sk_a: Scalar,
    sk_b: Scalar,
) -> RistrettoPoint {
    enc.c2 - (sk_a + sk_b) * enc.c1
}
```

## security properties

```
unpredictability:
  - after honest shuffle, each position equally likely
  - adversary learns nothing about card positions

verifiability:
  - invalid shuffle detected immediately
  - cheater identified by proof failure

fairness:
  - single honest participant ensures randomness
  - colluding players can't gain advantage

non-malleability:
  - can't modify shuffled deck without detection
  - subsequent shuffles build on previous
```

## implementation notes

```rust
// actual implementation uses curve25519-dalek
use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;

// shuffle proof from zk-shuffle crate
use zk_shuffle::{ShuffleProof, verify_shuffle};

// verification is the expensive part
// ~100ms for 52 cards on modern hardware
```
