# CarpAI 最终目标架构 — 服务端架构精确蓝图

> **版本**: v1.0 | **日期**: 2026-05-25  
> **定位**: 回答 "最终应该长什么样" — 精确到每个文件、每个依赖、每个接口

---

## 一、一句话定义

**CarpAI = 一个 Monorepo 中的 5 层 Crate，产出 3 个独立二进制产品，通过 Trait 接口实现零耦合。**

```
CarpAI 不是"带 TUI 的编程助手"。
CarpAI 是"AI 编程引擎的服务端运行时"，CLI 和 SDK 只是它的两个客户端。
```

---

## 二、最终目录结构（精确到文件级）

```
CarpAI/                              ← Git 仓库根 (workspace)
│
├── Cargo.toml                       ← workspace 定义 (仅 members 列表)
│   [workspace]
│   members = [
│     "crates/carpai-internal",      ← Layer 0
│     "crates/carpai-core",          ← Layer 1
│     "crates/carpai-server",        ← Layer 2a (产品)
│     "crates/carpai-cli",           ← Layer 2b (产品)
│     "crates/carpai-sdk",           ← Layer 2c (库)
│     "crates/jcode-*",              ← 100+ 辅助 crate (不变)
│   ]
│
├── crates/
│   │
│   ├── carpai-internal/             ═══ Layer 0: Pure Trait 抽象层 ═══
│   │   ├── Cargo.toml               ← 依赖: async-trait, serde, chrono, thiserror, uuid, tracing
│   │   └── src/
│   │       ├── lib.rs               ← re-export 所有 trait + 类型 + AgentContext
│   │       │
│   │       ├── session.rs           ← trait SessionStore + SessionId/SessionMeta/StoredMessage...
│   │       ├── tool_executor.rs     ← trait ToolExecutor + ToolRequest/ToolResponse/ToolSchema...
│   │       ├── inference_backend.rs ← trait InferenceBackend + ChatCompletionRequest/Response...
│   │       ├── filesystem.rs        ← trait VirtualFileSystem + FsError/FileMeta/SearchResult...
│   │       ├── event_bus.rs         ← trait EventBus + BusEvent/BusSubscriber/BusHealth...
│   │       ├── memory_backend.rs    ← trait MemoryBackend + EnhancedMemoryEntry/VectorSearchResult...
│   │       ├── completion.rs        ← trait CodeCompletion (已有)
│   │       ├── auth.rs              ← trait AuthProvider (已有)
│   │       ├── inference.rs         ← trait InferenceEngine (已有, 基础版)
│   │       ├── memory.rs            ← trait MemoryStore (已有, 基础版)
│   │       ├── tools.rs             ← trait ToolRegistry (已有)
│   │       └── agent_context.rs     ← struct AgentContext (DI 容器) + AppConfig + Builder
│   │
│   ├── carpai-core/                 ═══ Layer 1: 业务逻辑层 (本地实现) ═══
│   │   ├── Cargo.toml               ← 依赖: carpai-internal, tokio, serde, anyhow, tracing,
│   │   │                               chrono, uuid, toml, futures, reqwest, dirs, sha2, regex
│   │   └── src/
│   │       ├── lib.rs               ← re-export: CoreConfig, 6 个 Local* 实现, execute_agent_turn
│   │       │
│   │       ├── config.rs            ← struct CoreConfig (extends AppConfig) + ProviderConfig
│   │       │
│   │       ├── agent_loop.rs        ← fn execute_agent_turn() + fn build_local_agent_context()
│   │       │                          struct AgentTurnOutput (text, tool_calls, usage, session_id)
│   │       │                          struct ToolCallInfo (name, params, result, duration_ms)
│   │       │
│   │       ├── session_impl.rs      ← impl SessionStore for LocalFileSessionStore (JSONL 文件)
│   │       ├── tool_executor_impl.rs← impl ToolExecutor for LocalToolExecutor (Semaphore 并发)
│   │       ├── inference_impl.rs    ← impl InferenceBackend for SidecarInferenceBackend (HTTP/Ollama)
│   │       ├── filesystem_impl.rs   ← impl VirtualFileSystem for LocalFileSystem (沙盒路径)
│   │       ├── event_bus_impl.rs    ← impl EventBus for InProcessEventBus (broadcast channel)
│   │       └── memory_impl.rs       ← impl MemoryBackend for LocalMemoryBackend (JSONL 文件)
│   │
│   ├── carpai-server/               ═══ Layer 2a: 企业服务端产品 ═══
│   │   ├── Cargo.toml               ← 依赖: carpai-core, carpai-internal,
│   │   │                               tokio, axum, tonic, prost, sqlx, redis,
│   │   │                               jsonwebtoken, tracing-subscriber, prometheus-client
│   │   └── src/
│   │       ├── main.rs              ← fn main(): load ServerConfig → Application::new() → app.run()
│   │       ├── lib.rs               ← pub mod config/app/grpc/rest/ws/auth/enterprise/observability/service
│   │       │
│   │       ├── config.rs            ← struct ServerConfig (extends CoreConfig)
│   │       │                          + TlsConfig, DatabaseConfig, RedisConfig
│   │       │                          + listen_addr, port, jwt_secret, multi_tenant, rate_limit_*
│   │       │
│   │       ├── app.rs               ← struct Application { config, router: Router, ctx: Arc<ServerContext> }
│   │       │                          → build_router() 合并 health/rest/grpc/ws 路由
│   │       │                          → run() 启动 axum::serve()
│   │       │
│   │       ├── service/
│   │       │   └── context.rs        ← struct ServerContext { core_ctx: AgentContext, db: PgPool, ... }
│   │       │                          → from_config(): 组装企业级 AgentContext (PgSessionStore, etc.)
│   │       │
│   │       ├── grpc/                ← gRPC 协议层 (tonic + protobuf)
│   │       │   ├── mod.rs
│   │       │   ├── server.rs         ← 构建 tonic::Server, 注册所有 gRPC service
│   │       │   ├── agent_service.rs  ← AgentService: ExecuteTask, StreamCompletion RPC
│   │       │   ├── session_service.rs← SessionService: CreateSession, GetMessages RPC
│   │       │   ├── tool_service.rs   ← ToolService: ExecuteTool RPC
│   │       │   ├── health_service.rs ← HealthService: Check RPC
│   │       │   └── proto/            ← .proto 定义文件
│   │       │       ├── agent.proto
│   │       │       ├── session.proto
│   │       │       ├── tool.proto
│   │       │       └── health.proto
│   │       │
│   │       ├── rest/                ← REST API 层 (axum)
│   │       │   ├── mod.rs
│   │       │   └── router.rs        ← OpenAI-compatible /v1/chat/completions
│   │       │                          /v1/sessions/*, /v1/models, /health
│   │       │
│   │       ├── ws/                  ← WebSocket 层 (axum::extract::ws)
│   │       │   └── mod.rs           ← /ws/stream (实时 token 流)
│   │       │
│   │       ├── auth/                ← 认证授权层
│   │       │   ├── mod.rs
│   │       │   ├── jwt.rs           ← JWT token 创建/验证
│   │       │   ├── api_key.rs       ← API Key 验证中间件 (axum extractor)
│   │       │   └── rbac.rs          ← Role-Based Access Control 判定
│   │       │
│   │       ├── enterprise/          ← 企业特性
│   │       │   ├── mod.rs
│   │       │   ├── multi_tenant.rs  ← TenantContext 提取 (从 header/JWT)
│   │       │   ├── quota.rs         ← Token 配额管理 (per-tenant RPM/TPM)
│   │       │   └── audit.rs         ← 审计日志 (结构化 JSON → file/db)
│   │       │
│   │       └── observability/       ← 可观测性
│   │           ├── mod.rs
│   │           ├── metrics.rs       ← Prometheus 指标 (20+ counters/gauges/histograms)
│   │           ├── tracing.rs       ← OpenTelemetry 集成
│   │           └── health.rs        ← /health, /ready 就绪检查
│   │
│   ├── carpai-cli/                  ═══ Layer 2b: 单机客户端产品 ═══
│   │   ├── Cargo.toml               ← 依赖: carpai-core, carpai-internal,
│   │   │                               ratatui, crossterm, tokio, clap, chrono,
│   │   │                               anyhow, thiserror, tracing, uuid, toml
│   │   │   [features]
│   │   │   remote = ["reqwest", "tonic"]  ← 远程模式可选依赖
│   │   └── src/
│   │       ├── main.rs              ← fn main(): clap 解析 chat/ask/complete/serve 子命令
│   │       ├── lib.rs               ← pub mod config/cli/tui/agent_bridge/ambient/modes/notifications
│   │       │
│   │       ├── config.rs            ← struct CliConfig (主题/按键/剪贴板/远程模式)
│   │       ├── modes.rs             ← enum CliMode { Local, Remote }
│   │       │
│   │       ├── cli/                 ← 命令分发层
│   │       │   ├── mod.rs
│   │       │   ├── chat.rs          ← "carpai chat": 加载配置 → build_local_context → run_tui()
│   │       │   ├── ask.rs           ← "carpai ask <msg>": 一次性问答 → print 结果
│   │       │   ├── completion.rs    ← "carpai complete": 代码补全 (stdin/stdout)
│   │       │   └── serve.rs         ← "carpai serve": 启动子进程 carpai-server
│   │       │
│   │       ├── tui/                 ← 渲染层 (纯 UI, 零业务逻辑!)
│   │       │   ├── mod.rs           ← run_tui() 主循环: raw mode → draw → event poll → update
│   │       │   ├── app.rs           ← struct App { messages, input, bridge, config, should_quit }
│   │       │   ├── event.rs         ← enum AppEvent { Key, Mouse, Resize, Tick }
│   │       │   ├── handler.rs       ← handle_key()/handle_mouse() 分发
│   │       │   ├── theme.rs         ← 配色方案 (暗色/亮色)
│   │       │   └── widgets/
│   │       │       ├── mod.rs
│   │       │       ├── chat_view.rs    ← 消息列表渲染 (user/assistant/tool 区分)
│   │       │       ├── input_bar.rs    ← 输入框 (多行编辑, 快捷键提示)
│   │       │       ├── status_line.rs  ← 状态栏 (mode/model/session/tokens)
│   │       │       └── help_overlay.rs ← 帮助界面 (? 键)
│   │       │
│   │       ├── agent_bridge.rs      ← 桥接层 (只委托, 零业务逻辑!)
│   │       │                          enum BridgeMode { Local, Remote { url } }
│   │       │                          struct AgentBridge { mode, local_ctx, grpc_client? }
│   │       │                          → execute_turn() → AgentTurnOutput
│   │       │
│   │       ├── ambient/             ← 后台任务
│   │       │   ├── mod.rs
│   │       │   ├── runner.rs        ← BackgroundRunner (Semaphore + CancellationToken)
│   │       │   └── scheduler.rs     ← TaskScheduler (interval + select! 循环)
│   │       │
│   │       └── notifications/       ← 通知渠道
│   │           ├── mod.rs
│   │           ├── telegram.rs      ← Telegram Bot API
│   │           ├── gmail.rs         ← Gmail 摘要 (SMTP stub)
│   │           └── browser.rs       ← 跨平台 URL 打开
│   │
│   └── carpai-sdk/                  ═══ Layer 2c: IDE 插件 SDK (库, 非二进制) ═══
│       ├── Cargo.toml               ← 依赖: tokio, reqwest, tonic, serde, thiserror,
│       │                               tracing, uuid, chrono, futures, backoff, lru, dashmap
│       │   [features]
│       │   default = []
│       │   wasm = ["wasm-bindgen", "js-sys", "web-sys", ...]  ← 浏览器/WASM 可选
│       │   [lib]
│       │   crate-type = ["cdylib", "rlib"]  ← 可编译为动态库 (Node.js FFI) 或 rlib
│       └── src/
│           ├── lib.rs               ← pub mod client/cache/config/error/types/mcp/streaming/ide/protocol/session_api
│           ├── client.rs            ← struct CarpAiClient { http, grpc, cache, config }
│           │                          → complete(), chat(), stream_chat(), session_*()
│           ├── types.rs             ← 所有请求/响应 DTO (serde 序列化)
│           ├── protocol.rs          ← 协议常量 (API 版本, headers, error codes)
│           ├── streaming.rs         ← SSE/WebSocket 流式解析器
│           ├── session_api.rs       ← Session CRUD API 封装
│           ├── cache.rs             ← LRU ResponseCache (支持 TTL/stale-while-revalidate)
│           ├── config.rs            ← struct SdkConfig { api_key, base_url, model, timeout }
│           ├── error.rs             ← enum SdkError (网络/认证/解析/速率限制)
│           ├── mcp.rs               ← MCP Client (连接外部 MCP server)
│           ├── ide.rs               ← IDE 适配层 (VSCode Command/JetBrains Action)
│           └── wasm/                ← [feature = wasm] WASM 绑定
│               └── bindings.rs      ← wasm_export! 宏导出 JS-callable 函数
│
├── src/                             ═══ ⚠ 过渡区 (Phase 4 后应清空) ═══
│   ├── lib.rs                       ← 最终只剩 ~30 行 re-export (向后兼容)
│   └── (所有子模块已搬迁到上述 crate)
│
├── docs/                            ← 架构文档
│   ├── FINAL_TARGET_ARCHITECTURE.md  ← 本文档
│   ├── ARCHITECTURE_STATUS_ANALYSIS.md ← 现状分析
│   └── THREE_TEAM_REFACTOR_PLAN_V3_FINAL.md ← 执行计划
│
├── proto/                           ← gRPC proto 定义 (tonic-build 编译)
│   ├── agent.proto
│   ├── session.proto
│   ├── tool.proto
│   └── health.proto
│
├── deploy/                          ← 部署配置
│   ├── docker/
│   │   ├── Dockerfile.server        ← carpai-server 多阶段构建
│   │   └── Dockerfile.cli           ← carpai-cli 多阶段构建
│   ├── helm/                        ← Kubernetes Helm Chart
│   ├── terraform/                   ← IaC (AWS/GCP/Azure)
│   └── systemd/                     ← Linux systemd service unit
│
├── scripts/
│   ├── install.sh                   ← 单机安装脚本
│   ├── remote_build.sh              ← 远程构建 (资源不足时 offload)
│   └── vendor_agentgrep.sh          ← 供应商依赖本地化
│
└── tests/                           ← 集成测试 (跨 crate)
    ├── integration_cli_local.rs     ← CLI 本地模式端到端
    ├── integration_server_api.rs     ← Server REST/gRPC API 测试
    ├── integration_sdk_remote.rs     ← SDK 连接 Server 测试
    └── e2e_full_flow.rs             ← CLI → Server → SDK 全链路
```

