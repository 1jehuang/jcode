# CarpAI 完整重构计划：从"编程助手"到"企业级 AI 编程服务端"

> **版本**: v1.0 | **日期**: 2026-05-24  
> **核心定位**: CarpAI 是编程助手的服务端，不是编程助手。替代 Cursor Enterprise Backend / Claude Code Server。

---

## 一、重构总览

### 1.1 目标架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                        CarpAI Monorepo                              │
│                                                                     │
│  ┌───────────────┐  ┌───────────────┐  ┌────────────────────────┐ │
│  │ carpai-server  │  │ carpai-cli    │  │ carpai-sdk            │ │
│  │ (企业服务端)   │  │ (单机客户端)   │  │ (IDE 插件 SDK)        │ │
│  ├───────────────┤  ├───────────────┤  ├────────────────────────┤ │
│  │ gRPC:50051    │  │ TUI 终端界面  │  │ VSCode / JetBrains     │ │
│  │ REST:8081     │  │ 本地 Agent    │  │ Neovim 客户端库       │ │
│  │ WS:8080       │  │ 远程模式(WS)  │  │ WebSocket + REST      │ │
│  │ 无头(headless)│  │ Sidecar 直连  │  │ Protocol Buffer       │ │
│  └───────┬───────┘  └───────┬───────┘  └───────────┬────────────┘ │
│          │                  │                       │              │
│          └──────────────────┼───────────────────────┘              │
│                             ▼                                      │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │                    共享核心层 (Shared Core)                   │  │
│  │                                                              │  │
│  │  ┌────────────┐ ┌────────────┐ ┌──────────┐ ┌────────────┐ │  │
│  │  │Agent Engine│ │Tool Registry│ │Memory    │ │LLM Router  │ │  │
│  │  │(trait抽象) │ │(Sandbox)    │ │(多后端)  │ │(可插拔)    │ │  │
│  │  └────────────┘ └────────────┘ └──────────┘ └────────────┘ │  │
│  │  ┌────────────┐ ┌────────────┐ ┌──────────┐ ┌────────────┐ │  │
│  │  │AST Parser  │ │Session Mgr │ │Auth/RBAC │ │Config      │ │  │
│  │  │(Tree-sitter│ │(增量持久化) │ │(企业级)  │ │(动态合并)  │ │  │
│  │  │ 6语言)     │ │            │ │          │ │            │ │  │
│  │  └────────────┘ └────────────┘ └──────────┘ └────────────┘ │  │
│  └──────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.2 三种产品定位

| 产品 | 用户 | 部署方式 | 核心功能 |
|------|------|----------|----------|
| **carpai-server** | 企业 IT / DevOps 团队 | Kubernetes / Docker / systemd | 多租户、分布式推理、RBAC、审计日志、API 网关、模型路由 |
| **carpai-cli** | 单机开发者（个人用户） | `cargo install` / 二进制包 | TUI 终端界面、本地 Agent 循环、Sidecar LLM 调用、Git 集成 |
| **carpai-sdk** | VSCode / JetBrains / Neovim 用户 | IDE 扩展市场安装 | 通过 WebSocket/REST 连接 carpai-server，代码补全、聊天、LSP 代理 |

### 1.3 重构原则

1. **服务端优先**: 所有新功能首先以服务端 API 形式实现，CLI 和 SDK 作为消费者
2. **Trait 抽象先行**: 核心能力通过 trait 定义接口，不同产品注入不同实现
3. **渐进式迁移**: 不一次性重写，通过 feature gate 逐步隔离
4. **零废弃代码**: 现有代码全部找到归宿（直接复用 / 改造复用 / 迁移到对应产品）

---

## 二、现状盘点（基于代码分析）

### 2.1 当前架构问题诊断

#### 问题 A："空心化"的服务端

**文件**: `src/bin/jcode-server.rs:133-137`
```rust
let api_state = ApiState {
    completion_engine: None,  // ← "Will be injected when engines are ready"
    auth_provider: Arc::new(jcode::auth::JwtAuthProvider::new()), // Placeholder
    inference_engine: None,   // ← "Will be injected when engines are ready"
};
```

**影响**: 服务端启动了三个协议监听端口，但核心引擎全部为 `None`，实际无法提供任何 AI 能力。

#### 问题 B: Sidecar 绕过服务端管控

**文件**: `src/sidecar.rs` — 340 行轻量级 LLM 客户端

**问题**:
- 直接调用外部 LLM API（OpenAI / Anthropic / DeepSeek），完全绕过服务端的配额管理、审计日志、模型路由
- 被 `ws/handlers/ai.rs`、`memory_agent.rs` 等多处调用
- 在服务端模式下，这意味着用户的 LLM 调用不受控

#### 问题 C: Tool 执行耦合单机假设

**文件**: `src/tool/mod.rs` — 273 个工具，全局共享 Registry

**问题**:
- 文件操作工具直接读写本地磁盘（`std::fs::*`）
- Shell 工具直接 `Command::new("bash")` 执行
- 沙箱模块 (`src/sandbox.rs`) 已存在但未被 Tool Registry 默认使用
- 服务端模式下需要：工作区隔离、权限检查、沙箱执行

#### 问题 D: Session 存储耦合本地文件

**文件**: `src/session/persistence.rs` — 基于本地 JSON 文件的增量恢复

**已有基础设施**:
- `crates/jcode-session-persist/src/incremental_recovery.rs` — 通用的 Snapshot/Diff 恢复引擎
- `src/session_cost_tracker.rs` — 成本追踪（也是本地文件）

