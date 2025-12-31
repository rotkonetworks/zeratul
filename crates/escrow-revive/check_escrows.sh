#!/bin/bash
set -e

CHAIN_RPC="https://eth-passet-hub-paseo.dotters.network"
CONTRACT="0xe40dC8485142A4fb32356b958E05fE9a213A375E"
SELECTOR=$(cast sig "getEscrow(bytes32)")

echo "Contract: $CONTRACT"
echo "Selector: $SELECTOR"
echo ""

for ESCROW_ID in \
    "0xdcba4077100f016ea5c5c1cc20f86eedeb2c5467c0b724f19192fb559b8c1089" \
    "0x1111111111111111111111111111111111111111111111111111111111111111" \
    "0xaabbccdd11111111111111111111111111111111111111111111111111111111"; do

    echo "=== Checking escrow: ${ESCROW_ID:0:18}... ==="
    CALLDATA="${SELECTOR}${ESCROW_ID:2}"
    RESULT=$(cast call --rpc-url $CHAIN_RPC $CONTRACT "$CALLDATA" 2>&1 || true)
    if [ "$RESULT" = "0x" ] || [ -z "$RESULT" ]; then
        echo "  Status: Not found"
    else
        echo "  Status: Found!"
        echo "  Response length: ${#RESULT} chars"
        echo "  Data: ${RESULT:0:130}..."
    fi
    echo ""
done

echo "=== Checking events (last 100 blocks) ==="
# Get latest block
LATEST=$(cast block latest --rpc-url $CHAIN_RPC -j 2>/dev/null | jq -r '.number' || echo "0")
if [ "$LATEST" != "0" ] && [ -n "$LATEST" ]; then
    FROM_BLOCK=$((LATEST - 100))
    if [ $FROM_BLOCK -lt 0 ]; then FROM_BLOCK=0; fi
    echo "Scanning blocks $FROM_BLOCK to $LATEST"
    cast logs --rpc-url $CHAIN_RPC --address $CONTRACT --from-block $FROM_BLOCK 2>&1 | head -50
else
    echo "Could not get latest block"
fi