---

## 三、依赖关系铁律（最终态）

```
                         ┌─────────────────────┐
                         │   carpai-internal    │  Layer 0: Pure Traits
                         │   (零业务逻辑)        │
                         │   ~2000 行            │
                         └──────────┬──────────┘
                                    │
                    ┌───────────────┼───────────────┐
                    ▼               ▼               ▼
            ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
            │  carpai-core  │ │ carpai-server │ │  carpai-cli   │
            │  Layer 1     │ │ Layer 2a      │ │ Layer 2b      │
            │  ~3000 行    │ │ ~4000 行      │ │ ~3500 行      │
            └──────┬───────┘ └──────┬───────┘ └──────┬───────┘
                   │                │               │
                   │         (可选)  │         (可选)  │
                   ▼                ▼               ▼
            ┌─────────────────────────────────────────────┐
            │              carpai-sdk                     │  Layer 2c
            │              ~2500 行                        │  (轻量库)
            └─────────────────────────────────────────────┘

╔══════════════════════════════════════════════════════════════╗
║  ❌ 禁止的反向依赖 (CI 自动拦截):                          ║
║    • carpai-server → carpai-cli                           ║
║    • carpai-cli → carpai-server                           ║
║    • carpai-core → carpai-server / carpai-cli             ║
║    • carpai-internal → 任何业务 crate                     ║
║    • carpai-sdk → carpai-server (SDK 必须保持轻量)        ║
╚══════════════════════════════════════════════════════════════╝
```