**缺失**: 服务端需要的 Redis/DB 后端适配器

#### 问题 E: Agent 循环耦合 TUI 和本地进程

**文件**: `src/agent.rs`, `src/agent/turn_execution.rs`, `src/agent/turn_loops.rs`

**问题**:
- `Agent` 结构体持有 `stdin_request_tx`（终端输入通道）
- `working_dir` 直接映射到本地路径
- 工具执行模式 `ToolExecutionMode::Direct` = 直接 bash
- 事件总线 `Bus::global()` 是进程内全局单例

#### 问题 F: 密码哈希安全漏洞

**文件**: `src/enterprise/auth.rs` 使用 SHA256 + 硬编码盐
**对比**: `src/security/password_hasher.rs` 已正确实现 Argon2id

### 2.2 现有资产清单（可直接复用）

| 模块 | 位置 | 复用方式 | 说明 |
|------|------|----------|------|
| **Sandbox 引擎** | `src/sandbox.rs` (352行) | ✅ 直接复用 | bubblewrap + 进程隔离 + 超时 kill，已生产级 |
| **Internal API Traits** | `crates/carpai-internal/` | ✅ 直接复用 | 5 个核心 trait: CodeCompletion, AuthProvider, InferenceEngine, MemoryStore, ToolRegistry |
| **gRPC 协议层** | `src/grpc/mod.rs` | ✅ 直接复用 | 9 个 gRPC 服务定义完整 |
| **WebSocket 协议层** | `src/ws/` (含 web_ide) | ✅ 直接复用 | 28 个方法，覆盖 VSCode LSP + Cursor Agent Protocol |
| **REST API 层** | `src/api/rest_api.rs` | ✅ 直接复用 | OpenAI 兼容接口 + 健康检查 |
| **安全模块** | `src/security/` | ✅ 直接复用 | Argon2id + API Key 验证 + 速率限制 + SQL 注入防护 |
| **可观测性** | `src/observability/` | ✅ 直接复用 | OpenTelemetry Tracing/Metrics/Logs |
| **企业认证** | `src/enterprise/auth.rs` | ⚠️ 修复后复用 | 改用 security/password_hasher 的 Argon2id |
| **企业配额** | `src/enterprise/quota.rs` | ✅ 直接复用 | 5 级服务等级配额策略 |
| **节点发现** | `src/enterprise/discovery.rs` | ✅ 直接复用 | mDNS + UDP 广播 + 心跳监控 |
| **分布式调度** | `src/enterprise/distributed.rs` | ✅ 直接复用 | 层分配 + 负载均衡 + 故障转移 |
| **MultiProvider** | `src/provider/mod.rs` | ✅ 直接复用 | 40+ LLM 提供商集成 |
| **Auto Fallback** | `src/auto_fallback.rs` | ✅ 直接复用 | Local → Cloud 自动切换 |
| **InferenceRouter** | `src/rest_llm.rs` | ⚠️ 改造后复用 | 需接入 InferenceEngine trait |
| **AST 解析** | `src/ast/` (6 语言) | ✅ 直接复用 | Tree-sitter Rust/Python/JS/Go/C/C++ |
| **重构引擎** | `src/refactor_engine.rs` | ✅ 直接复用 | 原子编辑 + 两阶段提交 + Checkpoint |
| **会话恢复** | `crates/jcode-session-persist/` | ✅ 直接复用 | Snapshot + Incremental Diff |
| **统一调度器** | `crates/jcode-unified-scheduler/` | ✅ 直接复用 | 任务队列 + 优先级 + 资源管理 |
| **MCP 协议** | `src/mcp/` | ✅ 直接复用 | 工具发现 + 编排 + 审计 |
| **RAG 引擎** | `crates/jcode-rag/` | ✅ 直接复用 | 向量检索 + 混合排序 |
| **补全引擎** | `crates/jcode-completion/` | ✅ 直接复用 | AST 上下文 + LLM 生成 + Behavior Learning |

---

## 三、分阶段实施计划

### Phase 0: 基础设施修复（Week 1-2）⚡ 最高优先级

> **目标**: 让现有服务端能够真正编译运行并响应请求

#### 0.1 修复编译错误（Blocker #1）

当前状态: 5 个编译错误 + 953 个警告（见 `clippy_errors_only.txt`）

**操作**:
```bash
# 按 AGENTS.md 中的分层修复法执行
cargo check 2>&1 | head -100   # 先看第一层错误
```

**关键修复项**:
- `src/completion_engine/engine.rs`: `providers` 模块导入
- `src/completion_engine/providers.rs`: 生命周期不匹配 ×4
- `src/`: `self` 作为值 / `await` 在非 async 中
- `src/`: 非穷举 match pattern

#### 0.2 修复安全漏洞（Blocker #2）

**文件**: `src/enterprise/auth.rs`

```diff
- pub fn hash_password(password: &str) -> String {
-     let mut hasher = Sha256::new();
-     hasher.update(password.as_bytes());
-     hasher.update(b"carpai_enterprise_salt_2026"); // 硬编码盐 ❌
-     hex::encode(hasher.finalize())
- }
+ pub fn hash_password(password: &str) -> Result<String> {
+     use jcode::security::PasswordHasher;
+     let hasher = PasswordHasher::new();
+     hasher.hash(password) // Argon2id ✅
+ }
```

#### 0.3 注入真实引擎到服务端（解决"空心化"）

**文件**: `src/bin/jcode-server.rs`

