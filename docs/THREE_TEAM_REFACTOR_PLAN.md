# CarpAI 重构执行计划 v2.0 — 三组协作版

> **版本**: v2.0 (基于 ARCHITECTURE_PLAN_REVIEW.md 审阅意见修订)
> **日期**: 2026-05-24
> **模式**: 三组并行协作（Alpha 40% + Beta 30% + Gamma 30%）
> **总工期**: 8 周 / ~45 人天

---

## 一、统一架构与命名规范

### 1.1 Crate 层次结构（最终目标）

```
CarpAI Monorepo
│
├── crates/
│   │
│   ├── carpai-internal/     ← ✅ 已完成 (Phase 0)
│   │   └── Pure Trait Layer (零业务逻辑，仅接口定义)
│   │   7 traits: SessionStore, ToolExecutor, InferenceBackend,
│   │            VirtualFileSystem, EventBus, MemoryBackend,
│   │            CodeCompletion/AuthProvider/MemoryStore/InferenceEngine/ToolRegistry
│   │   + AgentContext (DI 容器)
│   │
│   ├── carpai-core/         ← 📍 Phase 1 目标 (新建)
│   │   └── Business Logic Layer (具体实现，依赖 carpai-internal)
│   │   - Local 实现 (6 个 trait 的 concrete impl)
│   │   - Agent 运行时 (agent_runtime, agent loop)
│   │   - 记忆系统 (memory*, knowledge*)
│   │   - 工具系统 (tool, mcp, tools)
│   │   - 补全引擎代理 (completion → jcode-completion)
│   │   - 重构引擎 (refactor*, diff_engine, compilation_engine)
│   │   - AST/语义分析 (ast, semantic, context)
│   │   - 会话管理 (session, session_export, runtime_manager)
│   │   - 文件操作 (storage, file_*, checkpoint, undo_*)
│   │   - Git 集成 (git, git_workflow, version_manager)
│   │   - 错误处理 (error_recovery, error_types, network_retry)
│   │   - 性能优化 (perf, cache_*, concurrency_optimizer, backpressure)
│   │   - 配置基础设施 (config, infrastructure)
│   │
│   ├── carpai-server/       ← 📍 Team Beta 负责
│   │   └── Product: Enterprise AI Programming Server
│   │   - gRPC 服务 (grpc/)
│   │   - REST API (rest/)
│   │   - WebSocket (ws/)
│   │   - 认证中间件 (auth/, security/, permission_rules)
│   │   - 企业功能 (enterprise/)
│   │   - 可观测性 (observability, telemetry, metrics, prometheus, audit_log)
│   │   - 分布式 (distributed, ai_optimization, ab_testing)
│   │   - 传输层 (transport, protocol, bridge)
│   │   - API 网关 (api, gateway)
│   │   - 服务入口 (server, sidecar)
│   │
│   ├── carpai-cli/          ← 📍 Team Gamma 负责
│   │   └── Product: Terminal TUI Client
│   │   - TUI 框架 (tui/)
│   │   - CLI 命令 (cli/, commands/)
│   │   - 终端启动 (terminal_launch, stdin_detect, input, setup_hints)
│   │   - Dashboard (dashboard, debug_panel, side_panel)
│   │   - 环境感知 (ambient, ambient_runner, ambient_scheduler)
│   │   - 通知系统 (notifications, telegram, gmail, browser)
│   │   - 高级 UI (buddy, voice, vim, i18n, dictation)
│   │   - 启动配置 (startup_profile, update, usage, video_export)
│   │   - 插件市场 (plugins, plugin_market, marketplace)
│   │
│   ├── carpai-sdk/          ← ✅ 已存在 (IDE 插件 SDK)
│   │
│   └── [jcode-* crates]     ← 保持不变 (100+ 个子 crate)
│
├── src/                     ← 过渡区 (逐步清空)
│   ├── lib.rs               ← 最终删除或变为 re-export 层
│   └── main.rs              ← 开发用入口
│
└── docs/
    └── ARCHITECTURE_REFACTOR_PLAN_V2.md (本文档)
```

### 1.2 命名规范（强制统一）

| 类别 | 规范 | 示例 |
|------|------|------|
| **Crate 名称** | `carpai-{layer}` | `carpai-internal`, `carpai-core`, `carpai-server`, `carpai-cli` |
| **Trait 名称** | `{名词}Store\|Executor\|Backend\|Provider\|System` | `SessionStore`, `ToolExecutor`, `InferenceBackend` |
| **Local 实现** | `Local{Trait名}` | `LocalFileSessionStore`, `LocalToolExecutor`, `SidecarInferenceBackend` |
| **Server 实现** | `{Protocol}{Trait名}` | `GrpcSessionStore`, `RedisMemoryBackend`, `SandboxToolExecutor` |
| **Config 结构体** | `{Layer}Config` | `CoreConfig`, `ServerConfig`, `CliConfig` |
| **模块文件名** | `snake_case.rs` | `local_file_store.rs`, `grpc_server.rs` |
| **Feature gate** | `product_layer` | `server`, `cli`, `enterprise`, `sdk` |

### 1.3 依赖方向规则（铁律）

