# Sending Transactions

## Setup

For local testing, use the pre-funded admin key:

```bash
export RBFT_ADMIN_KEY=0x0000000000000000000000000000000000000000000000000000000000000001
```

Derive the corresponding address:

```bash
cast wallet address --private-key "$RBFT_ADMIN_KEY"
# 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf
```

## Sending ETH

Using cast:

```bash
cast send 0x1234567890123456789012345678901234567890 \
  --value 1ether \
  --private-key "$RBFT_ADMIN_KEY" \
  --rpc-url http://localhost:8545
```

Using curl:

```bash
# First sign the transaction, then send the raw bytes via eth_sendRawTransaction
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "eth_sendRawTransaction",
    "params": ["0x<SIGNED_TX_HEX>"],
    "id": 1
  }'
```

## Checking balances

```bash
cast balance 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf
```

Or with curl:

```bash
curl -s -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "eth_getBalance",
    "params": ["0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf", "latest"],
    "id": 1
  }'
```

## Transaction finality

RBFT provides instant finality. Once a transaction is included in a committed
block, it is final — there are no reorgs or uncle blocks. A single block
confirmation is sufficient.
