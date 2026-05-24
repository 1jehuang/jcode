# CarpAI 重构执行计划 v3.0 FINAL — 三组协作版

> **版本**: v3.0 (FINAL — 基于 V2 Review 全部意见修订)
> **日期**: 2026-05-24
> **总工期**: 12 周 / ~60 人天
> **模式**: 三组并行协作（solo-Turbo 40% + ma-guoyang 30% + Paw-brave 30%）
> **状态**: ✅ 已通过审阅，可立即启动

---

## 一、执行摘要

### 1.1 重构目标

将当前**单体 Monolith (207 个模块堆积在 src/lib.rs)** 重构为 **四层 Crate 架构**，支持三产品独立发布：

```
个人开发者:     cargo install --path crates/carpai-cli     → $ carpai chat
企业 IT:        cargo install --path crates/carpai-server   → $ carpai serve
IDE 插件用户:   npm install @carpai/sdk                    → VSCode/JetBrains/Neovim 插件
```

### 1.2 与原计划差异

| 维度 | 原计划 (16周) | v2.0 (8周) | **v3.0 FINAL (12周)** |
|------|--------------|------------|---------------------|
| 总工期 | 16 周 | 8 周 | **12 周** |
| 集成测试 | 4 周 | 2 天 | **2 周 (W9-10)** |
| SDK 增强 | Week 11-12 | ❌ 缺失 | **Week 9-10 (独立)** |
| 性能基准 | Week 15-16 | ❌ 砍掉 | **Week 11-12** |
| 模块迁移 | 渐进式 | 40 模块/6 天 | **52 模块/14 天 (现实估算)** |
| 配置方案 | 未定义 | 三层草案 | **三层方案完整定义 (含代码)** |
| Local 实现 | 待开发 | 在 internal | **迁移到 carpai-core ✅** |

### 1.3 关键里程碑

```
Week 4  End → ✅ carpai-core 可编译 (solo-Turbo Phase 1 Done)
Week 8  End → ✅ carpai-server + carpai-cli 各自可编译
Week 10 End → ✅ cargo check --workspace 通过 + E2E 全链路
Week 12 End → 🚀 v1.0.0 release (性能基准 + 部署文档 + 安全审计)
```

---

## 二、统一架构与命名规范

### 2.1 最终 Crate 层次结构

```
CarpAI Monorepo
│
├── crates/
│   │
│   ├── carpai-internal/     ← ✅ Phase 0 已完成 (Pure Trait Layer)
│   │   定位: 零业务逻辑，仅接口定义
│   │   7 traits: SessionStore, ToolExecutor, InferenceBackend,
│   │            VirtualFileSystem, EventBus, MemoryBackend,
│   │            + CodeCompletion/AuthProvider/MemoryStore/InferenceEngine/ToolRegistry
│   │   + AgentContext (DI 容器) + AppConfig (Layer 0 配置)
│   │
│   ├── carpai-core/         ← 📍 Phase 1 目标 (Business Logic Layer)
│   │   定位: 具体实现，依赖 carpai-internal
│   │   - CoreConfig (Layer 1 配置)
│   │   - 6 个 Local 实现 (从 internal 迁入)
│   │   - Agent 运行时 (~12 模块)
│   │   - 记忆系统 (~13 模块)
│   │   - 工具系统 (~4 模块)
│   │   - 补全引擎代理 (~4 模块)
│   │   - 重构引擎 (~14 模块)
│   │   - AST/语义分析 (~8 模块)
│   │   - 会话管理 (~6 模块)
│   │   - 文件操作 (~7 模块)
│   │   - Git 集成 (~3 模块)
│   │   - 错误处理 (~4 模块)
│   │   - 性能优化 (~11 模块)
│   │   - 配置基础设施 (~2 模块)
│   │   公共 API: execute_agent_turn() + build_local_agent_context()
│   │
│   ├── carpai-server/       ← Team Beta (ma-guoyang) 负责 (Product: Enterprise Server)
│   │   ServerConfig (Layer 2a 配置)
│   │   gRPC + REST + WebSocket
│   │   Auth (JWT/RBAC/API-Key)
│   │   Enterprise (多租户/配额/审计)
│   │   Observability (metrics/tracing/audit)
│   │
│   ├── carpai-cli/          ← Team Gamma (Paw-brave) 负责 (Product: TUI Client)
│   │   CliConfig (Layer 2b 配置)
│   │   TUI 纯渲染层 (ratatui)
│   │   agent_bridge.rs (TUI ↔ Core 桥接)
│   │   双模式 (Local / Remote→Server)
│   │   Commands + Ambient + Notifications
│   │
│   ├── carpai-sdk/          ← Week 9-10 增强 (IDE Plugin SDK)
│   │   OpenAI 兼容 /v1/chat/completions 类型
│   │   Session CRUD API 接口契约
│   │   gRPC client + HTTP client helper
│   │   OpenAPI 3.0 spec
│   │
│   └── [jcode-* crates]     ← 保持不变 (~100 个子 crate)
│
├── src/                     ← 过渡区 (逐步清空，最终删除 lib.rs)
└── docs/
    └── THREE_TEAM_REFACTOR_PLAN_V3_FINAL.md (本文档)
```

### 2.2 命名规范（强制全员遵守）

| 类别 | 规范 | 正确示例 | 错误示例 |
|------|------|---------|---------|
| **Crate 名称** | `carpai-{layer}` | `carpai-internal`, `carpai-core` | `jcode-carpi-core`, `CarpaiCore` |
| **Trait 名称** | `{名词}Store\|Executor\|Backend\|Provider` | `SessionStore`, `ToolExecutor` | `ISessionManager`, `IToolRunner` |
| **Local 实现** | `Local{Trait名}` 或 `{Sidecar}...{Trait名}` | `LocalFileSessionStore`, `SidecarInferenceBackend` | `session_store_impl`, `InferenceBackendImpl` |
| **Server 实现** | `{Storage}{Trait名}` 或 `{Protocol}...{Trait名}` | `RedisSessionStore`, `GrpcAgentService` | `ServerSessionStore` |
| **Config 结构体** | `{Layer}Config` | `AppConfig`, `CoreConfig`, `ServerConfig`, `CliConfig` | `Config`, `Settings`, `Options` |
| **模块文件名** | `snake_case.rs` | `local_file_store.rs`, `grpc_server.rs` | `localFileStore.rs`, `GrpcServer.rs` |
| **Feature gate** | `product_layer` | `server`, `cli`, `enterprise`, `sdk` | `feature-server`, `with-gui` |
| **环境变量前缀** | `CARPAI_{LAYER}__{FIELD}` | `CARPAI_SERVER__PORT`, `CARPAI_CORE__DATA_DIR` | `SERVER_PORT`, `DATA_DIR` |

### 2.3 依赖方向铁律（违反 = CI 拦截）

