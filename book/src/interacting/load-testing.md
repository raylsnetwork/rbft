# Load Testing

RBFT includes `rbft-megatx`, a tool for generating high transaction load to
stress-test the network.

## Quick start

```bash
make testnet_load_test
```

This starts a 4-validator testnet with `rbft-megatx` submitting large
transactions every block, monitoring the transaction pool.

## Running megatx manually

Against a running testnet:

```bash
target/release/rbft-megatx spam \
  --num-txs 20000 \
  --target-tps 1000 \
  --max-wait-seconds 30
```

| Flag | Default | Description |
|---|---|---|
| `--num-txs` | `20000` | Transactions per batch |
| `--target-tps` | `0` (unlimited) | Target transactions per second |
| `--max-wait-seconds` | `30` | Max wait for pending txs to clear |

## Via the Makefile

```bash
make megatx
```

Defaults to 100,000 transactions.

## Monitoring the transaction pool

When running with `--monitor-txpool`, the testnet command displays real-time
transaction pool sizes for each node, color-coded by congestion level.

```bash
target/release/rbft-utils testnet --init --monitor-txpool --run-megatx \
  --assets-dir ~/.rbft/testnet/assets
```

## Tuning for high throughput

For maximum throughput, increase pool limits:

```bash
make testnet_load_test
```

This automatically sets:

- `--txpool.max-tx-input-bytes 10000000`
- `--builder.gaslimit 60000000`
- `--tx-propagation-mode all`
- Large pending, basefee, and queued pool sizes
