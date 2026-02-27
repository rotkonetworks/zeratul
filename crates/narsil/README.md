# narsil

private syndicate consensus on penumbra using threshold signatures.

a narsil syndicate is a decaf377 spending key split via OSST - it looks like a normal penumbra account to the chain, but requires threshold approval for any action. members coordinate off-chain via P2P, only the final signed transaction hits penumbra.

inspired by [henry de valence's narsil talk](https://www.youtube.com/watch?v=VWdHaKGrjq0&t=16m).

## privacy property

when a syndicate signs a transaction, penumbra validators cannot determine which members participated. internal voting patterns, dissent, and power dynamics remain hidden - the chain only learns that a valid t-of-n subset authorized the action.

this is possible because narsil uses [osst](../osst) (one-step schnorr threshold) signatures with decaf377, which have a "share-free" verification property.

## key model

```
personal account (each member's own penumbra wallet)
    - identity/authentication in P2P layer
    - contribute capital to syndicate
    - receive distributions
    - NOT used to derive syndicate shares (key isolation)

syndicate account (OSST group key)
    - created via DKG among members
    - looks like normal penumbra address
    - threshold required to sign any action
    - members share viewing key for scanning
```

## architecture

```
┌─────────────────────────────────────────────────────────┐
│                     syndicate                           │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐    │
│  │member 1 │  │member 2 │  │member 3 │  │member 4 │    │
│  │ share_1 │  │ share_2 │  │ share_3 │  │ share_4 │    │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘    │
│       │            │            │            │          │
│       └────────────┴─────┬──────┴────────────┘          │
│                          │                              │
│                    ┌─────▼─────┐                        │
│                    │    bft    │  aura-style rounds     │
│                    │ consensus │  rotating proposer     │
│                    └─────┬─────┘                        │
│                          │                              │
│                    ┌─────▼─────┐                        │
│                    │  state    │  replicated state      │
│                    │  machine  │  machine               │
│                    └─────┬─────┘                        │
└──────────────────────────┼──────────────────────────────┘
                           │
                           ▼
                  commitments + proofs → L1
```

## bft consensus

narsil uses aura-style instant finality:

1. rotating proposer creates round with state transition
2. members verify payload and generate osst contributions
3. once t contributions collected, round is finalized
4. the aggregated osst proof IS the finality certificate

no separate voting/commit phases needed - the threshold signature itself proves finality.

## use cases

- **investment syndicates**: pooled capital with private voting on trades
- **multisig treasuries**: DAO funds without revealing signer coalitions
- **joint custody**: shared assets (families, partnerships) with hidden approval patterns

## usage

```rust
use narsil::{Syndicate, Member, SyndicateConfig, StateRoot, StateTransition};

// configure 3-of-5 syndicate
let config = SyndicateConfig::new(5, 3);

// after DKG, create member with their share
let member = Member::new(my_index, my_share, group_pubkey, config);

// create syndicate state machine
let mut syndicate = Syndicate::new(config, group_pubkey, b"genesis");

// propose state transition
let transition = StateTransition::new(
    syndicate.state_root,
    StateRoot::compute(b"new state"),
    b"action data".to_vec(),
);
let mut round = syndicate.propose(transition)?;

// members contribute osst proofs
let contribution = member.contribute(&round, &mut rng);
round.add_contribution(contribution)?;

// finalize once t contributions collected
let block = round.finalize(config.threshold, &group_pubkey)?;
syndicate.apply(block)?;
```

## LLC-style governance

narsil includes traditional LLC governance rules with max 100 splittable shares:

```rust
use narsil::{ShareRegistry, GovernanceRules, Proposal, ActionType, Distribution};

// allocate shares (max 100 total)
let registry = ShareRegistry::with_allocation(&[
    (1, 30),  // member 1: 30%
    (2, 25),  // member 2: 25%
    (3, 45),  // member 3: 45%
])?;

// governance rules with different thresholds
let rules = GovernanceRules {
    routine_threshold: 51,      // simple majority
    major_threshold: 67,        // supermajority
    amendment_threshold: 75,    // 3/4
    existential_threshold: 90,  // near-unanimous
    require_quorum: true,
    quorum_percentage: 50,
    ..Default::default()
};

// create proposal
let mut proposal = Proposal::new(
    1,
    ActionType::Major,
    "acquire new asset".into(),
    encoded_action,
);

// members vote (weighted by shares)
proposal.vote(1, true, &registry)?;   // 30% yes
proposal.vote(3, true, &registry)?;   // +45% = 75% yes
// passes 67% threshold

// calculate distributions
let dist = Distribution::calculate(10_000, &registry);
// member 1 gets 3000, member 2 gets 2500, member 3 gets 4500
```

### action types

| type | default threshold | use case |
|------|-------------------|----------|
| `Routine` | 51% | transfers, small expenditures |
| `Major` | 67% | large investments, new contracts |
| `Amendment` | 75% | add/remove members, rule changes |
| `Existential` | 90% | dissolve, merge |

### share operations

- **issue**: allocate new shares (up to 100 total)
- **transfer**: move shares between members
- **burn**: remove shares (member exit/buyout)

## penumbra actions

syndicates can perform any action a penumbra wallet can:

| action | description | governance level |
|--------|-------------|------------------|
| `Spend` | transfer to address | routine (<1T) / major |
| `Swap` | dex trade | major |
| `Delegate` | stake to validator | major |
| `Undelegate` | unstake | major |
| `IbcTransfer` | cross-chain transfer | major |
| `Distribute` | pro-rata to members | routine |

```rust
use narsil::penumbra::{ActionPlan, SyndicateAction, SpendPlan, Value, Address};

// propose spending from syndicate
let action = SyndicateAction::Spend(SpendPlan::new(
    Value::native(1_000_000),
    dest_address,
));

let plan = ActionPlan::new(
    sequence,
    action,
    fee,
    expiry_height,
);

// signing payload for OSST
let payload = plan.signing_payload();
```

## note management

syndicates track their UTXOs (notes) using shared viewing key:

```rust
use narsil::penumbra::{NoteSet, SyndicateNote, AssetId};

let mut notes = NoteSet::new();

// after scanning chain, add discovered notes
notes.add_note(note);

// check balance
let balance = notes.native_balance();

// select notes for spending
let to_spend = notes.select_notes(&AssetId::native(), amount)?;
```

## modules

- `narsil::bft` - instant finality consensus rounds
- `narsil::governance` - LLC-style share registry and voting
- `narsil::penumbra` - penumbra integration (keys, actions, notes)
- `narsil::state` - state roots, transitions, nullifiers
- `narsil::syndicate` - high-level syndicate API

## curves

inherits curve support from osst:

| feature | curve | compatibility |
|---------|-------|---------------|
| `ristretto255` (default) | curve25519 | polkadot, sr25519 |
| `pallas` | pallas | zcash orchard |
| `secp256k1` | secp256k1 | bitcoin, ethereum |
| `decaf377` | decaf377 | penumbra |

## license

MIT OR Apache-2.0
