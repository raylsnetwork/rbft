# Configuration Reference

## Node flags (rbft-node)

These flags are specific to RBFT. The node also accepts all standard
[Reth CLI flags](https://paradigmxyz.github.io/reth/cli/reth/).

| Flag | Env Variable | Default | Description |
|---|---|---|---|
| `--validator-key` | `RBFT_VALIDATOR_KEY` | — | Validator private key file (omit for follower) |
| `--logs-dir` | `RBFT_LOGS_DIR` | `~/.rbft/testnet/logs` | Log directory |
| `--db-dir` | `RBFT_DB_DIR` | `~/.rbft/testnet/db` | Database directory |
| `--trusted-peers-refresh-secs` | `RBFT_TRUSTED_PEERS_REFRESH_SECS` | `10` | DNS peer refresh interval |
| `--full-logs` | `RBFT_FULL_LOGS` | `false` | Emit logs on every consensus cycle |
| `--resend-after-secs` | `RBFT_RESEND_AFTER` | — | Resend messages after N seconds without commits |
| `--disable-express` | `RBFT_DISABLE_EXPRESS` | `false` | Disable express transaction delivery |

## Testnet environment variables

| Variable | Default | Description |
|---|---|---|
| `RBFT_NUM_NODES` | `4` | Number of validators |
| `RBFT_GAS_LIMIT` | `600000000` | Block gas limit |
| `RBFT_BLOCK_INTERVAL` | `0.5` | Block time (seconds) |
| `RBFT_EPOCH_LENGTH` | `32` | Blocks per epoch |
| `RBFT_BASE_FEE` | `4761904761905` | Base fee per gas (wei) |
| `RBFT_MAX_ACTIVE_VALIDATORS` | — | Maximum active validators |
| `RBFT_EXIT_AFTER_BLOCK` | — | Stop at this block height |
| `RBFT_ADD_AT_BLOCKS` | — | Add validators at blocks (CSV, e.g., `10,20`) |
| `RBFT_ADD_FOLLOWER_AT` | — | Add followers at blocks (CSV) |
| `RBFT_ADMIN_KEY` | `0x000...0001` | Admin private key |
| `RBFT_DOCKER` | — | Use Docker containers |
| `RBFT_KUBE` | — | Use Kubernetes |
| `RBFT_KUBE_NAMESPACE` | `rbft` | Kubernetes namespace |
| `RBFT_REGISTRY` | — | Container registry for Docker push |
| `RBFT_USE_TRUSTED_PEERS` | — | Write `enodes.txt` for static peers |

## Log management

| Variable | Default | Description |
|---|---|---|
| `RBFT_LOG_MAX_SIZE_MB` | `100` | Max log file size before rotation (0 = disabled) |
| `RBFT_LOG_KEEP_ROTATED` | `3` | Number of rotated log files to keep |
| `RBFT_LOGJAM_QUIET` | — | Logjam quiet mode (histogram only) |
| `RBFT_LOGJAM_FOLLOW` | — | Logjam follow mode (like `tail -f`) |
| `RBFT_LOGJAM_MAX_MESSAGE_DELAY` | `1000` | Max delay before reporting (ms) |

## Genesis parameters

These are set at chain creation and encoded in `genesis.json`:

| Parameter | Default | Set via |
|---|---|---|
| Gas limit | `600000000` | `--gas-limit` or `RBFT_GAS_LIMIT` |
| Block interval | `500ms` | `--block-interval` or `RBFT_BLOCK_INTERVAL` |
| Epoch length | `32` | `--epoch-length` or `RBFT_EPOCH_LENGTH` |
| Base fee | `4761904761905` | `--base-fee` or `RBFT_BASE_FEE` |
| Max validators | unlimited | `--max-active-validators` |
| Validator contract | `0x...1001` | `--validator-contract-address` |
