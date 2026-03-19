# zk.poker protocol specification v1

## overview

zk.poker is a heads-up no-limit hold'em poker protocol where:

- no single party can steal funds or fabricate game outcomes
- every action is signed by the actor and counter-signed by the opponent
- disputes are resolved by a threshold jury that never holds the full key
- players can independently verify every aspect of the protocol

## 1. escrow

### 1.1 key structure

funds are locked in a **2-of-3 threshold Schnorr** address on the Pallas curve (Zcash Orchard compatible):

| position | holder | controls |
|----------|--------|----------|
| 1 | player A | single key |
| 2 | player B | single key |
| 3 | jury | threshold group (t-of-n validators) |

any 2 of 3 can sign. this means:

- **A + B** can settle cooperatively (no jury needed)
- **A + jury** can settle if B disappears or disputes
- **B + jury** can settle if A disappears or disputes
- **jury alone** cannot move funds (only 1 of 3)
- **neither player alone** can move funds (only 1 of 3)

### 1.2 nested threshold

position 3 (jury) is not a single key. it is a **nested threshold group**: the jury's secret `s_3` never exists as a scalar. it is born distributed via interleaved Feldman VSS across `n` validators, with threshold `t`.

each validator holds a Shamir share of `s_3`. when signing is needed, `t` validators each produce a partial signature using their share. these are aggregated into position 3's FROST signature share. the aggregated share is indistinguishable from a single signer's share.

### 1.3 escrow address derivation

the group public key `Y = g^(s_1 + s_2 + s_3)` determines the escrow address. both players and all validators can independently compute `Y` from the public commitments broadcast during DKG. the address is deterministic from `Y`.

### 1.4 verification

a player verifies the escrow by checking:

```
Y == Y_1 + Y_2 + Y_3
```

where `Y_i = g^(s_i)` are the verification shares broadcast during setup. each `Y_i` is a point on the Pallas curve. no secret material is needed for this check.

## 2. mental poker shuffle

### 2.1 the problem

in online poker, someone must deal the cards. if the server deals, the server sees all cards and can cheat. if one player deals, they see the deck and can cheat.

mental poker solves this: both players collaboratively shuffle and encrypt the deck so that **neither player** knows the card order, and cards are only revealed to the intended recipient via partial decryption.

### 2.2 ElGamal encryption on ristretto255

each card is encoded as a point on the ristretto255 curve. encryption uses ElGamal:

```
encrypt(M, pk) = (r * G, r * pk + M)
```

where `r` is a random scalar, `G` is the generator, `pk` is the recipient's public key, and `M` is the card point. decryption requires the secret key `sk` where `pk = sk * G`:

```
decrypt(c0, c1, sk) = c1 - sk * c0 = M
```

### 2.3 shuffle protocol

both players hold ElGamal key pairs `(sk_A, pk_A)` and `(sk_B, pk_B)`. the joint encryption key is `pk = pk_A + pk_B`. a card encrypted under `pk` requires both `sk_A` and `sk_B` to decrypt.

the shuffle proceeds in two rounds:

**round 1 — player A shuffles:**

1. start with the initial deck `D_0`: 52 card points encrypted under `pk`
2. player A picks a secret permutation `π_A` and remasking scalars `r_i`
3. player A computes `D_1[i] = D_0[π_A(i)] + (r_i * G, r_i * pk)` for each card
4. player A produces a **shuffle proof** `P_A` (see 2.4)
5. player A sends `(D_1, P_A)` to player B

**round 2 — player B shuffles:**

6. player B verifies `P_A` against `(D_0, D_1, pk)`
7. player B picks a secret permutation `π_B` and remasking scalars `r_j`
8. player B computes `D_2[j] = D_1[π_B(j)] + (r_j * G, r_j * pk)` for each card
9. player B produces a shuffle proof `P_B`
10. player B sends `(D_2, P_B)` to player A

the final deck `D_2` is encrypted under `pk` and shuffled by both `π_A ∘ π_B`. neither player knows the full permutation.

**deck commitment:** `SHA256(D_2)` is the `deck_commitment` in the `HandTranscript`.

### 2.4 shuffle proof (batch Chaum-Pedersen + grand product)

each shuffle must prove two things:

1. **valid remasking**: each output card is the encryption of some input card with fresh randomness (no card was replaced or fabricated)
2. **permutation**: the mapping from input to output is a bijection (every card appears exactly once)

**valid remasking** is proven via **batch Chaum-Pedersen**: for each card `i`, the delta `δ_i = output[i] - input[π(i)]` must have the form `(r_i * G, r_i * pk)`. this is a DLOG equality proof between the two components, batched across all 52 cards.

**permutation** is proven via **scalar grand product**: using a random challenge `γ`, the prover shows:

```
∏_i (x_i + γ) == ∏_i (y_i + γ)
```

