# Secret Collector - Privacy-Preserving Card Game

A minimal but interesting application demonstrating privacy-preserving state transitions using ZK proofs.

## What is it?

A collectible card game where:
- **You collect secret cards** (rare dragons, mythical creatures, etc.)
- **Prove you own cards** without revealing which ones
- **Trade cards privately** - no one knows what you traded
- **Leaderboard** shows who has most cards, but not which cards

## How it works

### Client (Browser - WASM)

```
User generates proof:
"I know a secret S such that hash(S) = commitment C"

Where S = {card_id, rarity, attributes, secret_key}
```

### Server (PolkaVM Verifier)

```
Verify proof without learning S:
- Check proof is valid ✓
- Check commitment not already claimed ✓
- Update global state ✓
```

### Privacy Properties

- ✅ Server doesn't know which cards you own
- ✅ Other players don't know your collection
- ✅ You can prove ownership without revealing the card
- ✅ Trades happen without exposing card details

## Example Flow

### 1. Mint a Card (Client)

```javascript
// User clicks "Open Pack"
const card = {
  id: randomUUID(),
  type: "Dragon",
  rarity: "Legendary",
  power: 9000,
  secret: randomBytes(32)
};

// Create commitment
const commitment = hash(card);

// Generate proof: "I know card such that hash(card) = commitment"
const proof = await generateProof(card, commitment);

// Submit to server
POST /mint {
  commitment: "0x1234...",
  proof: "0xabcd..."
}

// Server verifies proof and adds commitment to registry
// You keep the card secret locally
```

### 2. Prove Ownership

```javascript
// User clicks "Prove I have a Legendary"
const proof = await generateProof(myLegendaryCard, challenge);

POST /prove {
  challenge: "prove_legendary",
  proof: "0x5678..."
}

// Server verifies you own a legendary card
// Without learning which one!
```

### 3. Trade (Private Transfer)

```javascript
// User A wants to trade with User B
// Both generate proofs they own their cards
// Server atomically swaps ownership
// Neither reveals which card they're trading

POST /trade {
  proof_a: "0xaaaa...",
  proof_b: "0xbbbb...",
  commitment_a: "0x1111...",
  commitment_b: "0x2222..."
}
```

## Architecture

```
┌─────────────────────────────────────────────┐
│  Browser (Wasm)                             │
│  ┌────────────────────────────────────────┐ │
│  │ Card Collection (Local Storage)        │ │
│  │ [{id, type, rarity, power, secret}]    │ │
│  └────────────────────────────────────────┘ │
│              │                               │
│              ▼                               │
│  ┌────────────────────────────────────────┐ │
│  │ Ligerito Prover (WASM)                 │ │
│  │ - Generate ZK proof                    │ │
│  │ - Prove: hash(card) = commitment       │ │
│  └────────────────────────────────────────┘ │
│              │                               │
│              │ POST /mint, /prove, /trade   │
└──────────────┼──────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────┐
│  Server (Rust + PolkaVM)                    │
│  ┌────────────────────────────────────────┐ │
│  │ Public State                           │ │
│  │ - Commitments: [0x1234, 0x5678, ...]  │ │
│  │ - Ownership: {commitment → player}    │ │
│  │ - Leaderboard: {player → count}      │ │
│  └────────────────────────────────────────┘ │
│              │                               │
│              ▼                               │
│  ┌────────────────────────────────────────┐ │
│  │ Ligerito Verifier (PolkaVM)            │ │
│  │ - Verify ZK proof                      │ │
│  │ - Check commitment valid               │ │
│  │ - Update state                         │ │
│  └────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

## What makes it interesting?

### 1. **Actual Privacy**
Unlike regular card games where server sees everything, here:
- Server only sees cryptographic commitments
- Your collection is truly private
- Trades are atomic but secret

### 2. **Proof Challenges**
Server can challenge: "Prove you have a Legendary Dragon"
You generate proof without revealing which legendary dragon!

### 3. **Leaderboards with Privacy**
```
Top Collectors:
1. Alice: 47 cards ⭐⭐⭐
2. Bob: 32 cards ⭐⭐
3. Carol: 28 cards ⭐

