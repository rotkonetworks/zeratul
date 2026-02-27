# narsil: private syndicate consensus

*threshold custody with hidden voting patterns*

## the problem

penumbra gives individuals privacy. but what about groups?

consider a syndicate treasury. even with shielded assets, the coordination
layer leaks information: who proposed what, who voted how, which signers
participated. an adversary watching coordination can infer power dynamics,
identify dissent, and target key members.

the question: can we have threshold custody where the chain learns nothing
about internal governance?

## the insight

penumbra uses decaf377 for spending keys. decaf377 is a prime-order group.
OSST (one-step schnorr threshold) works over any prime-order group.

therefore: split a penumbra spending key via OSST. the syndicate looks like
a normal penumbra account to validators. when threshold shares sign, the
chain sees a valid signature - nothing more.

```
                    penumbra validators
                           │
                           │ see: valid signature
                           │      normal account
                           │
                    ┌──────┴──────┐
                    │  syndicate  │
                    │   account   │
                    └──────┬──────┘
                           │
          ┌────────────────┼────────────────┐
          │                │                │
     ┌────┴────┐      ┌────┴────┐      ┌────┴────┐
     │ alice   │      │ bob     │      │ carol   │
     │30 shares│      │30 shares│      │40 shares│
     └─────────┘      └─────────┘      └─────────┘

     coordination happens here (P2P)
     chain never sees it
```

## why OSST, not FROST

FROST has accountability: verifiers can identify which signers participated.
useful for auditing but terrible for privacy.

OSST has the opposite property: even with the transcript, you cannot determine
which subset signed. the chain sees a valid threshold signature but cannot
distinguish alice+bob from alice+carol from bob+carol.

this is exactly what we want. internal voting patterns remain private.

(note: OSST resharing does have accountability via liveness proofs. you can
verify that resharing participants are behaving correctly. but the final
signature itself reveals nothing about which subset participated.)

## the 100 share model

syndicates have exactly 100 OSST key shares. one share = one cryptographic
unit = one governance unit. no split between "members" and "shares".

why 100?
- fine-grained ownership (1% increments)
- maps to familiar legal structures (LLC percentages)
- OSST scales fine to 100 participants
- polynomial degree ~67 for 67% threshold - trivial computation

```
alice owns 30 shares = alice holds 30 OSST key shares
67% threshold        = need contributions from 67 shares
                       doesn't matter if held by 1 or 67 people
```

storage: 30 shares × 32 bytes = 960 bytes per member. trivial.

## the stack

minimal layers, no overkill:

```
┌─────────────────────────────────────────────────────────┐
│                      narsil                              │
├─────────────────────────────────────────────────────────┤
│                                                          │
│   OSST ──────────────► threshold signing                │
│     │                  (core, non-negotiable)           │
│     │                                                    │
│     └─► 100 key shares split via shamir                 │
│                                                          │
├─────────────────────────────────────────────────────────┤
│                                                          │
│   zoda-vss ──────────► verifiable share distribution    │
│                        - dealer sends shares to members │
│                        - header commits to polynomial   │
│                        - recipients verify before accept│
│                        - backup/recovery mechanism      │
│                                                          │
├─────────────────────────────────────────────────────────┤
│                                                          │
│   penumbra keys ─────► per-member encryption            │
│                        - encrypt your shares at rest    │
│                        - no group key needed            │
│                        - lose key = lose shares         │
│                          (syndicate can reshare)        │
│                                                          │
├─────────────────────────────────────────────────────────┤
│                                                          │
│   relay network ─────► async coordination               │
│                        - no direct P2P (leaks metadata) │
│                        - zoda-vss shares to relays      │
│                        - members poll, never connect    │
│                        - tor/mixnet for extra privacy   │
│                                                          │
├─────────────────────────────────────────────────────────┤
│                                                          │
│   simple state ──────► just a struct, serialize, hash   │
│                        - no merkle trees needed         │
│                        - everyone has full state        │
│                        - small syndicate, small state   │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

## what we don't need

**cnidarium/JMT**: merkle proofs are for proving state to untrusted parties.
in a private syndicate, everyone is a member. everyone has full state.
no need to prove "alice has 30 shares" - everyone already knows.

**commonware ZODA (DA coding)**: data availability coding is for distributing
large blocks to many nodes with immediate validity guarantees. our "blocks"
are tiny proposals. zoda-vss handles distribution with verifiable shares.

**direct P2P (iroh, libp2p)**: direct connections leak metadata - IP addresses,
connection timing, who talks to whom, activity patterns. even with encrypted
content, the graph structure reveals syndicate membership and dynamics.

narsil uses **HTTP/WebSocket relay** instead:
- members POST/GET to public relay endpoints using pseudonymous mailboxes
- relay sees only: mailbox ID (hash), message size, timestamp
- relay cannot read content (e2e encrypted) or link mailboxes to identities
- multiple relays can be used for redundancy
- works through firewalls, load balancers, CDNs

## syndicate formation

three modes:

**capital-weighted**: shares proportional to capital contributed.
deposit 30 UM into 100 UM syndicate → 30 shares.

**founder-controlled**: founder gets all 100 shares initially,
allocates to others as desired.

**equal-shares**: founding members split shares equally.

share policies:
- **fixed**: total supply locked at formation
- **open-ended**: can issue new shares (dilutes existing)
- **locked**: no transfers allowed

## governance thresholds

not all actions are equal:

| action type | threshold | example |
|-------------|-----------|---------|
| routine | 51 shares | small spend, queries |
| major | 67 shares | large spend, delegation |
| amendment | 75 shares | change rules |
| existential | 90 shares | dissolve, transfer all |

quorum: 51 shares must vote (not just approve) for any proposal.

## share distribution with zoda-vss

when dealer creates syndicate or transfers shares:

```
dealer has 30 OSST shares to give bob
              │
              ▼
        zoda-vss deal
              │
    ┌─────────┼─────────┐
    │         │         │
  header    share_1  share_n
    │         │         │
    │         └────┬────┘
    │              │
    │         bob receives
    │              │
    └──────► bob verifies against header
             "yes, these are valid shares
              from the committed polynomial"
