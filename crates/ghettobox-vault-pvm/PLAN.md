# poker + blockchain integration plan

## current state

### zeratul crates (~/rotko/zeratul)
- **zk-shuffle**: mental poker with elgamal/ristretto255, batch chaum-pedersen proofs, grand product permutation check
- **poker-client**: bevy game with chain.rs trait for blockchain ops (mock impl)
- **state-channel**: off-chain channel logic with dispute resolution
- **escrow-revive**: polkavm contract with VSS merkle verification
- **zeratul-blockchain**: commonware-based chain with ligerito proofs

### ghettobox (this repo)
- pin-protected secret recovery with tpm
- could be used for player key recovery

## integration architecture

```
┌──────────────────┐     ┌──────────────────┐
│  poker-client    │────▶│  poker-p2p       │
│  (bevy UI)       │     │  (iroh/libp2p)   │
└────────┬─────────┘     └────────┬─────────┘
         │                        │
         │ mental poker           │ shuffle proofs
         │ state updates          │ reveal messages
         ▼                        ▼
┌──────────────────────────────────────────┐
│              state-channel               │
│  - off-chain game state                  │
│  - signed state updates (nonce++)        │
│  - dispute evidence                      │
└────────────────────┬─────────────────────┘
                     │
                     │ on-chain ops only:
                     │ - open channel (deposit funds)
                     │ - close channel (withdraw)
                     │ - dispute (submit proof)
                     ▼
┌──────────────────────────────────────────┐
│         zeratul-blockchain               │
│  ┌─────────────────────────────────────┐ │
│  │ poker-channel pallet/contract       │ │
│  │ - verify zk-shuffle proofs          │ │
│  │ - enforce game rules                │ │
│  │ - settle disputes                   │ │
│  └─────────────────────────────────────┘ │
└──────────────────────────────────────────┘
```

## implementation tasks

### phase 1: on-chain shuffle verification

1. **zk-shuffle no_std verifier extraction**
   - extract `RemaskingVerifier::verify()` to standalone no_std lib
   - compile to polkavm target (riscv64emac-unknown-none-polkavm)
   - test with escrow-revive patterns

2. **poker-channel contract (revive)**
   - create `crates/poker-channel-revive/`
   - storage: games, player deposits, state hashes
   - entry points:
     - `createGame(gameId, params, deposit)`
     - `joinGame(gameId, deposit)`
     - `submitShuffleProof(gameId, round, proof)`
     - `submitState(gameId, nonce, stateHash, sigs)`
     - `dispute(gameId, lastValidState, evidence)`
     - `settle(gameId)`

### phase 2: chain client implementation

3. **subxt integration in poker-client**
   - replace MockChainClient with real impl
   - use smoldot for light client (wasm compatible)
   - subscribe to game events
   - submit transactions

4. **wallet integration**
   - use existing `keys.rs` / `signing.rs`
   - connect to ghettobox for key recovery (optional tier)

### phase 3: state channel refinement

5. **state-channel + zk-shuffle integration**
   - `PokerState` includes:
     - encrypted deck (elgamal ciphertexts)
     - shuffle proofs per player
     - betting state
     - revealed cards
   - `transition.rs` validates state changes locally

6. **dispute protocol**
   - on dispute: submit last signed state + proof of violation
   - contract verifies:
     - signatures valid
     - if shuffle: verify zk-shuffle proof
     - if reveal: check commitment matches
   - timeout → last valid state wins

### phase 4: p2p improvements