```
                    ┌─────────────────┐
                    │  carpai-internal │  Layer 0: Pure Traits
                    └────────┬────────┘
                             │
            ┌────────────────┼────────────────┐
            ▼                ▼                ▼
    ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
    │  carpai-core  │ │ carpai-server │ │  carpai-cli   │
    │  Layer 1      │ │ Layer 2a      │ │ Layer 2b      │
    └──────┬───────┘ └──────┬───────┘ └──────┬───────┘
           │                │               │
           └────────────────┼───────────────┘
                            ▼
                   ┌────────────────┐
                   │  carpai-sdk    │  Layer 2c: IDE Plugin SDK
                   └────────────────┘

❌ 禁止的反向依赖:
  - carpai-server → carpai-cli
  - carpai-cli → carpai-server
  - carpai-core → carpai-server OR carpai-cli
  - carpai-internal → 任何业务 crate
  - carpai-sdk → carpai-server (SDK 必须保持轻量)
```

---

## 三、三层配置方案（完整定义）

### 3.1 配置层次概览

```
Layer 0: AppConfig          (carpai-internal)  ← 运行模式 + 基础参数
    ↓ serde(flatten)
Layer 1: CoreConfig         (carpai-core)      ← 存储路径 + 并发控制 + Provider
    ↓ serde(flatten)
Layer 2: ServerConfig       (carpai-server)    ← 网络 + TLS + DB + Redis + JWT + 多租户
    ↓ serde(flatten)
Layer 2: CliConfig          (carpai-cli)       ← 主题 + 快捷键 + 剪贴板 + 远程模式

覆盖优先级: Hardcoded Default → TOML File → Environment Variable
```

### 3.2 Layer 0: AppConfig（carpai-internal，已存在）

```rust
// crates/carpai-internal/src/agent_context.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub mode: AppMode,              // Cli / Server / Client
    pub working_dir: PathBuf,
    pub default_model: String,
    pub max_context_tokens: usize,
    pub tools_enabled: bool,
    pub default_tool_mode: ExecutionMode,
    pub vfs_enabled: bool,
    pub vfs_root: Option<PathBuf>,
    pub memory_enabled: bool,
    pub event_bus_enabled: bool,
    #[serde(default = "default_log_level")]
    pub log_level: String,          // 新增: "trace"|"debug"|"info"|"warn"|"error"
}
fn default_log_level() -> String { "info".into() }
```

### 3.3 Layer 1: CoreConfig（carpai-core，新建）

```rust
// crates/carpai-core/src/config.rs
use carpai_internal::{AppConfig, ExecutionMode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: String,     // "local" | "openai" | "anthropic" | "qwen"
    pub endpoint: Option<String>,
    pub api_key: Option<String>,   // 从环境变量读取，不写入配置文件
    pub model: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    #[serde(flatten)]
    pub base: AppConfig,

    // === 存储相关 ===
    pub data_dir: PathBuf,
    #[serde(default = "default_session_dir")]
    pub session_subdir: String,
    #[serde(default = "default_memory_dir")]
    pub memory_subdir: String,

    // === 并发控制 ===
    #[serde(default = "default_max_concurrent_tools")]
    pub max_concurrent_tools: usize,
    #[serde(default = "default_max_agent_iterations")]
    pub max_agent_iterations: usize,

    // === 补全 ===
    #[serde(default)]
    pub completion_provider: ProviderConfig,

    // === 缓存 ===
    #[serde(default = "default_cache_size")]
    pub cache_size_mb: usize,
    #[serde(default = "default_disk_cache")]
    pub disk_cache_enabled: bool,
}

impl CoreConfig {
    pub fn session_store_path(&self) -> PathBuf { self.data_dir.join(&self.session_subdir) }
    pub fn memory_store_path(&self) -> PathBuf { self.data_dir.join(&self.memory_subdir) }

    /// 三级配置加载: 默认值 → 文件 → 环境变量
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        if path.exists() {
            let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
            config = toml::from_str(&content).map_err(ConfigError::Parse)?;
        }
        if let Ok(v) = std::env::var("CARPAI_DATA_DIR") { config.data_dir = v.into(); }
        if let Ok(v) = std::env::var("CARPAI_DEFAULT_MODEL") { config.base.default_model = v; }
        if let Ok(v) = std::env::var("CARPAI_LOG_LEVEL") { config.base.log_level = v; }
        if let Ok(v) = std::env::var("CARPAI_MAX_CONCURRENT_TOOLS") {
            config.max_concurrent_tools = v.parse().map_err(|_| ConfigError::Parse(toml::de::Error::custom("invalid number")))?;
        }
        Ok(config)
    }
}

fn default_session_dir() -> String { "sessions".into() }
fn default_memory_dir() -> String { "memory".into() }
fn default_max_concurrent_tools() -> usize { 5 }
fn default_max_agent_iterations() -> usize { 100 }
fn default_cache_size() -> usize { 512 }
fn default_disk_cache() -> bool { true }

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
}
```

### 3.4 Layer 2a: ServerConfig（carpai-server）

```rust
// crates/carpai-server/src/config.rs
use carpai_core::CoreConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig { pub cert_path: PathBuf, pub key_path: PathBuf }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig { pub url: String, pub max_connections: u32 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig { pub url: String, pub pool_size: u32 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(flatten)]
    pub core: CoreConfig,

    // === 网络监听 ===
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default = "default_port")]
    pub port: u16,

    // === TLS ===
    pub tls: Option<TlsConfig>,

    // === 数据库 ===
    pub database: DatabaseConfig,
    pub redis: Option<RedisConfig>,

    // === 认证 ===
    pub jwt_secret: String,
    #[serde(default = "default_jwt_expiry")]
    pub jwt_expiry_hours: u64,

    // === 多租户 ===
    #[serde(default)]
    pub multi_tenant: bool,
    #[serde(default = "default_tenant")]
    pub default_tenant_id: String,

    // === 企业功能开关 ===
    #[serde(default)]
    pub audit_log_enabled: bool,
    #[serde(default)]
    pub rate_limit_enabled: bool,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_rpm: u64,
}
```

### 3.5 Layer 2b: CliConfig（carpai-cli）

```rust
// crates/carpai-cli/src/config.rs
use carpai_core::CoreConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub syntax_theme: String,
    pub ui_color: String,
    pub enable_bold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindConfig {
    pub send_message: String,
    pub interrupt: String,
    pub toggle_help: String,
    pub toggle_file_tree: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardConfig {
    #[serde(default = "default_auto_copy")]
    pub auto_copy_response: bool,
    pub external_editor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupConfig {
    #[serde(default)]
    pub show_banner: bool,
    #[serde(default = "default_startup_timeout")]
    pub model_load_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(flatten)]
    pub core: CoreConfig,

    // === UI ===
    #[serde(default)] pub theme: ThemeConfig,
    #[serde(default)] pub keybinds: KeybindConfig,
    // === 编辑器集成 ===
    #[serde(default)] pub clipboard: ClipboardConfig,
    // === 启动 ===
    #[serde(default)] pub startup: StartupConfig,
    // === 远程模式 ===
    pub remote_server_url: Option<String>,
    #[serde(default = "default_remote_timeout")]
    pub remote_timeout_secs: u64,
}
```

