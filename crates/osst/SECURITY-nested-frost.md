# Security note: nested FROST ("frostito") — outer binding gap

Status: **v1 construction believed vulnerable; v2 implemented, needs external review**
Date: 2026-08-05
Scope: `osst::nested`, `frost-spend::hierarchical`, `poker-server::jury`

---

## 1. What the construction does

An inner threshold group (t_in of n_in) collectively holds ONE position in an
outer FROST scheme (t_out of n_out). The outer share never exists as a single
scalar; it is created via interleaved DKG and used via nested signing.

Deployed instances:

| Component | Uses nested FROST? | Holds value? |
|---|---|---|
| `poker-escrow` payouts | **No** — plain FROST via `frost_spend::orchestrate` → ZF `reddsa` | Yes (mainnet) |
| `frost-spend::hierarchical` (bridge custody) | **Yes** | Yes |
| `poker-server::jury` (`LocalJury`) | Yes | No — demo, all shares in one process |

The poker money path does **not** depend on this construction. The bridge does.

## 2. The vulnerability (v1)

### 2.1 Root cause

FROST's binding factor couples every signer's *effective* nonce to the full
commitment set:

```
effective nonce_i = d_i + ρ_i·e_i        ρ_i = H(i, m, B)
```

Change any commitment in `B` and every honest signer's effective nonce moves.
An adversary therefore cannot hold an honest nonce fixed while varying the
challenge. This coupling is what makes FROST ROS-resistant.

The v1 nested position breaks it. Its binding factor omits the outer context:

```
ρ_inner_k = H("frostito-inner-bind", k, m, inner_commitments)   ← no outer set
R_nested  = Σ_k (D_k + ρ_inner_k·E_k)
```

and it is handed to the outer protocol as `hiding = R_nested, binding = identity`,
so the outer binding factor multiplies the identity and vanishes:

```
R_outer = (D₁ + ρ₁·E₁) + R_nested
```

**The nested position's effective nonce is determined entirely by the inner
round and cannot be moved by anything the outer adversary does.** From the
outer adversary's viewpoint the position behaves like a pre-FROST two-round
Schnorr threshold scheme with no nonce binding.