```
                    ┌─────────────────┐
                    │  carpai-internal │  ← Pure Traits (零业务逻辑)
                    └────────┬────────┘
                             │
            ┌────────────────┼────────────────┐
            ▼                ▼                ▼
    ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
    │  carpai-core  │ │ carpai-server │ │  carpai-cli   │
    │  (Business)   │ │  (Product)   │ │  (Product)   │
    └──────┬───────┘ └──────┬───────┘ └──────┬───────┘
           │                │               │
           └────────────────┼───────────────┘
                            ▼
                   ┌────────────────┐
                   │  carpai-sdk    │  ← IDE Plugin SDK
                   └────────────────┘

禁止的反向依赖 ❌:
- carpai-server → carpai-cli (Server 不能依赖 TUI)
- carpai-cli → carpai-server (CLI 不能依赖 HTTP/gRPC)
- carpai-core → carpai-server 或 carpai-cli (核心不能依赖产品)
- carpai-internal → 任何业务 crate (trait 层保持纯净)
```

---

## 二、三组分工矩阵

### 2.1 总览

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        CarpAI 重构 — 三组协作                             │
├──────────────┬──────────────┬──────────────┬───────────────────────────┤
│              │  Team Alpha  │  Team Beta   │     Team Gamma            │
│              │  (我们/Solo) │  (服务端组)  │      (客户端组)            │
├──────────────┼──────────────┼──────────────┼───────────────────────────┤
│ 工作量占比   │     40%      │     30%      │         30%              │
│ 人天估算     │    ~18d      │    ~14d      │        ~13d               │
│ 核心职责     │ 协调+核心实现 │  Server 产品 │     CLI 产品              │
├──────────────┼──────────────┼──────────────┼───────────────────────────┤
│ 关键产出     │ carpai-core  │carpai-server │     carpai-cli            │
│              │ + 接口契约   │ + API 文档   │     + TUI 独立             │
│              │ + 联调debug  │ + 企业功能   │     + 远程模式             │
└──────────────┴──────────────┴──────────────┴───────────────────────────┘
```

### 2.2 Team Alpha 职责详述（我们 — 40%，~18 人天）

#### 定位
**架构协调者 + 核心实现者 + 最终集成者**

#### 工作范围

| 阶段 | 任务 | 人天 | 交付物 | 依赖 |
|------|------|------|--------|------|
| **Week 1** | carpai-core 初始化 + Local 实现 | 4d | 6 个 trait 的 concrete impl | 无 (基于 carpai-internal) |
| **Week 2** | 核心模块迁移 (第一批 20 个) | 3d | agent, memory, tools, session, error | Week 1 |
| **Week 3** | 核心模块迁移 (第二批 20 个) + 接口契约定义 | 3d | completion, refactor, ast, git, config | Week 2 |
| **Week 4** | 死代码清理 + 编译基线建立 | 2d | cargo check -p carpai-core 通过 | Week 3 |
| **Week 5** | Beta/Gamma 接口对接支持 | 2d | 共享类型 + Mock 实现 | 无 |
| **Week 6** | 性能模块迁移 + 缓存系统 | 2d | perf, cache_*, backpressure | Week 4 |
| **Week 7-8** | **统一联调 + 全集成测试 + Debug** | 4d | 三产品编译通过 + E2E 测试 | Beta + Gamma 产出 |

#### 具体任务清单

##### Week 1: carpai-core 骨架 + Local 实现 (4d)

**Day 1: Crate 初始化**
```
[ ] 创建 crates/carpai-core/Cargo.toml
    - edition = "2024"
    - dependencies: carpai-internal (path), tokio, anyhow, thiserror, serde, tracing
[ ] 创建 crates/carpai-core/src/lib.rs
    - pub mod session_impl;      // LocalFileSessionStore
    - pub mod tool_executor_impl; // LocalToolExecutor
    - pub mod inference_impl;     // SidecarInferenceBackend
    - pub mod filesystem_impl;    // LocalFileSystem
    - pub mod event_bus_impl;     // InProcessEventBus
    - pub mod memory_impl;        // LocalMemoryBackend
    - pub mod agent_loop;         // Agent 主循环 (纯逻辑)
    - Re-export 所有 public 类型
```

**Day 2-3: 6 个 Local 实现**
```rust
// crates/carpai-core/src/session_impl.rs
pub struct LocalFileSessionStore {
    base_path: PathBuf,
}

impl SessionStore for LocalFileSessionStore {
    async fn create_session(&self, meta: SessionMeta) -> Result<SessionId, SessionError> { ... }
    async fn append_message(&self, session_id: &SessionId, msg: StoredMessage) -> Result<(), SessionError> { ... }
    async fn load_session(&self, id: &SessionId) -> Result<LoadedSession, SessionError> { ... }
    // ... 复用 src/session/persistence.rs 的逻辑
}

// 同样模式实现其余 5 个 trait
```

**Day 4: AgentContext 组装 + 集成测试**
```rust
// crates/carpai-core/src/lib.rs
pub fn build_local_agent_context(config: &AppConfig) -> AgentContext {
    AgentContextBuilder::new()
        .with_config(config.clone())
        .with_sessions(Arc::new(LocalFileSessionStore::new(...)))
        .with_tools(Arc::new(LocalToolExecutor::new(...)))
        .with_inference(Arc::new(SidecarInferenceBackend::new(...)))
        .with_filesystem(Arc::new(LocalFileSystem::new(...)))
        .with_events(Arc::new(InProcessEventBus::new(1024)))
        .with_memory(Arc::new(LocalMemoryBackend::new(...)))
        .build()
        .expect("AgentContext assembly")
}
```

##### Week 2-3: 核心模块迁移 (~6d)

**迁移批次规划（每批 ≤10 个模块，验证编译）:**

**Batch A (Week 2, Day 1-2): Agent 系统**
```
从 src/ 迁移到 crates/carpai-core/src/agent/:
  - agent.rs          → agent/mod.rs
  - agent_runtime.rs  → agent/runtime.rs
  - sub_agents.rs     → agent/sub_agents.rs
  - skill_system.rs   → agent/skill_system.rs
  - plan_mode.rs      → agent/plan_mode.rs
  - task_planner.rs   → agent/task_planner.rs
  - task_manager.rs   → agent/task_manager.rs
  - task_decomposer.rs→ agent/task_decomposer.rs
  - task_scheduler.rs → agent/task_scheduler.rs
  - plan_verifier.rs  → agent/plan_verifier.rs
  - ultraplan.rs      → agent/ultraplan.rs