where `x_i` and `y_i` are derived from input and output cards. this holds if and only if `{x_i}` and `{y_i}` are the same multiset (i.e., a permutation).

the proof is **zero-knowledge**: the verifier learns nothing about the permutation or remasking scalars.

### 2.5 card reveal

to deal a card to a specific player, the other player provides a **partial decryption** (their share of the ElGamal decryption):

**dealing to player A:**
1. player B sends `reveal_B = sk_B * c0` for the target card
2. player A computes `M = c1 - sk_A * c0 - reveal_B`
3. the card point `M` is decoded to a card (rank + suit)

**dealing to player B:** symmetric, player A sends their reveal.

**community cards:** both players send reveals, both can decode.

each reveal is accompanied by a **Chaum-Pedersen proof** that `reveal = sk * c0` (proves the player used their actual secret key, not a fabricated value).

```
CardReveal {
    hand_number: u64,
    cards: [u8],           // card indices being revealed
    keys: [[u8; 32]],      // partial decryption points
    proof: [u8],            // Chaum-Pedersen proof of correct decryption
}
```

### 2.6 shuffle verification summary

| step | what is proven | proof type |
|------|----------------|------------|
| shuffle round 1 | A's output is a valid remasking + permutation of input | batch Chaum-Pedersen + grand product |
| shuffle round 2 | B's output is a valid remasking + permutation of A's output | batch Chaum-Pedersen + grand product |
| card reveal | reveal is computed with the correct secret key | Chaum-Pedersen |
| deck integrity | final deck matches commitment in transcript | SHA-256 |

### 2.7 security properties

- **no information leak**: neither player learns the permutation or any card not dealt to them
- **no card fabrication**: the shuffle proof guarantees every card in the original deck appears exactly once
- **no selective dealing**: the deck order is fixed after both shuffles; neither player can choose which card comes next
- **verifiable reveals**: Chaum-Pedersen proofs ensure correct decryption; a player cannot lie about a card's value
- **no trusted dealer**: the shuffle is collaborative; security requires only one honest shuffler

## 3. game protocol

### 3.1 setup

1. player A creates a room, receives an invite code
2. player B joins via invite code
3. both exchange ed25519 public keys (identity for this session)
4. DKG produces the 2-of-3 escrow address
5. both players verify the escrow address
6. both deposit to the escrow address

### 3.2 hand flow

a hand proceeds through phases: **preflop, flop, turn, river, showdown**.

the game engine is **deterministic**: given the same `(rules, stacks, button, deck, actions)`, both peers produce the same state. there is no server authority.

### 3.3 actions

every action is a signed message:

```
PlayerAction {
    hand_number: u64,
    seat: u8,
    action: ActionType,    // fold, check, call, bet(amount), raise(amount), allin
    sequence: u64,         // strictly increasing, replay protection
    signature: [u8; 64],   // ed25519 over (hand_number || seat || sequence || action)
}
```

the signature domain is `"poker.action.v1" || hand_number || seat || sequence || SCALE(action)`.

### 3.4 witness acknowledgement

after receiving a valid action, the opponent produces a **witness ack**:

```
WitnessAck {
    hand_number: u64,
    sequence: u64,
    signature: [u8; 64],   // ed25519 over (hand_number || sequence || action_hash)
}
```

the witness signature domain is `"poker.witness.v1" || hand_number || sequence || SHA256(PlayerAction)`.

once both signatures exist, the action is **co-signed**: neither party can deny it happened, and neither can fabricate it.

### 3.5 transcript

a completed hand produces a `HandTranscript`:

```
HandTranscript {
    hand_number: u64,
    button: u8,
    starting_stacks: [u64],
    deck_commitment: [u8; 32],
    actions: [CoSignedAction],     // each has action.signature + witness_sig
    reveals: [CardReveal],
    result: HandResult,
}
```

both players retain the transcript. it is the evidence for disputes.

## 4. timeouts

### 4.1 the problem

in P2P, there is no server clock. if a player stops responding, the opponent needs a way to claim timeout that the jury can verify.

### 4.2 timeout claim

```
TimeoutClaim {
    hand_number: u64,
    last_sequence: u64,        // last co-signed action
    timed_out_seat: u8,
    action_required_at: u64,   // unix timestamp
    claimed_at: u64,           // unix timestamp
    timeout_secs: u32,         // from table rules
    signature: [u8; 64],       // claimant's ed25519
    state_hash: [u8; 32],      // SHA256 of engine state
}
```

signature domain: `"poker.timeout.v1" || hand_number || last_sequence || seat || action_required_at || claimed_at || timeout_secs || state_hash`.

### 4.3 resolution

a timeout claim alone is not sufficient (the claimant controls the timestamps). resolution requires the jury as relay:

