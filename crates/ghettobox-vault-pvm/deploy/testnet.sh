#!/bin/bash
# deploy testnet: collator nodes + vault nodes
#
# usage:
#   ./testnet.sh start   - start all nodes
#   ./testnet.sh stop    - stop all nodes
#   ./testnet.sh status  - show status
#   ./testnet.sh logs    - show logs

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DATA_DIR="${DATA_DIR:-/tmp/ghettobox-testnet}"

# vault node ports
VAULT1_PORT=4201
VAULT2_PORT=4202
VAULT3_PORT=4203

# colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[testnet]${NC} $1"; }
warn() { echo -e "${YELLOW}[testnet]${NC} $1"; }
err() { echo -e "${RED}[testnet]${NC} $1"; }

build_vault() {
    log "building vault node..."
    cd "$PROJECT_DIR/vault"
    cargo build --release 2>&1 | tail -5
    log "vault binary: $PROJECT_DIR/vault/target/release/ghettobox-vault"
}

start_vaults() {
    log "starting vault nodes..."

    mkdir -p "$DATA_DIR/vault1" "$DATA_DIR/vault2" "$DATA_DIR/vault3"
    mkdir -p "$DATA_DIR/logs"

    VAULT_BIN="$PROJECT_DIR/vault/target/release/ghettobox-vault"

    if [ ! -f "$VAULT_BIN" ]; then
        build_vault
    fi

    # vault 1
    if ! pgrep -f "ghettobox-vault.*--port $VAULT1_PORT" > /dev/null; then
        log "starting vault1 on port $VAULT1_PORT (software mode)"
        nohup "$VAULT_BIN" \
            --port $VAULT1_PORT \
            --index 1 \
            --mode software \
            --data-dir "$DATA_DIR/vault1" \
            > "$DATA_DIR/logs/vault1.log" 2>&1 &
        echo $! > "$DATA_DIR/vault1.pid"
    else
        warn "vault1 already running"
    fi

    # vault 2
    if ! pgrep -f "ghettobox-vault.*--port $VAULT2_PORT" > /dev/null; then
        log "starting vault2 on port $VAULT2_PORT (software mode)"
        nohup "$VAULT_BIN" \
            --port $VAULT2_PORT \
            --index 2 \
            --mode software \
            --data-dir "$DATA_DIR/vault2" \
            > "$DATA_DIR/logs/vault2.log" 2>&1 &
        echo $! > "$DATA_DIR/vault2.pid"
    else
        warn "vault2 already running"
    fi

    # vault 3
    if ! pgrep -f "ghettobox-vault.*--port $VAULT3_PORT" > /dev/null; then
        log "starting vault3 on port $VAULT3_PORT (software mode)"
        nohup "$VAULT_BIN" \
            --port $VAULT3_PORT \
            --index 3 \
            --mode software \
            --data-dir "$DATA_DIR/vault3" \
            > "$DATA_DIR/logs/vault3.log" 2>&1 &
        echo $! > "$DATA_DIR/vault3.pid"
    else
        warn "vault3 already running"
    fi

    sleep 1

    # verify
    for port in $VAULT1_PORT $VAULT2_PORT $VAULT3_PORT; do
        if curl -s "http://localhost:$port/health" > /dev/null 2>&1; then
            log "vault on :$port is healthy"
        else
            warn "vault on :$port not responding yet"
        fi
    done
}

stop_vaults() {
    log "stopping vault nodes..."

    for i in 1 2 3; do
        pid_file="$DATA_DIR/vault$i.pid"
        if [ -f "$pid_file" ]; then
            pid=$(cat "$pid_file")
            if kill -0 "$pid" 2>/dev/null; then
                kill "$pid"
                log "stopped vault$i (pid $pid)"
            fi
            rm -f "$pid_file"
        fi
    done

    # cleanup any stragglers
    pkill -f "ghettobox-vault" 2>/dev/null || true
}

status() {
    echo ""
    echo "=== vault nodes ==="
    for port in $VAULT1_PORT $VAULT2_PORT $VAULT3_PORT; do
        if curl -s "http://localhost:$port" 2>/dev/null | head -1; then
            echo -e "  :$port ${GREEN}online${NC}"
        else
            echo -e "  :$port ${RED}offline${NC}"
        fi
    done
    echo ""

    echo "=== processes ==="
    ps aux | grep -E "(ghettobox-vault|collator)" | grep -v grep || echo "  no nodes running"
    echo ""

    echo "=== data ==="
    du -sh "$DATA_DIR"/* 2>/dev/null || echo "  no data yet"
    echo ""
}

logs() {
    node="${2:-vault1}"
    log_file="$DATA_DIR/logs/$node.log"
    if [ -f "$log_file" ]; then
        tail -f "$log_file"
    else
        err "no log file for $node"
    fi
}

show_info() {
    echo ""
    echo "=== testnet info ==="
    echo ""
    echo "vault endpoints:"
    echo "  http://localhost:$VAULT1_PORT"
    echo "  http://localhost:$VAULT2_PORT"
    echo "  http://localhost:$VAULT3_PORT"
    echo ""
    echo "data directory: $DATA_DIR"
    echo ""
    echo "to register an account:"
    echo '  curl -X POST http://localhost:4201/register \'
    echo '    -H "Content-Type: application/json" \'
    echo '    -d '"'"'{"user_id":"...", "unlock_tag":"...", "encrypted_share":"...", "allowed_guesses":5}'"'"
    echo ""
}

case "${1:-}" in
    start)
        start_vaults
        show_info
        ;;
    stop)
        stop_vaults
        ;;
    restart)
        stop_vaults
        sleep 1
        start_vaults
        show_info
        ;;
    status)
        status
        ;;
    logs)
        logs "$@"
        ;;
    build)
        build_vault
        ;;
    info)
        show_info
        ;;
    *)
        echo "usage: $0 {start|stop|restart|status|logs|build|info}"
        echo ""
        echo "commands:"
        echo "  start   - start all vault nodes"
        echo "  stop    - stop all vault nodes"
        echo "  restart - restart all vault nodes"
        echo "  status  - show node status"
        echo "  logs    - tail logs (logs vault1, logs vault2, etc)"
        echo "  build   - build vault binary"
        echo "  info    - show testnet info"
        exit 1
        ;;
esac
