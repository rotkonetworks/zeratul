# Narsil Poker Arbitration

## Overview

zk.poker is a peer-to-peer poker game where neither player nor server can
cheat. every hand begins with a mental poker shuffle — a ZK protocol where
both players contribute randomness to the deck. neither can stack it.

every step of the game produces a co-signed action log: you sign my action,
i sign yours. by the end of the hand both players hold an identical, mutually
signed transcript that neither can deny or fabricate.

## Three Courts

dispute resolution has three layers. each one is more expensive, more public,
and harder to corrupt than the last. the system is designed so that almost
every game settles at layer 1 and the higher courts exist primarily as
deterrents.

### Layer 1: Players (happy path)

both players sign the settlement and funds move. no jury, no chain, no
public record. this is how 99%+ of hands resolve.

### Layer 2: Narsil Jury

if players disagree, either submits the co-signed action log to narsil.
the jury replays the hand through a deterministic game engine — same inputs
always produce the same output — and signs the correct settlement.

the player who disagrees with the verdict can't block settlement — only
2-of-3 FROST signatures are needed, so the winning player + jury settle
without the loser's cooperation.

the jury can't lie about the outcome because the other player holds the
full co-signed transcript as fraud proof.

### Layer 3: On-Chain High Court (PolkaVM / JAM CoreVM)

if the jury is corrupt — bribed, colluding, or simply wrong — the victim
escalates to a PolkaVM contract or JAM CoreVM service that replays the
entire poker hand on-chain.

the victim submits:
1. the co-signed action log (self-authenticating — both players signed it)
2. the jury's signed incorrect verdict

the on-chain program:
1. replays the deterministic game engine (same Rust code compiled to PVM)
2. computes the correct settlement
3. compares with the jury's verdict
4. if they differ: slashes the jury, corrects the settlement

this court is permissionless — no one can censor the appeal. it is
deterministic — no judgment call, just math. and it is final.

### Information Leakage as Penalty

escalating to the high court requires publishing the co-signed action log
on-chain. this is a permanent public record of:
- how both players played their hands
- their betting patterns and tells
- their decision-making under pressure

for serious poker players, this information leakage is a reputational cost.
opponents can study your history, exploit your patterns, and profile your
strategy. your play style, bluffing frequency, tilt behavior — all
permanently indexed on-chain. this reputational damage compounds with the
financial penalty and cannot be undone:

```
layer 1 (happy path):  no cost, no leakage
layer 2 (jury):        loser's jury deposit forfeit, log visible to jury only
layer 3 (high court):  action log permanently public on-chain
  → jury was wrong:    jury slashed, appellant made whole
  → jury was right:    appellant loses deposit + privacy for nothing
```

each escalation is more expensive AND more transparent. this gradient
keeps disputes at the lowest possible level. frivolous appeals to the
high court are doubly irrational — you burn your poker history forever
and gain nothing if the jury was correct.

## Escrow Structure

each table creates a 2-of-3 FROST escrow:

```
share 1 = player A
share 2 = player B
share 3 = narsil jury (threshold-signed internally via OSST)
```

any two shares can sign a settlement:
- A + B = happy path (players agree)
- A + jury = dispute (jury sides with A)
- B + jury = dispute (jury sides with B)

the jury's single FROST share is itself backed by the narsil syndicate's
OSST threshold — the jury panel reaches internal consensus before producing
their share contribution. this means corrupting "the jury" requires
corrupting a majority of the syndicate, not a single node. a production
syndicate might have dozens or hundreds of staked nodes, making collusion
proportionally harder as the network grows.

## Staking

to keep narsil honest, each jury node stakes into a FROST address controlled
by the other nodes. your stake is held by your peers. if you get slashed the
honest majority signs a tx moving your stake to the victim.

narsil custodies its own stakes using the same OSST shares it uses for
consensus. the same cryptographic infrastructure that signs settlements
also enforces penalties.

```
Tier      Max Pot    Jury Stake   Jury Deposit
────────  ─────────  ───────────  ────────────
Micro     0.1 ZEC    0.1 ZEC      0.001 ZEC
Regular   1 ZEC      1 ZEC        0.005 ZEC
High      10 ZEC     10 ZEC       0.02 ZEC
Whale     100 ZEC    100 ZEC      0.10 ZEC
```

## Economics

### Rake: 0.4% of every pot

```
0.4% rake
  ├── 10% → protocol treasury (0.04% effective)
  └── 90% → narsil jury pool (0.36% effective)
```

### Jury earnings

- **base fee** (passive): share of 90% rake split, proportional to stake.
  earned just for being online and available.
- **dispute fee** (active): loser's jury deposit when a dispute occurs.
- **slashing**: corrupt juror's stake transferred to victim via high court.

### Dispute disincentive

both players lock jury_deposit at table creation. refunded on happy path.
on dispute the loser forfeits theirs. on appeal to high court the loser
forfeits AND their action log becomes public.

## Flow

```
1. player A creates table → narsil assigns jury panel
2. DKG: A + B + narsil → shared escrow address (2-of-3 FROST)
3. both deposit buy_in + jury_deposit to escrow
4. play P2P with mental poker (ZK shuffle, co-signed actions)
5a. happy path: A + B sign settlement → broadcast → deposits refunded
5b. dispute: player submits co-signed log → jury replays → jury + player sign
5c. appeal: victim publishes log + wrong verdict on-chain → PVM replays →
    jury wrong? slashed + correct settlement. jury right? appellant loses deposit.
6. timeout: pre-signed refund tx unlocks after N blocks, returns each
   player's original deposit (buy_in + jury_deposit) to their own address
```

