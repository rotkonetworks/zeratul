#!/bin/bash
set -e

CHAIN_RPC="https://eth-passet-hub-paseo.dotters.network"
PRIVATE_KEY="5fb92d6e98884f76de468fa3f6278f8807c48bebc13595d45af5bdc4da702133"
CONTRACT="0xe40dC8485142A4fb32356b958E05fE9a213A375E"

SELLER=$(cast wallet address --private-key $PRIVATE_KEY)
echo "Seller address: $SELLER"

echo "=== Verifying contract code ==="
CODE_LEN=$(cast code $CONTRACT --rpc-url $CHAIN_RPC | wc -c)
echo "Code length: $CODE_LEN bytes"

echo ""
echo "=== Creating Test Escrow ==="
ESCROW_ID="0x$(openssl rand -hex 32)"
COMMITMENT="0x$(openssl rand -hex 32)"
ESCROW_PUBKEY="0x$(openssl rand -hex 32)"
SHARE_C="0x$(openssl rand -hex 32)"

echo "Escrow ID: $ESCROW_ID"
echo "Commitment: $COMMITMENT"
echo "Escrow Pubkey: $ESCROW_PUBKEY"
echo "Share C: $SHARE_C"

echo ""
echo "=== Calling createEscrow ==="
cast send --private-key $PRIVATE_KEY \
    --rpc-url $CHAIN_RPC \
    --gas-limit 500000 \
    --json \
    $CONTRACT \
    "createEscrow(bytes32,bytes32,bytes32,bytes32)" \
    $ESCROW_ID $COMMITMENT $ESCROW_PUBKEY $SHARE_C

echo ""
echo "=== Querying escrow state ==="
cast call --rpc-url $CHAIN_RPC \
    $CONTRACT \
    "getEscrow(bytes32)(uint8,bytes32,bytes32,address,address)" \
    $ESCROW_ID
