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

## 2. game protocol

### 2.1 setup

1. player A creates a room, receives an invite code
2. player B joins via invite code
3. both exchange ed25519 public keys (identity for this session)
4. DKG produces the 2-of-3 escrow address
5. both players verify the escrow address
6. both deposit to the escrow address

### 2.2 hand flow

a hand proceeds through phases: **preflop, flop, turn, river, showdown**.

the game engine is **deterministic**: given the same `(rules, stacks, button, deck, actions)`, both peers produce the same state. there is no server authority.

### 2.3 actions

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

### 2.4 witness acknowledgement

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

### 2.5 transcript

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

## 3. timeouts

### 3.1 the problem

in P2P, there is no server clock. if a player stops responding, the opponent needs a way to claim timeout that the jury can verify.

### 3.2 timeout claim

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

### 3.3 resolution

a timeout claim alone is not sufficient (the claimant controls the timestamps). resolution requires the jury as relay:

1. claimant submits transcript + timeout claim to any jury validator
2. jury replays the transcript through the deterministic engine
3. jury confirms the game state matches `state_hash`
4. jury issues a **challenge**: the timed-out player has `T` seconds (jury's clock) to submit a valid action
5. if no action arrives within `T`, the jury auto-folds and co-signs the result
6. if an action arrives, the game resumes through the jury as relay

the jury's receipt timestamps are the only clock that matters.

## 4. dispute resolution

### 4.1 cooperative settlement (no dispute)

after the session, both players sign a final `StateUpdate` with agreed balances. this requires only positions 1 + 2 (no jury). the FROST signature unlocks the escrow.

### 4.2 disputed settlement

if players disagree:

1. either player submits the `HandTranscript` to the jury
2. jury replays through the deterministic engine
3. jury determines the correct outcome
4. jury co-signs with the honest player (positions 1+3 or 2+3) to unlock escrow

the dishonest player's funds go to the honest player per the engine's outcome.

### 4.3 jury signing

the jury produces its FROST signature share via nested signing:

1. `t` validators generate nonce commitments (inner FROST round 1)
2. inner binding factors prevent adaptive commitment attacks
3. commitments are aggregated into a single outer commitment
4. outer FROST challenge is computed
5. each validator produces a partial signature using their share
6. partials are aggregated into the jury's outer signature share
7. combined with the honest player's share for a valid 2-of-3 signature

the jury's secret `s_3` is never reconstructed at any point.

## 5. verification

### 5.1 what players can verify

| claim | how to verify |
|-------|---------------|
| escrow address is correct | recompute `Y = Y_1 + Y_2 + Y_3` from public commitments |
| action is authentic | check ed25519 signature against player's public key |
| action is co-signed | check witness signature from opponent |
| transcript is complete | sequence numbers are strictly increasing with no gaps |
| game outcome is correct | replay actions through deterministic engine |
| jury signature is valid | verify Schnorr signature against group public key `Y` |
| jury didn't reconstruct key | verify inner signing used threshold protocol (public commitments visible) |

### 5.2 what players must trust

| assumption | why |
|------------|-----|
| ed25519 is secure | standard, widely audited |
| Pallas curve DLP is hard | same assumption as Zcash |
| FROST threshold is correct | RFC 9591, formally analyzed |
| `t` validators don't collude | economic: validators are staked, collusion is slashable |
| deterministic engine has no bugs | engine is open for review, same code runs on both peers |

### 5.3 frontend verification

the game client should display:

- escrow address (derivable from public commitments)
- your ed25519 session key
- opponent's ed25519 session key
- each action's signature status (signed / co-signed / pending)
- hand transcript hash (for dispute reference)
- jury group public key and validator count

## 6. cryptographic parameters

| parameter | value |
|-----------|-------|
| curve | Pallas (y^2 = x^3 + 5 over Fp, p = 2^254 + ...) |
| outer FROST | 2-of-3 |
| inner FROST | t-of-n (configurable per deployment) |
| action signatures | ed25519 |
| hash (actions) | SHA-256 |
| hash (FROST challenge) | SHA-512 with domain separation |
| hash (inner binding) | SHA-512 with domain `"frostito-inner-bind"` |
| hash (outer binding) | SHA-512 with domain `"frost-binding-v1"` |
| deck commitment | SHA-256 over shuffled deck + randomness |
| state hash | SHA-256 over `"poker.state.v1" || hand_number || sequence || stacks || phase || action_on` |

## 7. wire format

all structured messages use **SCALE codec** (parity-scale-codec). signatures are raw 64-byte ed25519. public keys are 32-byte compressed. Pallas points are 32-byte compressed. scalars are 32-byte little-endian.

## 8. security model

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
