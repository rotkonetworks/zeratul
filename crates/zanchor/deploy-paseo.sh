#!/bin/bash
set -e

# Zanchor Paseo Deployment Script
# Consolidates funds, reserves ParaId, and registers parachain

ZANCHOR_CLI="/home/alice/rotko/zeratul/crates/zanchor/target/release/zanchor"
ZANCHOR_DIR="/home/alice/rotko/zeratul/crates/zanchor"

# Account mnemonics (KEEP THESE SECRET IN PRODUCTION!)
MAIN_SEED="move defense manage burden pudding core elite aware tenant payment assault federal"
SEED2="hand honey auction feel violin kitten retire issue current horror royal tree"
SEED3="opinion negative sand when year borrow want bike fiscal casual notable end"
SEED4="talk mechanic fuel crew cream scissors answer unique emotion advance boat behind"

# Main account address (SS58)
MAIN_ACCOUNT="15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT"

# Paseo uses 10 decimals: 1 PAS = 10,000,000,000 planck
PAS=10000000000

# Transfer amount (leave 1 PAS for fees and existential deposit)
TRANSFER_AMOUNT=$((49 * PAS))  # 49 PAS each

echo "==================================="
echo "Zanchor Paseo Deployment"
echo "==================================="
echo ""

# Step 1: Check current balances
echo "[1/6] Checking current balances..."
echo ""
echo "Main account:"
$ZANCHOR_CLI balance --account "$MAIN_ACCOUNT" 2>&1 | grep -E "(Account|free|unfunded)"
echo ""

# Step 2: Consolidate funds to main account
echo "[2/6] Consolidating funds to main account..."
echo ""

echo "Transferring from account 2..."
$ZANCHOR_CLI --seed "$SEED2" transfer --to "$MAIN_ACCOUNT" --amount $TRANSFER_AMOUNT 2>&1 || echo "Transfer 2 failed or already done"
sleep 6

echo "Transferring from account 3..."
$ZANCHOR_CLI --seed "$SEED3" transfer --to "$MAIN_ACCOUNT" --amount $TRANSFER_AMOUNT 2>&1 || echo "Transfer 3 failed or already done"
sleep 6

echo "Transferring from account 4..."
$ZANCHOR_CLI --seed "$SEED4" transfer --to "$MAIN_ACCOUNT" --amount $TRANSFER_AMOUNT 2>&1 || echo "Transfer 4 failed or already done"
sleep 6

echo ""
echo "Main account after consolidation:"
$ZANCHOR_CLI balance --account "$MAIN_ACCOUNT" 2>&1 | grep -E "(Account|free)"
echo ""

# Step 3: Reserve ParaId
echo "[3/6] Reserving ParaId..."
echo ""
RESERVE_OUTPUT=$($ZANCHOR_CLI --seed "$MAIN_SEED" reserve 2>&1)
echo "$RESERVE_OUTPUT"
sleep 12

# Extract ParaId from events (this is simplified - real implementation should parse events)
echo ""
echo "NOTE: Check the events above for 'Registered' event with your ParaId"
echo "Enter the reserved ParaId:"
read -p "ParaId: " PARA_ID

if [ -z "$PARA_ID" ]; then
    echo "No ParaId provided, exiting"
    exit 1
fi

echo ""
echo "Using ParaId: $PARA_ID"

# Step 4: Update chainspec with ParaId
echo "[4/6] Updating chainspec with ParaId $PARA_ID..."
$ZANCHOR_CLI chainspec \
    --para-id "$PARA_ID" \
    --input "$ZANCHOR_DIR/zanchor-paseo-chainspec.json" \
    --output "$ZANCHOR_DIR/zanchor-paseo-final.json"

# Step 5: Export genesis files
echo "[5/6] Exporting genesis files..."
$ZANCHOR_CLI export-genesis \
    --chainspec "$ZANCHOR_DIR/zanchor-paseo-final.json" \
    --output-dir "$ZANCHOR_DIR"

echo ""
echo "Genesis files:"
ls -la "$ZANCHOR_DIR"/zanchor-genesis-*.hex

# Step 6: Register parachain
echo ""
echo "[6/6] Registering parachain..."
echo ""
echo "This requires ~45 PAS deposit. Proceeding..."

$ZANCHOR_CLI --seed "$MAIN_SEED" register \
    --para-id "$PARA_ID" \
    --genesis-head "$ZANCHOR_DIR/zanchor-genesis-head.hex" \
    --validation-code "$ZANCHOR_DIR/zanchor-genesis-wasm.hex"

echo ""
echo "==================================="
echo "Deployment Complete!"
echo "==================================="
echo ""
echo "ParaId: $PARA_ID"
echo ""
echo "Next steps:"
echo "1. Go to Paseo Coretime UI"
echo "2. Assign your coretime region to Task ID: $PARA_ID"
echo "3. Choose 'Final' finality for renewal eligibility"
echo ""
