#!/bin/bash
# local testnet - runs 3 vault providers

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BLOB_SRC="$PROJECT_DIR/../vault-guest/target/riscv32emac-unknown-none-polkavm/release/ghettobox_vault_guest.elf"
BLOB="${1:-/tmp/vault-guest-stripped.elf}"
DATA_BASE="${2:-/tmp/vault-testnet}"

# strip debug symbols (avoids DWARF parsing issues)
if [ -f "$BLOB_SRC" ] && [ ! -f "$BLOB" ]; then
    echo "stripping debug symbols from vault-guest..."
    llvm-strip --strip-debug "$BLOB_SRC" -o "$BLOB" 2>/dev/null || \
    strip --strip-debug "$BLOB_SRC" -o "$BLOB" 2>/dev/null || \
    cp "$BLOB_SRC" "$BLOB"
fi

# build if needed
echo "building vault-pvm..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

VAULT_BIN="$PROJECT_DIR/target/release/ghettobox-vault-pvm"

# cleanup previous
pkill -f ghettobox-vault-pvm 2>/dev/null || true
rm -rf "$DATA_BASE"
mkdir -p "$DATA_BASE"

echo ""
echo "starting 3 vault providers..."
echo "  blob: $BLOB"
echo "  data: $DATA_BASE"
echo ""

# provider 1: port 4201
"$VAULT_BIN" \
    --port 4201 \
    --index 1 \
    --blob "$BLOB" \
    --data-dir "$DATA_BASE/node1" \
    2>&1 | sed 's/^/[node1] /' &

# provider 2: port 4202
"$VAULT_BIN" \
    --port 4202 \
    --index 2 \
    --blob "$BLOB" \
    --data-dir "$DATA_BASE/node2" \
    2>&1 | sed 's/^/[node2] /' &

# provider 3: port 4203
"$VAULT_BIN" \
    --port 4203 \
    --index 3 \
    --blob "$BLOB" \
    --data-dir "$DATA_BASE/node3" \
    2>&1 | sed 's/^/[node3] /' &

sleep 2

echo ""
echo "=== testnet running ==="
echo ""
echo "providers:"
echo "  node1: http://127.0.0.1:4201  metrics: http://127.0.0.1:5201"
echo "  node2: http://127.0.0.1:4202  metrics: http://127.0.0.1:5202"
echo "  node3: http://127.0.0.1:4203  metrics: http://127.0.0.1:5203"
echo ""

# show node info
echo "=== node info ==="
for port in 4201 4202 4203; do
    echo -n "  $port: "
    curl -s "http://127.0.0.1:$port/" | jq -c '{index, pubkey: .pubkey[0:16]}' 2>/dev/null || echo "not ready"
done
echo ""

echo "press ctrl+c to stop"
wait