## Implementation

### narsil crate

| Module | Role | Status |
|--------|------|--------|
| `syndicate.rs` | jury pool governance, 100-share model | implemented |
| `coordinator.rs` | DKG + signing ceremony coordination | implemented |
| `ceremony.rs` | signing ceremonies | implemented |
| `bft.rs` | jury verdict consensus | implemented |
| `relay.rs` | pseudonymous relay coordination | implemented |
| `reshare.rs` | jury rotation without changing escrow | implemented |
| `governance.rs` | fee/tier parameter changes | implemented |
| `scanner.rs` | deposit verification via chain scanning | implemented |
| `wallet.rs` | transaction building | implemented |
| `election.rs` | NPoS jury panel selection | implemented |
| `staking.rs` | exchange-rate staking model | implemented |
| `ballot.rs` | secret ballot for governance votes | implemented |
| `poker.rs` | dispute replay engine, verdict, liar detection | implemented |
| `escrow.rs` | escrow lifecycle with state channel tracking | implemented |
| `tiers.rs` | tier definitions, rake computation | implemented |

### poker-p2p crate

| Module | Role | Status |
|--------|------|--------|
| `engine.rs` | deterministic game engine (23 tests) | implemented |
| `protocol.rs` | P2P messages + `CoSignedAction` + `HandTranscript` | implemented |
| `table.rs` | table management | implemented |
| `rendezvous.rs` | PAKE-authenticated peer discovery | implemented |
| `webrtc.rs` | WebRTC data channel for low-latency | implemented |

### state-channel crate

| Module | Role | Status |
|--------|------|--------|
| `channel.rs` | on-chain channel state (nonce, hash, dispute timeout) | implemented |
| `state.rs` | SCALE-encoded `PokerState` with hash for signing | implemented |
| `transition.rs` | deterministic state transitions (all actions) | implemented |
| `dispute.rs` | dispute types + `process_dispute` for on-chain verification | implemented |
| `types.rs` | `SignedState<T>`, `Participant`, `LigeritoProof` | implemented |

### supporting crates

| Crate | Role | Status |
|-------|------|--------|
| `osst` | threshold signing (FROST + DKG + resharing) | implemented |
| `zk-shuffle` | mental poker shuffle proofs (Chaum-Pedersen, minimal verifier) | implemented |
| `wim` | PolkaVM execution proofs via ligerito (Rescue-Prime hash, batched Schwartz-Zippel) | implemented |
| `ligerito-escrow` | 2-of-3 ZODA escrow (Reed-Solomon VSS) | implemented |
| `poker-sdk` | key hierarchy (per-table seeds, per-hand card keys) | implemented |

### high court (`zeratul-circuit` + PVM)

the on-chain adjudicator lives in `zeratul-circuit::poker` and compiles
to PolkaVM (via polkaports) or runs as a JAM CoreVM service.

the poker game engine is pure Rust — no async, no allocator pressure,
no OS deps. it compiles cleanly to PVM via polkaports' std-capable
RISC-V target (`riscv64emac-polkavm-linux-musl`).

for the privacy-preserving path, `wim` can generate a proof that the
PVM executed the game engine correctly on committed inputs without
revealing the inputs themselves. the jury verifies the WIM proof
instead of seeing the raw action log.

```rust
// PVM entrypoint (zeratul-circuit::poker)
fn adjudicate(
    action_log: &[CoSignedAction],
    player_keys: &[[u8; 32]; 2],
    jury_verdict: &SignedJuryVerdict,
) -> AdjudicationResult {
    // 1. verify every action is co-signed by both players
    for action in action_log {
        verify_both_signatures(action, player_keys);
    }
    // 2. verify the jury actually signed this verdict
    verify_jury_signature(&jury_verdict);
    // 3. replay the deterministic engine
    let correct = replay_deterministic(action_log);
    // 4. compare
    if correct.payouts == jury_verdict.payouts {
        AdjudicationResult::VerdictCorrect
    } else {
        AdjudicationResult::JurySlashed {
            correct_payouts: correct.payouts,
        }
    }
}
```

## Design Rationale

the three-court structure mirrors how real legal systems work:

- most disputes settle privately (layer 1)
- a trusted mediator handles the rest (layer 2 — narsil)
- a slow, expensive, but incorruptible court of last resort exists for
  when the mediator fails (layer 3 — on-chain PVM)

the key insight is that the co-signed action log serves triple duty:
1. it IS the game record (both players attested to every action)
2. it IS the dispute evidence (self-authenticating, no third party needed)
3. it IS the fraud proof (if jury lies, publishing the log proves it)

almost nothing needs to leave a permanent on-chain record for the system
to work. but it could — and that possibility is what keeps everyone honest.

from the player's perspective all of this is automatic. the client co-signs
actions, constructs settlements, and handles disputes under the hood. a
player would have to deliberately modify their client to cause trouble —
and the system makes that irrational at every level. you'd be fighting
against your own software to lose money and leak your own game history.
