#!/bin/bash
# Restart a testnet node by HTTP port number
# Usage: ./scripts/restart_node.sh <port>
# Example: ./scripts/restart_node.sh 8542

set -euo pipefail

PORT=${1:-}

if [ -z "$PORT" ]; then
    echo "Usage: $0 <http_port>"
    echo "Example: $0 8542"
    exit 1
fi

# Calculate node index from port (BASE_HTTP_PORT=0, BASE_HTTP_PORT-1=1, ...)
BASE_HTTP_PORT="${BASE_HTTP_PORT:-8545}"
MAX_NODES="${MAX_NODES:-6}"
NODE_INDEX=$((BASE_HTTP_PORT - PORT))

if [ $NODE_INDEX -lt 0 ] || [ $NODE_INDEX -ge $MAX_NODES ]; then
    echo "Invalid port. Must be between $((BASE_HTTP_PORT - MAX_NODES + 1))-${BASE_HTTP_PORT}"
    exit 1
fi

# Paths
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TESTNET_DIR="${TESTNET_DIR:-$HOME/.rbft/testnet}"
ASSETS_DIR="${ASSETS_DIR:-$TESTNET_DIR/assets}"
DB_DIR="${DB_DIR:-$TESTNET_DIR/db}"
LOGS_DIR="${LOGS_DIR:-$TESTNET_DIR/logs}"
if [ ! -d "$DB_DIR" ] && [ -d "$ROOT_DIR/target/testnet/db" ]; then
    DB_DIR="$ROOT_DIR/target/testnet/db"
fi
if [ ! -d "$LOGS_DIR" ] && [ -d "$ROOT_DIR/target/testnet/logs" ]; then
    LOGS_DIR="$ROOT_DIR/target/testnet/logs"
fi
RBFT_BIN="${RBFT_BIN:-$ROOT_DIR/target/release/rbft-node}"
RETH_CONFIG="${RETH_CONFIG:-$TESTNET_DIR/reth-config.toml}"
NODE_DB_DIR="$DB_DIR/d${NODE_INDEX}"
FALLBACK_DB_DIR="$ROOT_DIR/target/testnet/db/d${NODE_INDEX}"
if [ ! -d "$NODE_DB_DIR" ] && [ -d "$FALLBACK_DB_DIR" ]; then
    NODE_DB_DIR="$FALLBACK_DB_DIR"
fi

# Ports
P2P_PORT=$((30303 + NODE_INDEX))
AUTHRPC_PORT=$((8551 + NODE_INDEX * 10))

# Check if already running
if pgrep -f "http.port $PORT" > /dev/null; then
    echo "Node on port $PORT is already running"
    exit 1
fi

# Ensure directories exist and clean up any stale lock files
mkdir -p "$NODE_DB_DIR" "$LOGS_DIR"
find "$NODE_DB_DIR" -name "lock" -type f -delete 2>/dev/null || true

echo "Starting node $NODE_INDEX on port $PORT..."

# Read trusted peers from the generated enodes list
TRUSTED_PEERS=""
if [ -f "$ASSETS_DIR/enodes.txt" ]; then
    TRUSTED_PEERS=$(cat "$ASSETS_DIR/enodes.txt" | tr -d '\n')
fi

if [ -z "$TRUSTED_PEERS" ]; then
    echo "Error: Could not determine trusted peers. Make sure other nodes are running."
    echo "You may need to start the testnet first with 'make testnet_start'"
    exit 1
fi

# Start the node
LOG_FILE="$LOGS_DIR/node${NODE_INDEX}-restart.log"

$RBFT_DIR/target/release/rbft-node node \
    --http --http.port $PORT --http.addr 0.0.0.0 --http.corsdomain '*' --http.api eth,txpool \
    --chain $ASSETS_DIR/genesis.json \
    --config $DB_DIR/reth-config.toml \
    --p2p-secret-key $ASSETS_DIR/p2p-secret-key${NODE_INDEX}.txt \
    --validator-key $ASSETS_DIR/validator-key${NODE_INDEX}.txt \
    --trusted-peers "$TRUSTED_PEERS" \
    --datadir $DB_DIR \
    --ipcpath $DB_DIR/reth.ipc \
    --authrpc.port $AUTHRPC_PORT \
    --port $P2P_PORT \
    --disable-discovery \
    --txpool.max-tx-input-bytes 4194304 \
    --builder.gaslimit 600000000 \
    --tx-propagation-mode all \
    --txpool.pending-max-count 150000 \
    --txpool.basefee-max-count 150000 \
    --txpool.queued-max-count 150000 \
    --txpool.max-account-slots 3000 \
    --txpool.max-pending-txns 100000 \
    --txpool.max-new-txns 100000 \
    > "$LOG_FILE" 2>&1 &

NODE_PID=$!
sleep 2

if ps -p $NODE_PID > /dev/null 2>&1; then
    echo "Node $NODE_INDEX started on port $PORT (PID: $NODE_PID)"
    echo "Logs: $LOG_FILE"
else
    echo "Failed to start node. Check logs: $LOG_FILE"
    tail -20 "$LOG_FILE"
    exit 1
fi
