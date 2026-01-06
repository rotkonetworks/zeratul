# mental poker

mental poker allows players to play a fair card game without a trusted dealer. the deck is shuffled cryptographically so that:

1. no player knows the card order until revealed
2. all players can verify the shuffle was fair
3. cards can be selectively revealed without exposing others

## the problem

in physical poker, a dealer shuffles and deals. online, who shuffles? if the server shuffles, the server can cheat. if a player shuffles, that player can cheat.

mental poker solves this with **commutative encryption**:

```
shuffle idea:
  each player encrypts the deck with their own key
  encryption is commutative: E_a(E_b(x)) = E_b(E_a(x))
  to reveal a card, all players decrypt in sequence
  no single player controls the order
```

## our implementation

we use elgamal encryption on {{SHUFFLE_CURVE}}:

```
card encoding:
  each card (2♠, A♥, etc) maps to a curve point
  52 cards = 52 distinct points

encryption:
  E(card, pubkey) = (r*G, card + r*pubkey)
  where r is random scalar

decryption:
  D(ciphertext, privkey) = card
```

## shuffle protocol

```
round 1: initial encryption
  ┌─────────────────────────────────────────┐
  │ player A encrypts all 52 cards with pk_A │
  │ sends encrypted deck to B               │
  └─────────────────────────────────────────┘
                    │
                    ▼
round 2: shuffle + re-encrypt
  ┌─────────────────────────────────────────┐
  │ player B:                               │
  │   1. permutes card order (secret π_B)   │
  │   2. re-encrypts with pk_B              │
  │   3. generates ZK proof of valid shuffle│
  │   4. sends to A                         │
  └─────────────────────────────────────────┘
                    │
                    ▼
round 3: shuffle + re-encrypt
  ┌─────────────────────────────────────────┐
  │ player A:                               │
  │   1. verifies B's proof                 │
  │   2. permutes with secret π_A           │
  │   3. re-encrypts with fresh randomness  │
  │   4. generates ZK proof                 │
  │   5. final deck ready                   │
  └─────────────────────────────────────────┘
```

## security properties

| property | guarantee |
|----------|-----------|
| hiding | encrypted cards indistinguishable |
| binding | card values fixed after shuffle |
| fairness | each player contributes entropy |
| verifiability | ZK proofs verify correct shuffle |

## ZK shuffle proof

the shuffle proof demonstrates:

1. output is a permutation of input (no cards added/removed)
2. each card was properly re-encrypted
3. prover knows the permutation and randomness

without revealing:

- the permutation itself
- the encryption randomness
- any card values

proof size: ~3KB for 52 cards
verification time: ~5ms

## code reference

```rust
// create shuffle proof
let (shuffled_deck, proof) = prover.shuffle_and_prove(
    &input_deck,
    &permutation,
    &randomness,
    &transcript,
)?;

// verify shuffle
verifier.verify_shuffle(
    &input_deck,
    &shuffled_deck,
    &proof,
    &transcript,
)?;
```

see [shuffle protocol](./shuffle.md) for the full protocol details.