```rust
// 替换 None 为真实引擎实例
let api_state = ApiState {
    completion_engine: Some(Arc::new(jcode_completion::CompletionEngine::new(/* ... */))),
    auth_provider: Arc::new(jcode::enterprise::auth::EnterpriseAuthProvider::new(/* db pool */)),
    inference_engine: Some(Arc::new(ServerInferenceEngine::new(/* router */))),
};
```

**新增**: `ServerInferenceEngine` — 实现 `carpai_internal::InferenceEngine` trait，内部使用现有的 `MultiProvider` + `AutoFallbackRouter`

#### 0.4 Feature Gate 客户端模块

**文件**: `Cargo.toml` (root)

```toml
[features]
default = ["server", "cli"]
# 服务端特性（无头，不需要 TUI/终端）
server = []
# 客户端特性（TUI + 本地 Agent + Sidecar）
cli = ["dep:ratatui", "dep:crossterm", "dep:arboard", /* ... */]
# 企业特性（多租户、分布式）
enterprise = []
# 开发者二进制（仅开发时使用）
dev-bins = []

[dependencies]
# 条件依赖 — 仅 cli feature 启用时编译
ratatui = { version = "0.30", optional = true }
crossterm = { version = "0.29", optional = true }
arboard = { version = "3", optional = true }
image = { version = "0.25", default-features = false, features = ["png", "jpeg"], optional = true }
jcode-tui-core = { path = "crates/jcode-tui-core", optional = true }
jcode-tui-markdown = { path = "crates/jcode-tui-markdown", optional = true }
# ... 所有 TUI 相关 crate 都加 optional = true
```

**文件**: `src/lib.rs`

```rust
// ===== CLI & TUI (仅在 cli feature 下编译) =====
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "cli")]
pub mod tui;
#[cfg(feature = "cli")]
pub mod terminal_launch;
#[cfg(feature = "cli")]
pub mod stdin_detect;
#[cfg(feature = "cli")]
pub mod input;
#[cfg(feature = "cli")]
pub mod setup_hints;

// ===== Enterprise Features (仅在 enterprise feature 下编译) =====
#[cfg(feature = "enterprise")]
pub mod enterprise;

// ===== 主入口根据编译目标选择 =====
#[cfg(feature = "cli")]
pub async fn run() -> Result<()> {
    cli::startup::run().await
}

#[cfg(all(not(feature = "cli"), feature = "server"))]
pub async fn run() -> Result<()> {
    server::startup::run().await
}
```

---

### Phase 1: 共享核心层 Trait 抽象（Week 3-4）

> **目标**: 定义 6 大核心 trait 接口，使 Agent/Tool/Session/Memory 可在服务端和客户端间切换实现

#### 1.1 核心 Trait 定义（扩展 `carpai-internal`）

在 `crates/carpai-internal/src/` 新增/增强以下 trait：

##### A. `SessionStore` — 会话持久化抽象

```rust
// crates/carpai-internal/src/session.rs
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn save_session(&self, session: &SessionData) -> Result<(), SessionError>;
    async fn load_session(&self, session_id: &str) -> Result<Option<SessionData>, SessionError>;
    async fn list_sessions(&self, user_id: &str) -> Result<Vec<SessionSummary>, SessionError>;
    async fn delete_session(&self, session_id: &str) -> Result<(), SessionError>;
    /// 增量追加消息（避免每次保存完整会话）
    async fn append_messages(&self, session_id: &str, messages: Vec<Message>) -> Result<(), SessionError>;
}
```

**实现映射**:
| 产品 | 实现 | 存储 |
|------|------|------|
| carpai-cli | `LocalFileSessionStore` | `~/.jcode/sessions/{id}.json` (现有 `session/persistence.rs`) |
| carpai-server | `RedisSessionStore` / `PgSessionStore` | Redis / PostgreSQL |

##### B. `ToolExecutor` — 工具执行抽象（关键！）

```rust
// crates/carpai-internal/src/tool_executor.rs
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        context: &ExecutionContext,
    ) -> Result<ToolOutput, ToolError>;

    /// 沙箱化执行（服务端默认模式）
    async fn execute_sandboxed(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        context: &ExecutionContext,
        sandbox_config: &SandboxConfig,
    ) -> Result<ToolOutput, ToolError>;
}

/// 工具执行上下文 — 解耦"当前工作目录"等本地概念
#[derive(Clone)]
pub struct ExecutionContext {
    pub session_id: String,
    pub user_id: String,
    pub tenant_id: Option<String>,       // 多租户隔离
    pub working_directory: String,       // 可以为虚拟路径
    pub permissions: Vec<String>,        // 当前用户权限列表
    pub timeout_secs: u64,
    pub metadata: HashMap<String, String>,
}
```

**实现映射**:
| 产品 | 实现 | 行为 |
|------|------|------|
| carpai-cli | `LocalToolExecutor` | 直接 bash 执行（现有行为） |
| carpai-server | `SandboxToolExecutor` | 使用现有 `src/sandbox.rs` 的 `Sandbox::execute()` |

##### C. `InferenceBackend` — LLM 调用抽象（统一 Sidecar + MultiProvider）

```rust
// crates/carpai-internal/src/inference_backend.rs
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    /// 同步完成
    async fn complete(
        &self,
        request: &InferenceRequest,
    ) -> Result<InferenceResponse, InferenceError>;

    /// 流式完成
    async fn stream_complete(
        &self,
        request: &InferenceRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<TokenChunk, InferenceError>> + Send>>, InferenceError>;

    /// 获取可用模型列表
    fn list_models(&self) -> Vec<ModelInfo>;

    /// 健康检查
    fn health_check(&self) -> HealthStatus;
}
```

