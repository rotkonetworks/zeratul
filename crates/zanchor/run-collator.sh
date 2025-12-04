#!/bin/bash
set -e

# Zanchor Collator Startup Script
# Uses polkadot-omni-node with our runtime

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OMNI_NODE="$SCRIPT_DIR/polkadot-omni-node"
CHAINSPEC="$SCRIPT_DIR/zanchor-paseo-5082.json"
DATA_DIR="${DATA_DIR:-$SCRIPT_DIR/collator-data}"

# Paseo relay chain RPC endpoints
RELAY_CHAIN_RPC="wss://paseo.rpc.amforc.com:443"

# Create data directory
mkdir -p "$DATA_DIR"

echo "Starting Zanchor Collator (ParaId 5082)"
echo "Data directory: $DATA_DIR"
echo "Chainspec: $CHAINSPEC"

exec "$OMNI_NODE" \
    --collator \
    --chain "$CHAINSPEC" \
    --base-path "$DATA_DIR" \
    --rpc-cors all \
    --rpc-methods unsafe \
    --rpc-external \
    --name "zanchor-collator-01" \
    --force-authoring \
    -- \
    --sync warp \
    --chain paseo
