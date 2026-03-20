# Using Cast (Foundry)

[Cast](https://book.getfoundry.sh/cast/) is a CLI tool from Foundry for
interacting with EVM-compatible chains. It is the recommended way to send
transactions and query state on an RBFT network.

## Installation

```bash
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

Verify:

```bash
cast --version
```

## Common operations

All examples assume the default testnet RPC at `http://localhost:8545`.

### Check block height

```bash
cast bn
```

### Query an account balance

```bash
cast balance 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf
```

### Send ETH

```bash
cast send 0x1234567890123456789012345678901234567890 \
  --value 1ether \
  --private-key 0x<YOUR_KEY> \
  --rpc-url http://localhost:8545
```

### Derive an address from a private key

```bash
cast wallet address --private-key 0x<YOUR_KEY>
```

### Call a contract (read-only)

```bash
cast call 0x0000000000000000000000000000000000001001 \
  "getValidators()(address[])" \
  --rpc-url http://localhost:8545
```

### Send a contract transaction

```bash
cast send 0x0000000000000000000000000000000000001001 \
  "addValidator(address,string)" \
  0xABCD... "enode://..." \
  --private-key 0x<ADMIN_KEY> \
  --rpc-url http://localhost:8545
```