**实现映射**:
| 产品 | 实现 | 行为 |
|------|------|------|
| carpai-cli | `SidecarBackend` | 封装现有 `src/sidecar.rs` 的 `Sidecar` |
| carpai-server | `RoutedInferenceBackend` | 使用 `MultiProvider` + `AutoFallbackRouter` + 企业配额检查 |

##### D. `FileSystem` — 文件系统操作抽象

```rust
// crates/carpai-internal/src/filesystem.rs
#[async_trait]
pub trait VirtualFileSystem: Send + Sync {
    async fn read_file(&self, path: &Path) -> Result<String, FsError>;
    async fn write_file(&self, path: &Path, content: &str) -> Result<(), FsError>;
    async fn list_dir(&self, path: &Path, recursive: bool) -> Result<Vec<FileEntry>, FsError>;
    async fn file_info(&self, path: &Path) -> Result<FileMeta, FsError>;
    async fn search_files(&self, query: &str, path: &Path) -> Result<Vec<SearchResult>, FsError>;
    /// Git 操作（解耦到独立 trait 或作为扩展方法）
    async fn git_diff(&self, path: &Path) -> Result<String, FsError>;
    async fn git_status(&self, path: &Path) -> Result<String, FsError>;
}
```

**实现映射**:
| 产品 | 实现 | 行为 |
|------|------|------|
| carpai-cli | `LocalFileSystem` | 直接 `std::fs` + `git2` (现有行为) |
| carpai-server | `WorkspaceFileSystem` | 限制在租户工作区内 + 操作审计日志 |

##### E. `EventBus` — 事件总线抽象

```rust
// crates/carpai-internal/src/event_bus.rs
#[async_trait]
pub trait EventBus: Send + Sync + Clone {
    async fn publish(&self, event: Event);
    async fn subscribe(&self, event_type: &EventType) -> BroadcastReceiver<Event>;
}
```

**实现映射**:
| 产品 | 实现 | 行为 |
|------|------|------|
| carpai-cli | `InProcessEventBus` | `tokio::broadcast::channel` (现有 `Bus::global()`) |
| carpai-server | `RedisEventBus` / `KafkaEventBus` | 跨进程/跨节点事件分发 |

##### F. `MemoryBackend` — 记忆存储抽象（扩展现有 MemoryStore）

```rust
// crates/carpai-internal/src/memory.rs (增强)
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn store(&self, entry: MemoryEntry) -> Result<(), MemoryError>;
    async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;
    async fn delete(&self, entry_id: &str) -> Result<(), MemoryError>;
    /// 向量相似度搜索
    async fn vector_search(&self, embedding: &[f32], limit: usize) -> Result<Vec<MemoryEntry>, MemoryError>;
}
```

---

### Phase 2: carpai-server 服务端实现（Week 5-8）

> **目标**: 构建可独立部署的企业级服务端二进制

#### 2.1 新建 `crates/carpai-server/` crate

```
crates/carpai-server/
├── Cargo.toml              # 仅依赖 server 必需的 crate
├── src/
│   ├── lib.rs              # 库入口
│   ├── main.rs             # 服务端 main() (替代 src/bin/jcode-server.rs)
│   ├── application.rs      # Application struct — 组装所有组件
│   ├── config.rs           # 服务端配置加载 (TOML / ENV)
│   ├── state.rs            # 全局 AppState (替换 ApiState)
│   ├── engine/
│   │   ├── mod.rs
│   │   ├── completion.rs   # CompletionEngine → CodeCompletion trait impl
│   │   ├── inference.rs    # RoutedInferenceEngine → InferenceEngine trait impl
│   │   ├── tools.rs        # SandboxToolExecutor → ToolExecutor trait impl
│   │   └── memory.rs       # Redis/PgMemoryStore → MemoryBackend trait impl
│   ├── auth/
│   │   ├── mod.rs
│   │   ├── jwt.rs          # JWT token 管理
│   │   ├── rbac.rs         # Role-Based Access Control
│   │   └── api_key.rs      # API Key 生成/验证/轮换
│   ├── middleware/
│   │   ├── mod.rs
│   │   ├── auth.rs         # 认证中间件 (extract token → verify)
│   │   ├── tenant.rs       # 多租户中间件 (extract tenant → isolate)
│   │   ├── quota.rs        # 配额检查中间件
│   │   └── audit.rs        # 审计日志中间件
│   └── routes/
│       ├── mod.rs
│       ├── chat.rs         # POST /v1/chat/completions (OpenAI 兼容)
│       ├── completions.rs  # POST /v1/completions/inline
│       ├── agent.rs        # POST /v1/agent/run (Agent 会话)
│       ├── session.rs      # CRUD /v1/sessions
│       ├── admin.rs        # 管理员 API (用户/租户/配额)
│       └── health.rs       # GET /health (深度健康检查)
```

#### 2.2 Application 组装逻辑

