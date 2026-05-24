# CarpAI Server - Production Docker Image
# Multi-stage build for minimal image size

# ============================================================================
# Stage 1: Builder
# ============================================================================
FROM rust:1.75-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y /
    pkg-config /
    libssl-dev /
    protobuf-compiler /
    && rm -rf /var/lib/apt/lists/*

# Copy workspace
COPY . .

# Build in release mode with optimizations
RUN cargo build --release -p carpai-server

# ============================================================================
# Stage 2: Runtime
# ============================================================================
FROM debian:bookworm-slim

# Create non-root user
RUN groupadd -r carpai && useradd -r -g carpai -m carpai

# Install runtime dependencies
RUN apt-get update && apt-get install -y /
    ca-certificates /
    libssl3 /
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /app/target/release/carpai-server /usr/local/bin/carpai-server

# Create data directory
RUN mkdir -p /var/lib/carpai/data && chown -R carpai:carpai /var/lib/carpai

# Switch to non-root user
USER carpai

# Expose ports
EXPOSE 8080 50051

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 /
    CMD curl -f http://localhost:8080/health || exit 1

# Set environment variables
ENV CARPAI_SERVER__PORT=8080
ENV CARPAI_SERVER__LISTEN_ADDR=0.0.0.0
ENV RUST_LOG=info

# Run server
ENTRYPOINT ["carpai-server"]
