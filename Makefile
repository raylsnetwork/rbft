help:
	@echo "Available commands:"
	@echo ""
	@echo "RBFT Node Commands:"
	@echo "  testnet_start        - Start a local testnet (default: 4 nodes, use RBFT_NUM_NODES to change)"
	@echo "  testnet_debug        - Start a local testnet in debug mode"
	@echo "  testnet_restart      - Restart the testnet"
	@echo "  testnet_load_test    - Start testnet with transaction load testing and auto-exit"
	@echo "  testnet_follower_test - Start testnet and add follower nodes at blocks 5 and 15, exit at block 30"
	@echo "  testnet-follower     - Run a follower node against a running testnet (uses enodes from nodes.csv)"
	@echo "  genesis              - Generate a genesis file"
	@echo "  node-gen             - Generate node enodes and secret keys in CSV format"
	@echo "  node-help            - Show help for the rbft-node binary"
	@echo "  megatx               - Run transaction spam tool against local node"
	@echo "  validator-inspector  - Run the validator inspector TUI (monitors running testnet)"
	@echo "  validator_status     - Display QBFTValidatorSet contract status in JSON format"
	@echo "                         Usage: make validator_status [RPC_URL=http://localhost:8545]"
	@echo "  add-validator        - Add a test validator to the contract (uses constant test values)"
	@echo "                         Usage: make add-validator [RPC_URL=http://localhost:8545]"
	@echo "  status               - Shorthand for validator_status"
	@echo "  logs                 - View logs from all testnet nodes"
	@echo "  logjam               - View logs in chronological order with message delivery histogram"
	@echo "                         (see Environment Variables section for RBFT_LOGJAM_* options)"
	@echo ""
	@echo "Docker Commands:"
	@echo "  docker-validate      - Validate Dockerfile syntax"
	@echo "  docker-build         - Build production Docker image"
	@echo "  docker-build-dev     - Build development Docker image (with tools)"
	@echo "  docker-build-debug   - Build debug Docker image (unstripped)"
	@echo "  docker-run           - Run Docker container"
	@echo "  docker-run-dev       - Run development Docker container"
	@echo "  docker-test          - Test Docker image"
	@echo "  docker-clean         - Clean Docker images and containers"
	@echo ""
	@echo "CI/CD Commands:"
	@echo "  docker-tag-registry  - Tag images for Digital Ocean registry"
	@echo ""
	@echo "Development Commands:"
	@echo "  test                 - Run all tests"
	@echo "  fmt                  - Format code"
	@echo "  clippy               - Run clippy lints"
	@echo "  clean                - Clean build artifacts"
	@echo "  book                 - Build the user documentation (mdbook)"
	@echo ""
	@echo "Environment Variables:"
	@echo ""
	@echo "  Genesis Configuration (applies to 'genesis' and 'testnet' commands):"
	@echo "    RBFT_NUM_NODES               - Number of validators/nodes (default: 4)"
	@echo "    RBFT_GAS_LIMIT               - Gas limit for genesis block (default: 600000000)"
	@echo "    RBFT_BLOCK_INTERVAL          - Block interval in seconds (default: 1.0)"
	@echo "    RBFT_EPOCH_LENGTH            - Epoch length in blocks (default: 32)"
	@echo "    RBFT_BASE_FEE                - Base fee per gas in wei (default: 4761904761905)"
	@echo "    RBFT_MAX_ACTIVE_VALIDATORS   - Maximum active validators (default: num_nodes)"
	@echo "    RBFT_DOCKER                  - Generate Docker-compatible enodes (default: false)"
	@echo "    RBFT_KUBE                    - Generate Kubernetes-compatible enodes (default: false)"
	@echo ""
	@echo "  Logjam Configuration (applies to 'logjam' command):"
	@echo "    RBFT_LOGJAM_QUIET=1                      - Quiet mode: only show histogram and unreceived messages"
	@echo "    RBFT_LOGJAM_FOLLOW=1                     - Follow mode: tail logs as they grow (like tail -f)"
	@echo "    RBFT_LOGJAM_MAX_MESSAGE_DELAY=<ms>       - Max delay before reporting unreceived (default: 1000ms)"
	@echo "    RBFT_LOGJAM_HISTOGRAM_BUCKET_SIZE=<ms>   - Histogram bucket size (default: max_delay / 10)"
	@echo ""
	@echo "  Node Runtime Configuration (applies to 'testnet_start' and node execution):"
	@echo "    RBFT_RESEND_AFTER=<secs>                 - Resend cached messages after this many seconds"
	@echo "                                               without block commits (default: 0=disabled)"
	@echo "    RBFT_FULL_LOGS=1                         - Emit logs on every advance (default: false)"
	@echo "    RBFT_TRUSTED_PEERS_REFRESH_SECS=<secs>   - Peer refresh interval (default: 10)"
	@echo "    RBFT_LOG_MAX_SIZE_MB=<mb>                - Max log file size in MB before rotation (default: 100, 0=disabled)"
	@echo "    RBFT_LOG_KEEP_ROTATED=<n>                - Number of rotated log files to keep (default: 3)"
	@echo "    RBFT_EXIT_AFTER_BLOCK=<block>            - Exit testnet after reaching this block height"
	@echo "    RBFT_ADD_AT_BLOCKS=<blocks>              - Comma-separated list of block heights to add"
	@echo "                                               validators at (e.g., '10,20,30')"
	@echo "    RBFT_ADD_FOLLOWER_AT=<blocks>             - Comma-separated list of block heights to add"
	@echo "                                               follower (non-validator) nodes at (e.g., '5,15')"
	@echo "                                               Uses key slots from nodes.csv starting at index num_nodes"
	@echo ""
	@echo "  Examples:"
	@echo "    RBFT_NUM_NODES=7 make genesis"
	@echo "    RBFT_BLOCK_INTERVAL=0.5 RBFT_BASE_FEE=1000000000000 make testnet_load_test"
	@echo "    RBFT_NUM_NODES=10 RBFT_MAX_ACTIVE_VALIDATORS=5 make testnet_start"
	@echo "    RBFT_LOGJAM_QUIET=1 RBFT_LOGJAM_FOLLOW=1 make logjam"
	@echo "    RBFT_LOGJAM_HISTOGRAM_BUCKET_SIZE=100 make logjam"
	@echo ""
	@echo "ℹ️  Registry deployment is handled by GitHub Actions CI/CD pipeline"
	@echo "   Automatic: Push to main branch"
	@echo "   Manual: Workflow dispatch with environment selection"

