# Troubleshooting

## Chain not producing blocks

**Symptom:** Block height does not increase.

1. Check that enough validators are running. QBFT requires `2f + 1` validators
   online for liveness (e.g., 3 out of 4).
2. Check logs for round change messages (`r` codes), which indicate the
   proposer may be offline.
3. Verify peers are connected:
   ```bash
   curl -s -X POST http://localhost:8545 \
     -H "Content-Type: application/json" \
     -d '{"jsonrpc":"2.0","method":"net_peerCount","params":[],"id":1}'
   ```
4. Ensure all nodes use the same `genesis.json`.

## Follower not syncing

**Symptom:** Follower node stays at a low block height.

1. Verify `--trusted-peers` contains the correct enode URLs.
2. Check that the follower's `--chain` genesis matches the validators.
3. Look for `NewBlock` or `BlockResponse` messages in the follower's log.
4. Ensure network ports are not blocked between follower and validators.

## Port conflicts

Each node needs unique ports for P2P, HTTP, and AuthRPC. Default allocations:

| Node | P2P | HTTP | AuthRPC |
|---|---|---|---|
| 0 | 30303 | 8545 | 10000 |
| 1 | 30304 | 8546 | 10001 |
| 2 | 30305 | 8547 | 10002 |
| 3 | 30306 | 8548 | 10003 |

If adding nodes, pick ports that don't collide.

## Permission denied on data directories

Docker testnet may create root-owned files. Clean up with:

```bash
make clean
```

Or manually:

```bash
docker run --rm -v ~/.rbft/testnet:/data busybox rm -rf /data/db /data/logs
```

## Transaction pool full

Under heavy load, the transaction pool may reject new transactions. Increase
pool limits:

```bash
target/release/rbft-node node \
  --txpool.pending-max-count 150000 \
  --txpool.basefee-max-count 150000 \
  --txpool.queued-max-count 150000 \
  ...
```

## Validator not participating after registration

New validators become active at the **next epoch boundary**, not immediately.
Wait for `epochLength` blocks (default: 32) after registration.

Check current epoch status:

```bash
target/release/rbft-utils validator status --rpc-url http://localhost:8545
```

## Collecting diagnostics

For bug reports, collect:

1. Node logs: `~/.rbft/testnet/logs/`
2. Logjam output: `target/release/rbft-utils logjam -q`
3. Validator status: `target/release/rbft-utils validator status`
4. Block height across nodes: `cast bn --rpc-url http://localhost:854{5,6,7,8}`