### 3.6 配置文件示例

```toml
# ~/.carpai/config.toml (CLI 模式)
mode = "cli"
working_dir = "/home/user/projects/myapp"
default_model = "claude-sonnet-4-20250514"
max_context_tokens = 200000
log_level = "info"

[core]
data_dir = "~/.carpai"
max_concurrent_tools = 5
cache_size_mb = 512

[core.completion_provider]
provider_type = "local"
endpoint = "http://localhost:11434"

[theme]
syntax_theme = "base16-dark"

[keybinds]
send_message = "Enter"
interrupt = "Escape"
```

```toml
# /etc/carpai/server.toml (Server 模式)
mode = "server"
working_dir = "/var/lib/carpai"
log_level = "warn"

[core]
data_dir = "/var/lib/carpai/data"
max_concurrent_tools = 20

[listen]
addr = "0.0.0.0"
port = 8080

[database]
url = "postgres://carpai:secret@db:5432/carpai"
max_connections = 20

[jwt]
secret = "<generated-hex>"
expiry_hours = 24

[multi_tenant]
enabled = true
default_tenant = "org-default"
```

---

## 四、三组分工矩阵（最终版）

### 4.1 总览

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     CarpAI 重构 v3.0 — 三组协作                            │
├──────────────┬──────────────┬──────────────┬─────────────────────────────┤
│              │ solo-Turbo   │ ma-guoyang    │      Paw-brave              │
│              │ (我们/Solo)  │ (服务端组)   │       (客户端组)             │
├──────────────┼──────────────┼──────────────┼─────────────────────────────┤
│ 工作量占比   │    40%       │    30%       │         30%                 │
│ 人天估算     │   ~24d       │   ~18d       │        ~18d                  │
│ 核心职责     │ 协调+核心+联调│ Server 产品  │      CLI 产品               │
│ 时间范围     │  Week 1-12   │  Week 1-10   │      Week 1-10              │
├──────────────┼──────────────┼──────────────┼─────────────────────────────┤
│ 关键产出     │ carpai-core  │carpai-server │     carpai-cli              │
│              │ +接口契约    │ +API文档     │     +TUI独立+双模式          │
│              │ +SDK增强     │ +企业功能    │     +Dashboard              │
│              │ +最终联调    │             │                             │
│              │ +性能基准    │             │                             │
└──────────────┴──────────────┴──────────────┴─────────────────────────────┘
```

### 4.2 solo-Turbo 职责详述（架构协调 + 核心实现 — 40%，~24 人天）

#### 定位
**架构协调者 + 核心实现者 + SDK 增强者 + 最终集成者 + 性能基准制定者**

#### 完整时间线

| 阶段 | 周数 | 任务 | 人天 | 交付物 |
|------|------|------|------|--------|
| **Phase 1A** | Wk1 | carpai-core 初始化 + Local 实现迁移 | 3d | Cargo.toml + 6 个 Local impl + CoreConfig |
| **Phase 1B** | Wk2-3 | Batch A: Agent 系统迁移 (~12 模块) | 4d | agent/, agent_loop.rs |
| **Phase 1C** | Wk3-4 | Batch B: 记忆+会话 (~19 模块) | 4d | memory/, session/ |
| **Phase 1D** | Wk4-5 | Batch C: 工具+补全 (~8 模块) | 2d | tools/, completion/ |
| **Phase 1E** | Wk5 | Batch D: 重构+AST+Git+错误 (~29 模块) | 5d | refactoring/, analysis/, git/, error/ |
| **清理** | Wk5-6 | 死代码清理 + 编译基线 | 2d | cargo check -p carpai-core 通过 |
| **接口** | Wk3 | 接口契约冻结 + 发布文档 | 1d | execute_agent_turn API doc |
| **支持** | Wk6-8 | ma-guoyang/Paw-brave 对接支持 + Mock | 2d | MockAgentContext 等 |
| **性能** | Wk6-7 | perf/cache/concurrency 模块迁移 | 2d | ~11 个性能模块 |
| **SDK** | Wk9-10 | carpai-sdk 增强 (OpenAI兼容+Session CRUD) | 4d | OpenAPI spec + client helpers |
| **联调** | Wk9-10 | 跨组集成 + E2E 测试 | 3d | workspace 全编译 + 4 条链路 E2E |
| **收尾** | Wk11-12 | 性能基准 + 部署文档 + 安全审计 | 2d | benchmark report + deploy guide |

#### 详细任务清单

##### Phase 1A: carpai-core 初始化 (Wk1, 3d)

**Day 1: 创建 crate + 迁移 Local 实现**

```
[ ] 创建 crates/carpai-core/Cargo.toml
    edition = "2024"
    dependencies:
      carpai-internal = { path = "../carpai-internal" }
      tokio, anyhow, thiserror, serde, tracing, chrono, uuid
      toml (仅 config 模块使用)

[ ] 创建 crates/carpai-core/src/lib.rs
    pub mod config;                    // CoreConfig
    pub mod session_impl;              // LocalFileSessionStore
    pub mod tool_executor_impl;        // LocalToolExecutor
    pub mod inference_impl;            // SidecarInferenceBackend
    pub mod filesystem_impl;           // LocalFileSystem
    pub mod event_bus_impl;            // InProcessEventBus
    pub mod memory_impl;               // LocalMemoryBackend
    pub mod agent_loop;                // execute_agent_turn()
    Re-export 所有 public类型

