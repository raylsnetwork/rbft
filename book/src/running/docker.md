# Docker Deployment

RBFT nodes can be built and run as Docker containers.

## Building the image

```bash
make docker-build
```

This uses a multi-stage Dockerfile with `cargo-chef` for dependency caching.
The resulting image is tagged `rbft-node:testnet`.

## Running a Docker testnet

```bash
RBFT_DOCKER=true make testnet_start
```

This builds the image and starts validators as Docker containers using
`--network host`. Each container mounts:

- `/assets` — genesis file and keys (read-only)
- `/data` — node database
- `/logs` — log files

## Exposed ports

| Port | Service |
|---|---|
| 8545+ | JSON-RPC (HTTP) |
| 8551+ | Engine API (AuthRPC) |
| 30303+ | P2P (RLPx) |

## Manual Docker run

```bash
docker run --rm --name rbft-node-0 \
  --network host \
  -v ~/.rbft/testnet/assets:/assets:ro \
  -v ~/.rbft/testnet/db:/data \
  -v ~/.rbft/testnet/logs:/logs \
  rbft-node:testnet ./rbft-node node \
  --http --http.port 8545 --http.addr 0.0.0.0 \
  --chain /assets/genesis.json \
  --validator-key /assets/validator-key0.txt \
  --p2p-secret-key /assets/p2p-secret-key0.txt \
  --trusted-peers "enode://..." \
  --datadir /data \
  --port 30303
```

## Pushing to a registry

```bash
RBFT_REGISTRY=your-registry.example.com make docker-tag-registry
```