### 各 Crate 依赖清单（最终态）

| Crate | 必选依赖 | 可选依赖 ([feature]) | 禁止依赖 |
|-------|---------|---------------------|---------|
| **carpai-internal** | async-trait, serde, serde_json, chrono, thiserror, uuid, tracing, tokio (sync only) | *(无)* | carpai-core, carpai-server, carpai-cli, reqwest, axum, ratatui |
| **carpai-core** | **carpai-internal**, tokio, serde, serde_json, toml, anyhow, thiserror, tracing, chrono, uuid, futures, dirs, sha2, regex | reqwest (sidecar HTTP) | axum, tonic, ratatui, crossterm, sqlx |
| **carpai-server** | **carpai-core**, **carpai-internal**, tokio, axum, tower, tower-http, tonic, prost, sqlx, redis, jsonwebtoken, tracing-subscriber, opentelemetry, prometheus-client, anyhow, thiserror, chrono, uuid, async-trait | *(无)* | ratatui, crossterm, arboard |
| **carpai-cli** | **carpai-core**, **carpai-internal**, ratatui, crossterm, tokio, clap, serde, toml, chrono, anyhow, thiserror, tracing, uuid | remote = [reqwest, tonic] | axum, sqlx, redis, jsonwebtoken |
| **carpai-sdk** | tokio, reqwest, tonic, prost, serde, serde_json, thiserror, anyhow, tracing, uuid, chrono, futures, async-stream, pin-project-lite, lru, dashmap, config, backoff, zeroize | wasm = [wasm-bindgen, js-sys, web-sys, ...] | carpai-server, ratatui, sqlx |

