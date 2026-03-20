# ERC20 Tokens

RBFT chains are fully EVM-compatible, so any standard ERC20 token contract can
be deployed and used.

## Deploying an ERC20 token

Using Foundry with OpenZeppelin contracts:

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
