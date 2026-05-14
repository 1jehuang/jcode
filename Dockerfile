# Multi-stage Docker build for CarpAI
# Stage 1: Build
FROM rust:1.75 AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY crates ./crates

RUN cargo build --release --bin carpai && \
    strip /app/target/release/carpai

# Stage 2: Runtime (minimal image)
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 1000 carpai

WORKDIR /app
COPY --from=builder /app/target/release/carpai .

# Create data directories
RUN mkdir -p /data/plugins /data/sessions /data/versions && \
    chown -R carpai:carpai /data

USER carpai
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/api/health || exit 1

CMD ["./carpai", "--port", "8080", "--config", "/etc/carpai/config.yaml"]