约束: 所有 use crate::xxx 改为 use carpai_internal::xxx 或 carpai_core::xxx
```

**Batch B (Week 2, Day 3): 记忆 + 会话**
```
迁移到 crates/carpai-core/src/memory/:
  memory.rs, memory_agent.rs, memory_graph.rs, memory_log.rs,
  memory_types.rs, memory_prompt.rs, memory_advanced.rs,
  semantic_memory.rs, hierarchical_memory.rs, knowledge_graph.rs,
  knowledge.rs, knowledge_agents.rs, protocol_memory.rs

迁移到 crates/carpai-core/src/session/:
  session.rs, session_export.rs, session_cost_tracker.rs,
  session_gc.rs, runtime_manager.rs, cgroup_isolation.rs
```

**Batch C (Week 3, Day 1-2): 工具 + 补全**
```
迁移到 crates/carpai-core/src/tools/:
  tool.rs, mcp.rs, tools.rs, slash_command.rs

迁移到 crates/carpai-core/src/completion/:
  completion.rs, completion_engine.rs, completion_quality.rs, auto_fallback.rs
注意: completion_engine 是对 jcode-completion crate 的包装/集成层
```

**Batch D (Week 3, Day 3): 重构 + AST + Git + 错误处理**
```
迁移到 crates/carpai-core/src/refactoring/:
  refactor.rs, refactor_engine.rs, orchestrator.rs, precise_edit.rs,
  atomic_edit_coordinator.rs, diff_engine.rs, diff_integration.rs,
  streaming_diff_preview.rs, compilation_engine.rs, diagnostics.rs,
  transaction.rs, refactor_verify_pipeline.rs, delivery_pipeline.rs

迁移到 crates/carpai-core/src/analysis/:
  ast.rs, classifier.rs, semantic.rs, context_pruner.rs,
  incremental_index.rs, proactive_context.rs, context.rs, reasoning.rs

迁移到 crates/carpai-core/src/git/:
  git.rs, git_workflow.rs, version_manager.rs

迁移到 crates/carpai-core/src/error/:
  error_recovery.rs, error_types.rs, network_retry.rs, allowlist.rs
```

##### Week 4: 死代码清理 + 编译基线 (2d)

**死代码处置清单:**

| 模块 | 处置方式 | 理由 |
|------|---------|------|
| crdt | 归档到 `crates/jcode-experimental/` | CRDT 未使用但可能有 P2P 未来价值 |
| dictation | 归档到 `crates/jcode-experimental/` | 语音听写功能未完成 |
| dap, debugger | 归档到 `crates/jcode-debug/` | DAP 调试协议未接入主流程 |
| env | **删除** | 被 config.rs 完全覆盖 |
| goal | **合并**到 task_planner | 功能重复 |
| import | **删除** | 被 refactor_engine 覆盖 |
| login_qr | **删除** | CLI 专属，移入 carpai-cli |
| process_memory | **删除** | 被 runtime_manager 覆盖 |
| process_title | **删除** | 仅 Windows 桌面端使用 |
| prompt | **合并**到 memory/prompt.rs | 单函数模块 |
| restart_snapshot | **删除** | 被 session_gc 覆盖 |
| runtime_memory_log | **删除** | 被 observability 覆盖 |
| safety | **合并**到 security/scanner.rs | 功能重复 |
| scheduler | **删除** | 被 task_scheduler 覆盖 |
| external | **删除** | 占位符无实现 |
| plan (lib.rs中的) | **合并**到 ultraplan | 命名冲突 |
| workspace_manager | **合并**到 session/workspace.rs | 单功能模块 |
| compaction | **合并**到 memory/compaction.rs | 单功能模块 |
| rule_reviewer | **移动**到 enterprise/review.rs | 属于企业功能 |
| subscription_catalog | **删除** | 未使用 |
| todo | **删除** | CLI 专属，移入 carpai-cli |
| update | **删除** | CLI 专属，移入 carpai-cli |
| usage | **删除** | CLI 专属，移入 carpai-cli |
| video_export | **删除** | CLI 专属 |

##### Week 5-8: 联调与集成 (见第七节)

### 2.3 Team Beta 职责详述（服务端组 — 30%，~14 人天）

#### 定位
**carpai-server 产品实现者**

#### 前置条件（由 Alpha 提供）
- ✅ `carpai-internal` trait 定义（已就绪）
- ✅ `carpai-core` 公共 API（Week 3 结束时提供接口契约文档）
- ✅ Mock 实现（Week 5 开始时可用的 test double）

#### 工作范围

| 阶段 | 任务 | 人天 | 交付物 | Alpha 依赖 |
|------|------|------|--------|-----------|
| **Week 1-2** | Server crate 初始化 + 项目骨架 | 2d | Cargo.toml + 目录结构 + ServerConfig | 无 |
| **Week 3** | gRPC 服务框架 + Proto 定义 | 3d | gRPC server 启动 + health check | 无 |
| **Week 4** | REST API + WebSocket 框架 | 2d | REST router + WS endpoint | Week 3 |
| **Week 5** | Auth 中间件 + RBAC | 2d | JWT auth + 权限校验 | carpai-core 接口 |
| **Week 6** | Engine wiring (对接 carpai-core) | 2d | ServerInferenceEngine + SessionManager | carpai-core stable API |
| **Week 7** | Enterprise features + Observability | 2d | 多租户 + metrics + audit log | Week 6 |
| **Week 8** | 集成测试 + 文档 | 1d | E2E 测试 + API 文档 | Alpha 联调 |

#### 具体任务清单

##### Week 1-2: Server 骨架 (2d)

```
创建 crates/carpai-server/

