# Local Testnet

The quickest way to start an RBFT chain is with `make testnet_start`, which
generates keys, creates a genesis file, and launches a 4-validator network.

## Quick start

```bash
make testnet_start
```

This will:

1. Generate validator keys and P2P secret keys
2. Create a `genesis.json` with the QBFTValidatorSet contract
3. Start 4 validator nodes on HTTP ports 8545–8548
4. Begin producing blocks (default interval: 500 ms)

Logs are written to `~/.rbft/testnet/logs/` and chain data to `~/.rbft/testnet/db/`.

## Customizing the testnet

Use environment variables to change defaults:

```bash
RBFT_NUM_NODES=7 RBFT_BLOCK_INTERVAL=1.0 make testnet_start
```

| Variable | Default | Description |
|---|---|---|
| `RBFT_NUM_NODES` | `4` | Number of validators (minimum 4) |
| `RBFT_BLOCK_INTERVAL` | `0.5` | Block time in seconds |
| `RBFT_GAS_LIMIT` | `600000000` | Block gas limit |
| `RBFT_EPOCH_LENGTH` | `32` | Blocks per epoch |
| `RBFT_BASE_FEE` | `4761904761905` | Base fee per gas (wei) |
| `RBFT_EXIT_AFTER_BLOCK` | — | Stop the testnet at this block height |
| `RBFT_MAX_ACTIVE_VALIDATORS` | — | Cap on active validators |

## Step-by-step setup

If you prefer to run each step manually:

### 1. Generate node keys

```bash
target/release/rbft-utils node-gen --assets-dir ~/.rbft/testnet/assets
```

This creates `nodes.csv` containing validator addresses, private keys, P2P keys,
and enode URLs.

### 2. Generate genesis

```bash
target/release/rbft-utils genesis --assets-dir ~/.rbft/testnet/assets
```

Compiles the QBFTValidatorSet contract, deploys it into the genesis state, and
writes `genesis.json`.

### 3. Start the testnet

```bash
target/release/rbft-utils testnet --init --assets-dir ~/.rbft/testnet/assets
```

The `--init` flag clears any previous data directories before starting.

## Restarting

To restart with the same keys and genesis (preserving chain state):

```bash
make testnet_restart
```

To start fresh, use `make testnet_start` again — it regenerates everything.

## Debug mode

For verbose logging:

```bash
make testnet_debug
```

This sets `RUST_LOG=debug` and builds in debug profile.

## Verifying block production

In a separate terminal:

```bash
# Using cast (Foundry)
cast bn

# Using curl
curl -s -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

Block height should increase steadily.