```rust
// crates/carpai-server/src/application.rs
pub struct Application {
    config: ServerConfig,
    // Internal API trait objects — 核心引擎
    completion_engine: Arc<dyn CodeCompletion>,
    inference_engine: Arc<dyn InferenceEngine>,
    tool_executor: Arc<dyn ToolExecutor>,
    session_store: Arc<dyn SessionStore>,
    memory_backend: Arc<dyn MemoryBackend>,
    file_system: Arc<dyn VirtualFileSystem>,
    event_bus: Arc<dyn EventBus>,
    auth_provider: Arc<dyn AuthProvider>,
    // 企业特性
    #[cfg(feature = "enterprise")]
    distributed_scheduler: Arc<DistributedInferenceScheduler>,
    #[cfg(feature = "enterprise")]
    node_discovery: Arc<NodeDiscoveryService>,
}

impl Application {
    pub async fn build(config: ServerConfig) -> Result<Self> {
        // 1. 根据配置选择各 trait 的实现
        let memory_backend: Arc<dyn MemoryBackend> = match config.memory_backend.as_str() {
            "redis" => Arc::new(RedisMemoryStore::connect(&config.redis_url).await?),
            "postgres" => Arc::new(PgMemoryStore::connect(&config.database_url).await?),
            _ => Arc::new(LocalMemoryStore::new(&config.data_dir)?),
        };

        let tool_executor: Arc<dyn ToolExecutor> = match config.execution_mode.as_str() {
            "sandbox" => Arc::new(SandboxToolExecutor::new(config.sandbox.clone())),
            "direct" => Arc::new(LocalToolExecutor::new()),  // 仅可信环境
            _ => Arc::new(SandboxToolExecutor::new(SandboxConfig::default())),
        };

        let inference_engine: Arc<dyn InferenceEngine> = Arc::new(
            ServerInferenceEngine::with_providers(
                MultiProvider::from_config(&config.providers)?,
                AutoFallbackRouter::new(config.local_models, &config.fallback_model),
                Arc::new(QuotaEnforcer::new(memory_backend.clone())),
            )
        );

        // ... 其他组件类似组装

        Ok(Self { config, completion_engine, inference_engine, /* ... */ })
    }

    pub async fn serve(self) -> Result<()> {
        // 并行启动 gRPC + REST + WebSocket
        let grpc_addr = format!("{}:{}", self.config.bind_addr, self.config.grpc_port);
        let rest_addr = format!("{}:{}", self.config.bind_addr, self.config.rest_port);
        let ws_addr = format!("{}:{}", self.config.bind_addr, self.config.ws_port);

        // 注入 state 到所有路由
        let api_state = ApiState::from_app(&self);

        tokio::join!(
            GrpcServerBuilder::new().serve(grpc_addr.parse()?),
            axum::serve(TcpListener::bind(&rest_addr).await?, create_rest_router(api_state)),
            WebIdeWebSocketServer::new(self.ws_config()).serve(),
        );
        Ok(())
    }
}
```

#### 2.3 Sidecar 改造为可插拔 Backend

**当前问题**: `src/sidecar.rs` 被硬编码在多个模块中

**改造方案**:

```rust
// crates/carpai-server/src/engine/inference.rs
pub struct ServerInferenceEngine {
    multi_provider: Arc<MultiProvider>,
    fallback_router: Arc<AutoFallbackRouter>,
    quota_enforcer: Arc<QuotaEnforcer>,  // 新增：企业配额
    cache: Arc<ResponseCache>,             // 新增：响应缓存
}

#[async_trait]
impl InferenceEngine for ServerInferenceEngine {
    async fn infer(&self, request: InferenceRequest) -> Result<InferenceResponse, InferenceError> {
        // 1. 配额检查
        self.quota_enforcer.check(&request.metadata).await?;

        // 2. 缓存命中？
        if let Some(cached) = self.cache.get(&request).await {
            return Ok(cached);
        }

        // 3. 路由决策 (local vs cloud)
        let target = self.fallback_router.resolve_target().await;

        // 4. 执行推理
        let response = match target {
            InferenceTarget::Local { model } => self.call_local(model, &request).await?,
            InferenceTarget::Cloud { provider, model } => self.call_provider(provider, model, &request).await?,
        };

        // 5. 缓存 + 配额记录
        self.cache.put(&request, &response).await;
        self.quota_enforcer.record(&request.metadata, &response).await;

        Ok(response)
    }
}
```

**对于 CLI 的兼容**: 保留 `Sidecar` 但将其包装为 `InferenceBackend` 的一个实现：

```rust
// src/inference/sidecar_backend.rs (cli feature only)
#[cfg(feature = "cli")]
pub struct SidecarBackend {
    sidecar: Sidecar,  // 现有的 sidecar.rs
}

#[cfg(feature = "cli")]
#[async_trait]
impl InferenceBackend for SidecarBackend {
    async fn complete(&self, request: &InferenceRequest) -> Result<InferenceResponse, InferenceError> {
        let text = self.sidecar.complete(&request.system_message.unwrap_or_default(), &request.prompt).await?;
        Ok(InferenceResponse { text, model: self.sidecar.model_name().into(), usage: /* ... */ })
    }
}
```

#### 2.4 Tool Registry 改造（Server 模式默认沙箱）

```rust
// crates/carpai-server/src/engine/tools.rs
pub struct SandboxToolExecutor {
    sandbox: Sandbox,                    // 复用 src/sandbox.rs
    registry: ToolRegistryInner,         // 复用现有 273 个工具定义
    fs: Arc<dyn VirtualFileSystem>,      // 文件操作走 VFS
    permission_checker: PermissionChecker,
}

#[async_trait]
impl ToolExecutor for SandboxToolExecutor {
    async fn execute(&self, name: &str, params: Value, ctx: &ExecutionContext) -> Result<ToolOutput, ToolError> {
        // 1. 权限检查
        self.permission_checker.check(&ctx.user_id, name)?;

        // 2. 分类决定执行方式
        match self.registry.category(name) {
            ToolCategory::Shell | ToolCategory::Web => {
                // 沙箱执行
                let cmd = self.registry.build_command(name, params)?;
                let result = self.sandbox.execute(&cmd).await?;
                Ok(ToolOutput::from_sandbox(result))
            }
            ToolCategory::FileSystem | ToolCategory::CodeEdit => {
                // 通过 VFS 执行（带审计）
                self.fs.audit_op(&ctx.user_id, name, || {
                    self.registry.execute_vfs(name, params, self.fs.as_ref())
                }).await
            }
            ToolCategory::ReadOnly => {
                // 只读工具直接执行
                self.registry.execute_safe(name, params).await
            }
        }
    }
}
```