---

## 四、三个产品的运行时行为

### 4.1 carpai-server (企业服务端)

```bash
# 构建
cargo build -p carpai-server --release

# 运行
./target/release/carpai-server \
    --config /etc/carpai/server.toml \
    # 或环境变量:
    # CARPAI_SERVER__PORT=8080
    # CARPAI_SERVER__DATABASE_URL=postgres://user:pass@db:5432/carpai
    # CARPAI_SERVER__JWT_SECRET=your-secret-here

# 运行时行为
1. 加载 ServerConfig (TOML + env override)
2. 连接 PostgreSQL + Redis (健康检查, 失败则退出)
3. 构建ServerContext:
   - sessions: PgSessionStore (SQLx)
   - tools: SandboxToolExecutor (容器隔离)
   - inference: RoutedInferenceBackend (多模型路由+降级)
   - fs: S3VirtualFileSystem (或 NFS)
   - events: RedisEventBus (pub/sub)
   - memory: PgMemoryBackend (向量扩展)
4. 启动 4 个端口:
   - :50051  gRPC  (tonic)  → AgentService, SessionService, ToolService
   - :8080   REST   (axum)  → /v1/chat/completions (OpenAI 兼容)
   - :8081   WS     (axum)  → /ws/stream (SSE fallback)
   - :8082   Admin  (axum)  → /metrics (Prometheus), /health
5. 信号处理: SIGTERM → graceful shutdown ( drain connections → 10s timeout → exit )
```

