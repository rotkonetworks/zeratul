# Escrow primitives consolidation — kill the "double trouble"

## The problem
Every ironwood/orchard wallet primitive exists **twice**: once in the escrow
(`crates/poker-escrow`, on a fork stack) and once in the maintained wallet stack
(`zcli`/`zafu-wasm`, on released orchard). The two copies drift, so every fix — the
whole 2026-08-06 bug cascade (scanner stub, orchard-only witness, activation floor,
"which orchard") — has to be found and fixed twice, and the escrow's copy is always
the stale one.

Proven this session: the fork `OrchardDomain` decrypts released-orchard `IronwoodDomain`
notes (same KDF, `b"Zcash_Orchardock"`). So the fork buys **nothing** but maintenance
burden. The escrow can and should run the same released-orchard code the wallets do.

## Target: one primitives crate, two consumers

```
        ┌──────────────────────────────┐
        │  ironwood-primitives (crate)  │   ← single source of truth, released orchard 0.15.5
        │  - Orchard/Ironwood note enc/dec (IronwoodDomain)
        │  - compact-block decode + trial-decryption
        │  - witness building — POOL-AWARE (WitnessReplay)   ← the thing the escrow lacks
        │  - tx build: shield / send / change (V6 ironwood)
        │  - tree / frontier / anchor ops
        └───────────────┬───────────────┴───────────────┐
                        │                               │
                 zafu (wallet)                    poker-escrow
                 (already ~uses it)         FROST custody + settlement ONLY
```

Candidate crate: extract/reuse `zcli-ironwood/crates/zync-core` (+ `zafu-wasm`'s witness/
tx-build cores). Whatever it is, it depends on **released** orchard, and BOTH the wallet
and the escrow depend on it.

## The boundary — what the escrow keeps (everything else is a primitive)
- FROST 2-of-3 keygen / DKG / spend-auth cosign **orchestration** (`frost-spend`, `frost_dkg`, `payout_signing`)
- room + deposit + settlement **state machine** (`main.rs` room map, deposit gating)
- payout **planning** + settlement-signature verification (`compute_settlement_outputs`, `/settle`)
- journal / dispute / accounting / ntfy-PIN

Rule of thumb: anything of the form *"given keys, scan/witness/build/encrypt an ironwood tx"*
is a primitive and leaves the escrow.

## Legacy reliance to REMOVE (explicit)
- `zecli = { git = zcli rev f497a277 }` — the fork light-client. Replaced by the shared crate.
- `orchard 0.14` fork (`zcash/orchard` branch `adam/qleak-dummy-ciphertexts-on-pr505`, rev 204d8ce)
  — replaced by released `orchard 0.15.5` (has `IronwoodDomain`, what wallets/chain use).
- `[patch."https://github.com/valargroup/librustzcash"]` block + the whole fork `[patch]` set.
- `core2` vendoring / the yanked-0.3.x `--locked` workaround — goes away with the fork stack.
- `.cargo/config.toml` `--cfg zcash_unstable="nu6.3"` — released crates ship NU6.3 **ungated**,
  so the cfg (and the `unexpected_cfgs` lint entry) is deleted.
- **osst legacy shim**: `make_legacy_osst`, `LegacyOsstShim`, `derive_escrow_ua` (now dead —
  its only caller was replaced by `derive_trusted_dealer_keys`), and the `player_a/b_share`
  osst path. Marked "will be removed in Phase 2.4" in the code — remove.
- The escrow's duplicate primitives: `scanner.rs` (scan/decrypt), `tx_build.rs` (witness+build),
  `zecli::witness` reliance — become thin call-throughs to the shared crate, or deleted.

## Migration order (do deliberately, not in one rush)
1. Pick / carve the shared primitives crate (released orchard). Publish its API surface.
2. Move **scanning** first (lowest risk; deposit detection just proven). Escrow's `scanner.rs` → wrapper.
3. Move **witness + tx build** (this is Layer 5 done right — pool-aware ironwood witness comes for free).
4. Move **note enc/dec** onto released `IronwoodDomain`.
5. Drop the fork `[patch]` set, core2 vendor, and the `nu6.3` cfg; escrow now on released orchard.
6. Delete the osst legacy shim + dead `derive_escrow_ua`.
7. Re-run the local regtest cycle (deposit → settle → V6 payout broadcast) to confirm parity.

## Why this IS Layer 5
Layer 5 (settlement → V6 payout **broadcast**) was blocked because the escrow's witness builder
is orchard-only. Rather than build a *third* ironwood witness builder, step 3 above adopts the
pool-aware one that already exists. Consolidation and "finish the payout" are the same work.
