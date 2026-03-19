# Validator Management

Validators can be added or removed dynamically through the QBFTValidatorSet
contract. Changes take effect at the next epoch boundary.

## Generating validator keys

```bash
target/release/rbft-utils validator keygen --ip <YOUR_IP> --port 30305
```

This outputs JSON with all required values:

```json
{
  "validator_address":     "0xABCD...",
  "validator_private_key": "0x1234...",
  "p2p_secret_key":        "abcd...",
  "enode":                 "enode://<pubkey>@<YOUR_IP>:30305"
}
```

Save the keys to files:

```bash
echo "0x1234..." > validator-key-new.txt
echo "abcd..."   > p2p-secret-key-new.txt
```

## Starting a new validator node

```bash
target/release/rbft-node node \
  --chain ~/.rbft/testnet/assets/genesis.json \
  --datadir /tmp/rbft-new-validator \
  --port 30305 \
  --authrpc.port 8652 \
  --http --http.port 8601 \
  --disable-discovery \
  --p2p-secret-key p2p-secret-key-new.txt \
  --validator-key validator-key-new.txt \
  --trusted-peers "$ENODES"
```

The node will sync but not participate in consensus until registered in the
contract.

## Registering a validator

The admin key must match the one used at genesis. For a default local testnet,
it is `0x000...0001` (set via `RBFT_ADMIN_KEY`).

```bash
target/release/rbft-utils validator add \
  --validator-address 0xABCD... \
  --enode "$ENODE" \
  --rpc-url http://localhost:8545
```

To specify the admin key explicitly:

```bash
target/release/rbft-utils validator add \
  --admin-key <ADMIN_PRIVATE_KEY> \
  --validator-address 0xABCD... \
  --enode "$ENODE" \
  --rpc-url http://localhost:8545
```

The validator becomes active at the next epoch boundary.

## Removing a validator

```bash
target/release/rbft-utils validator remove \
  --validator-address 0xABCD... \
  --rpc-url http://localhost:8545
```

## Checking validator status

```bash
target/release/rbft-utils validator status --rpc-url http://localhost:8545
```

Or via the Makefile:

```bash
make validator_status
```

## Updating chain parameters

The admin can update chain parameters through the contract:

```bash
# Set maximum active validators
target/release/rbft-utils validator set-max-active-validators \
  --value 10 --rpc-url http://localhost:8545

# Set block interval (milliseconds)
target/release/rbft-utils validator set-block-interval-ms \
  --value 1000 --rpc-url http://localhost:8545

# Set epoch length (blocks)
target/release/rbft-utils validator set-epoch-length \
  --value 64 --rpc-url http://localhost:8545

# Set base fee (wei)
target/release/rbft-utils validator set-base-fee \
  --value 1000000000 --rpc-url http://localhost:8545
```