**部署拓扑**:
```
                    ┌─────────────┐
                    │  Load Balancer│
                    └──────┬──────┘
              ┌────────────┼────────────┐
              ▼            ▼            ▼
       ┌────────────┐┌────────────┐┌────────────┐
       │carpai-server││carpai-server││carpai-server│
       │  Instance 1 ││  Instance 2 ││  Instance 3 │
       └─────┬──────┘└─────┬──────┘└─────┬──────┘
             │             │             │
        ┌────┴────┐   ┌────┴────┐   ┌────┴────┐
        │PostgreSQL│   │  Redis  │   │  S3/MinIO│
        │ (主/从)  │   │(Cluster)│   │ (对象存储)│
        └─────────┘   └─────────┘   └──────────┘
```

### 4.2 carpai-cli (单机客户端)

```bash
# 构建
cargo build -p carpai-cli --release
# 输出: target/release/carpai (或 carpai.exe on Windows)

# 运行 - 本地模式 (默认)
carpai chat                          # 交互式 TUI
carpai ask "解释这段代码"            # 一次性问答
carpai complete < file.rs           # stdin 代码补全
carpai serve                        # 启动本地 server 子进程

# 运行 - 远程模式 (连接 carpai-server)
carpai chat --remote --url https://carpai.example.com:8080 \
    --api-key sk-xxxxx

# 运行时行为 (Local 模式)
1. 解析 CliConfig (~/.carpai/cli.toml)
2. 构建AgentContext via carpai_core::build_local_agent_context():
   - sessions: LocalFileSessionStore (~/.carpai/sessions/)
   - tools: LocalToolExecutor (直接 bash 执行)
   - inference: SidecarInferenceBackend (http://localhost:11434/v1/chat/completions)
   - fs: LocalFileSystem (当前工作目录, 沙盒限制)
   - events: InProcessEventBus (broadcast channel)
   - memory: LocalMemoryBackend (~/.carpai/memory/)
3. 启动 TUI (ratatui + crossterm raw mode):
   - 渲染聊天视图 + 输入框 + 状态行
   - 用户输入 → AgentBridge.execute_turn() → carpai_core::execute_agent_turn()
   - 工具调用 → 显示确认 → 执行 → 追加结果 → 继续推理循环
4. Ctrl+C / :q → 优雅退出 (保存会话 → 清理临时文件)

# 运行时行为 (Remote 模式)
1-2 同上, 但 BridgeMode::Remote
3. TUI 渲染相同
4. 用户输入 → AgentBridge → HTTP/gRPC → carpai-server → 处理 → 流式返回
```

