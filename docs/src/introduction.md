# zk.poker

trustless peer-to-peer poker using mental poker cryptography and state channels.

## what is this

zk.poker is a decentralized poker protocol where:

- **no house edge** - players play directly against each other
- **no trusted dealer** - cryptographic shuffle ensures fairness
- **no extensions** - email + PIN login via ghettobox
- **no custody** - funds stay in your control via state channels
- **no cheating** - all game state is verifiable on-chain

## how it works

```
┌─────────────────────────────────────────────────────────────┐
│                      PLAYER A                               │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐  │
│  │ wallet  │───▶│ channel │───▶│  game   │───▶│  p2p    │  │
│  └─────────┘    └─────────┘    └─────────┘    └─────────┘  │
│       │              │              │              │        │
└───────│──────────────│──────────────│──────────────│────────┘
        │              │              │              │
        │         state channel      game state      │
        │          (on-chain)       (off-chain)      │
        │              │              │              │
┌───────│──────────────│──────────────│──────────────│────────┐
│       │              │              │              │        │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐  │
│  │ wallet  │◀───│ channel │◀───│  game   │◀───│  p2p    │  │
│  └─────────┘    └─────────┘    └─────────┘    └─────────┘  │
│                      PLAYER B                               │
└─────────────────────────────────────────────────────────────┘
```

## key properties

| property | how |
|----------|-----|
| fair shuffle | mental poker with {{SHUFFLE_CURVE}} proofs |
| hidden cards | elgamal encryption, revealed only when needed |
| instant play | off-chain state updates, on-chain settlement |
| trustless | all actions are signed and verifiable |
| recoverable | ghettobox {{VSS_THRESHOLD}}-of-{{VSS_TOTAL_SHARES}} vault backup |

## components

- **[ghettobox](./identity/ghettobox.md)** - PIN-protected identity without browser extensions
- **[mental poker](./crypto/mental-poker.md)** - cryptographic card shuffle and reveal
- **[state channels](./channels/lifecycle.md)** - off-chain game state with on-chain settlement
- **[reputation](./economics/reputation.md)** - on-chain history prevents griefing

## quick start

```rust
// login with email + PIN
let account = ghettobox::login("alice@example.com", "1234").await?;

// join a table (opens state channel)
let table = Table::join("abc123", account).await?;

// play poker
table.call().await?;
table.raise(100).await?;
table.fold().await?;

// leave table (cooperative close)
table.leave().await?;
```
