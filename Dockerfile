# Unified Multi-stage Dockerfile for RBFT Node
# OPTIMIZED VERSION with improved caching strategy

# Build arguments for configuration
ARG RUST_VERSION=1.88
ARG DEBIAN_VERSION=bookworm-slim
ARG STRIP_BINARY=true
ARG INSTALL_DEVELOPMENT_TOOLS=false

# ============================================================
# Stage 1: Base builder with system dependencies (cached)
# ============================================================
FROM rust:${RUST_VERSION}-slim-bookworm AS base-builder

WORKDIR /app

# Install system dependencies - this layer rarely changes
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    build-essential \
    pkg-config \
    libssl-dev \
    clang \
    cmake \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# Set cargo environment for optimal builds
ENV CARGO_HOME=/usr/local/cargo \
    CARGO_INCREMENTAL=0 \
    CARGO_NET_RETRY=10 \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
    RUSTFLAGS="-C link-arg=-fuse-ld=lld" \
    RUST_BACKTRACE=1

# Install lld for faster linking (10-30% faster)
RUN apt-get update && apt-get install -y --no-install-recommends lld && \
    rm -rf /var/lib/apt/lists/*

# ============================================================
# Stage 2: Dependency planner using cargo-chef
# ============================================================
FROM lukemathwalker/cargo-chef:latest-rust-${RUST_VERSION} AS planner
WORKDIR /app

# Copy only files needed for dependency resolution
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Generate recipe.json for dependency caching
RUN cargo chef prepare --recipe-path recipe.json

# ============================================================
# Stage 3: Dependency builder (heavily cached)
# ============================================================
FROM base-builder AS dependencies

# Install cargo-chef
RUN cargo install cargo-chef --locked

# Copy recipe from planner
COPY --from=planner /app/recipe.json recipe.json

# Build ONLY dependencies - this is the key caching layer
# Dependencies rarely change, so this layer stays cached
RUN cargo chef cook --release --recipe-path recipe.json

# ============================================================
# Stage 4: Application builder
# ============================================================
FROM dependencies AS builder

# Copy source code (this invalidates cache on code changes)
COPY . .

# Build arguments
ARG STRIP_BINARY

# Build only the application binary (dependencies already compiled)
# Using --offline since all deps are already cached
RUN cargo build --release --bin rbft-node && \
    cp target/release/rbft-node ./rbft-node && \
    if [ "$STRIP_BINARY" = "true" ]; then \
        echo "Stripping binary symbols..."; \
        strip ./rbft-node; \
    fi

# ============================================================
# Stage 5: Minimal runtime image
# ============================================================
FROM debian:${DEBIAN_VERSION} AS runtime

ARG INSTALL_DEVELOPMENT_TOOLS

# Install minimal runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    $(if [ "$INSTALL_DEVELOPMENT_TOOLS" = "true" ]; then echo "curl procps htop"; fi) \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 rbft \
    && useradd --uid 1000 --gid rbft --shell /bin/bash --create-home rbft

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/rbft-node ./rbft-node

# Set permissions
RUN chown rbft:rbft rbft-node && chmod +x rbft-node && \
    mkdir -p /data && chown rbft:rbft /data

USER rbft

# Metadata
LABEL maintainer="RBFT Team" \
      description="RBFT Consensus Node - Optimized Build" \
      version="1.0"

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD ./rbft-node --version > /dev/null || exit 1

EXPOSE 8545 8551 30303 9000 8080 9090

ENV RUST_LOG=info RUST_BACKTRACE=1

VOLUME ["/data"]

CMD ["./rbft-node", "node", "--datadir", "/data"]
