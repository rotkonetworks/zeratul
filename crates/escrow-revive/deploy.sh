#!/bin/bash
cd /home/alice/rotko/zeratul/crates/escrow-revive

export CHAIN_RPC=https://eth-passet-hub-paseo.dotters.network
export PRIVATE_KEY=5fb92d6e98884f76de468fa3f6278f8807c48bebc13595d45af5bdc4da702133

BYTECODE=$(xxd -p -c 99999 escrow.polkavm)

echo "=== Deploying updated escrow contract ==="
cast send --private-key $PRIVATE_KEY \
    --rpc-url $CHAIN_RPC \
    --gas-limit 10000000 \
    --json \
    --create \
    "0x${BYTECODE}"