Cargo.toml:
  [dependencies]
  carpai-internal = { path = "../carpai-internal" }
  carpai-core = { path = "../carpai-core" }  # Week 3 后加入
  tokio = { version = "1", features = ["full"] }
  axum = { version = "0.7", features = ["ws"] }      # REST + WS
  tonic = "0.11"                                      # gRPC
  prost = "0.13"                                      # Proto 序列化
  sqlx = { version = "0.7", features = ["runtime-tokio", "postgres"], optional = true }
  redis = { version = "0.25", optional = true }
  tracing = "0.1"
  serde = { version = "1", features = ["derive"] }

目录结构:
  crates/carpai-server/src/
  ├── lib.rs                 # pub mod 声明
  ├── main.rs                # fn main() -> 服务启动入口
  ├── config.rs              # ServerConfig { inherit CoreConfig, listen_addr, tls, ... }
  ├── app.rs                 # Application struct (Axum Router + Tonic Server)
  │
  ├── grpc/
  │   ├── mod.rs
  │   ├── server.rs           # gRPC Server 启动/关闭
  │   ├── agent_service.rs    # Agent RPC (ChatCompletion, StreamChat)
  │   ├── session_service.rs  # Session CRUD RPC
  │   ├── tool_service.rs     # Tool execution RPC
  │   └── health_service.rs   # Health check RPC
  │
  ├── rest/
  │   ├── mod.rs
  │   ├── router.rs           # Axum Router 组装
  │   ├── agent_handler.rs    # POST /v1/chat/completions
  │   ├── session_handler.rs  # GET/POST /v1/sessions/:id
  │   ├── tool_handler.rs     # POST /v1/tools/:name
  │   └── middleware.rs        # Auth + RateLimit + CORS
  │
  ├── ws/
  │   ├── mod.rs
  │   ├── handler.rs          # WebSocket upgrade handler
  │   ├── session.rs          # Per-connection session state
  │   └── broadcast.rs        # Message broadcast to all clients
  │
  ├── auth/
  │   ├── mod.rs
  │   ├── jwt.rs              # JWT token creation/validation
  │   ├── api_key.rs          # API Key validation
  │   └── rbac.rs             # Role-based access control
  │
  ├── enterprise/
  │   ├── mod.rs
  │   ├── multi_tenant.rs     # Tenant isolation
  │   ├── quota.rs            # Usage quota enforcement
  │   └── admin_api.rs        # Admin endpoints
  │
  └── observability/
      ├── mod.rs
      ├── metrics.rs          # Prometheus metrics
      ├── tracing.rs          # Distributed tracing
      └── audit.rs            # Audit logging
```

##### Week 3-4: 通信层 (5d)

**关键接口契约（必须与 Alpha 对齐）:**

```rust
// crates/carpai-server/src/grpc/agent_service.rs
// Beta 必须使用的 Alpha 提供的类型:
use carpai_internal::{AgentContext, ChatCompletionRequest, ChatCompletionResponse};
use carpai_core::{execute_agent_turn, AgentTurnOutput};  // Alpha 提供

#[tonic::async_trait]
impl AgentService for AgentServer {
    async fn chat_completion(
        &self,
        request: Request<ChatCompletionRequest>,
    ) -> Result<Response<ChatCompletionResponse>, Status> {
        // 1. 从 request metadata 提取 tenant_id, session_id
        // 2. 构建/获取 AgentContext (从连接池或新建)
        // 3. 调用 Alpha 提供的 execute_agent_turn()
        // 4. 返回 response
    }
}
```

##### Week 5-8: 业务实现 (7d)

**注意事项（来自审阅报告 4.1-4.3 节）:**
- ✅ 使用 `AgentContext` 作为 DI 容器，不要自建 `SessionContext`
- ✅ `EventBus` 使用 `clone_box()` 而非 `Clone`
- ✅ `ExecutionMode` 不是 `Copy`
- ✅ `BusEvent` 的 `Deserialize` bound 需要 HRTB: `for<'a> Deserialize<'a>`
- ❌ 不在 carpai-internal 中添加业务逻辑
- ❌ 不重新定义已有 trait
- ❌ Phase 1 不引入 `config` crate（用 serde 手动加载）

### 2.4 Team Gamma 职责详述（客户端组 — 30%，~13 人天）

#### 定位
**carpai-cli 产品实现者**

#### 前置条件（由 Alpha 提供）
- ✅ `carpai-internal` trait 定义（已就绪）
- ✅ `carpai-core` 公共 API（Week 3 结束时提供接口契约文档）
- ✅ 本地 AgentContext 构建器（`build_local_agent_context()`）

#### 工作范围

| 阶段 | 任务 | 人天 | 交付物 | Alpha 依赖 |
|------|------|------|--------|-----------|
| **Week 1-2** | CLI crate 初始化 + TUI 框架迁移 | 2d | Cargo.toml + ratatui skeleton | 无 |
| **Week 3** | TUI 业务逻辑剥离 (关键!) | 3d | 纯渲染层 + 业务逻辑分离 | 无 |
| **Week 4** | Commands 迁移 + 双模式架构 | 2d | Local Mode + Remote Mode | carpai-core 接口 |
| **Week 5** | Ambient + Notifications | 2d | 后台任务 + 通知渠道 | Week 4 |
| **Week 6** | Dashboard + 高级 UI | 2d | Debug panel + Side panel | Week 5 |
| **Week 7** | 打磨 + 边缘场景 | 1d | 错误处理 + 优雅降级 | Week 6 |
| **Week 8** | 集成测试 + 文档 | 1d | E2E 测试 (local + remote) | Alpha 联调 |

#### 具体任务清单

##### Week 1-2: CLI 骨架 (2d)

```
创建 crates/carpai-cli/

