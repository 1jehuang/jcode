# ============================================================
# JCode — 多阶段构建镜像
# 适用场景: jcode-server 模式 (gRPC + WebSocket + REST)
#
# 构建:
#   docker build -t jcode:latest .
#
# 运行:
#   docker run -d --name jcode-server \
#     -p 50051:50051 -p 8080:8080 -p 8081:8081 \
#     -v jcode-data:/home/jcode/.jcode \
#     jcode:latest
# ============================================================

# ── Stage 1: 构建 ──
FROM rust:1.83-alpine AS builder

RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    pkgconfig \
    perl

WORKDIR /build

# 依赖缓存层
COPY Cargo.toml Cargo.lock ./
COPY crates/vendor-agentgrep crates/vendor-agentgrep/
COPY crates/jcode-{agent-runtime,ambient-types,auth-types,background-types,batch-types,build-support,compaction-core,config-types,core,embedding,gateway-types,import-core,memory-types,message-types,overnight-core,plan,protocol,selfdev-types,session-types,storage,side-panel-types,azure-auth,notify-email,provider-core,provider-metadata,provider-openrouter,provider-openai,provider-gemini,tool-core,tool-types,usage-types,pdf,mobile-core,update-core,terminal-launch,lock-manager,multi-file-edit,cross-file-repair,completion,micro-ci,ci-generator,project-builder,skills,lora-train,sandbox,mcp-advanced,hooks,lsp,p2-features,session-persist,build-engine,swarm-core,agent-advanced,agent-runtime,tui-core,tui-markdown,tui-messages,tui-mermaid,tui-render,tui-style,tui-workspace,tui-account-picker,tui-session-picker,tui-tool-display,tui-usage-overlay,desktop,mobile-sim} crates/

# 真·构建
COPY . .
RUN cargo build --release --no-default-features --features pdf --bin jcode --bin jcode-server --bin jcode-grpc 2>&1

# ── Stage 2: 运行时 ──
FROM alpine:3.20 AS runtime

RUN apk add --no-cache \
    ca-certificates \
    openssl \
    tzdata \
    && addgroup -S jcode && adduser -S jcode -G jcode

USER jcode
WORKDIR /home/jcode

COPY --from=builder /build/target/release/jcode /usr/local/bin/
COPY --from=builder /build/target/release/jcode-server /usr/local/bin/
COPY --from=builder /build/target/release/jcode-grpc /usr/local/bin/

# gRPC, WebSocket, REST
EXPOSE 50051 8080 8081

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["jcode", "--version"] || exit 1

ENTRYPOINT ["jcode-server"]