7. **poker-p2p iroh integration**
   - fix sha2 version conflict (use iroh's sha2)
   - implement gossip for table discovery
   - direct connections for game messages

## file changes

### new files
- `crates/poker-channel-revive/` - polkavm contract
- `crates/zk-shuffle-verifier/` - no_std verifier lib
- `crates/chain-client/src/subxt.rs` - real chain client

### modifications
- `crates/poker-client/src/chain.rs` - impl ChainClient for SubxtClient
- `crates/state-channel/src/state.rs` - add zk-shuffle types
- `crates/zk-shuffle/Cargo.toml` - split verifier feature

## priority order

1. zk-shuffle no_std verifier (unblocks on-chain verification)
2. poker-channel contract (core on-chain logic)
3. chain client impl (connects client to chain)
4. state-channel refinement (full game flow)
5. p2p fixes (optional, mock works for testing)

## open questions

- use pallet (runtime) or revive contract for poker-channel?
  - pallet: faster, native, but needs runtime upgrade
  - revive: deployable, upgradable, but gas costs
  - **recommendation**: revive contract for flexibility

- shuffle proof size on-chain?
  - batch proof: 96 bytes (constant!)
  - deltas: 64 bytes × 52 cards = 3.3 KB per shuffle
  - **optimization**: only submit deltas for disputed shuffles

---

# proactive secret sharing (pss) vault

## problem

the 2-of-3 threshold is fragile. if 2 providers leave, user keys are lost forever.
all mitigations (user backups, bonds, long unstake periods) are bad ux.

## solution: zeratul/osst pss

use our own osst (one-step schnorr threshold) + reshare protocol to redistribute
shares periodically without reconstructing the secret. providers can join/leave,
user data stays encrypted to the same group pubkey.

### key differences from commonware approach

- **lightweight**: osst only, no massive dependency tree
- **ristretto255**: polkadot/sr25519 compatible (not BLS12-381)
- **non-interactive verification**: OSST proofs don't require coordination
- **simple coordination**: HTTP/axum (same as existing vault API)

### architecture

```
current flow:
  user → [share1, share2, share3] → 3 providers (ed25519 per-node)
  recovery: 2/3 threshold recombine

new flow (pss):
  1. initial: DKG generates group_pubkey + private shares for each provider
  2. user data encrypted to group_pubkey (ristretto255)
  3. each epoch: reshare protocol redistributes shares
     - new providers can join
     - old providers can leave
     - group_pubkey stays constant (invariant)
  4. recovery: threshold decryption via OSST contributions
```

### components

```
vault-pvm/
├── src/
│   ├── main.rs           # polkavm host + reshare routes
│   ├── pss/
│   │   ├── mod.rs        # module root
│   │   ├── client.rs     # HTTP client for provider-to-provider comms
│   │   ├── config.rs     # ProviderConfig, NetworkConfig
│   │   ├── http.rs       # reshare HTTP handlers (axum)
│   │   ├── recovery.rs   # threshold recovery via OSST
│   │   └── reshare.rs    # epoch-based reshare protocol
│   └── ...
```

### osst primitives (from zeratul/osst)

- `SecretShare<Scalar>`: holder's share with 1-indexed index
- `Contribution<RistrettoPoint>`: schnorr proof (commitment, response)
- `verify()`: non-interactive threshold verification
- `Dealer<RistrettoPoint>`: generates subshares during reshare
- `Aggregator<RistrettoPoint>`: collects and combines subshares
- `ReshareState<RistrettoPoint>`: on-chain coordination

### flow

1. **bootstrap (DKG)**
   - n providers run feldman VSS to generate shares
   - output: group_pubkey (ristretto255) + private share per provider
   - save to config, restart in threshold mode

2. **steady state (recovery)**
   - user registers: encrypt share to group_pubkey
   - user recovers: t providers each submit OSST contribution
   - verifier checks: g^{Σ μ_i·s_i} = Y^{c̄} · Π u_i^{μ_i}
   - if valid: threshold decrypt user share

3. **reshare (per epoch)**
   - triggered by: provider join/leave, periodic rotation
   - old dealers create polynomials with share as constant term
   - subshares distributed to new players via HTTP (/reshare/subshare)
   - players verify against dealer commitments
   - aggregate using lagrange coefficients
   - group_pubkey unchanged (invariant check)

### dependencies (minimal)

```toml
# threshold crypto (OSST + reshare)
osst = { path = "../zeratul/crates/osst", features = ["ristretto255"] }

# curve crypto (shared with osst)
curve25519-dalek = "4.1"

# HTTP client for provider coordination (already in vault-pvm via axum)
reqwest = { version = "0.12", features = ["json"] }
```

### threshold decryption (ristretto255)

elgamal-style encryption to group key:
- encrypt: `(R, C) = (g^r, m ⊕ H(Y^r))`
- partial decrypt: each provider computes `R^{x_i}` with OSST proof
- combine: lagrange interpolate partials → `Y^r` → `m`

### security properties

- **non-interactive**: provers generate OSST proofs independently
- **share-free verification**: verifier only needs group pubkey
- **asynchronous**: provers submit at their own pace
- **(t-1)-OMDL secure**: proven in random oracle model

### implementation status

1. ✅ add osst deps (zeratul-p2p not needed, using HTTP)
2. ✅ pss config types (ProviderConfig, NetworkConfig)
3. ✅ threshold recovery via OSST contributions (pss/recovery.rs)
4. ✅ reshare module for provider rotation (pss/reshare.rs)
5. ✅ HTTP endpoints for reshare coordination (pss/http.rs + main.rs routes)
6. ✅ HTTP client for provider-to-provider communication (pss/client.rs)
7. ⬜ update seal/unseal to use group key
