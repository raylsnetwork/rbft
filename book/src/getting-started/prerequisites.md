# Prerequisites

## Required

- **Rust 1.93+** with nightly toolchain (for formatting)
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  rustup toolchain install nightly
  ```
- **Git**
- **Make**
- **Linux or macOS** (Windows via WSL)

## Optional

- **Foundry** — for interacting with deployed contracts via `cast`
  ```bash
  curl -L https://foundry.paradigm.xyz | bash
  foundryup
  ```
- **Docker** — for containerized deployments
- **kubectl** — for Kubernetes deployments
