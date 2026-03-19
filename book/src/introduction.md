# RBFT User Guide

RBFT (Rayls Byzantine Fault Tolerant) is a high-performance BFT consensus
implementation based on QBFT, built on top of [Reth](https://github.com/paradigmxyz/reth).

## What is RBFT?

RBFT provides a permissioned Ethereum-compatible blockchain with fast finality.
Blocks are finalized as soon as they are committed by the validator quorum — there
are no forks or reorganizations under normal operation.

Key features:

- **QBFT Consensus** — standards-compliant QBFT with RLPx peer-to-peer networking
- **Reth Integration** — built on Reth for high-performance EVM execution
- **Dynamic Validators** — add or remove validators at runtime through smart contract interactions
- **Follower Nodes** — non-validator nodes that sync the chain without participating in consensus
- **Comprehensive Tooling** — genesis generation, testnet management, log analysis, and load testing

## Who is this guide for?

This guide is for operators and developers who want to:

- Set up and run an RBFT chain
- Deploy and interact with smart contracts
- Manage validators and follower nodes
- Monitor and troubleshoot a running network

## Project layout

| Crate | Description |
|---|---|
| `rbft` | Core QBFT consensus protocol library |
| `rbft-node` | Node binary with Reth integration |
| `rbft-utils` | CLI for genesis generation, testnet management, and validator operations |
| `rbft-megatx` | Transaction load testing tool |
| `rbft-validator-inspector` | Terminal UI for monitoring validators |