// No one knows WHICH cards they have!
```

### 4. **Rarity Tiers**
Cards have different rarities:
- Common (70%): 1-3 power
- Rare (20%): 4-6 power
- Epic (7%): 7-8 power
- Legendary (3%): 9-10 power

Prove you have X legendaries without revealing which ones!

## Building

### 1. Build Prover (WASM)
```bash
cd client
wasm-pack build --target web
```

### 2. Build Server
```bash
cd server
cargo build --release
```

### 3. Run
```bash
# Terminal 1: Server
cd server
cargo run

# Terminal 2: Client
cd client
python3 -m http.server 8080
# Open http://localhost:8080
```

## API

### POST /mint
Mint a new card (create commitment)

**Request:**
```json
{
  "commitment": "0x1234abcd...",
  "proof": "0xproof..."
}
```

**Response:**
```json
{
  "success": true,
  "card_id": "uuid",
  "message": "Card minted! Keep your secret safe."
}
```

### POST /prove
Prove you own a card matching criteria

**Request:**
```json
{
  "challenge": "prove_legendary",
  "commitment": "0x1234...",
  "proof": "0xproof..."
}
```

**Response:**
```json
{
  "success": true,
  "achievement_unlocked": "Legendary Collector"
}
```

### POST /trade
Atomic swap of two cards

**Request:**
```json
{
  "from_commitment": "0xaaaa...",
  "to_commitment": "0xbbbb...",
  "proof_a": "0x...",
  "proof_b": "0x..."
}
```

### GET /leaderboard
Get top collectors (by card count, not which cards)

**Response:**
```json
{
  "top_collectors": [
    {"player": "alice", "cards": 47, "legendaries": 5},
    {"player": "bob", "cards": 32, "legendaries": 2}
  ]
}
```

## Game Mechanics

### Opening Packs
```javascript
// Click "Open Pack" button
// Client generates random card with probability:
const rarity = random() < 0.70 ? "Common" :
               random() < 0.90 ? "Rare" :
               random() < 0.97 ? "Epic" : "Legendary";

const card = {
  type: randomChoice(["Dragon", "Phoenix", "Unicorn", "Griffin"]),
  rarity: rarity,
  power: rarityToPower(rarity),
  secret: randomBytes(32)
};

// Create commitment and prove ownership
```

### Achievements
- **First Card**: Open your first pack
- **Legendary Hunter**: Prove you own 3 legendaries
- **Complete Set**: Own one of each type
- **Trade Master**: Complete 10 trades
- **Secret Keeper**: Never reveal your cards publicly

## Why This Demo?

This demonstrates all key privacy primitives:

1. **Commitments**: Hide data behind hash
2. **ZK Proofs**: Prove properties without revealing data
3. **Private State**: Server manages public state, users keep secrets
4. **Atomic Swaps**: Trade without revealing what you're trading
5. **Selective Disclosure**: Prove you have legendary without revealing which

But packaged as a fun, understandable game instead of financial primitives!

## Technical Details

### Proof Circuit (Simplified)

```
Public inputs:
  - commitment C
  - challenge (e.g., "prove legendary")

Private inputs:
  - card data: {type, rarity, power}
  - secret s

Circuit proves:
  1. hash(card || secret) = C
  2. rarity = "Legendary" (if challenged)
  3. Card is valid (power in range, etc.)
```

### State Tree

```
Registry {
  commitments: Set<Hash>,        // All minted cards
  ownership: Map<Hash, Player>,  // Who owns which commitment
  nullifiers: Set<Hash>,         // Spent/traded cards
}
```

## Future Enhancements

- [ ] Card battles (prove power without revealing)
- [ ] Encrypted marketplace listings
- [ ] Card crafting (combine 3 commons → 1 rare)
- [ ] Guilds with shared secrets
- [ ] PvP tournaments with hidden decks

## Files

```
private_registry/
├── README.md                 # This file
├── client/                   # Browser app (WASM)
│   ├── Cargo.toml
│   ├── src/lib.rs           # Prover logic
│   ├── www/
│   │   ├── index.html       # Game UI
│   │   ├── app.js           # Game logic
│   │   └── style.css        # Styling
│   └── pkg/                 # WASM output
├── server/                   # Rust server
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # HTTP API
│       ├── state.rs         # Game state
│       └── verifier.rs      # PolkaVM verifier
└── shared/                   # Shared types
    └── types.rs             # Card, Commitment, etc.
```

This is a complete, working mini-application that demonstrates privacy-preserving state transitions in a fun, accessible way!
