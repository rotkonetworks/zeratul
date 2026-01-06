# architecture overview

zk.poker is built on four layers: identity, channels, game protocol, and p2p networking.

## layer diagram

```
┌─────────────────────────────────────────────────────────────┐
│                     APPLICATION LAYER                       │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  poker client (bevy + egui)                         │   │
│  │  - login UI                                          │   │
│  │  - table rendering                                   │   │
│  │  - hand history                                      │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                      GAME PROTOCOL                          │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  mental poker (zk-shuffle)                          │   │
│  │  - shuffle proofs                                    │   │
│  │  - card reveal                                       │   │
│  │  - hand evaluation                                   │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                     STATE CHANNELS                          │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  off-chain state machine                            │   │
│  │  - signed state updates                              │   │
│  │  - on-chain settlement                               │   │
│  │  - dispute resolution                                │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                      P2P NETWORK                            │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  iroh (QUIC-based p2p)                              │   │
│  │  - direct connections                                │   │
│  │  - relay fallback                                    │   │
│  │  - NAT traversal                                     │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                       IDENTITY                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  ghettobox                                          │   │
│  │  - email + PIN login                                 │   │
│  │  - VSS key recovery                                  │   │
│  │  - {{SIGNATURE_SCHEME}} signing                                  │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## component breakdown

### identity (ghettobox)

```
responsibility: who is the player?

components:
  - vault nodes ({{VSS_TOTAL_SHARES}} distributed)
  - PIN stretching ({{PIN_KDF}})
  - VSS secret sharing
  - {{SIGNATURE_SCHEME}} key derivation

trust model:
  - {{VSS_THRESHOLD}}-of-{{VSS_TOTAL_SHARES}} vaults must cooperate
  - user must know PIN
  - no single point of failure
```

### state channels

```
responsibility: track game state & balances

on-chain:
  - channel open (deposit funds)
  - channel close (withdraw funds)
  - dispute resolution

off-chain:
  - all gameplay
  - signed state updates
  - instant finality between players

trust model:
  - latest signed state is canonical
  - can always go on-chain if needed
  - timeout = {{DISPUTE_TIMEOUT_BLOCKS}} blocks
```

### game protocol

```
responsibility: fair poker game

components:
  - shuffle protocol ({{SHUFFLE_CURVE}})
  - card encryption (elgamal)
  - ZK proofs (shuffle validity)
  - hand evaluation

trust model:
  - cryptographic fairness
  - no trusted dealer
  - verifiable on-chain
```

### p2p network

```
responsibility: player communication

components:
  - iroh (QUIC transport)
  - relay nodes (NAT traversal)
  - signaling (peer discovery)

trust model:
  - direct connections when possible
  - relays don't see message contents
  - authenticated with {{SIGNATURE_SCHEME}}
```

## data flow

```
game action flow:

  player A                          player B
     │                                  │
     │  1. decide action (raise $50)    │
     │                                  │
     ▼                                  │
  ┌─────┐                               │
  │sign │ state_v2 = sign(state_v1 + action)
  └─────┘                               │
     │                                  │
     │  2. send via p2p ───────────────▶│
     │                                  │
     │                                  ▼
     │                              ┌─────┐
     │                              │verify│
     │                              └─────┘
     │                                  │
     │◀──────── 3. counter-sign ────────│
     │                                  │
     ▼                                  ▼
  state_v2                          state_v2
  (both have                        (both have
   identical                         identical
   signed state)                     signed state)
```

## storage

```
where data lives:

on-chain:
  - channel state (open/closed)
  - final balances
  - dispute outcomes
  - reputation history

vault nodes:
  - encrypted key shares
  - unlock_tag hashes

client local:
  - session signing key
  - hand history (optional)
  - preferences

nowhere stored:
  - PIN (only in user's head)
  - private keys (derived on demand)
```