```

bob knows his shares are valid before accepting.
no need to trust the dealer completely.

## share backup with zoda-vss

alice wants to backup her 30 OSST shares:

```
alice's 30 OSST shares
         │
    zoda-vss (3-of-5)
         │
    ┌────┴────┬────┬────┬────┐
   z1   z2   z3   z4   z5
    │    │    │    │    │
  usb  safe cloud friend_1 friend_2
```

if alice loses her device, any 3 backups reconstruct her OSST shares.
syndicate doesn't need to reshare unless alice is permanently gone.

## simple state

syndicate state is tiny. just serialize it:

```rust
struct SyndicateState {
    // 100 entries max, ~3KB
    shares: BTreeMap<ShareId, MemberPubkey>,

    // few bytes
    rules: GovernanceRules,

    // handful active
    proposals: Vec<Proposal>,

    // sparse
    votes: HashMap<(ProposalId, ShareId), Vote>,

    // metadata
    members: Vec<MemberInfo>,
}

// commit to state
let state_bytes = borsh::to_vec(&state)?;
let state_hash = sha256(&state_bytes);

// OSST signature over state hash = finality
let signature = threshold_sign(state_hash, contributions)?;
```

no merkle trees. no proofs. everyone has the full state.
state hash + OSST signature = consensus achieved.

## proposal flow

```
1. proposer creates proposal
   ┌──────────────────────────────────┐
   │ SignedProposal {                  │
   │   state_hash,  // replay protect │
   │   sequence,    // ordering        │
   │   proposal: Spend { to, amount }, │
   │   signature,   // proposer signs  │
   │ }                                 │
   └──────────────────────────────────┘
                 │
                 ▼
2. encrypt, zoda-vss split, post to relays
                 │
                 ▼
3. members poll relays, reconstruct, decrypt
   verify signature + state_hash + sequence
                 │
                 ▼
4. members vote (yes/no/abstain)
   votes via same relay model
   votes weighted by shares held
                 │
                 ▼
5. if threshold reached:
   - OSST contributions posted to relays
   - any member can aggregate locally
   - first valid tx submitted to penumbra
                 │
                 ▼
6. chain sees normal transaction
   no idea who proposed, who voted, who signed
```

## key isolation

syndicate keys independent of personal keys:

```
alice's penumbra wallet          alice's syndicate shares
       ┌──────────┐                   ┌──────────┐
       │ spend_key│                   │ 30 OSST  │
       │ fvk      │                   │ shares   │
       └────┬─────┘                   └────┬─────┘
            │                              │
            │                         encrypted with
            │                         alice's spend_key
            │                              │
            ▼                              ▼
       alice's funds              syndicate funds
       (personal)                 (collective)
```

compromise alice's personal key: attacker gets her shares, but needs
67 total to sign. syndicate detects, reshares to exclude alice.

compromise syndicate share: doesn't touch alice's personal funds.

## penumbra integration

syndicates execute standard penumbra actions:

- **Spend**: pay from syndicate balance
- **Swap**: trade assets via DEX
- **Delegate**: stake to validators
- **Undelegate**: unstake
- **IbcTransfer**: cross-chain moves
- **Distribute**: pro-rata payouts

each action generates signing payload. threshold shares contribute.
OSST aggregates. transaction submitted. chain sees normal account.

## token factory integration

penumbra-token-factory creates public tokens.
narsil creates private control.

combine them:

```
         PUBLIC LAYER
