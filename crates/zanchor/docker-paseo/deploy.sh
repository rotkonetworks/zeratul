#!/bin/bash
set -e

# Zanchor Collator Docker Deployment
# Uses wss://paseo.dotters.network for relay chain

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Alice's Aura key (from chainspec - well-known dev account)
ALICE_SEED="//Alice"
ALICE_PUBKEY="0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d"

echo "=== Zanchor Collator Deployment ==="
echo "Relay chain: wss://paseo.dotters.network"
echo ""

# Step 1: Start collator
echo "[1/2] Starting Zanchor collator..."
docker compose up -d zanchor-collator

echo "Waiting for collator to start..."
sleep 15

# Step 2: Insert session key for block authoring
echo ""
echo "[2/2] Inserting Aura session key..."

curl -s -H "Content-Type: application/json" \
  -d '{
    "id":1,
    "jsonrpc":"2.0",
    "method":"author_insertKey",
    "params":["aura", "'"$ALICE_SEED"'", "'"$ALICE_PUBKEY"'"]
  }' \
  http://localhost:9945

echo ""
echo ""
echo "=== Deployment Complete ==="
echo ""
echo "Collator RPC: ws://localhost:9945"
echo "Relay chain:  wss://paseo.dotters.network"
echo ""
echo "Logs: docker compose logs -f zanchor-collator"
echo ""
echo "Next: Get coretime at https://hub.regionx.tech/?network=paseo"
