#!/bin/bash
set -e

# Zeratul Local Testnet Deployment
# Spins up 4 validators on localhost

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TESTNET_DIR="$PROJECT_ROOT/testnet-local"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
NUM_VALIDATORS=4
THRESHOLD=3  # 2f+1
BASE_PORT=9000

log() {
    echo -e "${GREEN}[$(date +'%H:%M:%S')]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[$(date +'%H:%M:%S')]${NC} $1"
}

error() {
    echo -e "${RED}[$(date +'%H:%M:%S')]${NC} $1"
}

section() {
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
}

# Check if testnet is already running
check_running() {
    if pgrep -f "zeratul.*validator" > /dev/null; then
        error "Testnet is already running!"
        error "Run: $0 stop"
        exit 1
    fi
}

# Clean up old testnet data
cleanup() {
    section "🧹 Cleaning up old testnet data"

    if [ -d "$TESTNET_DIR" ]; then
        log "Removing $TESTNET_DIR"
        rm -rf "$TESTNET_DIR"
    fi

    # Kill any running validator processes
    if pgrep -f "zeratul.*validator" > /dev/null; then
        warn "Killing running validator processes..."
        pkill -f "zeratul.*validator" || true
        sleep 1
    fi

    log "Cleanup complete"
}

# Build binaries
build() {
    section "🔨 Building Zeratul binaries"

    cd "$PROJECT_ROOT"

    log "Building with optimizations..."
    RUSTFLAGS="-C target-cpu=native" cargo build --release \
        --package zeratul-blockchain \
        --bin validator \
        2>&1 | grep -v "warning:" || true

    if [ ! -f "$PROJECT_ROOT/target/release/validator" ]; then
        error "Build failed: validator binary not found"
        exit 1
    fi

    log "✅ Build complete"
}

# Setup validator configurations
setup_validators() {
    section "⚙️  Setting up $NUM_VALIDATORS validators"

    mkdir -p "$TESTNET_DIR"

    for i in $(seq 0 $((NUM_VALIDATORS - 1))); do
        VALIDATOR_DIR="$TESTNET_DIR/validator-$i"
        mkdir -p "$VALIDATOR_DIR"

        # Create validator config
        cat > "$VALIDATOR_DIR/config.yaml" <<EOF
# Validator $i Configuration

validator:
  index: $i
  name: "validator-$i"

network:
  listen_addr: "127.0.0.1:$((BASE_PORT + i))"
  peers:
$(for j in $(seq 0 $((NUM_VALIDATORS - 1))); do
    if [ $j -ne $i ]; then
        echo "    - \"127.0.0.1:$((BASE_PORT + j))\""
    fi
done)

dkg:
  validator_count: $NUM_VALIDATORS
  threshold: $THRESHOLD
  epoch: 0

storage:
  path: "$VALIDATOR_DIR/data"

logging:
  level: "info"
  file: "$VALIDATOR_DIR/validator.log"
EOF

        # Create data directory
        mkdir -p "$VALIDATOR_DIR/data"

        log "✅ Validator $i configured (port $((BASE_PORT + i)))"
    done

    log "All validators configured"
}

# Start validators
start_validators() {
    section "🚀 Starting validators"

    cd "$PROJECT_ROOT"

    for i in $(seq 0 $((NUM_VALIDATORS - 1))); do
        VALIDATOR_DIR="$TESTNET_DIR/validator-$i"
        LOG_FILE="$VALIDATOR_DIR/validator.log"
        PID_FILE="$VALIDATOR_DIR/validator.pid"

        log "Starting validator $i..."

        # Start validator in background
        nohup "$PROJECT_ROOT/target/release/validator" \
            --config "$VALIDATOR_DIR/config.yaml" \
            > "$LOG_FILE" 2>&1 &

        echo $! > "$PID_FILE"

        log "✅ Validator $i started (PID: $(cat $PID_FILE))"
        sleep 0.5
    done

    log "All validators started"
}