### 4.3 carpai-sdk (IDE 插件 SDK)

```bash
# 构建 (native)
cargo build -p carpai-sdk --release
# 输出: target/release/carpai_sdk.lib (Windows) / libcarpai_sdk.a (Linux/Mac)

# 构建 (WASM - 用于 VSCode Webview)
cargo build -p carpai-sdk --release --features wasm
# 输出: target/wasm32-unknown-unknown/release/carpai_sdk.wasm

# 发布到 crates.io
cargo publish -p carpai-sdk

# 使用方式 (IDE 插件开发者)
[dependencies]
carpai-sdk = "1.1"

// TypeScript (VSCode Extension via WASM FFI):
// import { complete, chat, createSession } from 'carpai-sdk-wasm';
```

**SDK 使用示例**:
```rust
use carpai_sdk::{CarpAiClient, SdkConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = CarpAiClient::new(SdkConfig {
        api_key: Some("sk-xxxx".into()),
        base_url: "https://carpai.example.com:8080".into(),
        model: "claude-sonnet-4-20250514".into(),
        ..Default::default()
    })?;

    // 代码补全 (OpenAI-compatible)
    let resp = client.complete(completion::Request {
        file_path: "/src/main.rs".into(),
        language: "rust".into(),
        cursor_line: 42,
        cursor_col: 10,
        context: "fn main() { println".into(),
        max_tokens: 50,
    }).await?;

    // 对话
    let chat_resp = client.chat(chat::Request {
        messages: vec![
            chat::Message::system("You are a Rust expert."),
            chat::Message::user("Explain this code."),
        ],
        stream: true,
    }).await?;

    // 流式处理
    while let Some(chunk) = chat_resp.stream.next().await {
        print!("{}", chunk.content);
    }

    Ok(())
}
```

---

## 五、配置系统最终设计

