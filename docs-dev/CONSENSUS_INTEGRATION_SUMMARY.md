# safrole consensus and network integration summary

integrated jam-style safrole (simplified sassafras) consensus with networking layer based on graypaper specification

## completed components

### 1. block structure enhancement (block.rs)

added safrole consensus fields:
- **timeslot** (`H_τ`): 6-second slots since jam common era
- **seal signature** (`H_sealsig`): ring vrf or regular bandersnatch signature
- **vrf signature** (`H_vrfsig`): for entropy accumulation
- **author bandersnatch key** (`H_authorbskey`): block author identity
- **epoch markers** (`H_epochmark`): validator set announcements at epoch transitions
- **winners markers** (`H_winnersmark`): winning tickets at submission period end
- **is_ticketed flag**: for fork choice (prefer chains with more ticketed blocks)

added backward-compatible `new_simple()` constructor for non-safrole code

### 2. block seal verification (consensus/block_verifier.rs)

implements jam spec verification:
- **ticketed mode**: ring vrf signature verification (equation 10.11)
- **fallback mode**: regular bandersnatch signature verification (equation 10.12)
- **vrf entropy**: validates entropy accumulation signatures (equation 10.13)
- **timeslot validation**: prevents blocks from far future
- **equivocation detection**: identifies multiple blocks at same timeslot

### 3. block production authorization (consensus/block_producer.rs)

safrole-aware block authoring:
- **authorization checking**: verifies we can author at current timeslot
- **epoch markers**: creates validator set announcements at epoch boundaries
- **winners markers**: publishes winning tickets at submission period end
- **ticket extrinsics**: includes tickets during submission period
- **timeslot calculation**: derives current timeslot from system time

### 4. fork choice rule (consensus/fork_choice.rs)

implements jam best chain selection (best_chain.tex):
- **ticketed preference**: prefers chains with more ring vrf seals
- **equivocation rejection**: excludes chains with conflicting blocks
- **finalization support**: prunes non-descendant chains after grandpa
- **ancestor tracking**: validates chain lineage from finalized block

### 5. network protocols (network/protocols.rs)

jam-style p2p messages:
- **ticket gossip**: broadcast tickets during submission period
- **block announcement** (up 0): notify peers of new blocks
- **block request** (ce 128): fetch blocks by hash or height
- **state request** (ce 129): query state with merkle proofs
- deduplication for ticket broadcasts
- request/response matching for block sync

### 6. time synchronization (network/time_sync.rs)

clock management for safrole:
- **timeslot tracking**: 6-second slots since jam common era
- **clock drift detection**: identifies misaligned clocks
- **network time sync**: adjusts local clock from observed blocks
- **exponential moving average**: smooths transient network delays
- **drift limits**: allows configurable maximum clock skew

### 7. entropy accumulation enhancement

added accessor methods to `EntropyAccumulator`:
- `current_entropy()`: get current accumulator value
- `epoch_entropy(index)`: access historical epoch entropy
- supports safrole's 3-epoch history for ticket verification and seal context

## architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        validator node                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌──────────────┐      ┌──────────────┐      ┌──────────────┐  │
│  │ blockproducer│─────▶│   safrole    │─────▶│ blockverifier│  │
│  │              │      │    state     │      │              │  │
│  │ - authorize  │      │ - epochs     │      │ - seal sigs  │  │
│  │ - create     │      │ - tickets    │      │ - vrf sigs   │  │
│  │   blocks     │      │ - fallback   │      │ - equivocate │  │
│  └──────────────┘      └──────────────┘      └──────────────┘  │
│         │                      │                      │          │
│         │                      │                      │          │
│         ▼                      ▼                      ▼          │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                      fork choice                          │  │
│  │  - prefer ticketed chains                                 │  │
│  │  - reject equivocations                                   │  │
│  │  - track finalized                                        │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              ▼                                   │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    network layer                          │  │
│  │                                                            │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐   │  │
│  │  │ticket gossip │  │block announce│  │  time sync   │   │  │
│  │  │              │  │              │  │              │   │  │
│  │  │- ring vrf    │  │- up 0        │  │- 6s slots    │   │  │
│  │  │  tickets     │  │- ce 128      │  │- drift adj   │   │  │
│  │  │- dedup       │  │  block req   │  │- ema smooth  │   │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘   │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
                        ┌──────────┐
                        │   quic   │
                        │ litep2p  │
                        └──────────┘