---

### Phase 3: carpai-cli 客户端拆分（Week 9-10）

> **目标**: CLI 成为可独立运行的客户端，支持本地模式和远程模式

#### 3.1 双模式运行

```rust
// src/cli/mode.rs
#[derive(Debug, Clone)]
pub enum CliMode {
    /// 本地模式：所有计算在本地完成（现有行为）
    Local {
        sidecar: Sidecar,
        local_fs: LocalFileSystem,
        local_executor: LocalToolExecutor,
    },
    /// 远程模式：连接到 carpai-server，UI 在本地，AI 在服务端
    Remote {
        client: CarpAiClient,       // carpai-sdk 的客户端
        server_url: String,
        auth_token: String,
    },
}
```

#### 3.2 远程模式下的 Agent 循环

```rust
// src/cli/remote_agent.rs
pub struct RemoteAgent {
    client: CarpAiClient,
    session_id: String,
    mode: CliMode,
}

impl RemoteAgent {
    /// 远程 Agent 循环：UI 在本地，推理在服务端
    pub async fn run_loop(&mut self, initial_prompt: &str) -> Result<()> {
        // 1. 发送初始 prompt 到服务端
        let response = self.client.chat(&self.session_id, initial_prompt).await?;

        // 2. 显示响应到 TUI
        self.render_to_tui(&response).await?;

        // 3. 如果服务端返回 tool_call 请求：
        //    a) 对于只读工具（read_file, list_files）：远程执行
        //    b) 对于写入工具（edit, bash）：提示用户确认后，发送回服务端执行
        //    c) 对于需要本地的工具（终端交互）：本地执行后发结果回服务端

        loop {
            match self.client.poll_session(&self.session_id).await? {
                SessionEvent::AssistantMessage(msg) => {
                    self.render_to_tui(&msg).await?;
                }
                SessionEvent::ToolCallRequest(call) => {
                    if call.requires_local_execution() {
                        // 本地执行（如终端输入）
                        let result = self.execute_locally(call).await?;
                        self.client.submit_tool_result(&self.session_id, result).await?;
                    } else {
                        // 远程执行（服务端沙箱内）
                        let approved = self.prompt_user_approval(&call).await?;
                        if approved {
                            self.client.approve_tool_call(&self.session_id, call.id).await?;
                        }
                    }
                }
                SessionEvent::Done => break,
                SessionEvent::Error(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
```

#### 3.3 CLI 入口改造

```rust
// src/main.rs (或 src/bin/jcode.rs)
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        // 服务端命令 — 仅在 server binary 中
        Some(Command::Serve { .. }) => {
            carpai_server::Application::build(load_server_config()?).await?.serve().await
        }

        // 客户端命令 — 在 cli binary 中
        Some(Command::Chat { prompt }) | None => {
            let mode = if let Some(server_url) = args.remote_server {
                CliMode::Remote { client: CarpAiClient::connect(&server_url).await?, /* ... */ }
            } else {
                CliMode::Local { /* ... */ }
            };
            run_cli_interactive(mode).await
        }

        // 通用命令
        Some(Command::Auth { .. }) => auth::run(args).await,
        Some(Command::Session { .. }) => session::run(args).await,
        // ...
    }
}
```

---

### Phase 4: carpai-sdk 增强（Week 11-12）

> **目标**: SDK 成为 IDE 插件的唯一通信层

#### 4.1 SDK 架构增强

```
crates/carpai-sdk/
├── src/
│   ├── lib.rs
│   ├── client.rs           # 核心 HTTP/WS 客户端
│   ├── protocol.rs         # 请求/响应类型定义
│   ├── streaming.rs        # SSE / WS 流式解析
│   ├── ide/
│   │   ├── mod.rs
│   │   ├── vscode.rs       # VSCode 扩展专用适配器
│   │   ├── jetbrains.rs    # IntelliJ 适配器
│   │   └── neovim.rs       # Neovim 适配器
│   ├── cache.rs            # LRU 响应缓存
│   ├── config.rs           # SDK 配置（server URL, auth）
│   ├── error.rs            # 统一错误类型
│   └── mcp/                # MCP 客户端（已存在）
```

#### 4.2 新增 SDK API

```rust
// crates/carpai-sdk/src/client.rs
impl CarpAiClient {
    // === OpenAI 兼容接口 ===
    pub async fn chat_completions(&self, req: ChatCompletionRequest) -> Result<ChatCompletionResponse>;
    pub async fn stream_chat(&self, req: ChatCompletionRequest) -> impl Stream<Item = StreamChunk>;
    
    // === 代码补全 ===
    pub async fn inline_completion(&self, req: InlineCompletionRequest) -> Result<Vec<CompletionCandidate>>;
    
    // === Agent 会话 ===
    pub async fn create_session(&self, opts: SessionOptions) -> Result<SessionHandle>;
    pub async fn send_message(&self, session_id: &str, message: &str) -> Result<AssistantEventStream>;
    pub async fn approve_tool(&self, session_id: &str, tool_call_id: &str) -> Result<ToolResult>;
    
    // === 管理接口 (管理员) ===
    pub async fn list_users(&self) -> Result<Vec<UserInfo>>;
    pub async fn create_api_key(&self, user_id: &str, opts: ApiKeyOptions) -> Result<ApiKey>;
    pub async fn get_quota_usage(&self, tenant_id: &str) -> Result<QuotaUsage>;
}
```