DOCKER_IMAGE_NAME := rbft-node
DOCKER_TAG := latest
REGISTRY ?= $(error Set RBFT_REGISTRY to your container registry, e.g. ghcr.io/raylsnetwork)
ASSETS_DIR := ~/.rbft/testnet/assets
LOGS_DIR := ~/.rbft/testnet/logs
DB_DIR := ~/.rbft/testnet/db

# Cargo executable (use system cargo or fallback to ~/.cargo/bin/cargo)
CARGO ?= $(shell which cargo 2>/dev/null || echo "$(HOME)/.cargo/bin/cargo")

CARGO_PROFILE ?= release

# Treat `make -d`/`--debug` as debug mode for testnet runs.
ifneq (,$(filter -d --debug --debug=%,$(MAKEFLAGS)))
CARGO_PROFILE := debug
endif

ifeq ($(CARGO_PROFILE),release)
CARGO_PROFILE_FLAG := --release
else
CARGO_PROFILE_FLAG :=
endif
RPC_URL ?= http://localhost:8545

PEERS=$(shell cat crates/rbft-node/assets/enodes.txt)


testnet_start:
	mkdir -p $(ASSETS_DIR) $(LOGS_DIR) $(DB_DIR)
	$(CARGO) build $(CARGO_PROFILE_FLAG) --bin rbft-node
	$(CARGO) run $(CARGO_PROFILE_FLAG) --bin rbft-utils -- node-gen --assets-dir $(ASSETS_DIR)
	$(CARGO) run $(CARGO_PROFILE_FLAG) --bin rbft-utils -- genesis --assets-dir $(ASSETS_DIR)
	$(CARGO) run $(CARGO_PROFILE_FLAG) --bin rbft-utils -- testnet --init \
		--monitor-txpool \
		--assets-dir $(ASSETS_DIR) --logs-dir $(LOGS_DIR) --db-dir $(DB_DIR) \
		--extra-args "--txpool.max-tx-input-bytes 4194304" \
		--extra-args "--builder.gaslimit 600000000" \
		--extra-args "--tx-propagation-mode all" \
		--extra-args "--txpool.pending-max-count 150000" \
		--extra-args "--txpool.max-account-slots 30000" \
		--extra-args "--txpool.basefee-max-count 150000" \
		--extra-args "--txpool.queued-max-count 150000" \
		--extra-args "--txpool.max-new-txns 100000" \
		--extra-args "--txpool.max-pending-txns 100000" \
		--extra-args "--engine.cross-block-cache-size 256" \
		--extra-args "--rpc-cache.max-blocks 100" \
		--extra-args "--rpc-cache.max-receipts 100" \
		--extra-args "--rpc-cache.max-headers 100"