1. claimant submits transcript + timeout claim to any jury validator
2. jury replays the transcript through the deterministic engine
3. jury confirms the game state matches `state_hash`
4. jury issues a **challenge**: the timed-out player has `T` seconds (jury's clock) to submit a valid action
5. if no action arrives within `T`, the jury auto-folds and co-signs the result
6. if an action arrives, the game resumes through the jury as relay

the jury's receipt timestamps are the only clock that matters.

## 5. dispute resolution

### 5.1 cooperative settlement (no dispute)

after the session, both players sign a final `StateUpdate` with agreed balances. this requires only positions 1 + 2 (no jury). the FROST signature unlocks the escrow.

### 5.2 disputed settlement

if players disagree:

1. either player submits the `HandTranscript` to the jury
2. jury replays through the deterministic engine
3. jury determines the correct outcome
4. jury co-signs with the honest player (positions 1+3 or 2+3) to unlock escrow

the dishonest player's funds go to the honest player per the engine's outcome.

### 5.3 jury signing

the jury produces its FROST signature share via nested signing:

1. `t` validators generate nonce commitments (inner FROST round 1)
2. inner binding factors prevent adaptive commitment attacks
3. commitments are aggregated into a single outer commitment
4. outer FROST challenge is computed
5. each validator produces a partial signature using their share
6. partials are aggregated into the jury's outer signature share
7. combined with the honest player's share for a valid 2-of-3 signature

the jury's secret `s_3` is never reconstructed at any point.

## 6. verification

### 6.1 what players can verify

| claim | how to verify |
|-------|---------------|
| escrow address is correct | recompute `Y = Y_1 + Y_2 + Y_3` from public commitments |
| action is authentic | check ed25519 signature against player's public key |
| action is co-signed | check witness signature from opponent |
| transcript is complete | sequence numbers are strictly increasing with no gaps |
| game outcome is correct | replay actions through deterministic engine |
| jury signature is valid | verify Schnorr signature against group public key `Y` |
| jury didn't reconstruct key | verify inner signing used threshold protocol (public commitments visible) |

### 6.2 what players must trust

| assumption | why |
|------------|-----|
| ed25519 is secure | standard, widely audited |
| Pallas curve DLP is hard | same assumption as Zcash |
| FROST threshold is correct | RFC 9591, formally analyzed |
| `t` validators don't collude | economic: validators are staked, collusion is slashable |
| deterministic engine has no bugs | engine is open for review, same code runs on both peers |

### 6.3 frontend verification

the game client should display:

- escrow address (derivable from public commitments)
- your ed25519 session key
- opponent's ed25519 session key
- each action's signature status (signed / co-signed / pending)
- hand transcript hash (for dispute reference)
- jury group public key and validator count

## 7. cryptographic parameters

| parameter | value |
|-----------|-------|
| escrow curve | Pallas (y^2 = x^3 + 5 over Fp, p = 2^254 + ...) |
| shuffle curve | ristretto255 (Curve25519 quotient group) |
| outer FROST | 2-of-3 |
| inner FROST | t-of-n (configurable per deployment) |
| action signatures | ed25519 |
| hash (actions) | SHA-256 |
| hash (FROST challenge) | SHA-512 with domain separation |
| hash (inner binding) | SHA-512 with domain `"frostito-inner-bind"` |
| hash (outer binding) | SHA-512 with domain `"frost-binding-v1"` |
| shuffle encryption | ElGamal on ristretto255 (joint key: pk_A + pk_B) |
| shuffle proof | batch Chaum-Pedersen + scalar grand product |
| card reveal proof | Chaum-Pedersen (DLOG equality) |
| deck commitment | SHA-256 over final encrypted deck |
| state hash | SHA-256 over `"poker.state.v1" || hand_number || sequence || stacks || phase || action_on` |

## 8. wire format

all structured messages use **JSON**. byte arrays (signatures, public keys, hashes) are hex-encoded strings. this makes the protocol human-readable and inspectable from any browser console.

- signatures: 64-byte ed25519, hex-encoded (128 chars)
- public keys: 32-byte compressed, hex-encoded (64 chars)
- Pallas points: 32-byte compressed, hex-encoded (64 chars)
- scalars: 32-byte little-endian, hex-encoded (64 chars)
- card data: JSON arrays of rank/suit strings

## 9. security model

the protocol is secure under the following threat model:

- **malicious player**: can send invalid actions (rejected by signature verification), can refuse to act (handled by timeout + jury relay), can submit false transcripts (jury replays and detects)
- **malicious minority of validators** (< t): cannot produce jury signature, cannot steal funds
- **colluding majority of validators** (>= t): can authorize fraudulent settlements. mitigated by staking + slashing, transparent validator set, and the fact that players choose which validator set to trust
- **network adversary**: can delay messages (handled by timeouts), cannot forge signatures
- **compromised client**: cannot forge opponent's signatures, cannot alter co-signed history

the protocol does NOT protect against:
- both players colluding (they can settle however they want via 1+2)
- all validators colluding with one player (>= t validators + 1 player = 2-of-3)
- side-channel attacks on the client device
