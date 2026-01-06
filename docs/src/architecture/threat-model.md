# threat model

comprehensive analysis of threats and mitigations in zk.poker.

## adversary types

```
┌─────────────────────────────────────────────────────────────┐
│ adversary         │ capabilities        │ goals             │
├───────────────────┼─────────────────────┼───────────────────┤
│ cheating player   │ protocol deviation  │ win unfairly      │
│ malicious vault   │ storage access      │ steal keys        │
│ network attacker  │ man-in-middle       │ observe/modify    │
│ colluding players │ multiple accounts   │ advantage         │
│ relay operator    │ traffic observation │ metadata          │
└─────────────────────────────────────────────────────────────┘
```

## cheating player attacks

### attack: submit old state

```
attack:
  - player has state v10 (losing $100)
  - submits state v5 (was winning $50)

defense:
  - challenge period ({{DISPUTE_TIMEOUT_BLOCKS}} blocks)
  - victim submits v10
  - higher version wins
  - attacker loses bond

result: attack fails, attacker penalized
```

### attack: selective reveal

```
attack:
  - don't reveal cards when losing
  - hope opponent gives up

defense:
  - timeout mechanism
  - non-responsive player forfeits
  - bond covers dispute fee

result: attack fails, attacker loses hand + fee
```

### attack: invalid shuffle

```
attack:
  - shuffle to known position
  - submit fake proof

defense:
  - ZK proof verification
  - invalid proof rejected
  - cheater identified

result: attack fails, cheater caught
```

### attack: peek at opponent's cards

```
attack:
  - try to see opponent's hole cards
  - gain information advantage

defense:
  - cards encrypted to joint key
  - need both decryption shares
  - can't decrypt without cooperation

result: attack impossible (cryptographic)
```

## vault attacks

### attack: vault database breach

```
attack:
  - compromise vault server
  - steal all encrypted shares

attacker gains:
  - encrypted share blobs
  - unlock_tag hashes

attacker needs (still):
  - user's PIN
  - access to {{VSS_THRESHOLD}} vaults

defense:
  - shares encrypted with user's key
  - TPM sealing (vault can't extract)
  - need 2+ vault breaches

result: single breach insufficient
```

### attack: vault collusion

```
attack:
  - {{VSS_THRESHOLD}} vault operators collude
  - combine encrypted shares

attacker needs (still):
  - user's PIN to decrypt

defense:
  - shares individually encrypted
  - PIN never stored anywhere
  - argon2id makes brute force expensive

result: collusion + PIN brute force = very expensive
```

### attack: online PIN brute force

```
attack:
  - try PINs against vault API
  - guess user's PIN

defense:
  - {{PIN_GUESSES_FREE}} attempts then lockout
  - share deleted after limit
  - rate limiting on API
  - argon2id makes each attempt slow

result: 3 attempts for 4-digit PIN = 0.03% success
```

## network attacks

### attack: man-in-the-middle

```
attack:
  - intercept player communications
  - modify messages

defense:
  - all connections encrypted (noise protocol)
  - authentication with node keys
  - message integrity (AEAD)

result: MITM prevented by encryption
```

### attack: traffic analysis

```
attack:
  - observe message patterns
  - infer game state

attacker learns:
  - message timing
  - approximate sizes
  - connection patterns

attacker doesn't learn:
  - message contents
  - cards
  - balances

result: limited metadata exposure (acceptable)
```

### attack: denial of service

```
attack:
  - flood peer with messages
  - prevent gameplay

defense:
  - rate limiting
  - connection limits
  - timeout → other player wins

result: DoS = self-defeating (attacker loses game)
```

## collusion attacks

### attack: multi-accounting

```
attack:
  - one person, multiple accounts
  - share information between

scenario:
  - 3-player table
  - 2 accounts are colluders
  - share hole card info

defense:
  - heads-up only (2 players)
  - no information to share
  - can't collude with yourself

result: not applicable to 2-player games
```

### attack: bot playing

```
attack:
  - automated perfect play
  - always optimal decisions

defense:
  - this is allowed
  - poker is solved for heads-up
  - no guaranteed profit in limit
  - skill competition, bots welcome

result: not prevented, by design
```

## smart contract attacks

### attack: reentrancy

```
attack:
  - exploit callback in withdrawal
  - drain contract

defense:
  - checks-effects-interactions pattern
  - reentrancy guards
  - thorough auditing

result: standard smart contract security
```

### attack: front-running

```
attack:
  - see dispute transaction
  - submit first with newer state

analysis:
  - actually legitimate behavior
  - newer state should win
  - mempool visibility doesn't help cheater

result: front-running is beneficial here
```

## trusted components

```
what we trust:
  - cryptographic primitives
    (ristretto255, ed25519, argon2id)
  - blockchain consensus
  - user's device (to some extent)
  - at least 1-of-3 vaults honest

what we don't trust:
  - opponents (they might cheat)
  - network (encrypted anyway)
  - individual vault operators
  - relay operators
```

## security assumptions

```
system security requires:
  ✓ argon2id is hard to brute force
  ✓ ristretto255 discrete log is hard
  ✓ ed25519 signatures are unforgeable
  ✓ chacha20-poly1305 is secure
  ✓ blockchain is live and consistent
  ✓ at least 1 vault is honest
  ✓ user keeps PIN secret

if any assumption breaks:
  - specific attack becomes possible
  - other defenses may still hold
  - defense in depth
```

## severity ratings

```
| attack                 | likelihood | impact   | rating   |
|------------------------|------------|----------|----------|
| old state submission   | medium     | medium   | mitigated|
| vault breach (single)  | low        | low      | mitigated|
| vault collusion + PIN  | very low   | high     | mitigated|
| MITM                   | low        | high     | prevented|
| shuffle cheating       | medium     | high     | prevented|
| smart contract exploit | low        | critical | audited  |
```

## reporting vulnerabilities

```
responsible disclosure:
  - email: security@zkpoker.com
  - PGP key available
  - 90-day disclosure timeline

bug bounty:
  - critical: up to $50,000
  - high: up to $10,000
  - medium: up to $2,000
  - low: up to $500
```