### 三层配置层次

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 2: ServerConfig / CliConfig (产品特定)                │
│  ┌──────────────────┐  ┌──────────────────┐                  │
│  │ ServerConfig     │  │ CliConfig         │                  │
│  │ • listen_addr     │  │ • theme           │                  │
│  │ • port            │  │ • keybindings      │                  │
│  │ • database.*      │  │ • clipboard       │                  │
│  │ • redis.*         │  │ • remote_url      │                  │
│  │ • jwt_secret      │  │ • startup_profile │                  │
│  │ • multi_tenant    │  └────────┬─────────┘                  │
│  │ • rate_limit_*   │           │                           │
│  └────────┬─────────┘           │                           │
│           │  serde(flatten)      │  serde(flatten)            │
│           └───────────┬──────────┘                           │
│                       │                                       │
│  ┌────────────────────▼──────────────────────┐               │
│  │  Layer 1: CoreConfig (业务逻辑)            │               │
│  │  • data_dir                                  │               │
│  │  • session_subdir / memory_subdir           │               │
│  │  • max_concurrent_tools                      │               │
│  │  • max_agent_iterations                      │               │
│  │  • completion_provider { type, endpoint, .. }│               │
│  │  • cache_size_mb / disk_cache_enabled       │               │
│  └────────────────────┬──────────────────────┘               │
│                       │  serde(flatten)                       │
│  ┌────────────────────▼──────────────────────┐               │
│  │  Layer 0: AppConfig (trait 层基础)        │               │
│  │  • mode (Cli/Server/Client)               │               │
│  │  • working_dir / data_dir                 │               │
│  │  • default_model / max_context_tokens      │               │
│  │  • tools_enabled / vfs_enabled            │               │
│  └───────────────────────────────────────────┘               │
└─────────────────────────────────────────────────────────────┘

覆盖优先级: Hardcoded Default → TOML File → Environment Variable
```

### 配置文件位置

| 产品 | 配置文件路径 | 环境变量前缀 |
|------|------------|-------------|
| Server | `/etc/carpai/server.toml` 或 `$CARPAI_CONFIG` | `CARPAI_SERVER__*` |
| CLI | `~/.carpai/cli.toml` | `CARPAI_CLI__*` |
| Core (共享) | `~/.carpai/config.toml` | `CARPAI_CORE__*` / `CARPAI_*` |

---

## 六、关键接口契约

### 6.1 核心入口: `execute_agent_turn()`

```rust
// 定义在: carpai-core/src/agent_loop.rs
// 这是整个系统的"心脏"——唯一的地方将用户输入变为 AI 响应

pub async fn execute_agent_turn(
    ctx: &AgentContext,           // 来自 carpai-internal 的 DI 容器
    user_message: &str,           // 用户原始输入
) -> Result<AgentTurnOutput>;

// 返回值:
pub struct AgentTurnOutput {
    pub text: String,              // 最终文本回复
    pub tool_calls: Vec<ToolCallInfo>, // 本次工具调用记录
    pub usage: TokenUsage,         // Token 用量统计
    pub session_id: SessionId,     // 会话 ID
    pub duration_ms: u64,          // 耗时 (ms)
}
```

**谁调用它？**
- `carpai-cli`: 通过 `AgentBridge` → `execute_agent_turn()` (local 模式)
- `carpai-server`: 通过 `AgentService.ExecuteTask()` RPC handler → `execute_agent_turn()`
- `carpai-sdk`: 不直接调用！SDK 通过 HTTP/gRPC 发请求给 Server

### 6.2 DI 容器组装

```rust
// CLI 模式组装 (carpai-core 提供)
let ctx = carpai_core::build_local_agent_context(&core_config);

// Server 模式组装 (carpai-server 自定义)
let ctx = ServerContext::from_config(&server_config).await?;
// 内部使用 PgSessionStore, SandboxToolExecutor, RoutedInferenceBackend 等
```

### 6.3 gRPC 协议 (Server ↔ SDK/CLI Remote)

```protobuf
// proto/agent.proto
service AgentService {
  rpc ExecuteTask(TaskRequest) returns (TaskResponse);
  rpc StreamCompletion(CompletionRequest) returns (stream CompletionChunk);
}