Cargo.toml:
  [dependencies]
  carpai-internal = { path = "../carpai-internal" }
  carpai-core = { path = "../carpai-core" }
  ratatui = "0.27"              # TUI framework
  crossterm = "0.27"             # Terminal backend
  tui-textarea = "0.5"           # Text input widget
  arboard = "3.0"                # Clipboard
  tokio = { version = "1", features = ["full"] }

目录结构:
  crates/carpai-cli/src/
  ├── main.rs                  # fn main() → cli::run()
  ├── lib.rs
  ├── config.rs                # CliConfig { theme, keybinds, ... }
  │
  ├── cli/
  │   ├── mod.rs
  │   ├── startup.rs           # TUI 初始化 + raw mode
  │   ├── dispatch.rs          # Command dispatch
  │   └── commands/
  │       ├── chat.rs          # $ carpai chat
  │       ├── serve.rs         # $ carpai serve (launcher)
  │       ├── ask.rs           # $ carpai ask "question"
  │       └── ...
  │
  ├── tui/                     ← 纯渲染层 (不含业务逻辑!)
  │   ├── mod.rs
  │   ├── app.rs              # App struct (ratatui::App trait impl)
  │   ├── widgets/
  │   │   ├── chat_view.rs     # 消息显示
  │   │   ├── input_bar.rs     # 输入框
  │   │   ├── status_line.rs   # 状态栏
  │   │   ├── file_tree.rs     # 文件树侧边栏
  │   │   └── help_overlay.rs  # 帮助面板
  │   ├── event.rs            # Event enum (Key, Mouse, Resize, Tick)
  │   ├── handler.rs          # 事件分发 (只负责渲染，不包含 Agent 逻辑!)
  │   └── theme.rs            # 颜色主题
  │
  ├── agent_bridge.rs          # ⭐ TUI ↔ AgentContext 桥接 (纯调用)
  │                            #   只调用 carpai_core::execute_agent_turn()
  │                            #   不包含任何 Agent 执行逻辑!
  │
  ├── ambient/
  │   ├── mod.rs
  │   ├── runner.rs           # Background task runner
  │   └── scheduler.rs        # Task scheduling
  │
  └── notifications/
      ├── mod.rs
      ├── telegram.rs
      ├── gmail.rs
      └── browser.rs
```

##### Week 3: TUI 业务逻辑剥离 (3d) — **最高优先级**

这是 CLI 能独立编译的**前置条件**。

**剥离前（当前状态 — 反模式）:**
```rust
// src/tui/app.rs (当前代码 — 问题所在)
impl App {
    async fn execute_agent_command(&mut self, msg: String) {
        // ❌ 这里混合了: 用户输入解析 + Agent 调度 + LLM 调用 + 结果渲染
        let context = self.build_completion_context();  // 业务逻辑
        let candidates = self.engine.complete(&context).await?;  // 业务逻辑
        self.render_completion_results(&candidates);  // 渲染逻辑
    }
}
```

**剥离后（目标状态）:**
```rust
// crates/carpi-cli/src/tui/app.rs (只有渲染逻辑)
impl App {
    async fn handle_user_input(&mut self, input: String) {
        // 1. 渲染用户消息
        self.messages.push(UIMessage::User(input.clone()));

        // 2. 调用桥接层 (纯委托，零业务逻辑)
        match self.agent_bridge.execute_turn(&input).await {
            Ok(output) => {
                // 3. 渲染结果 (唯一职责)
                self.messages.push(UIMessage::Assistant(output.text));
                if let Some(tools) = output.tool_calls {
                    self.render_tool_calls(&tools);
                }
            }
            Err(e) => {
                self.messages.push(UIMessage::Error(e.to_string()));
            }
        }
    }
}

// crates/carpai-cli/src/agent_bridge.rs (桥接层)
pub struct AgentBridge {
    ctx: Arc<RwLock<Option<AgentContext>>>,  // 来自 carpai-core
}

impl AgentBridge {
    /// 本地模式: 直接调用 carpai-core 的 Agent 循环
    pub async fn execute_turn_local(&self, user_msg: &str) -> Result<AgentTurnOutput> {
        let ctx = self.ctx.read().await.as_ref().ok_or(BridgeError::NoContext)?;
        carpai_core::execute_agent_turn(ctx, user_msg).await  // ← Alpha 提供
    }

