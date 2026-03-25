# Bot Design

## Architecture

The bot is a **private, standalone binary** that connects to the poker relay as a regular player. It is never shipped with the server. The server is a pure relay — no game logic, no bot, no engine state.

```
poker-server (shipped)          = relay + lobby + matchmaking (dumb pipe)
poker-p2p (shipped)             = game engine in each player's client
cfr-bot (private, never shipped) = headless client with Brain, connects via WS
```

## Components

```
cfr-bot
├── lobby monitor    — watches /ws/lobby, observes table listings
├── join scheduler   — human-like delay (30s-5min), picks tables naturally
├── game client      — connects as peer, plays via game_session protocol
├── chat agent       — game-related reactions only
└── personality      — timing jitter, bet sizing variance, session limits
```

## Decision Engine (Brain)

Five composable layers, any can be toggled independently:

```
L0: Blueprint   — MCCFR Nash equilibrium table (the floor)
L1: Search      — depth-limited real-time CFR refinement (~50ms)
L2: Range       — Bayesian opponent hand tracking
L3: Exploit     — player profiling (VPIP/PFR/AF) + counter-strategy
L4: Neural      — CTM-MoE value/policy evaluation (<1ms)
```

Composition follows Eriksen's "Your Server as a Function" filter pattern:

```rust
Brain::new(&strategy)
    .with_filters(
        FilterStack::new()
            .and_then(SearchFilter)
            .and_then(RangeFilter)
            .and_then(ExploitFilter)
            .and_then(NeuralFilter { moe, weight: 0.3 })
    )
```

## Bet Sizing

9-action space matching the frontend buttons exactly:

```
fold | check | call | 1/4 pot | 1/2 pot | 3/4 pot | pot | 2x pot | all-in
```

Same sizing for humans and bot. No UI tells.

## Behavioral Camouflage

**Timing:**
- Easy decisions (fold trash): 1-3s
- Standard decisions: 3-8s
- Hard decisions: 8-15s
- Occasional tank: 15-30s
- Never instant

**Session behavior:**
- Watches lobby before joining (30s-5min browse time)
- Plays 30-90 minute sessions, then leaves
- Doesn't grind 24/7
- Uses zafu keypair, signs actions like any player

**Chat — less is more:**
- Most messages: no response (silence is the most human behavior)
- Occasional game reactions: "nh", "wp", "gg", "lol", "wow", "sick hand", "..."
- Never explains strategy
- Never justifies a play
- Never responds to tilt bait
- Never answers all queries — real players ignore half the chat
- Game-related commentary only, never random conversation

## Training Pipeline

```
self-play (Rust, search mode on EPYC) → training samples
  → train CTM-MoE experts (Python/PyTorch)
  → export ONNX
  → measure exploitability
  → repeat
```

6 experts: preflop_multi, postflop_wet, postflop_dry, shortstack, river_polar, headsup
Router picks top-2 experts per decision, blends outputs.

## Anti-Bot Principles

Since we take anti-bot measures on the platform, the bot must:
1. Never have special server access — connects via same WebSocket as humans
2. Never see information a human couldn't see
3. Obey the same protocol, same identity system, same co-signed actions
4. Be indistinguishable from a human player at the network level
