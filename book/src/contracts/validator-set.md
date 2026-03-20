# QBFTValidatorSet Contract

The QBFTValidatorSet contract manages the validator set on-chain. It is deployed
at genesis and lives at a fixed address.

## Contract details

| Property | Value |
|---|---|
| Address | `0x0000000000000000000000000000000000001001` |
| Pattern | UUPS upgradeable proxy |
| Source | `contracts/QBFTValidatorSet.sol` |

## Roles

| Role | Permissions |
|---|---|
| `DEFAULT_ADMIN_ROLE` | Grant/revoke roles, upgrade contract |
| `VALIDATOR_MANAGER_ROLE` | Add/remove validators, update parameters |
| `UPGRADER_ROLE` | Upgrade contract implementation |

The admin key used at genesis holds all roles by default.

## Key functions

### Read-only

| Function | Description |
|---|---|
| `getValidators()` | Returns the current validator address list |
| `isValidator(address)` | Checks if an address is a validator |
| `maxActiveValidators()` | Returns the maximum validator count |
| `baseFee()` | Returns the current base fee |
| `blockIntervalMs()` | Returns the block interval in milliseconds |
| `epochLength()` | Returns the epoch length in blocks |

### State-changing (admin only)

| Function | Description |
|---|---|
| `addValidator(address, enode)` | Add a validator |
| `removeValidator(address)` | Remove a validator |
| `setMaxActiveValidators(uint256)` | Update max validators |
| `setBaseFee(uint256)` | Update base fee |
| `setBlockIntervalMs(uint256)` | Update block interval |
| `setEpochLength(uint256)` | Update epoch length |

## Epochs

Validator set changes are batched per epoch. When a validator is added or
removed, the change is recorded immediately but only takes effect when the
current epoch ends (every `epochLength` blocks). This prevents validator
churn from disrupting consensus mid-epoch.

## Querying with rbft-utils

```bash
target/release/rbft-utils validator status --rpc-url http://localhost:8545
```

## Querying with cast

```bash
# List validators
cast call 0x0000000000000000000000000000000000001001 \
  "getValidators()(address[])"

# Check if an address is a validator
cast call 0x0000000000000000000000000000000000001001 \
  "isValidator(address)(bool)" 0xABCD...
```
