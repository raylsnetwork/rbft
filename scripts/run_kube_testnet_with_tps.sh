#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

RPC_URLS="${RPC_URLS:-}"
if [[ -z "$RPC_URLS" ]]; then
  echo "RPC_URLS is not set."
  echo "Example:"
  echo "  export RPC_URLS=\"http://localhost:8545,http://localhost:8544\""
  exit 1
fi

RBFT_KUBE="${RBFT_KUBE:-true}"
RBFT_NUM_NODES="${RBFT_NUM_NODES:-13}"
RBFT_MAX_ACTIVE_VALIDATORS="${RBFT_MAX_ACTIVE_VALIDATORS:-13}"
RUST_LOG="${RUST_LOG:-info,rbft=debug,qbft_protocol=debug}"
POLL_SECONDS="${POLL_SECONDS:-2}"

TP_SCRIPT="$REPO_ROOT/tps-checker/run_tps_burst_cases.sh"
TP_OUT="${TP_OUT:-}"
TP_SUMMARY_OUT="${TP_SUMMARY_OUT:-}"
TP_QUIET="${TP_QUIET:-1}"

get_height() {
  local url="$1"
  local resp hex
  resp="$(
    curl -s "$url" \
      -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
      || true
  )"
  if [[ -z "$resp" ]]; then
    echo ""
    return 0
  fi

  hex="$(printf '%s' "$resp" | sed -n 's/.*"result":"\\(0x[0-9a-fA-F]*\\)".*/\\1/p')"
  if [[ -z "$hex" ]]; then
    echo ""
    return 0
  fi
  printf '%d\n' $((16#${hex#0x}))
}

first_url="${RPC_URLS%%,*}"
echo "Starting testnet..."
(
  export RBFT_KUBE RBFT_NUM_NODES RBFT_MAX_ACTIVE_VALIDATORS RUST_LOG
  export RBFT_NO_COLOR="${RBFT_NO_COLOR:-1}"
  export RBFT_HIDE_RSS="${RBFT_HIDE_RSS:-1}"
  make testnet_start
) &
testnet_pid=$!

initial_height=""
echo "Waiting for RPC to become available on $first_url..."
while [[ -z "$initial_height" ]]; do
  initial_height="$(get_height "$first_url")"
  sleep "$POLL_SECONDS"
done

echo "Waiting for block production on $first_url..."
while true; do
  current_height="$(get_height "$first_url")"
  if [[ -n "$current_height" && -n "$initial_height" ]]; then
    if (( current_height > initial_height )); then
      break
    fi
  elif [[ -n "$current_height" ]]; then
    break
  fi
  sleep "$POLL_SECONDS"
done

echo "Blocks are producing (height: ${current_height:-?})."

launch_tps_cmd=(
  "cd \"$REPO_ROOT\""
  "RPC_URLS=\"$RPC_URLS\""
  "QUIET=\"$TP_QUIET\""
)
if [[ -n "$TP_OUT" ]]; then
  launch_tps_cmd+=("OUT=\"$TP_OUT\"")
fi
if [[ -n "$TP_SUMMARY_OUT" ]]; then
  launch_tps_cmd+=("SUMMARY_OUT=\"$TP_SUMMARY_OUT\"")
fi
launch_tps_cmd+=("\"$TP_SCRIPT\"")
launch_tps_cmd_str=$(IFS=' '; echo "${launch_tps_cmd[*]}")

case "$(uname -s)" in
  Darwin)
    osascript -e "tell application \"Terminal\" to do script \"$launch_tps_cmd_str\""
    ;;
  Linux)
    if command -v gnome-terminal >/dev/null 2>&1; then
      gnome-terminal -- bash -lc "$launch_tps_cmd_str"
    elif command -v xterm >/dev/null 2>&1; then
      xterm -e bash -lc "$launch_tps_cmd_str" &
    else
      nohup bash -lc "$launch_tps_cmd_str" >/dev/null 2>&1 &
    fi
    ;;
  *)
    nohup bash -lc "$launch_tps_cmd_str" >/dev/null 2>&1 &
    ;;
esac

echo "TPS burst launched in a separate terminal (or background fallback)."