The v1 source states this as an intentional property ("the inner binding factors
provide the equivalent security at the inner level"). That equivalence does not
hold: inner binding constrains inner holders relative to each other, it does not
constrain the *outer* adversary at all.

### 2.2 Attack A — ROS forgery (no nonce reuse required)

Adversary: any legitimate outer signer (in poker, a player).

1. Open ℓ concurrent signing sessions; collect R_nested,₁…R_nested,ℓ. Each is
   fixed for the life of its session and cannot be influenced afterwards.
2. Per session, grind own (D₁ⱼ, E₁ⱼ) to sample many R_outer,ⱼ and hence many
   candidate challenges cⱼ = H(R_outer,ⱼ, Y, mⱼ), with the jury's contribution
   pinned throughout.
3. ℓ fixed honest nonces + adversarially selectable challenges is the ROS
   problem. Benhamouda–Lepoint–Loss–Orrù–Raykova (EUROCRYPT 2021, ePrint
   2020/945) solve it in polynomial time for ℓ > log₂(q) ≈ 256.
4. Forge a nested-position signature on a message the group never authorised.

Precondition: ~256 concurrently open sessions. Nothing in v1 bounded this.

### 2.3 Attack B — two-session key extraction (requires nonce reuse)

ρ_inner does not depend on the outer challenge, so two outer sessions over the
same message with the same inner commitment round yield identical nonces and
identical ρ_inner but different challenges c₁ ≠ c₂:

```
z_k¹ − z_k² = λ·μ_k·(c₁ − c₂)·σ_k    ⇒    σ_k recovered directly
```

`inner_sign` consumes nonces by value, which prevents this within a process. It
does **not** survive a process boundary: a node that persists and restores
signing state can be replayed.

### 2.4 Attack C — unattributed invalid shares

`aggregate_inner_shares` sums responses without verifying any of them. One
faulty or malicious holder yields a signature that fails to verify with no
indication of which holder caused it. Availability and accountability only.

## 3. Confidence

The structural claim in §2.1 — ρ_inner omits the outer context, and the identity
binding commitment nullifies outer binding for that position — is directly
verifiable from the source and is not in doubt.

The conclusion that this admits the standard ROS attack follows a well
established pattern for unbound two-round Schnorr threshold schemes, but has not
been established here by a formal reduction. It requires confirmation by a
cryptographer. Absence of further findings in this note is not evidence of
their absence: this was code review, not cryptanalysis.

## 4. v2 — the fix

Implemented in `osst::nested` alongside v1 (v1 retained, marked insecure, so
existing deployments compile while they migrate).

**Present a commitment PAIR, not a pre-bound point.**

```
D_nested = Σ_k D_k        E_nested = Σ_k E_k
```

The nested position enters the outer protocol as an ordinary signer. The outer
binding factor ρ = H(index, m, B) is computed over the full outer commitment set
exactly as for anyone else, and each inner holder signs with that same ρ:

```
z_k = d_k + ρ·e_k + (λ_out·c·μ_k)·σ_k
```

Summing over the quorum, with d = Σd_k, e = Σe_k, and Σ μ_k·σ_k = σ_out:

```
z_nested = d + ρ·e + λ_out·c·σ_out
```

which is exactly what a single FROST signer holding σ_out with nonces (d, e)
produces.

### 4.1 Why this is easier to trust

The nested position becomes **indistinguishable from a flat FROST signer** whose
nonce and key happen to be additively shared. Security therefore reduces to
FROST's existing proof plus inner-group honesty, instead of requiring a novel
composition argument.

`nested_v2_equals_flat_frost` asserts this on real values: it builds an outer
2-of-2, splits position 2's key 3-of-5, and checks the nested signature share is
bit-for-bit equal to the flat share and that the assembled signature verifies.
A reviewer can check that property in minutes.

### 4.2 Inner adaptive selection

Removing ρ_inner reopens the concern it addressed, so v2 adds an explicit
commit–reveal round: every holder publishes H(k ‖ D_k ‖ E_k) before any
commitment is revealed (`inner_precommit` / `verify_inner_precommit`). No holder
sees another's commitment before fixing its own.

### 4.3 Share verification

`verify_inner_share` checks

```
z_k·G  ==  (D_k + ρ·E_k) + (λ_out·c·μ_k)·P_k
```

and `aggregate_inner_shares_verified` returns the indices of holders whose
shares failed, so a faulty node is evicted rather than silently corrupting the
signature. Covered by `v2_names_the_dishonest_inner_holder`.

### 4.4 Independent verification of outer params

`SigningPackage::{binding_factor, group_commitment, challenge}` are now public.
v1's documentation claimed inner holders could independently verify the
coordinator's parameters; they were private, so this was impossible. Holders can
now recompute the outer context from public data instead of trusting the
coordinator.

## 5. Deployed mitigations (v1 sites, pending migration)

- **Concurrency bound.** `poker-server::jury` caps concurrent signing sessions at
  `MAX_CONCURRENT_JURY_SESSIONS = 4`, held for the whole session, refusing rather
  than queueing. ROS needs ℓ > ~256; 4 is far below. Mitigation, not a fix —
  raise only after migrating to v2.
- Nonce pairs remain consume-by-value and zeroize on drop.

Still outstanding:

- **Durable one-time nonce enforcement** for any holder that persists state
  across restarts (closes §2.3 independently of v2).
- **Bridge migration** — `frost-spend::hierarchical` still uses v1 and is the
  only value-bearing v1 consumer.
- **`LocalJury` holds all five shares in one process**, so the 3-of-5 provides no
  real distribution today. Do not distribute jury nodes onto v1.

## 6. Recommended sequence

1. ~~Bound concurrency~~ (done).
2. Migrate `frost-spend::hierarchical` to v2, or cap what the bridge custodies
   until it is migrated.
3. Obtain external review of the §4.1 equivalence claim. It is a narrow,
   well-posed question.
4. Only then consider a genuinely distributed jury.

## 7. Alternative worth weighing

The nesting exists solely to obtain one aggregate signature for one Orchard
address. The same policy is expressible with no novel cryptography: run the jury
as a plain t-of-n FROST group signing an *authorization* over the settlement
payload, verified in software as a precondition to a standard 2-of-2.

Honest trade-off: this moves the jury requirement from a cryptographic guarantee
to a software check, so a compromised escrow could bypass it — whereas today it
is enforced by the signature itself. Weigh "escrow compromise" against
"unreviewed novel threshold cryptography". A middle path is flat 2-of-3 FROST
with the jury position in a TEE: standard primitives throughout, at the cost of
jury distribution.

## References

- Benhamouda, Lepoint, Loss, Orrù, Raykova. *On the (in)security of ROS.*
  EUROCRYPT 2021. ePrint 2020/945.
- Drijvers et al. *On the Security of Two-Round Multi-Signatures.* IEEE S&P 2019.
- Komlo, Goldberg. *FROST: Flexible Round-Optimized Schnorr Threshold Signatures.*
  SAC 2020.
