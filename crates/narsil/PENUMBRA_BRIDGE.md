# Penumbra as Narsil: Zcash Custody via Validator Set

## Thesis

Penumbra's validator set can serve as a trustless Zcash custody network,
enabling pZEC (shielded ZEC on Penumbra) without introducing a new trust
assumption. The same stake that secures Penumbra consensus secures the
ZEC bridge. No separate bridge operator, no federation, no new token.

## The Problem

Bridging ZEC to Penumbra currently requires trusting a bridge operator
(federation, multisig committee, or centralized custodian). This introduces
a new trust assumption beyond Penumbra's own consensus.

## The Solution: OSST + FROST Split

Separate **authorization** (who approves) from **execution** (who signs):

### Authorization: OSST (2/3 stake-weighted)

All validators participate in a one-step threshold identification round.
Each validator submits a single Schnorr proof weighted by their stake.
Once 2/3 of stake has voted, the action is authorized.

- O(n) communication — each validator submits independently
- Non-interactive — no pairwise messages
- Asynchronous — validators submit at their own pace
- Scales to 100+ validators trivially

### Execution: FROST (top 1/3 validators)

The highest-staked validators form a small FROST signing committee.
They hold shares of the Zcash spending key (RedPallas on Pallas curve).
They can only sign when presented with a valid OSST authorization proof.

- O(k²) communication where k = committee size (~10-20)
- 2-round interactive protocol (fast, ~seconds)
- Produces a standard RedPallas signature (valid Orchard spend)
- Committee rotates via reshare when validator set changes

### Security Model

To steal funds, an attacker needs BOTH:
1. Corrupt all FROST executors (top 1/3 by stake)
2. Forge an OSST supermajority (2/3 of remaining stake)

This is strictly harder than compromising either set alone.

If FROST executors refuse a valid OSST warrant → slashable (the OSST
proof is public evidence of authorization).

If FROST executors sign without OSST → invalid (the Zcash transaction
embeds the OSST proof hash, verifiers reject without it).

## Architecture

```
Zcash                          Penumbra
─────                          ────────

ZEC deposits ──→ Custody       Validators
  (Orchard)      Address  ←── ├── OSST authorization (all)
                 (FROST)  ←── ├── FROST execution (top)
                               ├── zidecar verification (all)
                               └── consensus (existing)

                               pZEC minted ──→ DEX, swaps,
                                               shielded pool,
                                               poker escrow
```

### Deposit Flow (ZEC → pZEC)

1. User sends ZEC to the FROST custody address (standard Orchard transfer)
2. zidecar proves the Zcash block containing the deposit
3. Penumbra validators verify the ligerito proof (no full Zcash node)
4. Once confirmed: pZEC minted to user's Penumbra address
5. No OSST/FROST round needed for minting (proof-verified deposit)

### Redemption Flow (pZEC → ZEC)

1. User burns pZEC on Penumbra (creates a redemption action)
2. OSST authorization round: 2/3 stake approves the redemption
3. FROST execution: top validators sign the Zcash Orchard spend
4. ZEC sent to user's Zcash address
5. Transaction finality: Zcash confirmation + Penumbra finality

### Key Rotation (Validator Set Changes)

When Penumbra's validator set changes (epoch boundary):
1. New FROST committee selected from top validators
2. Reshare protocol transfers custody key to new committee
3. OSST set updates automatically (stake-weighted, no key material)
4. Custody address doesn't change (reshare preserves group key)

## Why This Works

### For Penumbra
- ZEC liquidity on the DEX without trusting an external bridge
- Revenue from bridge fees (similar to IBC relayer fees)
- Demonstrates Penumbra as a general-purpose privacy infrastructure

### For Zcash
- Access to Penumbra's DEX, shielded swaps, delegator rewards
- Bridge security backed by Penumbra's full stake (not a small federation)
- zidecar verification means Penumbra validates Zcash properly

### For Poker (zk.poker)
- pZEC as the native poker chip
- Escrow custody backed by the full validator set
- Dispute resolution (jury = validators) with the same stake

## Implementation Path

### Phase 1: Proof Verification
- Integrate zidecar into Penumbra client
- Validators verify Zcash header proofs as part of block processing
- No custody yet — just proof of concept that Penumbra can track Zcash state

### Phase 2: OSST Module
- Add OSST authorization as a Penumbra component
- Validators submit threshold proofs as part of consensus
- Generic — not just for Zcash, any threshold authorization

### Phase 3: FROST Custody
- DKG among top validators for Zcash spending key
- RedPallas signing via frost-rerandomized
- Reshare protocol on epoch boundaries

### Phase 4: pZEC Token
- Mint/burn module in Penumbra
- Deposit verification via zidecar proofs
- Redemption via OSST + FROST

## Existing Code

| Crate | Role | Status |
|-------|------|--------|
| `zidecar` | Zcash header chain prover (ligerito) | production (crates.io) |
| `osst` | One-step threshold identification | implemented, tested |
| `osst/redpallas.rs` | RedPallas FROST for Orchard | implemented |
| `osst/reshare.rs` | Proactive key rotation | implemented |
| `narsil` | Syndicate consensus framework | implemented (189 tests) |
| `ligerito` | Polynomial commitment proofs | production (crates.io) |
| `zcli/frost.rs` | Generic FROST multisig for wallets | implemented |

## Comparison with Existing Bridges

| Approach | Trust | Validators | Scalability |
|----------|-------|------------|-------------|
| Federation (WBTC) | Small committee | 5-15 | Fast but fragile |
| MPC bridge (Thorchain) | Rotating set | 50-100 | O(n²) per sign |
| Optimistic (Arbitrum) | Fraud proofs | Any | 7-day delay |
| **OSST+FROST (this)** | **Penumbra stake** | **All + top subset** | **O(n) auth + O(k²) exec** |

The key advantage: no new trust assumption. The bridge is as secure as
Penumbra consensus itself.
