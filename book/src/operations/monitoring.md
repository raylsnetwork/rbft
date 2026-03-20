# Monitoring and Logs

## Log files

By default, node logs are written to `~/.rbft/testnet/logs/`. Each node writes
to its own log file (`node0.log`, `node1.log`, etc.).

Logs are automatically rotated when they exceed 100 MB (configurable via
`RBFT_LOG_MAX_SIZE_MB`), keeping 3 rotated copies.

## Logjam — log analysis tool

Logjam merges logs from all nodes in chronological order and produces a message
delivery histogram.

### Basic usage

```bash
# View merged logs with delivery histogram
target/release/rbft-utils logjam

# Quiet mode — histogram and unreceived messages only
target/release/rbft-utils logjam -q

# Follow mode (like tail -f)
target/release/rbft-utils logjam -f

# Wire-level tracing
target/release/rbft-utils logjam --trace
```

Or via the Makefile:

```bash
make logjam
make logs    # view raw logs
```

### Interpreting the log format

Consensus state is summarized in a compact format:

```
[1 h=5 bt=1 rt=0 chain=4/8 r=0 prva=000 i=1.5:P P=234]
```

| Field | Meaning |
|---|---|
| `1` | Node index |
| `h=5` | Current height (working on block 5) |
| `bt=1` | Seconds until block timeout |
| `rt=0` | Seconds until round timeout |
| `chain=4/8` | Chain head at height 4, timestamp 8 |
| `r=0` | Current round |
| `prva=000` | Proposal/prepare/commit acceptance flags |
| `i=1.5:P` | Incoming: proposal for height 1, round 5 |
| `P=234` | Prepares received from validators 2, 3, 4 |

Message type codes:

| Code | Message |
|---|---|
| `P` | Proposal |
| `p` | Prepare |
| `c` | Commit |
| `r` | Round Change |
| `b` | NewBlock |
| `Q` | BlockRequest |
| `A` | BlockResponse |

## Validator Inspector

A terminal UI for monitoring validator state in real time:

```bash
make validator-inspector
```

## Prometheus and Grafana

The `monitoring/` directory contains Docker Compose configuration for
Prometheus and Grafana:

```bash
cd monitoring
docker compose up -d
```

This sets up:

- **Prometheus** — scrapes node metrics
- **Grafana** — dashboards for block production, latency, and resource usage

## Memory monitoring

The testnet command tracks RSS memory per node and reports it periodically.
Watch for memory growth that could indicate leaks in long-running tests.