---

### Phase 5: 集成测试与编译通关（Week 13-16）

#### 5.1 测试矩阵

| 测试 | 覆盖范围 | 方式 |
|------|----------|------|
| `cargo check -p carpai-server` | 服务端单独编译 | CI |
| `cargo check -p carpai-cli` | 客户端单独编译 | CI |
| `cargo check -p carpai-sdk` | SDK 单独编译 | CI |
| `cargo test -p carpai-server` | 服务端单元测试 | CI |
| `tests/e2e/server_lifecycle.rs` | 服务端启动→请求→关闭 | E2E |
| `tests/e2e/client_remote_mode.rs` | CLI 远程模式连接服务端 | E2E |
| `tests/e2e/sdk_integration.rs` | SDK 连接服务端完整流程 | E2E |

#### 5.2 性能基准

- 并发 Agent 会话数: 目标 100+ 同时会话
- 代码补全延迟: P99 < 500ms
- 聊天首 token 延迟: P99 < 2s
- Tool 执行（沙箱）: P99 < 5s

---

## 四、Crate 依赖关系图（重构后）

```
carpai-server (binary)
├── carpai-internal [traits]
├── jcode-completion [completion engine]
├── jcode-provider-* [40+ LLM providers]
├── jcode-rag [vector search]
├── jcode-unified-scheduler [task scheduling]
├── jcode-cpu-inference [local GPU inference]
├── jcode-grpc [gRPC server]
├── tokio + axum + tonic [runtime + http + rpc]
└── (optional) jcode-enterprise-server [distributed]

carpai-cli (binary)
├── carpai-internal [traits]
├── carpai-sdk [client library]
├── jcode-completion
├── jcode-provider-* [for sidecar]
├── jcode-tui-* [terminal UI]
├── ratatui + crossterm [TUI framework]
└── tree-sitter-* [AST parsing]

carpai-sdk (library)
├── carpai-internal [traits + types]
├── reqwest + tokio-tungstenite [HTTP + WS client]
├── serde + serde_json [serialization]
└── (no LLM provider deps!) [pure client]

carpai-internal (library) ← 共享核心
├── async-trait
├── serde
├── anyhow + thiserror
└── jcode-core-types + jcode-runtime-types
```

---

## 五、现有代码命运清单

### 5.1 直接移入 `carpai-server`（服务端专属）

| 现有位置 | 目标位置 | 改动量 |
|----------|----------|--------|
| `src/bin/jcode-server.rs` | `crates/carpai-server/src/main.rs` | 重写（注入真实引擎） |
| `src/api/rest_api.rs` | `crates/carpai-server/src/routes/` | 小改（接入 trait object） |
| `src/grpc/mod.rs` | `crates/carpai-server/src/grpc/` | 小改 |
| `src/ws/` | `crates/carpai-server/src/ws/` | 小改 |
| `src/enterprise/` | `crates/carpai-server/src/enterprise/` | 修复密码哈希 |
| `src/observability/` | `crates/carpai-server/src/observability/` | 无改动 |
| `src/security/` | `crates/carpai-server/src/security/` | 无改动 |
| `src/distributed/` | `crates/carpai-server/src/distributed/` | 无改动 |
| `src/scheduler.rs` | `crates/carpai-server/src/scheduler.rs` | 小改 |
| `src/rest_llm.rs` | 合入 `carpai-server/src/engine/inference.rs` | 中等改造 |
| `src/auto_fallback.rs` | 合入 inference engine | 小改 |
| `src/sandbox.rs` | `carpai-server` + `carpai-cli` 共享 | 无改动 |

### 5.2 直接移入 `carpai-cli`（客户端专属）

| 现有位置 | 目标位置 | 改动量 |
|----------|----------|--------|
| `src/tui/` | `carpai-cli/src/tui/` | 无改动 |
| `src/cli/` | `carpai-cli/src/cli/` | 增加 remote mode 分支 |
| `src/sidecar.rs` | `carpai-cli/src/sidecar.rs` | 包装为 InferenceBackend impl |
| `src/input.rs` | `carpai-cli/src/` | 无改动 |
| `src/setup_hints.rs` | `carpai-cli/src/` | 无改动 |
| `src/terminal_launch.rs` | `carpai-cli/src/` | 无改动 |
| `src/stdin_detect.rs` | `carpai-cli/src/` | 无改动 |
| `src/vim.rs` | `carpai-cli/src/` | 无改动 |
| `src/voice.rs` | `carpai-cli/src/` | 无改动 |
| `src/buddy.rs` | `carpai-cli/src/` | 无改动 |

### 5.3 移入共享核心（`carpai-internal` 或新的共享 crate）