┌─────────────────────────────────┐
│  Token Factory                   │
│  - SYNDICATE-TOKEN              │
│  - tradeable on DEX             │
│  - 1M total supply              │
└───────────────┬─────────────────┘
                │ represents claim on
                ▼
         PRIVATE LAYER
┌─────────────────────────────────┐
│  Narsil Syndicate               │
│  - 5 founders, 100 shares       │
│  - 67% threshold                │
│  - hidden governance            │
│  - holds actual assets          │
└─────────────────────────────────┘
```

public holders: economic exposure, liquidity, dividends.
public holders don't get: votes, governance visibility, signer knowledge.

founders maintain operational control.
economic interest is widely distributed.

traditional finance model with cryptographic privacy guarantees.

## networking: why not direct P2P

direct P2P connections leak metadata even with encrypted content:

```
direct P2P (leaky)
──────────────────
alice ◄──────► bob
  │              │
  └──────► carol ◄┘

network observer sees:
- alice, bob, carol IP addresses
- they form a connected group
- alice sent at 3am (insomniac? different timezone?)
- bob responded in 5 minutes (eager participant)
- carol took 2 days (reluctant? traveling?)
- connection frequency reveals activity level
- graph structure reveals power dynamics
```

this is devastating for a "private" syndicate. the content is encrypted
but the coordination pattern is exposed. adversary knows who's in the
group, who's active, who responds to whom.

## networking: zoda-vss relay model

instead: members never connect to each other. only to public relays.

```
zoda-vss + distributed relays
─────────────────────────────

alice wants to broadcast proposal:

    proposal (encrypted)
           │
      zoda-vss split (3-of-5)
           │
    ┌──────┼──────┬──────┬──────┐
   s1     s2     s3     s4     s5
    │      │      │      │      │
relay_a relay_b relay_c relay_d relay_e
    │      │      │      │      │
    └──────┴──────┴──────┴──────┘
                  │
           members fetch async
           any 3 shares reconstruct
```

bob comes online later, fetches from relay_a, relay_c, relay_d.
reconstructs proposal. no direct connection to alice.

```
network observer sees:
- alice uploaded to some relays (millions do this)
- bob downloaded from some relays (millions do this)
- no connection between alice and bob
- shares are meaningless individually
- can't even tell they're in the same syndicate
```

## networking: adding anonymity layer

for maximum privacy, access relays via Tor or mixnet:

```
alice ──► tor ──► relay_a
              └──► relay_b

bob ──► tor ──► relay_c
            └──► relay_a