# Check validator status
status() {
    section "📊 Testnet Status"

    if [ ! -d "$TESTNET_DIR" ]; then
        warn "Testnet not initialized"
        echo "Run: $0 start"
        return 1
    fi

    echo "Validators:"
    for i in $(seq 0 $((NUM_VALIDATORS - 1))); do
        PID_FILE="$TESTNET_DIR/validator-$i/validator.pid"

        if [ -f "$PID_FILE" ]; then
            PID=$(cat "$PID_FILE")
            if ps -p $PID > /dev/null 2>&1; then
                echo -e "  ${GREEN}✓${NC} Validator $i (PID $PID, port $((BASE_PORT + i)))"
            else
                echo -e "  ${RED}✗${NC} Validator $i (dead, PID file exists)"
            fi
        else
            echo -e "  ${RED}✗${NC} Validator $i (not started)"
        fi
    done

    echo ""
    echo "Logs: $TESTNET_DIR/validator-*/validator.log"
    echo "Data: $TESTNET_DIR/validator-*/data"
}

# Stop validators
stop() {
    section "🛑 Stopping validators"

    if [ ! -d "$TESTNET_DIR" ]; then
        warn "Testnet not running"
        return 0
    fi

    for i in $(seq 0 $((NUM_VALIDATORS - 1))); do
        PID_FILE="$TESTNET_DIR/validator-$i/validator.pid"

        if [ -f "$PID_FILE" ]; then
            PID=$(cat "$PID_FILE")
            if ps -p $PID > /dev/null 2>&1; then
                log "Stopping validator $i (PID $PID)..."
                kill $PID || true
                sleep 0.5
            fi
            rm "$PID_FILE"
        fi
    done

    # Force kill any remaining processes
    if pgrep -f "zeratul.*validator" > /dev/null; then
        warn "Force killing remaining processes..."
        pkill -9 -f "zeratul.*validator" || true
    fi

    log "All validators stopped"
}

# Tail logs
logs() {
    local validator=${1:-0}

    LOG_FILE="$TESTNET_DIR/validator-$validator/validator.log"

    if [ ! -f "$LOG_FILE" ]; then
        error "Log file not found: $LOG_FILE"
        exit 1
    fi

    log "Tailing validator $validator logs (Ctrl+C to exit)"
    tail -f "$LOG_FILE"
}

# Run test suite
test() {
    section "🧪 Running test suite"

    cd "$PROJECT_ROOT"

    log "Testing privacy tiers..."
    cargo run --release --example test_privacy_tiers

    log ""
    log "Testing MPC transfer..."
    cargo run --release --example test_mpc_transfer

    log ""
    log "Testing PolkaVM-ZODA..."
    cargo run --release --example test_polkavm_reconstruction

    log ""
    log "Testing FROST DKG..."
    cargo run --release --example test_frost_dkg

    log ""
    log "✅ All tests passed!"
}

# Show help
help() {
    cat <<EOF
Zeratul Local Testnet Manager

Usage: $0 <command>

Commands:
    start       - Build and start 4-validator testnet
    stop        - Stop all validators
    restart     - Restart testnet
    status      - Show validator status
    logs [N]    - Tail logs for validator N (default: 0)
    test        - Run test suite
    cleanup     - Remove all testnet data
    help        - Show this help

Examples:
    $0 start              # Start testnet
    $0 status             # Check status
    $0 logs 0             # View validator 0 logs
    $0 test               # Run tests
    $0 stop               # Stop testnet

Configuration:
    Validators: $NUM_VALIDATORS
    Threshold:  $THRESHOLD (2f+1)
    Base Port:  $BASE_PORT
    Data Dir:   $TESTNET_DIR

EOF
}

# Main command dispatcher
case "${1:-}" in
    start)
        check_running
        cleanup
        build
        setup_validators
        start_validators
        echo ""
        log "🎉 Testnet started successfully!"
        echo ""
        echo "Next steps:"
        echo "  1. Check status: $0 status"
        echo "  2. View logs:    $0 logs 0"
        echo "  3. Run tests:    $0 test"
        echo ""
        ;;

    stop)
        stop
        ;;

    restart)
        stop
        sleep 1
        "$0" start
        ;;

    status)
        status
        ;;

    logs)
        logs "${2:-0}"
        ;;

    test)
        test
        ;;

    cleanup)
        stop
        cleanup
        ;;

    help|--help|-h)
        help
        ;;

    *)
        error "Unknown command: ${1:-}"
        echo ""
        help
        exit 1
        ;;
esac