    /// 远程模式: 通过 gRPC 连接到 carpai-server
    pub async fn execute_turn_remote(&self, user_msg: &str) -> Result<AgentTurnOutput> {
        let client = self.grpc_client.as_ref().ok_or(BridgeError::NoConnection)?;
        client.chat_completion(ChatCompletionRequest::from_user_message(user_msg)).await
    }
}
```

##### Week 4-8: Commands + Ambient + 打磨 (6d)

**双模式架构:**
```rust
// crates/carpai-cli/src/cli/startup.rs
pub async fn run(mode: CliMode, config: &CliConfig) -> Result<()> {
    match mode {
        CliMode::Local => {
            // 使用 Alpha 提供的本地构建器
            let ctx = carpai_core::build_local_agent_context(&config.core);
            run_tui_with_context(ctx, config).await
        }
        CliMode::Remote { server_url } => {
            // 连接到 Beta 提供的 carpai-server
            let client = GrpcClient::connect(server_url).await?;
            run_tui_with_remote(client, config).await
        }
    }
}
```

---

## 三、接口契约（Interface Contracts）

### 3.1 Alpha → Beta/Gamma 公共 API（Alpha 负责提供）

以下接口在 **Week 3 结束前** 由 Alpha 冻结并发布：

```rust
// ====== carpai-core 公共 API (crate-level re-exports) ======

/// Agent 核心循环 — 纯业务逻辑，无 UI 依赖
///
/// # Arguments
/// * `ctx` - AgentContext (包含所有 trait object)
/// * `user_message` - 用户输入
///
/// # Returns
/// * `AgentTurnOutput` - 包含 assistant 回复、tool_calls、usage
pub async fn execute_agent_turn(
    ctx: &AgentContext,
    user_message: &str,
) -> Result<AgentTurnOutput>;

/// 构建本地开发环境的 AgentContext
///
/// 自动选择 Local* 实现类:
/// - LocalFileSessionStore (~/.carpai/sessions/)
/// - LocalToolExecutor (直接执行)
/// - SidecarInferenceBackend (localhost:11434)
/// - InProcessEventBus (tokio broadcast channel)
/// - LocalMemoryBackend (内存 + JSONL 持久化)
pub fn build_local_agent_context(config: &AppConfig) -> AgentContext;

/// AgentTurnOutput — 一次 Agent 对话的输出
pub struct AgentTurnOutput {
    pub text: String,                          // Assistant 回复文本
    pub tool_calls: Vec<ToolCallInfo>,          // 触发的工具调用
    pub usage: TokenUsage,                      // Token 用量
    pub session_id: SessionId,                  // 会话 ID
    pub duration_ms: u64,                       // 耗时
}

/// AppConfig — 统一配置 (继承 carpai_internal::AppConfig)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    #[serde(flatten)]
    pub base: AppConfig,                        // carpai-internal 基础配置
    // --- Core-specific ---
    pub data_dir: PathBuf,                     // 数据目录 (~/.carpai/)
    pub max_concurrent_tools: usize,            // 最大并发工具数
    pub completion_provider: ProviderConfig,    // 补全 provider 配置
    pub memory_enabled: bool,                   // 是否启用记忆
    pub cache_size_mb: usize,                   // 缓存大小限制
}
```

### 3.2 Beta → Alpha 接入点（Beta 负责实现）

Beta 需要实现的 Server 版本 trait impl（供 Alpha 在联调时注入测试）:

```rust
// Beta 提供给 Alpha 的 Server 实现 (用于集成测试)
//
// 这些实现在 crates/carpai-server/src/ 内部，
// Alpha 在 Week 7-8 联调时使用它们替换 Local 实现

// crates/carpai-server/src/session_impl.rs
pub struct RedisSessionStore { redis: redis::Client }
impl SessionStore for RedisSessionStore { ... }

// crates/carpai-server/src/inference_impl.rs
pub struct MultiProviderInferenceEngine { ... }
impl InferenceBackend for MultiProviderInferenceEngine { ... }

// crates/carpai-server/src/tool_executor_impl.rs
pub struct SandboxToolExecutor { ... }
impl ToolExecutor for SandboxToolExecutor { ... }
```

### 3.3 Gamma → Alpha 接入点（Gamma 负责实现）

Gamma 需要实现的 CLI 特定组件:

```rust
// crates/carpai-cli/src/config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(flatten)]
    pub core: CoreConfig,                       // 继承 core 配置
    // --- CLI-specific ---
    pub theme: ThemeConfig,                     // 颜色主题
    pub keybinds: KeybindConfig,                // 快捷键
    pub editor: Option<String>,                 // 外部编辑器 ($EDITOR)
    pub clipboard: ClipboardConfig,             // 剪贴板行为
    pub startup: StartupConfig,                 # 启动画面
}

// crates/carpai-cli/src/agent_bridge.rs (Gamma 实现)
pub struct AgentBridge { ... }  // 见上文 Week 3 部分
```

---

## 四、跨组依赖与同步机制

### 4.1 依赖时间线

```
Week:   1    2    3    4    5    6    7    8
        ├────┼────┼────┼────┼────┼────┼────┤
Alpha:  [====CORE====][===MIGRATE===][CLEAN][SUPPORT][POLISH][INTEGRATE]
              │              │         │             │
              ▼              ▼         ▼             ▼
Beta:   [SKELETON][GRPC_FRAME][REST_WS][AUTH][WIRE][ENTERPRISE][TEST]
                                    ↑                   │
                                    │(interface contract)│
Gamma:  [CLI_SKEL][TUI_STRIP][COMMANDS][AMBIENT][DASH][POLISH][TEST]
                      ↑                                        │
                      │(agent_bridge interface)                 │
                      └─────────────────────────────────────────┘
                                           (final integration by Alpha)
