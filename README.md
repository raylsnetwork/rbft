# RBFT - Rayls Byzantine Fault Tolerant Consensus

[![CI/CD](https://github.com/raylsnetwork/rbft/actions/workflows/ci.yml/badge.svg)](https://github.com/raylsnetwork/rbft/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE.md)
[![Rust](https://img.shields.io/badge/rust-1.93%2B-orange.svg)](https://www.rust-lang.org/)

A high-performance Byzantine Fault Tolerant (BFT) consensus implementation based on QBFT, built on top of Reth. This implementation includes a native ETH-wrapping ERC20 contract and comprehensive testing infrastructure.

## Features

- **QBFT Consensus**: Standards-compliant QBFT implementation with RLPx peer-to-peer networking
- **Reth Integration**: Built on Reth for high-performance block execution
- **Native ERC20**: QBFTErc20 contract that wraps native ETH balances
- **Dynamic Validators**: Add/remove validators through smart contract interactions
- **Comprehensive Testing**: Automated testnet with transaction load testing and contract verification

## Quick Start

### Prerequisites

- Rust 1.93 or later (with nightly for formatting)
- Git
- Make
- Linux or macOS (Windows via WSL)

### Installation

1. Clone the repository:
```bash
git clone https://github.com/raylsnetwork/rbft.git
cd rbft
```

2. Build the project:
```bash
cargo build --release
```

### Running a Local Testnet

Start a 4-node testnet with a single command:

```bash
make testnet_start
```

This will:
- Generate validator keys and genesis configuration
- Start 4 validator nodes (HTTP ports 8545-8548)
- Deploy the QBFTValidatorSet contract
- Begin producing blocks every 500ms

The nodes will log to `~/.rbft/testnet/logs/` and store data in `~/.rbft/testnet/db/`.

### Running a Follower Node

A follower node connects to an existing validator network, receives blocks, and keeps a
full copy of the chain. It does not participate in consensus (no `--validator-key`), so it
can be added to or removed from the network at any time without affecting liveness.

Before running a follower you need:
- A running RBFT testnet (e.g. started with `make testnet_start`)
- The `genesis.json` and `nodes.csv` from the testnet assets directory
  (default: `~/.rbft/testnet/assets/`)

Extract the validator enode URLs from `nodes.csv` (column 5):

```bash
ENODES=$(awk -F',' 'NR>1{printf "%s%s",sep,$5; sep=","}' ~/.rbft/testnet/assets/nodes.csv)
```

Then start the follower:

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

Key flags:
- `--chain` — path to the shared `genesis.json` (must match the running network)
- `--datadir` — a fresh directory for the follower's database; must not be shared with
  a validator
- `--port` — P2P listen port; pick one that does not conflict with the validator nodes
  (default validator ports start at 30303)
- `--authrpc.port` — engine API port; also must not conflict (validator default: 8551+)
- `--disable-discovery` — prevents unwanted discv4/discv5 discovery; peers are supplied
  explicitly via `--trusted-peers`
- `--trusted-peers` — comma-separated enode URLs of the validators to connect to
- `--validator-key` is intentionally **omitted** — its absence is what makes this a
  follower

### Installing Cast (Foundry)

Cast is a useful tool for interacting with the blockchain:

```bash
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

Verify installation:
```bash
cast --version
```

### Interacting with the Testnet

The testnet command will show the block heights of the nodes in the validator set
and should increase at a rate of around two blocks a second.

Switch to another terminal to interact with the chain via commands.

#### Check Current Block Height

Using cast
```bash
cast bn
```

Using curl:
```bash
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

#### Submit a Transaction

For local testing, provide a dev-only key via environment variable:

```bash
export RBFT_ADMIN_KEY=0x<dev-private-key>
```

To derive the corresponding address:

```bash
cast wallet address --private-key "$RBFT_ADMIN_KEY"
```

Send ETH using cast:
```bash
cast send 0x1234567890123456789012345678901234567890 \
  --value 1ether \
  --private-key "$RBFT_ADMIN_KEY" \
  --rpc-url http://localhost:8545
```

#### Check Account Balance

```bash
cast balance 0x<address-derived-from-RBFT_ADMIN_KEY>
```

Or with curl:
```bash
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "eth_getBalance",
    "params": ["0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf", "latest"],
    "id": 1
  }'
```

## Available Make Targets

Run `make help` to see all available commands:

- `testnet_start` - Start a local testnet (default: 4 nodes)
- `testnet_load_test` - Start testnet with transaction load testing and auto-exit
- `test-erc20-contract` - Test ERC20 contract (funds, mints, verifies balanceOf)
- `genesis` - Generate a genesis file
- `validator_status` - Display QBFTValidatorSet contract status
- `add-validator` - Add a test validator to the contract

## Smart Contracts

### QBFTValidatorSet

Manages the validator set with features:
- Add/remove validators (admin only)
- Query active validators
- Epoch-based validator set updates

Contract address: `0x0000000000000000000000000000000000001001`

## Testing

### Run Unit Tests

```bash
cargo test
```

### Run Load Tests

```bash
make testnet_load_test
```

This starts a testnet with automated transaction generation and exits after reaching a target block height.

### Test ERC20 Contract

```bash
make test-erc20-contract
```

This runs automated tests that:
1. Fund the ERC20 contract
2. Mint tokens to a test address
3. Verify `balanceOf()` matches native balance
4. Exit with success/failure status

## Code Quality

- Rust formatting is enforced with `cargo +nightly fmt` (100 column width via `rustfmt.toml`)
- CI runs `scripts/check_line_length.py` to cap code/config lines at 100 characters
- Pre-commit hooks available for local validation:
  ```bash
  pip install pre-commit
  pre-commit install
  ```

## Architecture

```
┌─────────────────┐
│  RPC Interface  │ (HTTP on ports 8545+)
└────────┬────────┘
         │
┌────────▼────────┐
│  Reth Engine    │ (Block execution, state management)
└────────┬────────┘
         │
┌────────▼────────┐
│ RBFT Consensus  │ (QBFT protocol, validator rotation)
└────────┬────────┘
         │
┌────────▼────────┐
│  RLPx Network   │ (P2P messaging between validators)
└─────────────────┘
```

## Environment Variables

Configure the testnet with environment variables:

- `RBFT_NUM_NODES` - Number of validators (default: 4)
- `RBFT_EXIT_AFTER_BLOCK` - Exit testnet after reaching this block
- `RBFT_TEST_ERC20` - Enable ERC20 contract testing at block 10
- `RBFT_RUN_MEGATX` - Run transaction load generator
- `RBFT_ADD_AT_BLOCKS` - Add validators at specific blocks (e.g., "10,20,30")
- `RBFT_REGISTRY` - Container registry for Docker images (required for `make docker-push`)

Example:
```bash
RBFT_NUM_NODES=7 RBFT_EXIT_AFTER_BLOCK=100 make testnet_start
```

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on:
- Code style and formatting
- Testing requirements
- Pull request process
- Development workflow

## License

This project is licensed under the Apache License, Version 2.0.
See [LICENSE.md](LICENSE.md) for details.

### Third-Party Licenses

This project incorporates or derives from the following third-party works:

| Dependency | License | Copyright |
|---|---|---|
| [reth](https://github.com/paradigmxyz/reth) | Apache-2.0 OR MIT | Copyright (c) 2022–2024 Paradigm |
| [OpenZeppelin Contracts](https://github.com/OpenZeppelin/openzeppelin-contracts) | MIT | Copyright (c) 2016–2024 OpenZeppelin |

Full license texts for these dependencies are available in their respective
repositories or in the `target/forge_dependencies/` directory.

## Resources

- [IBFT Paper — The Istanbul BFT Consensus Algorithm](https://arxiv.org/abs/2002.03613)
- [QBFT Formal Specification (ConsenSys)](https://github.com/ConsenSys/qbft-formal-spec-and-verification)
- [Monitoring Setup](monitoring/)

## Support

- Open an issue on [GitHub](https://github.com/raylsnetwork/rbft/issues)
- Read the [documentation](doc/)

---

**Note**: This is a development build. Do not use in production without thorough security auditing.