[ ] 从 carpai-internal 迁移 local_impls 到 carpai-core
    ⚠️ 注意: 改手动 Stream impl，不引入 async-stream/pin_utils (Improvement #1)

[ ] 更新 carpai-internal: 移除 local_impls 相关声明和 re-exports
    确保 cargo check -p carpai-internal 仍然通过
```

**Day 2: CoreConfig 定义 + AgentContext 组装器**

```
[ ] 实现 crates/carpai-core/src/config.rs (完整代码见 §3.3)

[ ] 实现 build_local_agent_context():
    pub fn build_local_agent_context(config: &CoreConfig) -> AgentContext {
        AgentContextBuilder::new()
            .with_config(config.base.clone())
            .with_sessions(Arc::new(LocalFileSessionStore::new(
                config.session_store_path()
            )))
            .with_tools(Arc::new(LocalToolExecutor::new(
                config.max_concurrent_tools,
            )))
            .with_inference(Arc::new(SidecarInferenceBackend::new(
                &config.completion_provider,
            )))
            .with_filesystem(Arc::new(LocalFileSystem::new(
                &config.base.working_dir,
                config.base.vfs_root.as_deref(),
            )))
            .with_events(Arc::new(InProcessEventBus::new(1024)))
            .with_memory(Arc::new(LocalMemoryBackend::new(
                config.memory_store_path(),
            )))
            .build()
            .expect("AgentContext assembly")
    }

[ ] 补充 LogProbs 类型定义 (Improvement #2)
    在 inference_impl.rs 中或 inference_backend trait 中补充
```

**Day 3: 验证 + 文档**

```
[ ] cargo check -p carpai-core 通过 (0 errors)
[ ] cargo test -p carpai-core (数据结构序列化测试)
[ ] 为每个 Local impl 添加架构注释 (/// 层级说明)
[ ] 输出接口契约草案 (供 Wk3 冻结)
```

##### Phase 1B-E: 模块迁移 (Wk2-5, ~17d)

**修正后的批次规划（基于 V2 Review Blocker #3 的现实估算）:**

**Batch A — Agent 系统 (Wk2-Wk3, 4d):**
```
目标: crates/carpai-core/src/agent/
源: src/{agent.rs, agent_runtime.rs, sub_agents.rs, skill_system.rs,
      plan_mode.rs, task_planner.rs, task_manager.rs, task_decomposer.rs,
      task_scheduler.rs, plan_verifier.rs, ultraplan.rs}
数量: ~12 模块
关键风险: agent_runtime.rs (711 行, fan-in ~40) 是上帝模块
策略: 先迁移 agent_loop.rs (新写纯逻辑)，再逐步拆分 agent_runtime
验证: 每迁移 3 个模块运行一次 cargo check -p carpai-core
```

**Batch B — 记忆 + 会话 (Wk3-Wk4, 4d):**
```
记忆: crates/carpai-core/src/memory/
  memory.rs, memory_agent.rs, memory_graph.rs, memory_log.rs,
  memory_types.rs, memory_prompt.rs, memory_advanced.rs,
  semantic_memory.rs, hierarchical_memory.rs, knowledge_graph.rs,
  knowledge.rs, knowledge_agents.rs, protocol_memory.rs
  数量: ~13 模块

会话: crates/carpai-core/src/session/
  session.rs, session_export.rs, session_cost_tracker.rs,
  session_gc.rs, runtime_manager.rs, cgroup_isolation.rs
  数量: ~6 模块
总计: ~19 模块
```

**Batch C — 工具 + 补全 (Wk4, 2d):**
```
工具: crates/carpai-core/src/tools/
  tool.rs, mcp.rs, tools.rs, slash_command.rs (~4)

补全: crates/carpai-core/src/completion/
  completion.rs, completion_engine.rs, completion_quality.rs, auto_fallback.rs (~4)
注意: completion_engine 是 jcode-completion 的包装层
总计: ~8 模块
```

**Batch D — 重构 + AST + Git + 错误 (Wk5, 5d):**
```
重构: crates/carpai-core/src/refactoring/
  refactor.rs, refactor_engine.rs, orchestrator.rs, precise_edit.rs,
  atomic_edit_coordinator.rs, diff_engine.rs, diff_integration.rs,
  streaming_diff_preview.rs, compilation_engine.rs, diagnostics.rs,
  transaction.rs, refactor_verify_pipeline.rs, delivery_pipeline.rs (~14)

AST/语义: crates/carpai-core/src/analysis/
  ast.rs, classifier.rs, semantic.rs, context_pruner.rs,
  incremental_index.rs, proactive_context.rs, context.rs, reasoning.rs (~8)

Git: crates/carpai-core/src/git/
  git.rs, git_workflow.rs, version_manager.rs (~3)

错误: crates/carpai-core/src/error/
  error_recovery.rs, error_types.rs, network_retry.rs, allowlist.rs (~4)
总计: ~29 模块
```

**Batch E — 性能模块 (Wk6-7, 2d) — Improvement #3 提前:**
```
crates/carpai-core/src/performance/
  perf.rs, cache_tracker.rs, cache_optimizer.rs, cache_integration.rs,
  cache_break_detector.rs, concurrency_optimizer.rs, compression.rs,
  circuit_breaker.rs, backpressure.rs, token_budget.rs, denial_tracking.rs (~11)
```

##### 死代码清理 (Wk5-6, 2d)

**处置清单（与 V2 完全一致）:**

| 模块 | 处置 | 理由 |
|------|------|------|
| crdt | 归档 `jcode-experimental/` | P2P 未来价值 |
| dictation | 归档 `jcode-experimental/` | 未完成 |
| dap, debugger | 归档 `jcode-debug/` | 未接入 |
| env | **删除** | 被 config 覆盖 |
| goal | **合并** task_planner | 重复 |
| import | **删除** | 被 refactor_engine 覆盖 |
| login_qr | **删除** | CLI 专属→Paw-brave |
| process_memory | **删除** | 被 runtime_manager 覆盖 |
| process_title | **删除** | Windows 桌面端 |
| prompt | **合并** memory/prompt.rs | 单函数 |
| restart_snapshot | **删除** | 被 session_gc 覆盖 |
| runtime_memory_log | **删除** | 被 observability 覆盖 |
| safety | **合并** security/scanner.rs | 重复 |
| scheduler | **删除** | 被 task_scheduler 覆盖 |
| external | **删除** | 占位符 |
| plan | **合并** ultraplan | 命名冲突 |
| workspace_manager | **合并** session/workspace.rs | 单功能 |
| compaction | **合并** memory/compaction.rs | 单功能 |
| rule_reviewer | **移动** enterprise/review.rs | 企业功能 |
| subscription_catalog | **删除** | 未使用 |
| todo, update, usage, video_export | **删除** | CLI 专属→Paw-brave |

##### 接口契约冻结 (Wk3, 1d)

```rust
// ====== carpai-core 公共 API (Week 3 冻结，后续变更需全员同意) ======

/// Agent 核心循环 — 纯业务逻辑，无 UI/网络依赖
///
/// 完整流程:
/// 1. 追加用户消息到 session
/// 2. 调用 inference backend 生成回复
/// 3. 如有 tool calls，执行工具并收集结果
/// 4. 将 tool results 送回 inference 继续生成
/// 5. 返回最终输出
pub async fn execute_agent_turn(
    ctx: &AgentContext,
    user_message: &str,
) -> Result<AgentTurnOutput>;

/// 构建本地开发环境的 AgentContext (自动选择 Local* 实现)
pub fn build_local_agent_context(config: &CoreConfig) -> AgentContext;

/// 一次 Agent 对话的输出
pub struct AgentTurnOutput {
    pub text: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub usage: TokenUsage,
    pub session_id: SessionId,
    pub duration_ms: u64,
}

/// Tool Call 详情
pub struct ToolCallInfo {
    pub name: String,
    pub params: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub duration_ms: u64,
}
```

##### SDK 增强 (Wk9-10, 4d) — Blocker #1 修复

**为什么独立 SDK（不复用 server REST 层）:**
- IDE 插件编译体积避免膨胀 10-20x
- IDE 进程内存避免增加 50-100MB
- 安全审计边界清晰
- 支持多语言 SDK 自动生成

**具体任务:**
```
Day 1: OpenAI 兼容类型定义
  [ ] ChatCompletionRequest/Response (复用 inference_backend 已有类型)
  [ ] /v1/chat/completions 请求/响应结构
  [ ] StreamingChunk (SSE format)

Day 2: Session CRUD API 契约
  [ ] SessionCreateRequest/Response
  [ ] SessionGetRequest/Response
  [ ] SessionListRequest/Response
  [ ] MessageAppendRequest/Response

Day 3: Client Helpers
  [ ] CarpaiClient struct (HTTP + gRPC 双模式)
  [ ] connect(url) → Result<CarpaiClient>
  [ ] chat_completion(req) → Result<ChatCompletionResponse>
  [ ] session_crud(op) → Result<SessionResponse>

Day 4: OpenAPI 3.0 spec + 文档
  [ ] 生成 openapi.yaml
  [ ] SDK 使用示例 (examples/)
  [ ] 多语言绑定说明 (Python/Go/Java stub generator)
```

##### 最终联调 (Wk9-12, ~5d)

见第七节完整联调计划。

### 4.3 ma-guoyang 职责详述（服务端组 — 30%，~18 人天）

#### 完整时间线

| 阶段 | 周数 | 任务 | 人天 | solo-Turbo 依赖 |
|------|------|------|------|-------------------|
| **骨架** | Wk1-2 | Server crate 初始化 + 项目骨架 | 2d | 无 |
| **通信层** | Wk2-4 | gRPC Proto + 服务框架 + REST + WS | 5d | 无 |
| **认证** | Wk4-5 | JWT + RBAC + API-Key 中间件 | 2d | carpai-core 接口 |
| **Engine** | Wk5-6 | Server 实现 wiring (对接 core) | 3d | carpai-core stable API |
| **企业** | Wk6-7 | Multi-tenant + Quota + Audit | 2d | Wk5-6 |
| **观测** | Wk7 | Metrics + Tracing + Health | 1d | Wk6 |
| **测试** | Wk8 | 单元测试 + 集成测试 | 2d | Wk7 |
| **联调配合** | Wk9-10 | 配合 solo-Turbo E2E + Bug 修复 | 1d | solo-Turbo 主导 |

#### 目录结构（最终版）

```
crates/carpai-server/src/
├── lib.rs
├── main.rs                      # fn main() → 启动 gRPC + REST + WS
├── config.rs                    # ServerConfig (§3.4)
├── app.rs                       # Application struct (Router 组装)
│
├── grpc/
│   ├── mod.rs
│   ├── server.rs                # tonic Server 启动/关闭/graceful shutdown
│   ├── proto/                   # .proto 文件
│   │   └── agent.proto          # Agent service 定义
│   ├── agent_service.rs         # ChatCompletion RPC handler
│   ├── session_service.rs       # Session CRUD RPC
│   ├── tool_service.rs          # Tool execution RPC
│   └── health_service.rs        # Health check (gRPC health protocol)
│
├── rest/
│   ├── mod.rs
│   ├── router.rs                # Axum Router::new() ... .route(...) 组装
│   ├── agent_handler.rs         # POST /v1/chat/completions (OpenAI compatible)
│   ├── session_handler.rs       # GET/POST /v1/sessions/:id
│   ├── tool_handler.rs          # POST /v1/tools/:name
│   ├── middleware.rs            # AuthExtractor + RateLimitLayer + CorsLayer
│   └── errors.rs                # API Error response types
│
├── ws/
│   ├── mod.rs
│   ├── handler.rs               # WebSocket upgrade endpoint
│   ├── connection.rs            # Per-connection state (session_id, sender)
│   └── broadcast.rs             # Broadcast to all subscribers
│
├── auth/
│   ├── mod.rs
│   ├── jwt.rs                   # encode/decode/verify JWT
│   ├── api_key.rs               # validate X-API-Key header
│   └── rbac.rs                  # check_permission(user_role, resource, action)
│
├── enterprise/
│   ├── mod.rs
│   ├── multi_tenant.rs          # TenantContext extractor middleware
│   ├── quota.rs                 # UsageQuota tracker (Redis-backed)
│   └── admin_api.rs             # Admin-only endpoints (/admin/users, /admin/stats)
│
└── observability/
    ├── mod.rs
    ├── metrics.rs               # Prometheus Gauge/Counter/Histogram
    ├── tracing.rs               # opentelemetry-rust setup
    └── audit.rs                 # AuditLog writer (async file + optional DB)
```

#### Beta 必须遵守的约束（来自 V2 Review §4.1-4.3）

- ✅ DI 容器使用 `AgentContext`，不自建 `SessionContext/AppState`
- ✅ EventBus 使用 `clone_box()` 而非 `Clone`
- ✅ `ExecutionMode` 不是 `Copy`
- ✅ BusEvent 的 `Deserialize` bound 需要 HRTB: `for<'a> Deserialize<'a>`
- ✅ `EventBusExt` blanket impl 需要 `?Sized`: `impl<T: ?Sized + EventBus> EventBusExt for T {}`
- ❌ 不在 carpai-internal 加业务逻辑
- ❌ 不重新定义已有 trait
- ❌ 不让 carpai-server 和 carpai-cli 同时依赖
- ❌ Phase 1 不引入 `config` crate（用 serde 手动加载）
- ❌ EventBus 不带 `Clone` supertrait

### 4.4 Paw-brave 职责详述（客户端组 — 30%，~18 人天）

#### 完整时间线

| 阶段 | 周数 | 任务 | 人天 | solo-Turbo 依赖 |
|------|------|------|------|-------------------|
| **骨架** | Wk1-2 | CLI crate 初始化 + ratatui skeleton | 2d | 无 |
| **TUI剥离** | Wk3-4 | 业务逻辑提取 (最高优先!) | 4d | 无 |
| **Commands** | Wk4-5 | CLI commands 迁移 + 双模式架构 | 3d | carpai-core 接口 |
| **Ambient** | Wk5-6 | 后台任务 + 通知渠道 | 2d | Wk4-5 |
| **Dashboard** | Wk6-7 | Debug panel + Side panel | 2d | Wk5-6 |
| **打磨** | Wk7-8 | 错误处理 + 优雅降级 + 边缘场景 | 2d | Wk7 |
| **测试** | Wk8-9 | 集成测试 (local + remote mode) | 2d | Wk8 |
| **联调配合** | Wk9-10 | 配合 solo-Turbo E2E + Bug 修复 | 1d | solo-Turbo 主导 |

#### 目录结构（最终版）

```
crates/carpai-cli/src/
├── main.rs                      # fn main() → cli::run()
├── lib.rs
├── config.rs                    # CliConfig (§3.5)
│
├── cli/
│   ├── mod.rs
│   ├── startup.rs               # TUI init (raw mode, alternate screen)
│   ├── dispatch.rs              # Command routing
│   └── commands/
│       ├── chat.rs              # $ carpai chat
│       ├── serve.rs             # $ carpai serve (launcher for server binary)
│       ├── ask.rs               # $ carpai ask "question"
│       ├── completion.rs        # $ carpai complete <file> <line> <col>
│       └── ...
│
├── tui/                         ← 纯渲染层! 不含任何 Agent 业务逻辑!
│   ├── mod.rs
│   ├── app.rs                   # App { state, messages, input_mode }
│   ├── widgets/
│   │   ├── chat_view.rs         # 消息列表渲染
│   │   ├── input_bar.rs         # 输入框 (textarea + 自补全)
│   │   ├── status_line.rs       # 底部状态栏 (model, tokens, mode)
│   │   ├── file_tree.rs         # 左侧文件树
│   │   └── help_overlay.rs      # ? 快捷键帮助
│   ├── event.rs                 # enum Event { Key, Mouse, Resize, Tick }
│   ├── handler.rs               # update() + draw() 分发 (只调用 bridge)
│   └── theme.rs                 # Color scheme definitions
│
├── agent_bridge.rs              # ⭐ 核心: TUI ↔ carpai-core 桥接
│                                #   只委托，零业务逻辑
│
├── ambient/
│   ├── mod.rs
│   ├── runner.rs                # Background task executor
│   └── scheduler.rs             # Cron-like scheduling
│
├── notifications/
│   ├── mod.rs
│   ├── telegram.rs              # Telegram bot notify
│   ├── gmail.rs                 # Gmail summary
│   └── browser.rs               # Browser open link
│
└── modes/
    ├── mod.rs
    ├── local.rs                 # LocalMode { ctx: AgentContext }
    └── remote.rs                # RemoteMode { client: GrpcClient }
```

#### TUI 业务逻辑剥离详解（Wk3-4, 4d）— 最高优先级

**这是 CLI 能独立编译的前置条件。**

**剥离前（反模式）:**
```rust
// 当前 src/tui/app.rs (问题代码)
impl App {
    async fn execute_agent_command(&mut self, msg: String) {
        let context = self.build_completion_context();  // ← 业务逻辑混入 UI
        let candidates = self.engine.complete(&context).await?;
        self.render_completion_results(&candidates);     // ← 渲染逻辑
    }
}
```

**剥离后（目标状态）:**
```rust
// crates/carpai-cli/src/tui/app.rs (纯渲染层)
impl App {
    async fn handle_user_input(&mut self, input: String) {
        self.messages.push(UIMessage::User(input.clone()));
        match self.bridge.execute_turn(&input).await {
            Ok(output) => {
                self.messages.push(UIMessage::Assistant(output.text));
                if !output.tool_calls.is_empty() {
                    self.render_tool_calls(&output.tool_calls);
                }
            }
            Err(e) => self.messages.push(UIMessage::Error(e.to_string())),
        }
    }
}

// crates/carpai-cli/src/agent_bridge.rs (桥接层)
pub struct AgentBridge {
    mode: BridgeMode,
    local_ctx: Option<Arc<RwLock<AgentContext>>>,   // 来自 carpai-core
    grpc_client: Option<CarpaiGrpcClient>,             // 连接 carpai-server
}

enum BridgeMode { Local, Remote { url: String } }

impl AgentBridge {
    pub async fn execute_turn(&self, user_msg: &str) -> Result<AgentTurnOutput> {
        match self.mode {
            BridgeMode::Local => {
                let ctx = self.local_ctx.as_ref().ok_or(BridgeError::NoContext)?.read().await;
                carpai_core::execute_agent_turn(&ctx, user_msg).await
            }
            BridgeMode::Remote { ref url } => {
                let client = self.grpc_client.as_ref().ok_or(BridgeError::NoConnection)?;
                let req = ChatCompletionRequest::from_user_message(user_msg);
                client.chat_completion(req).await.map(|r| r.into())
            }
        }
    }
}
```

---

## 五、接口契约（Interface Contracts）

### 5.1 solo-Turbo → ma-guoyang/Paw-brave 公共 API（Week 3 冻结）

```rust
// ====== carpai-core 公共 API ======

/// Agent 核心循环 — 纯业务逻辑
pub async fn execute_agent_turn(ctx: &AgentContext, msg: &str) -> Result<AgentTurnOutput>;

/// 构建 AgentContext (自动选择 Local* 实现)
pub fn build_local_agent_context(config: &CoreConfig) -> AgentContext;

/// 输出类型
pub struct AgentTurnOutput {
    pub text: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub usage: TokenUsage,
    pub session_id: SessionId,
    pub duration_ms: u64,
}
pub struct ToolCallInfo {
    pub name: String, pub params: Value, pub result: Option<Value>, pub duration_ms: u64,
}
```

### 5.2 ma-guoyang → solo-Turbo Server 实现（Week 5-7 提供）

ma-guoyang 在 `crates/carpai-server/src/` 内实现 Server 版本 trait impl:

// 供 solo-Turbo 在联调时注入测试
pub struct RedisSessionStore { /* ... */ } impl SessionStore for RedisSessionStore { /* ... */ }
pub struct MultiProviderInferenceEngine { /* ... */ } impl InferenceBackend for MultiProviderInferenceEngine { /* ... */ }
pub struct SandboxToolExecutor { /* ... */ } impl ToolExecutor for SandboxToolExecutor { /* ... */ }
pub struct PgEventBus { /* ... */ } impl EventBus for PgEventBus { /* ... */ }
pub struct VectorMemoryBackend { /* ... */ } impl MemoryBackend for VectorMemoryBackend { /* ... */ }
```

### 5.3 Paw-brave → solo-Turbo CLI 特定组件

```rust
// crates/carpai-cli/src/config.rs (见 §3.5)
// crates/carpai-cli/src/agent_bridge.rs (见 §4.4)
```

### 5.4 SDK 公共 API（Week 9-10 新增）

```rust
// crates/carpai-sdk/src/lib.rs (增强后)

// --- OpenAI Compatible ---
pub use types::{ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatRole};
pub use streaming::{SSEStream, StreamChunk};

// --- Session CRUD ---
pub use session::{SessionCreateRequest, SessionResponse, MessageAppendRequest};

// --- Client ---
pub use client::{CarpaiClient, CarpaiClientBuilder};

impl CarpaiClient {
    pub async fn chat_completion(&self, req: &ChatCompletionRequest) -> Result<ChatCompletionResponse>;
    pub async fn chat_completion_stream(&self, req: &ChatCompletionRequest) -> Result<SSEStream>;
    pub async fn session_create(&self, req: &SessionCreateRequest) -> Result<SessionResponse>;
    pub async fn session_get(&self, id: &str) -> Result<SessionResponse>;
    pub async fn message_append(&self, id: &str, req: &MessageAppendRequest) -> Result<()>;
}
```

---

## 六、12 周完整时间线与同步机制

### 6.1 甘特图

```
Week:    1     2     3     4     5     6     7     8     9    10    11    12
         ├─────┼─────┼─────┼─────┼─────┼─────┼─────┼─────┼─────┼─────┼─────┼─────┤
solo-Turbo: [CORE][BATCH][BATCH][BAT-D][CLEAN][PERF][SUPP][----][SDK--][E2E--][PERF][DOCS]
              │ A    │ B    │ C/D  │      │      │      │      │      │      │      │
ma-guoyang:   [SKEL][GRPC][REST ][AUTH ][WIRE][ENTRP][OBSV][TEST][......BUGFIX......]
                         │           │      │      |
Paw-brave:    [SKEL][TUI_S][CMD  ][AMBI][DASH][POLISH][TEST][......BUGFIX......]
                    │     │      │      │
              └───────┴───────┴──────┼──────┼──────┼──────┼──────┼──────┘
                                    │      │      │      │
                              Interface Freeze   Group Test  Cross-Group Integration
                               (Wk3)           (Wk8)      (Wk9-10)
```

### 6.2 同步会议节奏

| 会议 | 频率 | 参与者 | 时长 | 议题 |
|------|------|--------|------|------|
| **Daily Standup** | 每天 | 全员 | 15min | 昨日/今日/阻塞 |
| **Interface Sync** | 每周二 | Leads | 1h | 接口契约评审 |
| **Integration Prep** | Wk8 开始每天 | 全员 | 1h | 联调进度/Bug 分派 |
| **Milestone Review** | Wk4/Wk8/Wk10/Wk12 | 全员 + Stakeholder | 2h | 阶段验收/下一步 |

### 6.3 代码合并策略

```
main (protected, only solo-Turbo can push fast-forward)
  │
  ├── alpha/core-build      (Wk1-6)  ← solo-Turbo 专用
  │   └── merge: cargo check -p carpai-core + carpai-internal must pass
  │
  ├── beta/server-build      (Wk1-8)  ← ma-guoyang 专用 (PR to main)
  │   └── CI check: no carpai-cli imports + interface compliance
  │
  ├── gamma/cli-build        (Wk1-9)  ← Paw-brave 专用 (PR to main)
  │   └── CI check: no carpai-server imports + interface compliance
  │
  └── integration            (Wk9-12) ← Only solo-Turbo pushes
      └── cargo check --workspace + cargo test --workspace
```

**CI 门禁（必须全部通过才能 merge）:**
```yaml
# .github/workflows/ci.yml
on: [pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo check --workspace
      - run: cargo clippy --workspace -- -D warnings
      - run: cargo test --workspace

  interface-compliance:
    runs-on: ubuntu-latest
    steps:
      - name: No cross-product imports
        run: |
          grep -rq "carpai-server" carpai-cli/ && echo "ERROR: cli imports server" && exit 1 || true
          grep -rq "carpai-cli" carpai-server/ && echo "ERROR: server imports cli" && exit 1 || true
          grep -rq "carpai-core" carpai-internal/src/ && echo "ERROR: internal depends on core" && exit 1 || true
```

---

## 七、Week 9-12 联调与收尾计划

### 7.1 Week 9-10: 跨组集成（solo-Turbo 主导，3d + SDK 4d）

**Day 1-2 (solo-Turbo): 环境搭建 + 合并**
```
[ ] Merge beta/server-build branch → main (resolve conflicts,预计 <15 个)
[ ] Merge gamma/cli-build branch → main (resolve conflicts)
[ ] cargo check --workspace (记录所有错误，按来源分类)
[ ] 分类: solo-Turbo自有 / ma-guoyang引入 / Paw-brave引入 / 跨组交互

**Day 3-4 (solo-Turbo + ma-guoyang + Paw-brave): 跨组 Bug 修复**
[ ] solo-Turbo 优先修复自有模块编译错误
[ ] 分配 ma-guoyang 引入 Bug → ma-guoyang fix → PR → solo-Turbo review → merge
[ ] 分配 Paw-brave 引入 Bug → Paw-brave fix → PR → solo-Turbo review → merge
[ ] 修复跨组交互问题 (接口不匹配、类型不一致)
[ ] 每次 fix 后 full workspace check
```

**Day 5-8 (solo-Turbo): SDK 增强 + E2E 测试（并行进行）**
```
[ ] SDK: OpenAI 兼容类型定义 (Day 5)
[ ] SDK: Session CRUD API (Day 5-6)
[ ] SDK: Client helpers (Day 6-7)
[ ] E2E Test 1: CLI local mode (TUI → type → receive reply)
[ ] E2E Test 2: Server standalone (health check → gRPC call → REST call)
[ ] E2E Test 3: CLI remote mode (CLI → gRPC → Server → reply)
[ ] E2E Test 4: SDK basic flow (client.connect → chat → receive)
[ ] SDK: OpenAPI spec generation (Day 8)
```

**E2E 测试时间保障 (Improvement #4: 从 1d 增加到 2-3d)**

### 7.2 Week 11-12: 性能基准 + 收尾（solo-Turbo 主导，2d + 2d）

**Week 11: 性能基准**
```
[ ] 编译时间基线: cargo build --release --timings=v2 (每个 crate 贡献)
[ ] 二进制大小检查: ls -lh target/release/{carpai,carpai-server,carpai-cli}
[ ] 内存占用基线: 启动各产品，记录 RSS
[ ] Agent turn 延迟基准: execute_agent_turn p50/p95/p99
[ ] 并发压力测试: 10/50/100 并发连接
[ ] 生成 performance_benchmark.md
```

**Week 12: 部署文档 + 安全审计 + Release**
```
[ ] 部署文档: Dockerfile, docker-compose.yml, systemd unit file
[ ] 配置文档: production.toml 示例 + 环境变量清单
[ ] 安全审计: 依赖漏洞扫描 (cargo audit), 权限检查, 密钥轮换指南
[ ] Architecture.md 更新 (最终版)
[ ] README.md 更新 (安装/快速开始/三产品说明)
[ ] git tag v1.0.0
[ ] Release notes
```

### 7.3 Bug 分派流程

```
Bug 发现 (Week 9-10)
   │
   ├─→ solo-Turbo 自有 → solo-Turbo 立即修
   │
   ├─→ ma-guoyang 模块 (server/) → solo-Turbo 复现 → Issue → assign ma-guoyang
   │   └─→ ma-guoyang fix → PR → solo-Turbo review → merge
   │
   ├─→ Paw-brave 模块 (cli/) → solo-Turbo 复现 → Issue → assign Paw-brave
   │   └─→ Paw-brave fix → PR → solo-Turbo review → merge
   │
   └─→ 跨组交互 (core↔server, core↔cli)
        ├─→ Interface contract bug → solo-Turbo 修 contract + ma-guoyang/Paw-brave adapt
        └─→ Implementation bug → 对应组修复
```

---

## 八、风险评估与缓解

### 8.1 协作风险

| 风险 | 概率 | 影响 | 缓解措施 | 负责人 |
|------|------|------|---------|--------|
| **接口契约变更导致返工** | 中 | 高 | Wk3 冻结；变更需全员同意；Major 变更走 RFC | Alpha |
| **跨组循环依赖** | 中 | 高 | Alpha 先画完整依赖图；每周审查新 import | Alpha |
| **Beta/Gamma 进度不同步** | 高 | 中 | Weekly Sync；Alpha 准备 Mock 解耦 | Alpha |
| **命名不一致导致冲突** | 高 | 低 | 强制 §2.2 规范；CI lint 检查 | Alpha |
| **编译时间增长** | 中 | 中 | sccache + mold linker；增量编译 | Alpha |
| **SDK 增强范围蔓延** | 中 | 中 | 严格限制在 OpenAI 兼容 + Session CRUD；不做 IDE 特性 | Alpha |

### 8.2 技术风险（来自 V2 Review §5）

| 风险 | 来源 | 缓解措施 | 状态 |
|------|------|---------|------|
| **Trait object safety** | EventBus dyn 兼容 | Alpha Wk1 解决；clone_box() 模板已就绪 | ✅ 已知 |
| **async_trait 边界案例** | trait object 上行为 | Alpha 编写示例测试覆盖 | ⏳ Wk1 做 |
| **Serde + trait object** | AgentContext 序列化 | 自定义 serializer（跳过 dyn 字段） | ⏳ Wk2 做 |
| **async_stream 依赖膨胀** | local_impls 迁移到 core | 改用手动 Stream impl (I1) | ✅ 已决策 |
| **LogProbs 类型缺失** | inference_backend | I2: 在 local impls 中补充 | ⏳ Wk1 Day 2 |

---

## 九、验收标准

### 9.1 每个 Milestone 的 Done Definition

**Week 4 End (Alpha Phase 1 Done):**
- [ ] `cargo check -p carpai-core` 通过 (0 errors)
- [ ] `cargo test -p carpai-core` 全绿 (>50% 核心路径覆盖)
- [ ] `cargo doc -p carpai-core` 无警告
- [ ] `cargo check -p carpai-internal` 仍通过 (未破坏)
- [ ] 18 个遗留模块已处置
- [ ] 接口契约文档已发布给 Beta/Gamma

**Week 8 End (Beta/Gamma Phase Done):**
- [ ] `cargo check -p carpai-server` 通过
- [ ] `cargo check -p carpai-cli` 通过
- [ ] Server: `cargo run --bin carpai-server` 能启动并响应 health check
- [ ] CLI: `cargo run --bin carpai-cli -- chat` 能进入 TUI 并发送消息
- [ ] 所有 PR 符合接口契约

**Week 10 End (Integration Done):**
- [ ] `cargo check --workspace` 通过
- [ ] `cargo test --workspace` 全绿
- [ ] `cargo clippy --workspace` 0 errors (<200 warnings)
- [ ] E2E: CLI(local), Server, CLI(remote→Server), SDK 全链路通过
- [ ] SDK: OpenAPI spec 生成 + client helpers 可用

**Week 12 End (Release):**
- [ ] Performance benchmark report 完成
- [ ] Deploy docs 完成 (Docker + systemd + config)
- [ ] Security audit 通过
- [ ] `git tag v1.0.0` + Release notes

---

## 十、附录

### A. 文件映射速查表

| 如果你要... | 看/改这个文件 | 所属组 |
|-------------|-------------|--------|
| 了解 trait 定义 | `crates/carpai-internal/src/*.rs` | Alpha (已完成) |
| 了解 AgentContext DI | `crates/carpai-internal/src/agent_context.rs` | Alpha (已完成) |
| 了解 Local 实现 | `crates/carpai-core/src/*_impl.rs` | Alpha (Wk1) |
| 了解 Agent 循环 | `crates/carpai-core/src/agent_loop.rs` | Alpha (Wk2) |
| 了解 CoreConfig | `crates/carpai-core/src/config.rs` | Alpha (Wk1) |
| 了解 Server Config | `crates/carpai-server/src/config.rs` | Beta (Wk1) |
| 了解 gRPC proto | `crates/carpai-server/grpc/proto/*.proto` | Beta (Wk2) |
| 了解 REST routes | `crates/carpai-server/src/rest/router.rs` | Beta (Wk3) |
| 了解 TUI 纯渲染层 | `crates/carpai-cli/src/tui/app.rs` | Gamma (Wk3) |
| 了解 Agent Bridge | `crates/carpai-cli/src/agent_bridge.rs` | Gamma (Wk4) |
| 了解 CLI Config | `crates/carpai-cli/src/config.rs` | Gamma (Wk1) |
| Feature Gate 定义 | `Cargo.toml` (root) | Alpha |
| 服务端入口 | `src/bin/jcode-server.rs` | Beta 参考 |
| 安全修复记录 | `src/enterprise/auth.rs` | Beta 参考 |
| SDK OpenAPI spec | `crates/carpai-sdk/openapi.yaml` | Alpha (Wk9) |

### B. 禁止事项清单（全员遵守，违反 = PR 拒绝）

| # | ❌ 禁止 | 原因 |
|---|---------|------|
| 1 | 在 `carpai-internal/` 添加业务逻辑 | trait 层纯净性 |
| 2 | 重新定义已有的 7 个 trait | 重复混乱 |
| 3 | `carpai-server` import `carpai-cli` | 违反单向依赖 |
| 4 | `carpai-cli` import `carpai-server` | 违反单向依赖 |
| 5 | `EventBus` 带 `Clone` supertrait | 破坏 object safety |
| 6 | Phase 1 引入 `config` crate | 编译时间浪费 |
| 7 | 自创命名风格 | 破坏统一性 |
| 8 | 跨组修改他人模块未经沟通 | merge conflict |
| 9 | Local 实现留在 `carpai-internal` | 违反分层定位 (Blocker #4) |
| 10 | SDK 合并到 server REST 层 | IDE 插件膨胀 (Blocker #1) |

### C. 术语表

| 术语 | 定义 |
|------|------|
| **Trait Layer** | `carpai-internal` — 纯接口，零实现 |
| **Core Layer** | `carpai-core` — 业务逻辑 + Local 实现 |
| **Product Layer** | `carpai-server` / `carpai-cli` — 面向用户的产品 |
| **SDK Layer** | `carpai-sdk` — IDE 插件开发包（轻量） |
| **Local 实现** | 开发模式的 concrete trait impl |
| **Server 实现** | 生产环境的 concrete trait impl |
| **AgentContext** | DI 容器，组装所有 trait object |
| **Interface Contract** | 跨组约定的公共 API (Wk3 冻结) |
| **Bridge** | Gamma 的 `agent_bridge.rs` — TUI-Agent 纯委托层 |
| **Three-Layer Config** | AppConfig → CoreConfig → ServerConfig/CliConfig |

---

> **文档版本**: v3.0 FINAL
> **基于**: ARCHITECTURE_REFACTOR_PLAN.md (v1) → V2 Review → V3 Final
> **审批**: ✅ 通过全部 4 Blocker + 4 Improvement 修订
> **下次更新**: Week 4 结束时评估是否需要微调
> **维护者**: solo-Turbo (架构组)
