# Deploying Your Own Contracts

RBFT chains are fully EVM-compatible. Any contract that works on Ethereum can be
deployed to an RBFT network using standard tools.

## Using Foundry (forge)

Create a project and deploy:

```bash
forge init my-contract
cd my-contract

# Edit src/Counter.sol, then deploy:
forge create src/Counter.sol:Counter \
  --rpc-url http://localhost:8545 \
  --private-key 0x<YOUR_KEY>
```

## Using cast with raw bytecode

```bash
cast send --create <BYTECODE> \
  --private-key 0x<YOUR_KEY> \
  --rpc-url http://localhost:8545
```

## Using Hardhat

In `hardhat.config.js`:

```javascript
module.exports = {
  networks: {
    rbft: {
      url: "http://localhost:8545",
      accounts: ["0x<YOUR_PRIVATE_KEY>"]
    }
  }
};
```

Deploy:

```bash
npx hardhat run scripts/deploy.js --network rbft
```

## Getting test funds

On a default local testnet, the admin account
(`0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf`, derived from key `0x000...0001`)
is pre-funded with ETH. Transfer from it to fund your deployment account:

```bash
cast send 0x<YOUR_ADDRESS> \
  --value 100ether \
  --private-key 0x0000000000000000000000000000000000000000000000000000000000000001 \
  --rpc-url http://localhost:8545
```

## Chain ID

The chain ID is set in `genesis.json`. Check it with:

```bash
cast chain-id --rpc-url http://localhost:8545
```