```

### 4.2 同步会议节奏

| 会议 | 频率 | 参与者 | 议题 |
|------|------|--------|------|
| **Daily Standup** | 每天 15min | 全员 | 昨日进展、今日计划、阻塞问题 |
| **Interface Sync** | 每周二 1h | Alpha Lead + Beta Lead + Gamma Lead | 接口契约评审、依赖确认 |
| **Integration Prep** | Week 7 开始每天 1h | 全员 | 联调进度、Bug 分配、风险升级 |
| **Final Review** | Week 8 末 2h | 全员 | 验收标准检查、遗留问题决策 |

### 4.3 代码合并策略

```
main (protected branch)
  │
  ├── alpha/core-build     ← Alpha 专用分支 (Week 1-4)
  │   └── merge to main after cargo check passes
  │
  ├── beta/server-build    ← Beta 专用分支 (Week 1-7)
  │   └── PR to main (Alpha reviews interface compliance)
  │
  ├── gamma/cli-build      ← Gamma 专用分支 (Week 1-7)
  │   └── PR to main (Alpha reviews interface compliance)
  │
  └── integration          ← Week 7-8 只有 Alpha 可 push
      └── final cargo check --workspace + cargo test --workspace
```

**规则:**
- Alpha 拥有 `main` 分支的 merge 权限
- Beta/Gamma 通过 PR 提交，Alpha 必须 review 接口契约合规性
- 任何破坏 `cargo check -p carpai-internal` 或 `cargo check -p carpai-core` 的 PR **拒绝合并**

---

## 五、风险与缓解

### 5.1 协作风险

| 风险 | 概率 | 影响 | 缓解措施 | 负责人 |
|------|------|------|---------|--------|
| **接口契约变更导致 Beta/Gamma 大量返工** | 中 | 高 | Week 3 冻结接口；变更需全员同意 | Alpha |
| **循环依赖跨越组边界** | 中 | 高 | Alpha 先画完整依赖图；每周审查新 import | Alpha |
| **Beta/Gamma 进度不同步** | 高 | 中 | Weekly Sync 会议；Alpha 准备 Mock 实现解耦 | Alpha |
| **命名不一致导致合并冲突** | 高 | 低 | 强制遵守 1.2 节命名规范；CI lint 检查 | Alpha |
| **编译时间增长导致反馈变慢** | 中 | 中 | sccache + mold linker；增量编译 | Alpha |

### 5.2 技术风险（来自审阅报告）

| 风险 | 来源 | 缓解措施 |
|------|------|---------|
| **Trait object safety** | EventBus, BusSubscriber dyn 兼容 | Alpha 在 Week 1 解决；Beta/Gamma 直接使用 |
| **async_trait 边界案例** | trait object 上行为差异 | Alpha 编写示例测试覆盖 |
| **Serde + trait object** | AgentContext 序列化 | 自定义 serializer（跳过 dyn 字段） |
| **EventBus Clone 限制** | object-safety 要求 | 使用 `clone_box()` 方法 |

---

## 六、验收标准

### 6.1 每个 Phase 结束时的 Done Definition

**Week 4 结束 (Alpha Phase 1 完成):**
- [ ] `cargo check -p carpai-core` 通过 (0 errors, warnings acceptable)
- [ ] `cargo test -p carpai-core` 全绿 (>50% 核心路径覆盖)
- [ ] `cargo doc -p carpai-core` 无警告
- [ ] 18 个遗留模块已处置（删除/归档/合并）
- [ ] 接口契约文档已发布给 Beta/Gamma

**Week 7 结束 (Beta/Gamma Phase 完成):**
- [ ] `cargo check -p carpai-server` 通过
- [ ] `cargo check -p carpai-cli` 通过
- [ ] Server: `cargo run --bin carpai-server` 能启动并响应 health check
- [ ] CLI: `cargo run --bin carpai-cli -- chat` 能进入 TUI 并发送消息
- [ ] 所有 PR 符合接口契约

**Week 8 结束 (全量集成):**
- [ ] `cargo check --workspace` 通过 (全 Monorepo)
- [ ] `cargo test --workspace` 全绿
- [ ] `cargo clippy --workspace` 0 errors
- [ ] E2E 测试: CLI(local) → CLI(remote→Server) → IDE Plugin 全链路通过
- [ ] 文档: README + API docs + Architecture doc 更新

### 6.2 质量门禁

```yaml
# .github/workflows/quality_gate.yml (CI 检查)
name: Quality Gate
on: [pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo check --workspace
      - run: cargo clippy --workspace -- -D warnings
      - run: cargo test --workspace
      - run: cargo doc --workspace --no-deps

  interface-compliance:
    runs-on: ubuntu-latest
    steps:
      - name: Check no cross-product imports
        run: |
          # 禁止: carpai-server → carpai-cli
          # 禁止: carpai-cli → carpai-server
          # 禁止: carpai-core → carpai-server/cli
          grep -r "carpai-server" carpai-cli/src/ && exit 1 || true
          grep -r "carpai-cli" carpai-server/src/ && exit 1 || true
```

---

## 七、Week 7-8 统一联调计划（Alpha 专属）

### 7.1 联调阶段 Alpha 任务 (4d)

**Day 1-2: 集成环境搭建**
```
[ ] 合并 Beta carpai-server 分支到 main
[ ] 合并 Gamma carpai-cli 分支到 main
[ ] 解决 merge conflicts (预计 <10 个，主要是 use path 变更)
[ ] 运行 cargo check --workspace，记录所有错误
[ ] 按错误分类: Alpha 自有 / Beta 引入 / Gamma 引入 / 跨组交互
```

**Day 3: 跨组 Bug 修复**
```
[ ] 修复 Alpha 自有编译错误 (优先级最高)
[ ] 分配 Beta 引入的 Bug 给 Beta 修复 (Alpha review)
[ ] 分配 Gamma 引入的 Bug 给 Gamma 修复 (Alpha review)
[ ] 修复跨组交互问题 (接口不匹配、类型不一致等)
[ ] 每次 fix 后运行 full workspace check
```

**Day 4: E2E 测试 + 收尾**
```
[ ] 运行完整 E2E 测试套件:
    - Test 1: carpai-cli local mode (TUI 启动 → 发送消息 → 收到回复)
    - Test 2: carpai-server (health check → gRPC call → REST call)
    - Test 3: CLI remote mode (连接 Server → 发送消息 → 收到回复)
    - Test 4: 编译产物大小检查 (<100MB per binary)
[ ] 运行 clippy --workspace，修复剩余 warnings (目标 <200)
[ ] 运行 cargo doc --workspace，修复 doc warnings
[ ] 生成性能基准报告 (编译时间、二进制大小、内存占用)
[ ] 最终 commit + tag: v0.12.0-refactored
```

### 7.2 Bug 分类与分派流程

```
Bug 发现
   │
   ├─→ Alpha 自有模块 → Alpha 立即修复
   │
   ├─→ Beta 模块 (server/) → Alpha 复现 → 创建 Issue → assign Beta
   │   └─→ Beta 修复 → PR → Alpha review → merge
   │
   ├─→ Gamma 模块 (cli/) → Alpha 复现 → 创建 Issue → assign Gamma
   │   └─→ Gamma 修复 → PR → Alpha review → merge
   │
   └─→ 跨组交互 (core↔server, core↔cli) → Alpha 分析根因
        ├─→ Interface contract bug → Alpha 修 contract + Beta/Gamma adapt
        └─→ Implementation bug → 对应组修复
```

---

## 八、附录

### A. 文件映射速查表

| 如果你要... | 看/改这个文件 | 所属组 |
|-------------|-------------|--------|
| 了解 trait 定义 | `crates/carpai-internal/src/*.rs` | Alpha (已完成) |
| 了解 AgentContext DI | `crates/carpai-internal/src/agent_context.rs` | Alpha (已完成) |
| 了解 Local 实现 | `crates/carpai-core/src/*_impl.rs` | Alpha (Week 1-2) |
| 了解 Agent 循环 | `crates/carpai-core/src/agent_loop.rs` | Alpha (Week 2) |
| 了解 Server Config | `crates/carpai-server/src/config.rs` | Beta |
| 了解 gRPC proto | `crates/carpai-server/proto/*.proto` | Beta |
| 了解 REST routes | `crates/carpai-server/src/rest/router.rs` | Beta |
| 了解 TUI 纯渲染层 | `crates/carpai-cli/src/tui/app.rs` | Gamma |
| 了解 Agent Bridge | `crates/carpai-cli/src/agent_bridge.rs` | Gamma |
| 了解 CLI Config | `crates/carpai-cli/src/config.rs` | Gamma |
| Feature Gate 定义 | `Cargo.toml` (root) | Alpha |
| 服务端入口 | `src/bin/jcode-server.rs` | Beta 参考 |
| 安全修复记录 | `src/enterprise/auth.rs` | Beta 参考 |

### B. 禁止事项清单（全员遵守）

| # | 禁止事项 | 原因 | 违规后果 |
|---|---------|------|---------|
| 1 | ❌ 在 `carpai-internal/` 中添加业务逻辑 | trait 层必须保持纯净 | PR 拒绝 |
| 2 | ❌ 重新定义已有的 7 个 trait | 造成重复和混乱 | PR 拒绝 |
| 3 | ❌ `carpai-server` import `carpai-cli` | 违反单向依赖 | CI 拦截 |
| 4 | ❌ `carpai-cli` import `carpai-server` | 违反单向依赖 | CI 拦截 |
| 5 | ❌ `EventBus` 带 `Clone` supertrait | 破坏 object safety | 编译失败 |
| 6 | ❌ Phase 1 引入 `config` crate | 不必要的编译时间增长 | PR 拒绝 |
| 7 | ❌ 自创命名风格（如 camelCase 模块名） | 破坏统一性 | PR 要求修改 |
| 8 | ❌ 跨组修改他人负责的模块未经沟通 | 造成 merge conflict | Issue + 通报 |

### C. 术语表

| 术语 | 定义 |
|------|------|
| **Trait Layer** | `carpai-internal` — 纯接口定义，零实现 |
| **Core Layer** | `carpai-core` — 业务逻辑 + Local 实现 |
| **Product Layer** | `carpai-server` / `carpai-cli` — 面向用户的产品 |
| **SDK Layer** | `carpai-sdk` — IDE 插件开发包 |
| **Local 实现** | 开发/单机模式的具体 trait impl |
| **Server 实现** | 生产环境的具体 trait impl (Redis, PostgreSQL 等) |
| **AgentContext** | DI 容器，组装所有 trait object |
| **Interface Contract** | 跨组约定的公共 API 签名 |
| **Bridge** | Gamma 的 agent_bridge.rs — TUI 与 Agent 之间的纯委托层 |

---

> **文档维护**: 此文档由 Team Alpha 维护。任何架构变更需要更新本文档。
>
> **下次评审**: Week 4 结束时（Phase 1 完成后），评估是否调整 Week 5-8 计划。