service SessionService {
  rpc CreateSession(CreateSessionRequest) returns (SessionResponse);
  rpc GetMessages(GetMessagesRequest) returns (GetMessagesResponse);
  rpc AppendMessage(AppendMessageRequest) returns (AppendMessageResponse);
}

service HealthService {
  rpc Check(HealthCheckRequest) returns (HealthCheckResponse);
}
```

---

## 七、代码量估算 (最终态)

| Crate | 估计行数 | 职责 |
|-------|---------|------|
| `carpai-internal` | ~2,000 | 7 个 trait 定义 + 类型 + AgentContext + Builder |
| `carpai-core` | ~3,000 | 6 个 Local 实现 + agent_loop + CoreConfig |
| `carpai-server` | ~4,000 | gRPC/REST/WS/Auth/Enterprise/Observability |
| `carpai-cli` | ~3,500 | TUI/Bridge/Commands/Ambient/Notifications |
| `carpai-sdk` | ~2,500 | Client/Cache/Streaming/Protocol/WASM |
| **合计 (不含 jcode-*)** | **~15,000** | **完整的三产品系统** |
| `jcode-*` (辅助) | ~50,000+ | 保持不变,按需被引用 |

对比当前 `src/` 的 **170+ 模块 ~80,000+ 行**，拆分后各 crate 职责清晰，边界明确。

---

## 八、与当前状态的差距对照

| 维度 | 当前状态 | 最终目标 | 差距 |
|------|---------|---------|------|
| **carpai-internal** | ✅ 编译通过, 7 trait 完成 | 无变化 | ✅ 已达成 |
| **carpai-core** | ⚠️ 骨架在,编译修复中 | 6 个实现全部通过编译 | 🟡 编译错误修复中 |
| **carpai-server** | ⚠️ 骨架在,gRPC/REST 路由搭建 | 完整的企业中间件 | 🟡 需补充 Auth/RBAC/Quota 实现 |
| **carpai-cli** | ⚠️ 骨架在,TUI widget 搭建 | 完整的交互式客户端 | 🟡 需补全 TUI 主循环 |
| **carpai-sdk** | ✅ 可用,v1.1.0-dev | 小幅精简依赖 | 🟢 基本达标 |
| **`src/` 过渡区** | ❌ 170+ 模块,全部历史代码 | ≤ 30 行 re-export | 🔴 最大差距 (需渐进搬迁) |
| **Feature Gate** | ⚠️ server/cli 在根 Cargo.toml | 各 crate 独立构建 | 🟡 需要移除根 crate 的 feature gate |
| **独立发布** | ❌ 无法单独构建任一产品 | `cargo build -p carpai-server` 即可 | 🔴 等待编译通过后验证 |
| **CI 拦截** | ❌ 无依赖方向检查 | 自动拒绝反向依赖 | 🔴 需要建立 pipeline |

---

## 九、验证标准 (如何知道"到了终点")

当以下 **全部条件** 满足时,架构重构完成:

1. **编译关**: `cargo check -p {internal,core,server,cli,sdk}` 分别 0 error 0 warning
2. **独立构建关**: 
   - `cargo build -p carpai-server --release` → 产物 ≤ 15MB (strip 后)
   - `cargo build -p carpai-cli --release` → 产物 ≤ 8MB
   - `cargo build -p carpai-sdk --release` → lib + wasm 均正常
3. **功能关**:
   - Server: `./carpai-server` → 监听 4 端口 → `/health` 返回 OK
   - CLI: `./carpai chat` → TUI 界面可输入 → Local 模式返回响应
   - SDK: `cargo test -p carpai-sdk` → 全部测试通过
4. **集成关**: CLI (`--remote`) → Server → SDK 三者端到端打通
5. **清理关**: `src/lib.rs` ≤ 50 行,且全部为 `pub use xxx::` re-export
6. **安全关**: `cargo deny` / CI 检查通过,无反向依赖违规

---

*"这不是空想的目标架构。每一个文件、每一个依赖、每一个接口都基于对现有代码库的实际扫描。carpai-internal 已经证明这条路走得通;接下来只需要把 carpai-core/server/cli 从'骨架'填成'血肉'。"*