testnet_restart:
	mkdir -p $(LOGS_DIR) $(DB_DIR)
	$(CARGO) build $(CARGO_PROFILE_FLAG) --bin rbft-node
	$(CARGO) run $(CARGO_PROFILE_FLAG) --bin rbft-utils -- testnet \
		--assets-dir $(ASSETS_DIR) --logs-dir $(LOGS_DIR) --db-dir $(DB_DIR) \
		--extra-args "--txpool.max-tx-input-bytes 4194304" \
		--extra-args "--builder.gaslimit 600000000" \
		--extra-args "--tx-propagation-mode all" \
		--extra-args "--txpool.pending-max-count 150000" \
		--extra-args "--txpool.basefee-max-count 150000" \
		--extra-args "--txpool.queued-max-count 150000" \
		--extra-args "--txpool.max-account-slots 30000" \
		--extra-args "--txpool.max-pending-txns 100000" \
		--extra-args "--txpool.max-new-txns 100000" \
		--extra-args "--engine.cross-block-cache-size 256" \
		--extra-args "--rpc-cache.max-blocks 100" \
		--extra-args "--rpc-cache.max-receipts 100" \
		--extra-args "--rpc-cache.max-headers 100"

testnet_debug:
	RUST_LOG=debug $(MAKE) CARGO_PROFILE=debug testnet_start


# Test follower node support: starts a 4-validator testnet, adds 2 non-validator follower
# nodes at blocks 5 and 15 via RBFT_ADD_FOLLOWER_AT, then exits at block 30.
# node-gen is given RBFT_NUM_NODES + 2 slots so nodes.csv has keys for the followers.
testnet_follower_test:
	mkdir -p $(ASSETS_DIR)
	$(CARGO) build --release --bin rbft-node
	$(CARGO) build --release --bin rbft-utils
	target/release/rbft-utils node-gen --assets-dir $(ASSETS_DIR) \
		--num-nodes $$(( $${RBFT_NUM_NODES:-4} + 2 ))
	target/release/rbft-utils genesis --assets-dir $(ASSETS_DIR) \
		--initial-nodes $${RBFT_NUM_NODES:-4}
	RBFT_ADD_FOLLOWER_AT=5,15 RBFT_EXIT_AFTER_BLOCK=30 \
	target/release/rbft-utils testnet --init \
		--assets-dir $(ASSETS_DIR)
	target/release/rbft-utils logjam -q

