# QBFT Consensus

RBFT implements the QBFT (Quorum Byzantine Fault Tolerant) consensus protocol,
a practical BFT algorithm designed for permissioned blockchain networks.

## References

- [The Istanbul BFT Consensus Algorithm (IBFT paper)](https://arxiv.org/abs/2002.03613)
- [QBFT Formal Specification (ConsenSys)](https://github.com/ConsenSys/qbft-formal-spec-and-verification)

## Protocol overview

QBFT operates in rounds within each block height. Each round has a designated
proposer determined by round-robin rotation.

### Normal case (round 0)

1. **Block timeout** — the proposer waits for the block interval, then
   broadcasts a `Proposal` containing the new block.
2. **Prepare** — validators verify the proposal and broadcast a `Prepare`
   message.
3. **Commit** — once a validator receives a quorum of prepares, it broadcasts
   a `Commit` message with a commit seal.
4. **Finalize** — once a quorum of commits is collected, the block is added
   to the chain with the commit seals embedded in the header.
5. **NewBlock** — the committed block is broadcast so all nodes (including
   followers) can update their chains.

### Round change

If the proposer fails to propose within the timeout, validators trigger a
round change:

1. Each validator sends a `RoundChange` message for the next round.
2. When a quorum of round changes is collected, the new round's proposer
   creates a proposal with a justification (the round change messages).
3. The protocol continues from step 2 (prepare phase).

### Timeout schedule

Round timeouts grow exponentially to avoid thrashing:

```
timeout(r) = first_interval * growth_factor^r
```

Default values:

| Parameter | Default |
|---|---|
| `first_interval` | 1.0 s |
| `growth_factor` | 2.0 |
| `max_round` | 10 |

## Proposer election

The proposer for a given round is determined by:

```
proposer_index = (height + round) % num_validators
```

This ensures fair rotation across validators and rounds.

## Quorum and fault tolerance

For `n` validators:

- Maximum Byzantine faults tolerated: `f = ⌊(n-1)/3⌋`
- Quorum size: `q = ⌈(2n+1)/3⌉`

The protocol guarantees safety (no conflicting blocks are finalized) as long as
at most `f` validators are faulty. It guarantees liveness as long as at least
`q` validators are honest and connected.

## Validator sets and epochs

The validator set can change dynamically through the QBFTValidatorSet contract.
Changes take effect at epoch boundaries (every `epochLength` blocks) to ensure
all validators agree on the set for a given block range.
