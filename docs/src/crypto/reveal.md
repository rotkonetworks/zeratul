# card reveal

after shuffling, cards must be revealed to specific players without exposing them to others. this uses threshold decryption with ZK proofs.

## reveal types

```
hole cards (private):
  - revealed only to receiving player
  - other players see nothing

community cards (public):
  - revealed to all players
  - flop, turn, river in hold'em

showdown:
  - losing player can keep cards hidden
  - winner must reveal to claim pot
```

## private reveal protocol

revealing card at position `i` to player A:

```
step 1: player B provides partial decryption

  encrypted card: E = (C1, C2)
  player B's share: D_B = sk_B * C1

  B broadcasts (D_B, proof_B)
  proof shows D_B is correct partial decryption

step 2: player A completes decryption locally

  D_A = sk_A * C1  (computed locally, not broadcast)
  card = C2 - D_A - D_B

  only A learns the card
```

## partial decryption proof

prove correct partial decryption:

```rust
struct PartialDecryptionProof {
    /// commitment to randomness
    commitment: RistrettoPoint,
    /// challenge (fiat-shamir)
    challenge: Scalar,
    /// response
    response: Scalar,
}

fn prove_partial_decryption(
    c1: RistrettoPoint,
    sk: Scalar,
    pk: RistrettoPoint,
) -> (RistrettoPoint, PartialDecryptionProof) {
    // D = sk * C1
    let d = sk * c1;

    // prove DLOG equality:
    //   D/C1 = pk/G  (same secret exponent)
    let proof = dlog_equality_proof(c1, d, BASEPOINT, pk, sk);

    (d, proof)
}
```

## public reveal protocol

revealing community cards to all players:

```
step 1: all players broadcast partial decryptions

  player A: (D_A, proof_A)
  player B: (D_B, proof_B)

step 2: anyone can combine to get card

  card = C2 - D_A - D_B

  everyone sees the same card
```

## reveal ordering

texas hold'em reveal sequence:

```
1. deal hole cards (2 per player, private)
   - reveal positions 0,1 to player A
   - reveal positions 2,3 to player B

2. flop (3 cards, public)
   - reveal positions 4,5,6 to all

3. turn (1 card, public)
   - reveal position 7 to all

4. river (1 card, public)
   - reveal position 8 to all

5. showdown (winner's cards, public)
   - winner reveals hole cards to claim pot
```

## commitment scheme

players commit to their decryption shares before revealing:

```rust
struct RevealCommitment {
    /// hash of partial decryption
    hash: [u8; 32],
    /// position being revealed
    position: usize,
}

// prevents last-player advantage:
// 1. all players commit to their shares
// 2. all players reveal their shares
// 3. if commitment doesn't match, player is cheating
```

## cheating detection

```
invalid partial decryption:
  - proof verification fails
  - player identified as cheater
  - dispute goes on-chain

withheld decryption:
  - player refuses to reveal
  - timeout triggers
  - other players can prove non-cooperation

wrong commitment:
  - revealed share doesn't match commitment
  - automatic dispute loss
```

## card verification

after reveal, verify card is valid:

```rust
fn verify_revealed_card(
    card_point: RistrettoPoint,
) -> Option<Card> {
    // check card is one of 52 valid cards
    for i in 0..52 {
        let expected = hash_to_curve(i);
        if card_point == expected {
            return Some(Card::from_index(i));
        }
    }
    // invalid card (corrupted shuffle)
    None
}
```

## batch reveal optimization

reveal multiple cards efficiently:

```rust
// reveal all 5 community cards at once
fn reveal_community_cards(
    deck: &[EncryptedCard],
    positions: &[usize; 5],  // [4,5,6,7,8]
    partial_a: &[RistrettoPoint; 5],
    partial_b: &[RistrettoPoint; 5],
    proof_a: BatchProof,
    proof_b: BatchProof,
) -> Result<[Card; 5], RevealError> {
    // verify batch proofs (faster than individual)
    verify_batch_partial_decryption(proof_a)?;
    verify_batch_partial_decryption(proof_b)?;

    // combine and decode
    let cards = positions.iter()
        .enumerate()
        .map(|(i, &pos)| {
            let enc = &deck[pos];
            let point = enc.c2 - partial_a[i] - partial_b[i];
            Card::from_point(point)
        })
        .collect();

    Ok(cards)
}
```

## security guarantees

```
privacy:
  - private cards revealed only to intended recipient
  - other players learn nothing

integrity:
  - revealed card matches encrypted card
  - can't swap cards during reveal

accountability:
  - cheating detected with proof
  - honest player can always prove innocence
```
