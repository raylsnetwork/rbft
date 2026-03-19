# Installation

Clone the repository and build in release mode:

```bash
git clone https://github.com/raylsnetwork/rbft.git
cd rbft
cargo build --release
```

This produces three main binaries in `target/release/`:

| Binary | Purpose |
|---|---|
| `rbft-node` | Run a validator or follower node |
| `rbft-utils` | Genesis generation, testnet management, validator operations |
| `rbft-megatx` | Transaction load testing |

Verify the build:

```bash
target/release/rbft-node --version
target/release/rbft-utils --help
```