# Test transaction load handling: starts a 4-validator testnet with megatx enabled, which submits large transactions every block.
testnet_load_test:
	mkdir -p $(ASSETS_DIR)
	$(CARGO) build --release --bin rbft-node
	$(CARGO) build --release --bin rbft-utils
	$(CARGO) build --release --bin rbft-megatx
	target/release/rbft-utils node-gen --assets-dir $(ASSETS_DIR)
	target/release/rbft-utils genesis --assets-dir $(ASSETS_DIR)
	target/release/rbft-utils testnet --init \
		--monitor-txpool --run-megatx \
		--assets-dir $(ASSETS_DIR) \
		--extra-args "--txpool.max-tx-input-bytes 10000000" \
		--extra-args "--builder.gaslimit 60000000" \
		--extra-args "--tx-propagation-mode all" \
		--extra-args "--txpool.pending-max-count 150000" \
		--extra-args "--txpool.basefee-max-count 150000" \
		--extra-args "--txpool.queued-max-count 150000" \
		--extra-args "--txpool.pending-max-size 100" \
		--extra-args "--txpool.basefee-max-size 100" \
		--extra-args "--txpool.queued-max-size 100" \
		--extra-args "--txpool.max-account-slots 30000" \
		--extra-args "--txpool.max-pending-txns 100000" \
		--extra-args "--txpool.max-new-txns 100000" \
		--extra-args "--engine.cross-block-cache-size 256" \
		--extra-args "--rpc-cache.max-blocks 100" \
		--extra-args "--rpc-cache.max-receipts 100" \
		--extra-args "--rpc-cache.max-headers 100"
	target/release/rbft-utils logjam -q

# Run a minimal test follower node against an already-running testnet.
# Assumes the testnet was started with node-gen and genesis commands so nodes.csv and genesis.json exist in ASSETS_DIR.
# You may need to remove the reth database directory for the follower node to start successfully.
#
# --trusted-peers is set to all the enodes from nodes.csv for simplicity, but in a real scenario you would typically only trust a subset of validators.
# --chain is set to the same genesis file as the main testnet to ensure it can sync properly, but in a real scenario you would typically use a more minimal genesis for followers.
testnet_follower:
	$(CARGO) build --release --bin rbft-node
	target/release/rbft-node node --port 12345 \
		--chain $(ASSETS_DIR)/genesis.json \
		--trusted-peers "$$(awk -F',' 'NR>1{printf "%s%s",sep,$$5; sep=","}' $(ASSETS_DIR)/nodes.csv)"

genesis:
	mkdir -p $(ASSETS_DIR)
	$(CARGO) run --release --bin rbft-utils -- genesis --assets-dir $(ASSETS_DIR)

node-gen:
	$(CARGO) run --release --bin rbft-utils -- node-gen --assets-dir $(ASSETS_DIR)

node-help:
	$(CARGO) run --release --bin rbft-node -- node --help

megatx:
	$(CARGO) run --release --bin rbft-megatx -- spam -n 100000

validator-inspector:
	$(CARGO) run --release -p rbft-validator-inspector -- \
		--rbft-bin-dir $(shell pwd)/target/release \
		--rpc v0=http://127.0.0.1:8545,v1=http://127.0.0.1:8544,v2=http://127.0.0.1:8543,v3=http://127.0.0.1:8542

validator_status:
	$(CARGO) run --release --bin rbft-utils -- validator status --rpc-url $(RPC_URL)

# Add a test validator to the contract
# Uses constant test values for address and enode
add-validator:
	$(CARGO) run --release --bin rbft-utils -- validator add \
		--admin-key $(RBFT_ADMIN_KEY) \
		--validator-address 0x1234567890123456789012345678901234567890 \
		--enode "enode://1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef@127.0.0.1:30303" \
		--rpc-url $(RPC_URL)

# Convenient shorthand for validator status
status:
	@echo "Querying validator contract status from $(RPC_URL)..."
	@$(CARGO) run --release --bin rbft-utils -- validator status --rpc-url $(RPC_URL)

# View logs from all testnet nodes
logs:
	@cat ~/.rbft/testnet/logs/node*.log

