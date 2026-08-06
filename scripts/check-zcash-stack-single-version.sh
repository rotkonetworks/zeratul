#!/usr/bin/env bash
# Guard: the Zcash cryptographic stack must resolve to exactly ONE version per crate.
#
# Why this exists. Before the de-forking migration we carried patched forks of
# orchard/zcash_primitives alongside the released crates, so `cargo tree` showed
# several `orchard` versions at once. That is not a cosmetic lint — the pools,
# domains and key types are DISTINCT TYPES per version, and mixing them produces
# code that compiles and then silently misbehaves at runtime. The concrete bug we
# shipped: the escrow scanner used `OrchardDomain` for BOTH the orchard and the
# ironwood pool, so a real ironwood deposit failed to trial-decrypt and was never
# credited. A duplicate-version tree is exactly the condition that hides that class
# of mistake, because "which orchard does this `OrchardDomain` come from?" stops
# having a single answer.
#
# Why not `cargo deny` with `multiple-versions = "deny"`. The workspace currently
# has ~135 duplicated crates overall (axum 0.7/0.8, criterion 0.5/0.7, bitflags
# 1/2, ...). Those are ordinary transitive skew in non-consensus dependencies and
# denying them wholesale would mean a permanently red gate that everyone learns to
# ignore — worse than no gate. This check is deliberately narrow: it fails only on
# the crates where a duplicate is a correctness hazard, so a failure here is always
# real and always worth stopping for.
#
# Usage: scripts/check-zcash-stack-single-version.sh [cargo-manifest-dir]
set -euo pipefail

cd "${1:-$(dirname "$0")/..}"

# Crates whose types cross the consensus / note-encryption boundary. A second
# version of any of these means two incompatible type universes in one binary.
PATTERN='^(orchard|sapling-crypto|zcash_primitives|zcash_protocol|zcash_address|zcash_keys|zcash_transparent|zcash_note_encryption|zcash_encoding|pczt|halo2_proofs|halo2_gadgets|incrementalmerkletree)[[:space:]]'

dupes="$(cargo tree --duplicates 2>/dev/null | grep -E "$PATTERN" || true)"

if [[ -n "$dupes" ]]; then
    echo "ERROR: duplicate versions in the Zcash cryptographic stack." >&2
    echo >&2
    echo "$dupes" >&2
    echo >&2
    echo "Each of these crates must appear exactly once. Two versions means two" >&2
    echo "distinct sets of pool/domain/key types, which mix silently at runtime" >&2
    echo "(see the ironwood trial-decryption bug documented at the top of this" >&2
    echo "script). Unify the versions — do not add a fork or a [patch] to paper" >&2
    echo "over it. Run 'cargo tree -i <crate>' to find who pulls the odd one in." >&2
    exit 1
fi

echo "ok: Zcash cryptographic stack resolves to a single version per crate"
