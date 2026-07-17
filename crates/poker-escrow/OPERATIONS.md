# poker-escrow — operator runbook

The escrow custodies real Zcash via FROST (2-of-3, Orchard). It is an **internal** service:
bound to `127.0.0.1:3034` and firewalled (`iptables` DROP on `:3034` except loopback). Reach its
HTTP endpoints from the box, or over an SSH tunnel:

```
ssh -L 3034:127.0.0.1:3034 root@zkbtc.org
# then http://localhost:3034/status  /accounting  /disputes
```

## At-a-glance health

```
curl -s localhost:3034/status | jq        # config + readiness + live room counts
curl -s localhost:3034/accounting | jq    # house revenue, volume, games settled, deposits
```

`GET /status` reports `ready_for_real_money` and a `go_live_blockers[]` list. It's real-money-ready
when: `network:Main`, `verify_deposits:true`, `use_dkg:true`, `house_payable:true`,
`journal_enabled:true`, **and `persistence_enabled:true`**.

## Go-live checklist

1. **Real house wallet** — `HOUSE_ADDRESS` = a mainnet Orchard `u1…` you control (cold wallet ideal).
   The escrow decode-validates it at boot ("house address VALIDATED"). It only *receives* rake; no
   funding needed. `rake_bps` is per-table (set at table creation) — use 0 for the first clean cycles.
2. **Persistence (ENCRYPTED) — the last blocker.** Without it, an escrow restart strands in-flight
   deposits (FROST shares aren't on disk). Enable:
   ```
   openssl rand -hex 32                    # 32-byte key — BACK THIS UP OFFLINE.
   # ^ losing this key makes all persisted (encrypted) state UNRECOVERABLE.
   # add to /etc/systemd/system/poker-escrow.service.d/*.conf:
   #   Environment=ESCROW_PERSIST_KEY=<that hex>
   #   Environment=ESCROW_STATE_DIR=/opt/poker-escrow/state
   systemctl daemon-reload && systemctl restart poker-escrow
   ```
   Shares are then XChaCha20-Poly1305 sealed at rest. `/status` → `persistence_enabled:true`.
3. **ntfy alerts (recommended)** — set `ESCROW_NTFY_URL` (+ `ESCROW_NTFY_TOKEN` if the server needs
   it). You then get pushed on: payout broadcast (✅ txid+amount), payout failed, DKG failed, client
   fault, and 0-conf shortfall. Unset = alerts no-op.
4. **Arbiter PIN (for disputes)** — set `ESCROW_ARBITER_PIN_HASH` (argon2). Without it the dispute
   dashboard is read-only (no pay/refund actions). Per-IP 3-try lockout.

## Money-safety invariants (already enforced)

- Settlement/rake/payout move **CONFIRMED** value only. A hand may DEAL on 0-conf (both buy-ins seen
  in mempool), but every money-out path (`/settle`, `/arbitrate` pay_a/pay_b, `initiate_payout`)
  refuses unless both deposits are confirmed on-chain.
- A fast hand that ends before its block confirms is **queued** (`settle_pending`) and auto-completes
  when confirmations land — funds are never stranded, never paid early.
- If a mempool buy-in is **evicted** while its seat is short (0-conf double-spend signature),
  `evicted_shortfall` is set, auto-payout is blocked, and you're alerted — resolve via cancel/arbitrate.

## Responding to alerts

- **payout failed** → check `journal/events.jsonl` for the room's `payout_failed` reason; the vault
  still holds the funds. Retry settlement once the cause is fixed, or `/arbitrate` (PIN).
- **0-conf shortfall** → a buy-in never confirmed. Don't force the winner's payout; `/cancel`
  refunds each seat its own confirmed deposit, or `/arbitrate` refund.
- **DKG failed** → the room is terminally unusable; no funds should have been deposited. Tell players
  to start a fresh table.

## Audit / accounting

- `GET /audit/{code}` — full chronological money trail for one room (deposits w/ txids, settlement
  with BOTH player co-signatures, payout txid). This is the non-repudiable dispute evidence.
- `GET /accounting` — totals: `rake_collected` (house revenue), `volume`, `games_settled`, deposits,
  payouts, disputes. Watch `rake_collected` grow as real games settle.
- `journal/events.jsonl` — the durable append-only source (survives restart).

## Recovery

- Persisted room files live in `ESCROW_STATE_DIR` (0600, dir 0700), one per room, encrypted. On
  restart the escrow loads them and resumes deposit polling / queued settlements. A corrupt/unreadable
  file is skipped and logged (manual review) so one bad file can't block the rest.
