#!/bin/bash
set -e

CHAIN_RPC="https://eth-passet-hub-paseo.dotters.network"
CONTRACT="0xe40dC8485142A4fb32356b958E05fE9a213A375E"
PRIVATE_KEY="5fb92d6e98884f76de468fa3f6278f8807c48bebc13595d45af5bdc4da702133"

# Use unique escrow ID based on timestamp
TIMESTAMP=$(date +%s)
ESCROW_ID="0x$(printf '%064x' $TIMESTAMP)"
COMMITMENT="0x$(openssl rand -hex 32)"
PUBKEY="0x$(openssl rand -hex 32)"
SHARE_C="0x$(openssl rand -hex 32)"

echo "=== Creating Escrow ==="
echo "Escrow ID:  ${ESCROW_ID:0:18}..."
echo "Commitment: ${COMMITMENT:0:18}..."
echo "Pubkey:     ${PUBKEY:0:18}..."
echo "Share C:    ${SHARE_C:0:18}..."
echo ""

# Build calldata manually
SELECTOR="0x38e9823e"
CALLDATA="${SELECTOR}${ESCROW_ID:2}${COMMITMENT:2}${PUBKEY:2}${SHARE_C:2}"
echo "Calldata length: $((${#CALLDATA} / 2)) bytes"
echo ""

echo "=== Sending transaction ==="
TX_HASH=$(cast send --private-key $PRIVATE_KEY \
    --rpc-url $CHAIN_RPC \
    --gas-limit 500000 \
    --async \
    $CONTRACT \
    "$CALLDATA" 2>&1)

echo "TX Hash: $TX_HASH"
echo ""
echo "Escrow ID for query: $ESCROW_ID"
echo ""
echo "To check escrow status:"
echo "  bash -c 'cast call --rpc-url $CHAIN_RPC $CONTRACT \"0xf023b811${ESCROW_ID:2}\"'"