```

## safrole consensus flow

1. **ticket submission** (first 2/3 of epoch):
   - validators submit ring vrf tickets
   - tickets accumulated and sorted by score (lower is better)
   - gossip protocol deduplicates across network

2. **winner announcement** (at submission tail start):
   - accumulator saturated → include winners marker in block
   - apply outside-in sequencing
   - tickets become next epoch's seal schedule

3. **block production**:
   - check safrole authorization for current timeslot
   - ticketed mode: use ring vrf seal
   - fallback mode: deterministic validator selection
   - include ticket extrinsics during submission period

4. **verification**:
   - validate seal matches expected key for timeslot
   - verify ring vrf or bandersnatch signature
   - accumulate vrf output to entropy
   - check no equivocations

5. **fork choice**:
   - prefer chains with more ticketed blocks
   - exclude chains with equivocations
   - track best head for grandpa finalization

6. **epoch transition**:
   - rotate validator sets (pending → active → previous)
   - rotate entropy history
   - generate new seal tickets (from accumulator or fallback)
   - include epoch marker in first block

## key jam spec sections implemented

- **§10.3** (safrole.tex): block production and chain growth
- **§10.4**: timekeeping with 6-second slots
- **§10.5**: safrole basic state (tickets, entropy, validator sets)
- **§10.6**: key rotation (pending/active/previous sets)
- **§10.7**: sealing and entropy accumulation
- **§10.8**: slot key sequence (outside-in, fallback)
- **§10.9**: epoch markers
- **§10.10**: tickets extrinsic and ring vrf
- **§11** (best_chain.tex): fork choice with ticket preference

## testing infrastructure

all modules include unit tests:
- safrole state transitions
- ticket accumulation and sequencing
- fallback key generation
- block seal verification (stub for full ring vrf)
- fork choice with ticketed preference
- time synchronization and drift correction
- network protocol message handling

## todo for full production

1. **complete ring vrf verification**:
   - full bandersnatch ring vrf signature checks
   - ticket id validation against vrf output
   - context construction per jam spec

2. **grandpa finality integration**:
   - finalize blocks via fork choice
   - prune non-finalized branches
   - coordinate with beefy for bridge security

3. **litep2p protocols**:
   - custom user protocols for ticket gossip
   - block announcement stream (up 0)
   - block request/response (ce 128/129)

4. **validator key management**:
   - bandersnatch key generation
   - secure key storage
   - ticket signing with ring vrf

5. **performance optimization**:
   - parallel ticket verification
   - efficient merkle proof generation
   - batch signature validation

6. **network resilience**:
   - peer discovery and reputation
   - dos prevention (rate limiting)
   - eclipse attack mitigation

## implementation notes

- used simplified constructors (`Block::new_simple`) for backward compatibility
- manual serialize/deserialize for types with `Digest` (doesn't impl serde)
- entropy accumulator uses blake3 for fast hashing
- time sync uses exponential moving average for smooth adjustment
- fork choice tracks equivocations via timeslot index
- ticket gossip maintains seen-set for deduplication

## files modified/created

### modified
- `crates/zeratul-blockchain/src/block.rs` - added safrole fields
- `crates/zeratul-blockchain/src/application.rs` - use new_simple()
- `crates/zeratul-blockchain/src/consensus/entropy.rs` - added accessors
- `crates/zeratul-blockchain/src/consensus/mod.rs` - export new modules
- `crates/zeratul-blockchain/src/network/mod.rs` - export protocols

### created
- `crates/zeratul-blockchain/src/consensus/block_verifier.rs`
- `crates/zeratul-blockchain/src/consensus/block_producer.rs`
- `crates/zeratul-blockchain/src/consensus/fork_choice.rs`
- `crates/zeratul-blockchain/src/network/protocols.rs`
- `crates/zeratul-blockchain/src/network/time_sync.rs`

all changes maintain backward compatibility with existing code via dual constructor pattern and optional safrole fields
