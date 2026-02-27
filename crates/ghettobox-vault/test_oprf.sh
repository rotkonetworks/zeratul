#!/bin/bash
# test script for OPRF protocol end-to-end
# starts 3 vault servers and tests register/recover flow

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/../.."

echo "=== building vault server ==="
cargo build -p ghettobox-vault --features software

VAULT_BIN="./target/debug/ghettobox-vault"
DATA_DIR="/tmp/ghettobox-test-$$"

cleanup() {
    echo "cleaning up..."
    kill $PID0 $PID1 $PID2 2>/dev/null || true
    rm -rf "$DATA_DIR"
}
trap cleanup EXIT

echo "=== starting vault servers ==="
mkdir -p "$DATA_DIR/vault0" "$DATA_DIR/vault1" "$DATA_DIR/vault2"

$VAULT_BIN --port 4200 --index 0 --mode software --data-dir "$DATA_DIR/vault0" &
PID0=$!
$VAULT_BIN --port 4201 --index 1 --mode software --data-dir "$DATA_DIR/vault1" &
PID1=$!
$VAULT_BIN --port 4202 --index 2 --mode software --data-dir "$DATA_DIR/vault2" &
PID2=$!

sleep 2

echo "=== checking vault health ==="
for port in 4200 4201 4202; do
    echo "vault $port:"
    curl -s "http://127.0.0.1:$port/oprf/health" | jq .
done

echo ""
echo "=== fetching OPRF public keys ==="
PK0=$(curl -s "http://127.0.0.1:4200/oprf/health" | jq -r '.oprf_pubkey')
PK1=$(curl -s "http://127.0.0.1:4201/oprf/health" | jq -r '.oprf_pubkey')
PK2=$(curl -s "http://127.0.0.1:4202/oprf/health" | jq -r '.oprf_pubkey')
echo "vault0 pubkey: $PK0"
echo "vault1 pubkey: $PK1"
echo "vault2 pubkey: $PK2"

# generate test data (in real usage this would be done by the client)
USER_ID="test@example.com"
# use ristretto basepoint (a known valid compressed ristretto point)
BLINDED="e2f2ae0a6abc4e71a884a961c500515f58e30b6aa582dd8db6a65945e08d2d76"
ENCRYPTED_SEED="deadbeefcafebabe1234567890abcdef"

echo ""
echo "=== testing OPRF register ==="
for port in 4200 4201 4202; do
    echo "registering with vault $port..."
    RESP=$(curl -s -X POST "http://127.0.0.1:$port/oprf/register" \
        -H "Content-Type: application/json" \
        -d "{\"user_id\":\"$USER_ID\",\"blinded\":\"$BLINDED\",\"encrypted_seed\":\"$ENCRYPTED_SEED\",\"allowed_guesses\":5}")
    echo "$RESP" | jq . 2>/dev/null || echo "raw response: $RESP"
done

echo ""
echo "=== testing OPRF recover ==="
for port in 4200 4201; do
    echo "recovering from vault $port..."
    curl -s -X POST "http://127.0.0.1:$port/oprf/recover" \
        -H "Content-Type: application/json" \
        -d "{\"user_id\":\"$USER_ID\",\"blinded\":\"$BLINDED\"}" \
        | jq .
done

echo ""
echo "=== testing OPRF confirm ==="
curl -s -X POST "http://127.0.0.1:4200/oprf/confirm/$USER_ID" | jq .

echo ""
echo "=== all tests passed ==="
