# CarpAI v1.0.0 Release Notes

**Release Date**: 2026-05-24
**Commit**: $(git rev-parse --short HEAD 2>/dev/null || echo "N/A")

---

## What is CarpAI?

CarpAI is an enterprise-grade AI programming assistant that provides:
- **CLI Mode**: Terminal-based TUI for individual developers (`carpai chat`)
- **Server Mode**: Multi-tenant enterprise server with gRPC/REST/WebSocket APIs (`carpai serve`)
- **IDE SDK**: Plugin development kit for VSCode/JetBrains/Neovim integration

---

## Architecture

CarpAI v1.0.0 implements a **4-layer crate architecture**:

```
Layer 0: carpai-internal    (Pure trait definitions, zero implementation)
   ↓
Layer 1: carpai-core        (Business logic + Local implementations)
   ↓
Layer 2: carpai-server      (Enterprise server product)
         carpai-cli         (TUI client product)
         carpai-sdk         (IDE plugin SDK)
```

---

## New Features

### Core Platform
- ✅ **AgentContext DI Container**: Central dependency injection for all backend services
- ✅ **execute_agent_turn()**: Pure business logic agent loop (no UI/network dependencies)
- ✅ **build_local_agent_context()**: One-line setup for CLI/local development mode
- ✅ **7 Trait Interfaces**: SessionStore, ToolExecutor, InferenceBackend, VirtualFileSystem, EventBus, MemoryBackend, CodeCompletion

### Enterprise Server (carpai-server)
- ✅ **gRPC Services**: AgentService, SessionService, ToolService, HealthService (tonic)
- ✅ **REST API**: OpenAI-compatible `/v1/chat/completions` endpoint (axum)
- ✅ **WebSocket**: Real-time streaming support
- ✅ **Multi-tenancy**: TenantContext extraction via headers or JWT
- ✅ **Quota Management**: Per-tenant token limits, RPM rate limiting, concurrent request control
- ✅ **Audit Logging**: Structured JSON audit logs for compliance
- ✅ **Observability**: Prometheus metrics (20+ indicators), OpenTelemetry tracing, health checks

### CLI Client (carpai-cli)
- ✅ **TUI Rendering Layer**: Pure ratatui-based UI (zero business logic)
- ✅ **AgentBridge**: Delegation layer between TUI and carpai-core
- ✅ **Dual Mode**: Local (direct core access) and Remote (gRPC to server)
- ✅ **Commands**: `carpai chat`, `carpai ask`, `carpai complete`
- ✅ **Ambient Tasks**: Background task runner with scheduler
- ✅ **Notifications**: Telegram, Gmail, browser integration

### IDE SDK (carpai-sdk)
- ✅ **OpenAI Compatible Types**: ChatCompletionRequest/Response, ChatMessage
- ✅ **Session CRUD API**: Create/List/Get/Delete sessions, append messages
- ✅ **MCP Client**: Model Context Protocol support
- ✅ **Response Caching**: LRU cache with TTL
- ✅ **Retry Logic**: Exponential backoff with jitter

---

## Deployment

### Docker
```bash
docker build -t carpai-server .
docker run -p 8080:8080 -p 50051:50051 carpai-server
```

### Docker Compose
```bash
docker-compose up -d  # Starts server + PostgreSQL + Redis
docker-compose --profile monitoring up -d  # + Prometheus + Grafana
```

### systemd (Linux)
```bash
sudo cp deploy/carpai-server.service /etc/systemd/system/
sudo systemctl enable carpai-server
sudo systemctl start carpai-server
```

---

## Configuration

See `deploy/production.toml` for a complete production-ready configuration template.

Key environment variables:
- `CARPAI_SERVER__PORT`: Listen port (default: 8080)
- `CARPAI_SERVER__JWT_SECRET`: JWT signing secret
- `CARPAI_SERVER__DATABASE_URL`: PostgreSQL connection string
- `RUST_LOG`: Log level (trace/debug/info/warn/error)

---

## Performance Benchmarks

| Metric | Value |
|--------|-------|
| Binary size (stripped) | ~25MB (server), ~18MB (cli) |
| Memory at startup | ~50MB (server), ~30MB (cli) |
| Agent turn p50 latency | ~500ms (local), ~600ms (server) |
| Concurrent connections | ~350 req/s @ 100 connections |
| Token throughput | ~500 prompt tok/s, ~30 completion tok/s |

Full report: [docs/PERFORMANCE_BENCHMARK.md](docs/PERFORMANCE_BENCHMARK.md)

---

## Security

- ✅ Non-root Docker container
- ✅ systemd security hardening (NoNewPrivileges, ProtectSystem, PrivateTmp)
- ✅ JWT authentication with configurable expiry
- ✅ RBAC authorization (Admin/Member/Viewer roles)
- ✅ Audit logging for all sensitive operations
- ✅ Cargo dependency audit passed (cargo-audit)

---

## Breaking Changes from v0.12.0

1. **Monolith → Layered architecture**: The old `src/lib.rs` monolith has been split into 4 layers
2. **New crate names**: `carpai-internal`, `carpai-core`, `carpai-server`, `carpai-cli`, `carpai-sdk`
3. **API changes**: REST endpoints now follow OpenAI-compatible format
4. **Configuration**: Three-layer config system (AppConfig → CoreConfig → ServerConfig/CliConfig)

---

## Migration Guide

### For existing users
1. Backup your `~/.jcode` data directory
2. Install v1.0.0: `cargo install --path .`
3. Migrate config: copy old settings to `~/.carpai/config.toml`
4. Restart: `carpai chat` (CLI) or `carpai serve` (Server)

### For enterprise deployments
1. Deploy new Docker image or update systemd service
2. Update environment variables (see `deploy/production.toml`)
3. Run database migrations (if using PostgreSQL)
4. Verify health check: `curl http://localhost:8080/health`

---

## Contributors

This release was made possible by the three-team collaboration:
- **solo-Turbo**: Architecture coordination, core implementation, SDK enhancement, performance benchmarking
- **ma-guoyang**: Enterprise server (gRPC/REST), multi-tenancy, quota management, audit logging, observability
- **Paw-brave**: CLI/TUI client, AgentBridge, dual-mode architecture, ambient tasks

---

## What is Next?

- **v1.1.0**: IDE plugin implementations (VSCode extension, JetBrains plugin)
- **v1.2.0**: Advanced RAG features (vector search, knowledge graph)
- **v2.0.0**: Distributed inference (multi-GPU, model parallelism)

---

**Full Changelog**: See git log for detailed commit history.
