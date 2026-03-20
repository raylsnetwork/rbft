# Architecture Overview

RBFT is built as a layered system on top of Reth, the Rust Ethereum client.

```
┌─────────────────┐
│  RPC Interface  │  HTTP JSON-RPC (ports 8545+)
└────────┬────────┘
         │
┌────────▼────────┐
│  Reth Engine    │  EVM execution, state management, transaction pool
└────────┬────────┘
         │
┌────────▼────────┐
│ RBFT Consensus  │  QBFT protocol, proposer rotation, validator management
└────────┬────────┘
         │
┌────────▼────────┐
│  RLPx Network   │  P2P messaging between validators and followers
└─────────────────┘
```

## Crate structure

### `rbft` (core library)

The consensus protocol implementation with no Reth dependencies. Contains:

- QBFT state machine (`NodeState`)
- Message types (Proposal, Prepare, Commit, RoundChange, NewBlock)
- Block validation and proposer election
- Round change and timeout logic

This crate is designed to be testable in isolation using an in-memory
`NodeSwarm` simulation.

### `rbft-node` (binary)

Integrates the core library with Reth:

- Custom consensus engine (`RbftConsensus`)
- RLPx protocol handler for QBFT messages
- Block building and execution via Reth
- P2P connection management

### `rbft-utils` (binary)

Operator tooling:

- Genesis generation (compiles and deploys QBFTValidatorSet contract)
- Testnet orchestration (start/stop/monitor nodes)
- Validator management (add/remove/status via contract calls)
- Log analysis (logjam)

### `rbft-megatx` (binary)

Transaction load generator for stress testing.

### `rbft-validator-inspector` (binary)

Terminal UI for real-time validator monitoring.

## Node types

| Type | Has validator key | Participates in consensus | Produces blocks |
|---|---|---|---|
| Validator (proposer) | Yes | Yes | Yes (when elected) |
| Validator (non-proposer) | Yes | Yes | No |
| Follower | No | No | No |

## Data flow

1. Transactions arrive via JSON-RPC
2. The current proposer builds a block and broadcasts a `Proposal`
3. Validators verify and send `Prepare` messages
4. On quorum of prepares, validators send `Commit` messages
5. On quorum of commits, the block is finalized
6. A `NewBlock` message is broadcast so followers can update

## Fault tolerance

With `n` validators, RBFT tolerates `f = (n-1) / 3` Byzantine faults.
The quorum size is `(2n - 1) / 3 + 1`.

| Validators | Max faults | Quorum |
|---|---|---|
| 4 | 1 | 3 |
| 7 | 2 | 5 |
| 10 | 3 | 7 |