| 现有位置 | 目标位置 | 改动量 |
|----------|----------|--------|
| `src/agent/` | `crates/carpai-agent-core/` (新建) | **大改** — 抽象掉 TUI/本地依赖 |
| `src/tool/mod.rs` (273 工具定义) | `crates/carpai-tool-registry/` (新建) | 中等 — 分离定义与执行 |
| `src/session/` | `crates/carpai-session-core/` (新建) | 中等 — 抽象存储后端 |
| `src/memory/` | `crates/carpai-memory-core/` (新建) | 中等 — 抽象存储后端 |
| `src/ast/` | `crates/carpai-ast/` (新建) | 无改动（纯函数） |
| `src/refactor/` + `src/refactor_engine.rs` | `crates/carpai-refactor/` (新建) | 小改 |
| `src/git/` + `src/git_workflow.rs` | `crates/carpai-git/` (新建) | 小改 |
| `src/config/` | `crates/carpai-config/` (新建) | 小改 |
| `src/provider/mod.rs` + 子目录 | 保持独立 crate | 小改（已基本独立） |
| `src/mcp/` | 保持独立 crate (`jcode-mcp-advanced`) | 无改动 |

### 5.4 条件编译保留在 monorepo root（过渡期）

以下模块在过渡期通过 `#[cfg(feature = "...")]` 保留在 `src/`:

- `src/lib.rs` — feature gate 入口
- `src/main.rs` — 根据 feature 选择启动 server 或 cli
- `src/bus.rs` — 进程内事件总线（cli 模式）

### 5.5 可以废弃/归档的代码

| 模块 | 原因 | 处理方式 |
|------|------|----------|
| `src/prototype/` | 项目脚手架生成器，非核心功能 | 归档到 `crates/carpai-prototype/` |
| `src/dictation.rs` | 语音输入实验性功能 | 归档 |
| `src/login_qr.rs` | QR 登录（中国特有） | 移入 cli |
| `src/video_export.rs` | 视频导出 | 移入 cli |
| `src/nlp.rs` | NLP 实验 | 归档 |
| `src/process_memory_log.rs` | 调试工具 | 归档 |
| `src/protocol_memory.rs` | 已被 memory/ 替代 | 删除 |
| `src/crdt/` | CRDT 协同编辑（未完成） | 归档 |

---

## 六、时间线总览

```
Week  1-2  ████████████████████  Phase 0: 基础设施修复
            ├─ 编译错误清零
            ├─ 安全漏洞修复
            ├─ Feature Gate 骨架
            └─ 服务端引擎注入

Week  3-4  ████████████████████  Phase 1: Trait 抽象层
            ├─ SessionStore
            ├─ ToolExecutor (关键!)
            ├─ InferenceBackend (统一 Sidecar)
            ├─ VirtualFileSystem
            ├─ EventBus
            └─ MemoryBackend

Week  5-8  ████████████████████  Phase 2: carpai-server
            ├─ 新建 carpai-server crate
            ├─ Application 组装逻辑
            ├─ ServerInferenceEngine (配额+缓存+路由)
            ├─ SandboxToolExecutor (默认沙箱)
            ├─ REST/gRPC/WS 路由接入
            └─ 企业中间件 (auth/tenant/quota/audit)

Week  9-10 ████████████████████  Phase 3: carpai-cli
            ├─ 双模式 (local/remote)
            ├─ RemoteAgent 循环
            ├─ Sidecar → InferenceBackend 包装
            └─ CLI 入口拆分

Week 11-12 ████████████████████  Phase 4: carpai-sdk 增强
            ├─ OpenAI 兼容 API
            ├─ Agent Session API
            ├─ IDE 适配器 (VSCode/JB/Nvim)
            └─ 流式传输

Week 13-16 ████████████████████  Phase 5: 测试+文档
            ├─ 三产品独立编译
            ├─ E2E 集成测试
            ├─ 性能基准测试
            └─ 部署文档 (K8s/Docker/systemd)

总计: **16 周 (~4 个月)** 到达可用的 MVP
```

---

## 七、风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| Agent 循环改造引入回归 | 高 | 高 | 先跑通 RemoteAgent 最小路径，保持 LocalAgent 不变 |
| 273 个工具全部沙箱化工作量巨大 | 高 | 中 | 分批迁移：先 Shell 类(高风险)，再 FS 类(中风险)，最后 ReadOnly(低风险) |
| 编译时间膨胀（feature gate 导致） | 中 | 中 | 使用 `cargo-hack` 验证每个 feature 组合；考虑拆 binary crate |
| 现有 CLI 用户习惯改变 | 中 | 低 | remote 模式可选 opt-in，local 模式行为不变 |
| Session 存储格式兼容性 | 中 | 中 | 新增 `SessionStore` trait 时保留旧 JSON 格式读取支持 |

---

## 八、成功标准

### MVP（Phase 2 结束时）

- [ ] `cargo build -p carpai-server --release` 产出独立二进制
- [ ] `./carpai-server serve` 启动后 `/health` 返回 `{ "status": "ok", "engines": { "completion": true, "inference": true } }`
- [ ] `POST /v1/chat/completions` 返回真实 LLM 响应（非 mock）
- [ ] `POST /v1/tools/execute` 在沙箱内执行 shell 命令
- [ ] 所有 Tool 执行经过权限检查和审计日志
- [ ] 密码哈希使用 Argon2id（非 SHA256）

### v1.0（Phase 5 结束时）

- [ ] 三种产品可独立编译和发布
- [ ] CLI 支持 `--remote https://carpai.mycompany.com` 远程模式
- [ ] VSCode 插件通过 SDK 连接服务端，功能完整
- [ ] 企业版支持多租户隔离 + RBAC + 配额管理
- [ ] 分布式推理可在 ≥2 节点集群上运行层分配
- [ ] 95%+ 测试覆盖核心 trait 实现
