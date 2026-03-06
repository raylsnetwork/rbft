# Copilot Instructions — rbft (RBFT QBFT Node)

## Architecture Overview

This is a **QBFT Byzantine Fault Tolerant consensus** implementation for the RBFT EVM network,
built as a custom consensus plugin on top of [reth](https://github.com/paradigmxyz/reth) v1.10.1.

### Crate Responsibilities

| Crate | Role |
|---|---|
| `crates/rbft` | Pure QBFT state machine — a near-1:1 translation of `doc/qbft-spec/dafny/` |
| `crates/rbft-node` | Reth node binary; plugs `rbft` into reth via the Engine API and RLPx |
| `crates/rbft-utils` | CLI tooling: genesis generation, testnet orchestration, `logjam` log analyser |
| `crates/rbft-megatx` | Transaction spam tool for load testing |
| `crates/rbft-validator-inspector` | TUI for monitoring a running testnet |

### Data Flow

```
rbft::NodeState  ──(QbftMessages)──►  rbft_consensus.rs  ──(Engine API)──►  reth
     ▲                                        │
     │                                        ▼
     └─── node_auxilliary_functions.rs   rbft_protocol.rs  ◄──►  RLPx peers
```

- `rbft::node` (`node.rs`) implements `upon_*` transition functions from the Dafny spec.
- `rbft_consensus.rs` is the integration layer: it drives the state machine, calls
  `new_payload` / `fork_choice_updated` on reth's Engine API, and relays QBFT messages over RLPx.
- `rbft_consensus/on_chain_config.rs` reads the live validator set from the on-chain
  `QBFTValidatorSet` contract (storage slot layout matters — don't change it without updating
  `genesis.rs`).
- Validator selection at each epoch: the block hash is used as a random seed to deterministically
  select a subset when `max_validators < total_validators`.

## Critical Developer Workflows

### Never compile directly — always use Makefile targets

The node requires a genesis file and pre-built assets before it can run.

```bash
make testnet_load_test          # Full end-to-end build + run + TPS check (CI gate)
make testnet_start              # Start 4-node local testnet (persistent)
make test                       # cargo test --all
make fmt                        # cargo fmt --all
make clippy                     # cargo clippy -D warnings
```

`rustfmt.toml` enables unstable options, so a nightly toolchain must be the
active default (or set via `rustup override`) for `cargo fmt` to succeed.

### Pre-commit checklist (also required before pushing)

```bash
cargo fmt --all
python scripts/check_line_length.py   # max 100 chars; fails CI if violated
cargo test
cargo clippy --all-targets --all-features -- -D warnings
make testnet_load_test
```

### Testnet assets location

- Assets (keys, genesis, enodes): `~/.rbft/testnet/assets/`
- Logs: `~/.rbft/testnet/logs/node*.log`
- DB: `~/.rbft/testnet/db/`

Use `make logjam` (or `RBFT_LOGJAM_QUIET=1 RBFT_LOGJAM_FOLLOW=1 make logjam`) to analyse logs in
chronological order with a message-delivery histogram.

### Key environment variables

| Variable | Default | Purpose |
|---|---|---|
| `RBFT_NUM_NODES` | 4 | Nodes in testnet/genesis |
| `RBFT_BLOCK_INTERVAL` | 0.5 | Block interval (seconds, float) |
| `RBFT_BASE_FEE` | 4761904761905 | Genesis base fee (wei) |
| `RBFT_EXIT_AFTER_BLOCK` | — | Auto-exit testnet at this height |
| `RBFT_RESEND_AFTER` | unset (disabled) | Resend cached messages after N seconds stall; **`0` is not disabled** — it causes continuous resends |
| `RBFT_FULL_LOGS` | false | Log on every advance cycle |

## Project-Specific Conventions

- **`node_auxilliary_functions.rs` must mirror the Dafny spec**: variable names use `snake_case`
  but otherwise stay as close as possible to `doc/qbft-spec/dafny/`. Do not refactor for style.
- **Sets are `Vec`/slices, not `HashSet`**: deliberately matches the spec; don't change without
  careful thought about ordering semantics.
- **Custom timestamp validation**: `consensus_builder.rs` overrides reth's timestamp check to
  allow equal timestamps between parent/child (needed for sub-second blocks).
- **RLPx sub-protocol**: QBFT messages are sent over a custom RLPx capability defined in
  `rbft_protocol.rs`, not over the standard Eth wire protocol.
- **On-chain config is authoritative**: validator set, block interval, and epoch length are read
  from the `QBFTValidatorSet` contract at each epoch boundary — not from config files.
- **`RbftConfig`** (in `rbft-utils/src/types.rs`) is embedded in `genesis.json` under the
  `rbft` key and governs message-buffer sizes, reconnect backoff, and relay behaviour.

## `rbft_consensus` Sub-modules

`crates/rbft-node/src/rbft_consensus/` contains the reth integration layer, split across:

| File | Purpose |
|---|---|
| `rbft_consensus.rs` | Main consensus loop: drives `NodeState`, calls `new_payload` / `fork_choice_updated`, relays messages |
| `rbft_protocol.rs` | Custom RLPx capability (`RbftProtocolHandler`); manages per-peer `Connection` state, message cache, idle-timeout disconnects |
| `on_chain_config.rs` | Reads `QBFTValidatorSet` contract storage at epoch boundaries; resolves enode hostnames via DNS |
| `consensus_builder.rs` | `RbftBeaconConsensus` wrapping `EthBeaconConsensus`; overrides `validate_header_against_parent` to allow `child.timestamp == parent.timestamp` |
| `payload_validator.rs` | `RbftPayloadValidator` wrapping `EthereumExecutionPayloadValidator`; delegates `ensure_well_formed_payload` then calls `try_recover` |
| `validator_builder.rs` | `RbftPayloadValidatorBuilder` — wires `RbftPayloadValidator` into reth's RPC add-ons via `PayloadValidatorBuilder` |
| `aligned_interval.rs` | Timer that fires on wall-clock-aligned boundaries (keeps block cadence in sync across nodes) |

`RbftNode` (in `rbft_node.rs`) assembles these into reth's `ComponentsBuilder` using
`RbftConsensusBuilder` and `RbftPayloadValidatorBuilder`.

## Metrics & Health

`crates/rbft-node/src/metrics.rs` exposes a `warp` HTTP server whose port is derived
from the main HTTP port: `metrics_port = 9000 + (http_port - 8545)` (e.g. `8545` → port `9000`):

- `GET /metrics` — Prometheus text format (`qbft_block_height`, `qbft_round`, `qbft_is_proposer`)
- `GET /health` — JSON `HealthStatus`; returns `503` when no block has been committed within
  `max(expected_block_interval_ms × 5, 30s)` where `expected_block_interval_ms` defaults to
  `1000ms` (it is not currently updated from the on-chain `block_interval`)

`QbftMetrics` is `Arc`-shared across the consensus loop and the HTTP server. Update it via
`record_consensus_state(height, round, is_proposer)` on every committed block.

## Key Files

- `crates/rbft/src/node.rs` — QBFT `upon_*` state transition functions
- `crates/rbft/src/node_auxilliary_functions.rs` — cryptographic primitives, quorum logic
- `crates/rbft-node/src/rbft_consensus.rs` — main consensus loop (2500+ lines)
- `crates/rbft-node/src/rbft_consensus/rbft_protocol.rs` — RLPx wire protocol
- `crates/rbft-node/src/rbft_consensus/on_chain_config.rs` — live validator set reader
- `crates/rbft-node/src/metrics.rs` — Prometheus + health endpoint
- `crates/rbft-utils/src/genesis.rs` — genesis + contract deployment (uses `revm` inline)
- `contracts/QBFTValidatorSet.sol` — on-chain validator registry