# View logs from all testnet nodes in chronological order with message delivery tracking
# View logs from all testnet nodes in chronological order with message delivery tracking
# See 'make help' for RBFT_LOGJAM_* environment variable options
logjam:
	@$(CARGO) run --release --bin rbft-utils -- logjam

# Docker commands
docker-validate:
	@echo "Validating Dockerfiles..."
	@./scripts/validate-dockerfiles.sh

docker-build:
	@echo "Building production Docker image..."
	docker build --build-arg STRIP_BINARY=true --build-arg INSTALL_DEVELOPMENT_TOOLS=false \
		-t $(DOCKER_IMAGE_NAME):$(DOCKER_TAG) .

docker-build-dev:
	@echo "Building development Docker image with tools..."
	docker build --build-arg STRIP_BINARY=false --build-arg INSTALL_DEVELOPMENT_TOOLS=true \
		-t $(DOCKER_IMAGE_NAME):$(DOCKER_TAG)-dev .

docker-build-debug:
	@echo "Building debug Docker image (unstripped)..."
	docker build --build-arg STRIP_BINARY=false --build-arg INSTALL_DEVELOPMENT_TOOLS=true \
		-t $(DOCKER_IMAGE_NAME):$(DOCKER_TAG)-debug .

docker-run:
	@echo "Running Docker container (production)..."
	docker run -it --rm \
		-p 8545:8545 \
		-p 8551:8551 \
		-p 30303:30303 \
		-p 9000:9000 \
		-p 8080:8080 \
		-p 9090:9090 \
		-v rbft_data:/data \
		$(DOCKER_IMAGE_NAME):$(DOCKER_TAG)

docker-run-dev:
	@echo "Running development Docker container..."
	docker run -it --rm \
		-p 8545:8545 \
		-p 8551:8551 \
		-p 30303:30303 \
		-p 9000:9000 \
		-p 8080:8080 \
		-p 9090:9090 \
		-v rbft_data:/data \
		$(DOCKER_IMAGE_NAME):$(DOCKER_TAG)-dev

docker-test:
	@echo "Testing Docker image..."
	docker run --rm $(DOCKER_IMAGE_NAME):$(DOCKER_TAG) ./rbft-node --version

docker-clean:
	@echo "Cleaning Docker images and containers..."
	docker container prune -f
	docker image prune -f
	docker rmi \
		$(DOCKER_IMAGE_NAME):$(DOCKER_TAG) \
		$(DOCKER_IMAGE_NAME):$(DOCKER_TAG)-dev \
		$(DOCKER_IMAGE_NAME):$(DOCKER_TAG)-debug \
		2>/dev/null || true

# Development commands
test:
	$(CARGO) test --all

fmt:
	$(CARGO) fmt --all

clippy:
	$(CARGO) clippy --all-targets --all-features -- -D warnings

clean:
	$(CARGO) clean

.PHONY: book
book:
	mdbook build book

# Dafny commands
dafny-translate:
	@echo "Translating Dafny specification to Rust..."
	@mkdir -p doc/qbft-spec/rust
	cd doc/qbft-spec/dafny/spec/L1 && \
		dafny translate rs node.dfy --output=../../../rust/node.rs

# Build and tag for registry (used by CI/CD)
docker-tag-registry:
	docker tag $(DOCKER_IMAGE_NAME):$(DOCKER_TAG) $(REGISTRY)/$(DOCKER_IMAGE_NAME):$(DOCKER_TAG)
	docker tag $(DOCKER_IMAGE_NAME):$(DOCKER_TAG) $(REGISTRY)/$(DOCKER_IMAGE_NAME):development

.PHONY: help docker-validate docker-build docker-build-dev docker-build-debug docker-run \
	docker-run-dev docker-test docker-clean test fmt clippy clean docker-tag-registry \
	dafny-translate validator_status status testnet_start testnet_restart genesis node-help \
	megatx validator-inspector testnet_load_test testnet_follower_test testnet_debug testnet-follower
