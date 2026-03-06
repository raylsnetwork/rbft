#!/bin/bash
# Kill a testnet node by HTTP port number
# Usage: ./scripts/kill_node.sh <port>
# Example: ./scripts/kill_node.sh 8542

set -euo pipefail

PORT=${1:-}

if [ -z "$PORT" ]; then
    echo "Usage: $0 <http_port>"
    echo "Example: $0 8542"
    exit 1
fi

# Find and kill the process
PID=$(pgrep -f "http.port $PORT")

if [ -z "$PID" ]; then
    echo "No node found running on port $PORT"
    exit 1
fi

echo "Killing node on port $PORT (PID: $PID)"
kill $PID 2>/dev/null

# Wait and verify
sleep 2

if pgrep -f "http.port $PORT" > /dev/null; then
    echo "Process still running, force killing..."
    pkill -9 -f "http.port $PORT"
    sleep 1
fi

# Clean up lock files for this node
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BASE_HTTP_PORT="${BASE_HTTP_PORT:-8545}"
MAX_NODES="${MAX_NODES:-6}"
NODE_INDEX=$((BASE_HTTP_PORT - PORT))
if [ "$NODE_INDEX" -lt 0 ] || [ "$NODE_INDEX" -ge "$MAX_NODES" ]; then
    echo "Warning: computed node index $NODE_INDEX out of range; skipping lock cleanup"
    exit 0
fi

TESTNET_DIR="${TESTNET_DIR:-$HOME/.rbft/testnet}"
DB_DIR="${DB_DIR:-$TESTNET_DIR/db}"
if [ ! -d "$DB_DIR" ] && [ -d "$ROOT_DIR/target/testnet/db" ]; then
    DB_DIR="$ROOT_DIR/target/testnet/db"
fi
DATADIR="$DB_DIR/d${NODE_INDEX}"
if [ -d "$DATADIR" ]; then
    find "$DATADIR" -name "lock" -type f -delete 2>/dev/null || true
fi

echo "Node on port $PORT killed and locks cleaned"
