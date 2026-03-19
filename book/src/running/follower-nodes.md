# Follower Nodes

A follower node connects to the validator network, receives blocks, and
maintains a full copy of the chain. It does **not** participate in consensus,
so it can be added or removed at any time without affecting liveness.

Follower nodes are useful for:

- Read-only RPC endpoints
- Chain explorers and indexers
- Backup and archival

## Prerequisites

- A running RBFT testnet
- The `genesis.json` from the testnet assets directory
- The `nodes.csv` file (for validator enode URLs)

## Starting a follower

Extract validator enode URLs from `nodes.csv`:

```bash
ENODES=$(awk -F',' 'NR>1{printf "%s%s",sep,$5; sep=","}' \
  ~/.rbft/testnet/assets/nodes.csv)
```

Start the follower (note: no `--validator-key`):

```bash
target/release/rbft-node node \
  --chain ~/.rbft/testnet/assets/genesis.json \
  --datadir /tmp/rbft-follower \
  --port 12345 \
  --authrpc.port 8651 \
  --http --http.port 8600 \
  --disable-discovery \
  --trusted-peers "$ENODES"
```

## Key flags

| Flag | Purpose |
|---|---|
| `--chain` | Path to shared `genesis.json` (must match the network) |
| `--datadir` | Fresh directory for the follower's database |
| `--port` | P2P listen port (must not conflict with validators; default starts at 30303) |
| `--authrpc.port` | Engine API port (must not conflict; default starts at 8551) |
| `--disable-discovery` | Prevents discv4/discv5 discovery; peers are set explicitly |
| `--trusted-peers` | Comma-separated enode URLs of validators |

The absence of `--validator-key` is what makes this a follower.

## How followers sync

Followers receive `NewBlock` messages from validators as blocks are committed.
If a follower falls behind (e.g., it was offline), it sends a `BlockRequest` to
peers and receives a `BlockResponse` containing up to 100 blocks at a time,
catching up to the chain tip.

## Testnet follower test

The Makefile includes a target that exercises follower behavior automatically:

```bash
make testnet_follower_test
```

This starts 4 validators, adds follower nodes at blocks 5 and 15, runs until
block 30, and exits.
