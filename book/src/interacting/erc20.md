# ERC20 Tokens

RBFT includes a built-in QBFTErc20 contract that wraps native ETH balances as
ERC20 tokens.

## Testing the ERC20 contract

The Makefile provides an automated test:

```bash
make test-erc20-contract
```

This:

1. Funds the ERC20 contract with native ETH
2. Mints tokens to a test address
3. Verifies `balanceOf()` matches the native balance
4. Exits with success or failure status

## Deploying a custom ERC20

Any standard ERC20 contract can be deployed. For example, using Foundry:

```bash
forge create lib/openzeppelin-contracts/contracts/token/ERC20/ERC20.sol:ERC20 \
  --constructor-args "MyToken" "MTK" \
  --rpc-url http://localhost:8545 \
  --private-key "$RBFT_ADMIN_KEY"
```

## Interacting with ERC20 contracts

```bash
# Check balance
cast call <TOKEN_ADDRESS> \
  "balanceOf(address)(uint256)" 0x<YOUR_ADDRESS>

# Transfer tokens
cast send <TOKEN_ADDRESS> \
  "transfer(address,uint256)" 0x<RECIPIENT> 1000000000000000000 \
  --private-key "$RBFT_ADMIN_KEY" \
  --rpc-url http://localhost:8545

# Check allowance
cast call <TOKEN_ADDRESS> \
  "allowance(address,address)(uint256)" 0x<OWNER> 0x<SPENDER>

# Approve spending
cast send <TOKEN_ADDRESS> \
  "approve(address,uint256)" 0x<SPENDER> 1000000000000000000 \
  --private-key "$RBFT_ADMIN_KEY" \
  --rpc-url http://localhost:8545
```