observer sees:
- tor traffic to public infrastructure
- indistinguishable from millions of other users
- no timing correlation (async fetch)
- no graph structure
```

## networking: relay selection

relays can be:
- **public IPFS nodes**: anyone can pin shares
- **syndicate-run relays**: members run relays (but access via tor)
- **commercial storage**: S3, cloudflare R2, etc.
- **DHT**: distributed hash table (like bittorrent)

shares are encrypted + meaningless alone. relays learn nothing.
even if all relays collude, they only see encrypted fragments.

## networking: async coordination flow

```
1. alice creates proposal
   - encrypts to syndicate (all members' pubkeys or group key)
   - splits via zoda-vss into n shares

2. alice posts shares to relays
   - each relay gets one share
   - alice can use tor, timing randomization

3. members poll relays periodically
   - "any new shares for syndicate X?"
   - fetch on random schedule, not immediately
   - hides "who's active when"

4. member reconstructs from t shares
   - decrypts proposal
   - creates vote, repeats process

5. once threshold votes collected
   - OSST contributions via same relay model
   - aggregator combines signature
   - transaction submitted to penumbra
```

no member ever learns another member's IP.
no network observer can link members together.
the syndicate is a ghost.

## security: replay protection

relays could replay old messages. defense: bind messages to state.

```rust
SignedProposal {
    syndicate_id: [u8; 32],
    state_hash: [u8; 32],   // "valid for THIS state"
    sequence: u64,          // monotonic counter
    proposal: Proposal,
    signature: Signature,   // proposer signs all above
}
```

recipient checks:
1. `state_hash` matches current state? (reject stale)
2. `sequence > last_seen`? (reject replay)
3. signature valid? (reject forgery)

relay can replay all day. recipients reject.

## security: message addressing

how do members find syndicate messages without revealing membership?

**pseudonymous mailbox**:

```
mailbox_address = hash(syndicate_viewing_key, "narsil-mailbox-v1")
```

- all syndicate members derive same address
- relay sees "mailbox 0x7f3a... has activity"
- relay doesn't know which syndicate or who's in it
- different syndicates → different addresses (unlinkable)

members poll their mailbox. relay learns activity pattern of abstract mailbox, nothing more.

## security: decentralized aggregation

no designated aggregator role. contributions go to relays like everything else.

```
threshold votes reached
         │
members post OSST contributions to relays
         │
    ┌────┴────┬────┬────┐
   c1   c2   c3   c4   c5
    │    │    │    │    │
  relays (same mailbox)
    │    │    │    │    │
    └────┴────┴────┴────┘
         │
any member can:
  - fetch all contributions
  - aggregate locally
  - submit tx to penumbra
```

first valid transaction wins. no single point of trust.

tradeoff: all members see all contributions (internal transparency).
for most syndicates this is fine - goal is hiding from outside, not from each other.

## security: internal anonymity (optional)

default: members see who proposed, who voted, who contributed.
OSST hides this from chain, but syndicate members know.

for full internal anonymity: **ring-vrf**

```
proposal + ring_signature(syndicate_member_pubkeys)
```

verifier learns: "valid syndicate member signed this"
verifier doesn't learn: which member

**use cases for internal anonymity**:
- propose unpopular ideas without social cost
- whistleblowing within syndicate
- prevent retaliation for dissent

ring size = number of members (small). signature size manageable.

**the fully anonymous syndicate**:

| layer | external observer | syndicate members |
|-------|-------------------|-------------------|
| proposer | ❌ unknown | ❌ unknown (ring-vrf) |
| voters | ❌ unknown | ❌ unknown (ring-vrf) |
| signers | ❌ unknown (OSST) | ❌ unknown (OSST) |

nobody knows anything. truly anonymous coordination.

## security: threat model summary

| adversary | capability | defense |
|-----------|------------|---------|
| chain | sees transactions | OSST: normal account, hidden signers |
| relay | sees encrypted shares | encryption + zoda-vss: meaningless fragments |
| relay | replays messages | state_hash + sequence: rejected as stale |
| relay | MITM (replace shares) | signature inside encrypted content |
| network | traffic analysis | tor + timing randomization |
| network | graph structure | relay model: no direct connections |
| member | learn proposer | ring-vrf (optional) |
| member | learn voters | ring-vrf (optional) |

## future directions

**ring-vrf integration**: implement internal anonymity for syndicates
that want even members hidden from each other.

**extension wallets**: store OSST shares in Prax-like extensions.
similar UX to personal keys, but for syndicate participation.

**recursive syndicates**: syndicate members are themselves syndicates.
hierarchical governance structures.

**cross-chain**: IBC-connected syndicates across chains.
unified governance, distributed assets.

**stealth mailboxes**: derive fresh address per message for even
stronger relay privacy. recipients scan with viewing key.

## summary

narsil = OSST + zoda-vss + penumbra keys + relays + simple state

```
                          ┌─────────────┐
                          │   chain     │
                          │  (sees      │
                          │  nothing)   │
                          └──────┬──────┘
                                 │
                          valid signature
                          normal account
                                 │
┌────────────────────────────────┴────────────────────────────────┐
│                         narsil syndicate                         │
│                                                                  │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                       │
│  │  alice   │  │   bob    │  │  carol   │   ◄── members         │
│  │30 shares │  │30 shares │  │40 shares │       (never connect  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘        directly)      │
│       │             │             │                              │
│       │             │             │                              │
│       ▼             ▼             ▼                              │
│   ┌───────┐     ┌───────┐     ┌───────┐                         │
│   │relay_a│     │relay_b│     │relay_c│  ◄── public relays      │
│   └───┬───┘     └───┬───┘     └───┬───┘      (ipfs, dht, etc)   │
│       │             │             │                              │
│       └─────────────┼─────────────┘                              │
│                     │                                            │
│              zoda-vss shares                                     │
│              (async, redundant)                                  │
│                     │                                            │
│              reconstruct + decrypt                               │
│                     │                                            │
│              OSST threshold sign                                 │
│              (hidden participation)                              │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

**chain learns**: valid signature exists, normal account transacted.

**network observer learns**: some people accessed public relays.

**nobody learns**: who's in the syndicate, who proposed, who voted,
who signed, internal power dynamics, dissent patterns.

privacy isn't just for individuals. groups need it too.

---

*"the sword that was broken shall be reforged"*
