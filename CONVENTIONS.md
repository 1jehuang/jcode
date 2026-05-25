# CarpAI 代码命名与风格规范 (CONVENTIONS.md)

> **版本**: v2.1 | **日期**: 2026-05-25 | **强制级别**: 全员必须遵守
>
> 🏁 **正式发布** — 经 Batch 1→5 重构验证，规范落地可行
>
> **v2.1 变更摘要**: 引入混合命名规则 (§2.5) — 通用缩写 + 领域完整；结构体命名同步吸收缩写 (§2.3)；完整词库 (§附录 A)。

---

## 一、Rust 标准规范 (RFC 430)

以下规则遵循 Rust 官方命名规范 (RFC 430)，**不可违反**：

| 类别 | 规则 | 正确 | 错误 |
|------|------|------|------|
| **Crate 名** | `snake_case` (连字符) | `carpai-core`, `jcode-llm` | `CarpaiCore`, `jcode_llm` |
| **模块名** | `snake_case` | `session_store`, `inference_backend` | `SessionStore`, `sessionStore` |
| **类型/结构体** | `UpperCamelCase` | `SessionStore`, `AgentContext` | `session_store`, `SESSION_STORE` |
| **Trait** | `UpperCamelCase` | `InferenceBackend`, `EventBus` | `IInferenceBackend`, `inference_backend` |
| **枚举变体** | `UpperCamelCase` | `SessionStatus::Active`, `BusEvent::FileModified` | `ACTIVE`, `active` |
| **函数/方法** | `snake_case` | `execute_agent_turn()`, `build_context()` | `ExecuteAgentTurn()`, `executeAgentTurn()` |
| **常量** | `SCREAMING_SNAKE_CASE` | `MAX_RETRIES`, `DEFAULT_TIMEOUT` | `MaxRetries`, `max_retries` |
| **类型参数** | 短 `UpperCamelCase` (通常单字母) | `T`, `K`, `V`, `Ctx`, `Err` | `Type`, `TYPE` |
| **生命周期** | 短小写 (通常单字母) | `'a`, `'ctx` | `'lifetime`, `'LIFETIME` |

---

## 二、CarpAI 项目专属规范

### 2.1 Crate 命名

```
carpai-{layer}        → 主产品 Crate
jcode-{domain}        → 子功能 Crate
```

| 层级 | 前缀 | 示例 |
|------|------|------|
| Layer 0: Traits | `carpai-internal` | `carpai-internal` |
| Layer 1: Core | `carpai-core` | `carpai-core` |
| Layer 2a: Server | `carpai-server` | `carpai-server` |
| Layer 2b: CLI | `carpai-cli` | `carpai-cli` |
| Layer 2c: SDK | `carpai-sdk` | `carpai-sdk` |
| 子功能 | `jcode-{domain}` | `jcode-llm`, `jcode-auth`, `jcode-lsp` |

### 2.2 Trait 命名

**核心规则**: Trait 名 = `{名词}{角色后缀}`

| 角色后缀 | 含义 | 示例 |
|---------|------|------|
| `Store` | 持久化 CRUD | `SessionStore`, `MemoryStore` |
| `Executor` | 执行操作 | `ToolExecutor` |
| `Backend` | 外部系统对接 | `InferenceBackend`, `MemoryBackend` |
| `Provider` | 能力提供 | `AuthProvider`, `CodeCompletion` |
| `Registry` | 发现/注册 | `ToolRegistry` |
| `Bus` | 消息/事件 | `EventBus` |
| `Engine` | 核心引擎 | `InferenceEngine`, `RefactorEngine` |
| `System` | 虚拟化封装 | `VirtualFileSystem` |

**❌ 禁止**: `I` 前缀 (C# 风格), `Impl` 后缀 (应为具体实现名)

### 2.3 实现类命名 — 混合模式 (通用缩写 + 领域完整)

**核心规则**: 实现名 = `{技术/位置}{领域词}{角色后缀}`

**混合原则**:
- **角色后缀** 和 **技术前缀** 是**通用词**，吸收缩写 → 短而明确
- **领域词** 是**业务专属**，保持完整 → 可读性优先

#### 技术前缀 (通用缩写)

| 前缀 | 含义 | 示例 |
|------|------|------|
| `Local` | 本地进程内 | `LocalToolExec`, `LocalMemBackend` |
| `Sidecar` | Sidecar 进程 | `SidecarInferBackend` |
| `InProcess` | 进程内 (强调无 IPC) | `InProcEventBus` |
| `Redis` | Redis 存储 | `RedisSessionStore` |
| `Grpc` | gRPC 协议 | `GrpcAgentService` |
| `Mock` | 测试桩 | `MockSessionStore` |

#### 角色后缀 (通用缩写)

| 完整后缀 | 缩写后缀 | 适用场景 | 示例 |
|---------|---------|---------|------|
| `Executor` | `Exec` | 工具/任务执行 | `LocalToolExec` |
| `Backend` | `Backend` (保留) | 外部系统对接，语义不可省 | `SidecarInferBackend` |
| `Store` | `Store` (保留) | 持久化，语义不可省 | `RedisSessionStore` |
| `Bus` | `Bus` (保留) | 事件/消息 | `InProcEventBus` |
| `System` | `Sys` | 虚拟化封装 | `VirtualFileSys` |

#### 领域词 (保持完整，不缩写)

| 领域词 | 含义 | 缩写? | 理由 |
|--------|------|-------|------|
| `Session` | 会话 | ❌ | 核心领域概念 |
| `Inference` / `Infer` | 推理 | ✅ `Infer` | ML 生态公认缩写 |
| `Memory` / `Mem` | 记忆 | ⚠️ 视上下文 | 结构体名保留 `Mem`，模块/文件用 `mem` |
| `Event` | 事件 | ❌ | 核心概念 |
| `Tool` | 工具 | ❌ | 核心概念 |
| `Auth` | 认证 | ✅ 已是缩写 | `Authentication` → `Auth` 公认 |
| `File` | 文件 | ❌ | 核心概念 |
| `Agent` | 代理 | ❌ | 核心概念 |
| `Config` | 配置 | ✅ 已是缩写 | `Configuration` → `Config` 公认 |

#### 实现类命名示例 (混合模式)

| 实现 | 旧命名 (全称) | 新命名 (混合) | 节省 |
|------|--------------|--------------|------|
| 本地会话存储 | `LocalFileSessionStore` | `LocalFileSessionStore` | — (已是最优) |
| 本地工具执行 | `LocalToolExecutor` | `LocalToolExec` | -4 chars |
| 本地文件系统 | `LocalFileSystem` | `LocalFileSys` | -3 chars |
| 本地记忆后端 | `LocalMemoryBackend` | `LocalMemBackend` | -2 chars |
| 进程内事件总线 | `InProcessEventBus` | `InProcEventBus` | -4 chars |
| Sidecar 推理后端 | `SidecarInferenceBackend` | `SidecarInferBackend` | -4 chars |
| Redis 会话存储 | `RedisSessionStore` | `RedisSessionStore` | — (已是最优) |

> **设计哲学**: 缩写只用在"每个 Rust 程序员都认识"的词上。领域词如 `Session`, `Event`, `Agent`
> 是读代码时理解上下文的关键，必须完整保留。

### 2.4 Config 结构体命名

| 层级 | 命名 | 示例 |
|------|------|------|
| Layer 0 | `AppConfig` | `AppConfig { mode, working_dir, ... }` |
| Layer 1 | `CoreConfig` | `CoreConfig { data_dir, provider, ... }` |
| Layer 2a | `ServerConfig` | `ServerConfig { listen, database, ... }` |
| Layer 2b | `CliConfig` | `CliConfig { theme, keybinds, ... }` |
| 子配置 | `{Domain}Config` | `DatabaseConfig`, `JwtConfig`, `RedisConfig` |

**❌ 禁止**: `Config` (裸名), `Settings`, `Options`, `Conf`

### 2.5 模块文件命名 — 混合模式 (通用缩写 + 领域完整)

#### 核心原则：子目录分组 + 缩写文件名

当同一目录下存在多个**实现同一 trait 的不同后端**文件时，使用**子目录分组** +
**缩写文件名**，避免文件名过长。

缩写策略与 §2.3 保持一致：**通用词缩写，领域词完整**。

```
❌ 旧模式 (扁平长名):
  src/local_file_session_store.rs      # 28 chars
  src/local_tool_executor.rs           # 25 chars
  src/local_memory_backend.rs          # 25 chars
  src/in_process_event_bus.rs          # 23 chars
  src/local_file_system.rs             # 20 chars
  src/sidecar_inference_backend.rs     # 29 chars

✅ 新模式 (子目录分组 + 混合缩写):
  src/infra/store.rs      # LocalFileSessionStore  implements SessionStore
  src/infra/exec.rs       # LocalToolExec          implements ToolExecutor
  src/infra/fs.rs         # LocalFileSys           implements VirtualFileSystem
  src/infra/mem.rs        # LocalMemBackend        implements MemoryBackend
  src/infra/bus.rs        # InProcEventBus         implements EventBus
  src/sidecar/infer.rs    # SidecarInferBackend    implements InferenceBackend
```

#### 缩写词库 (Rust 生态公认)

| 全称 | 缩写 | 出处/理由 | 适用位置 |
|------|------|-----------|---------|
| `file_system` | `fs` | `std::fs`, `tokio::fs` | 文件名、结构体后缀 |
| `session_store` | `store` | 子目录上下文已含 session | 文件名 |
| `tool_executor` | `exec` | Unix: executor → exec | 文件名、结构体后缀 |
| `inference` | `infer` | ML 生态惯例 | 文件名、结构体前缀 |
| `event_bus` | `bus` | 子目录上下文已含 event | 文件名 |
| `memory` | `mem` | `std::mem` 惯例 | 文件名、结构体前缀 |
| `in_process` | `in_proc` | OS 术语 in-process | 结构体前缀 |
| `system` | `sys` | `std::sys`, Unix 惯例 | 结构体后缀 |
| `authentication` | `auth` | 公认缩写 | 全位置 |
| `configuration` | `config` | 公认缩写 | 全位置 |
| `synchronization` | `sync` | 公认缩写 | 全位置 |
| `communication` | `comm` | 公认缩写 | 全位置 |
| `utilities` | `utils` | 公认缩写 | 全位置 |
| `argument` | `args` | 公认缩写 | 全位置 |
| `management` / `manager` | `mgr` | 企业级惯用 (可选) | 仅文件名 |
| `repository` | `repo` | 公认缩写 | 全位置 |
| `reference` | `ref` | 公认缩写 | 全位置 |
| `database` | `db` | 公认缩写 | 全位置 |
| `message` | `msg` | 网络编程惯例 | 结构体名 |
| `request` | `req` | HTTP 生态惯例 | 结构体名 |
| `response` | `resp` | HTTP 生态惯例 | 结构体名 |
| `context` | `ctx` | Go/Rust 惯例 (可选) | 仅局部变量 |

#### ❌ 禁止缩写的词

| 禁止缩写 | 错误缩写 | 理由 |
|---------|---------|------|
| `session` | ~~`sess`~~ | 核心领域词 |
| `agent` | ~~`agnt`~~ | 核心领域词 |
| `event` | ~~`evt`~~ | 核心领域词 |
| `tool` | ~~`tl`~~ | 核心领域词 |
| `file` | ~~`fl`~~ | 核心领域词 |
| `permission` | ~~`perm`~~ | 安全领域，歧义大 (perm 也指 permanent) |
| `security` | ~~`sec`~~ | 歧义大 (sec 也指 second/section) |
| `service` | ~~`svc`~~ | Kubernetes 惯用，但 Rust 中少见，保留完整 |

#### 文件命名规则总表

| 模式 | 规则 | 示例 |
|------|------|------|
| Trait 定义文件 | `{trait_name_snake}.rs` | `session.rs`, `event_bus.rs`, `tool_executor.rs` |
| 实现文件 (单实现) | `{impl_name_snake}.rs` | `jwt.rs`, `rbac.rs`, `api_key.rs` |
| 实现文件 (多实现分组) | `{group}/{abbrev}.rs` | `infra/store.rs`, `sidecar/infer.rs` |
| Mock 测试桩 | `mock/{trait_name_snake}.rs` | `mock/session_store.rs`, `mock/event_bus.rs` |
| 功能模块 | `{domain}.rs` | `memory.rs`, `agent.rs`, `git.rs` |
| 子模块目录 | `{domain}/mod.rs` | `memory/mod.rs`, `agent/mod.rs` |
| gRPC 服务 | `{service}_service.rs` | `agent_service.rs`, `session_service.rs` |
| gRPC 工具 | `grpc_utils.rs` | (非 `grpc/grpc_utils.rs`) |

**❌ 禁止**: `_impl` 后缀的文件名 (如 `session_impl.rs`)

#### 子目录 mod.rs 范例

```rust
// crates/carpai-core/src/infra/mod.rs
pub mod bus;
pub mod exec;
pub mod fs;
pub mod mem;
pub mod store;

// Re-exports — 外部可直接 use carpai_core::infra::InProcEventBus
pub use bus::InProcEventBus;
pub use exec::LocalToolExec;
pub use fs::LocalFileSys;
pub use mem::LocalMemBackend;
pub use store::LocalFileSessionStore;
```

### 2.6 Feature Gate 命名

| 规则 | 示例 |
|------|------|
| 产品层: 单词 | `server`, `cli`, `enterprise`, `sdk` |
| 功能层: `{domain}_{feature}` | `gpu_inference`, `distributed`, `local_model` |
| 默认 | `default = ["server", "cli"]` |

**❌ 禁止**: `with-` 前缀, `feature-` 前缀, 大写

### 2.7 环境变量命名

```
CARPAI_{LAYER}__{FIELD}    (双下划线分隔层级)
```

| 示例 | 含义 |
|------|------|
| `CARPAI_SERVER__PORT` | Server 层的 port 配置 |
| `CARPAI_CORE__DATA_DIR` | Core 层的数据目录 |
| `CARPAI_CORE__PROVIDER__TYPE` | Core 层的 provider type |

### 2.8 路径重复禁令

**核心规则**: 目录路径中禁止出现连续的同名段。

```
❌ 路径重复:
  grpc/grpc/utils.rs          → grpc/grpc_utils.rs ✅
  enterprise/enterprise/audit → enterprise/audit ✅
  rest/rest/server.rs         → rest/server.rs ✅

✅ 正确: 逐层递进，每层名字有新信息:
  grpc/grpc_utils.rs
  enterprise/audit.rs
  rest/server.rs
```

---

## 三、代码风格规范

### 3.1 rustfmt 配置

```toml
# rustfmt.toml
edition = "2024"
max_width = 100
hard_tabs = false
tab_spaces = 4
newline_style = "Unix"           # LF only
use_small_heuristics = "Default" # 让 rustfmt 自动决定
imports_granularity = "Crate"    # 同一 crate 的 import 合并
group_imports = "StdExternalCrate" # 分组: std → external → crate
reorder_imports = true
reorder_modules = true
```

### 3.2 clippy 配置

```toml
# clippy.toml
cognitive-complexity-threshold = 25
too-many-arguments-threshold = 7
disallowed-methods = [
    { path = "std::panic::panic_any", reason = "use anyhow::bail or panic! with message" },
]
disallowed-types = [
    { path = "std::sync::Mutex", reason = "use tokio::sync::Mutex for async code" },
]
```

### 3.3 import 顺序

```rust
// 1. 标准库
use std::path::PathBuf;
use std::sync::Arc;

// 2. 外部 crate (按字母排序)
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// 3. 当前 crate 内部
use crate::config::CoreConfig;
use crate::memory::MemoryBackend;
```

### 3.4 错误处理风格

```rust
// ✅ 使用 anyhow::Result 作为函数返回类型
pub async fn execute_turn(ctx: &AgentContext) -> Result<AgentTurnOutput> { ... }

// ✅ 使用 anyhow::bail 提前返回错误
if model.is_none() {
    anyhow::bail!("No model configured");
}

// ✅ 使用 .context() 添加上下文
let data = std::fs::read_to_string(&path)
    .with_context(|| format!("Failed to read config: {}", path.display()))?;

// ❌ 禁止 panic! 在业务代码中 (仅测试允许)
// ❌ 禁止 unwrap() 在生产代码中 (仅测试允许)
// ❌ 禁止 expect() 在生产代码中 (仅测试允许)
```

### 3.5 异步风格

```rust
// ✅ 使用 async_trait 定义异步 trait
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn load(&self, id: &SessionId) -> Result<LoadedSession>;
}

// ✅ 使用 Arc<dyn Trait> 作为共享引用
let store: Arc<dyn SessionStore> = Arc::new(LocalFileSessionStore::new(path));

// ✅ 使用 tokio::sync 而非 std::sync (异步代码)
use tokio::sync::{Mutex, RwLock, Notify};
```

### 3.6 注释风格

```rust
/// 模块级文档注释 (三斜线) — 用于 pub item
///
/// # Arguments
/// * `ctx` - Agent 上下文
/// * `message` - 用户输入消息
///
/// # Returns
/// Agent 的回复输出
pub async fn execute_agent_turn(ctx: &AgentContext, message: &str) -> Result<AgentTurnOutput> { ... }

// 普通注释 (双斜线) — 用于内部实现说明
// 注意: 此处需要加锁因为多个 task 可能并发访问
let mut state = self.state.lock().await;
```

### 3.7 模块组织风格

#### lib.rs 声明顺序

```rust
// lib.rs — 模块声明顺序 (严格遵守):

// 1. 配置
pub mod config;

// 2. 基础设施 (无依赖的基础模块)
pub mod id;
pub mod utils;
pub mod platform;
pub mod abort;
pub mod retry;

// 3. Trait 实现 — 按职责分组子目录 + 缩写文件名
pub mod infra;    // infra/{store,exec,fs,mem,bus}.rs
pub mod sidecar;  // sidecar/infer.rs

// 4. 业务逻辑 (按开发阶段/领域分组)
pub mod agent_loop;
pub mod agent;
pub mod memory;
pub mod session;
pub mod completion;
pub mod tools;
pub mod refactoring;
pub mod analysis;
pub mod git;
pub mod error;
pub mod performance;

// 5. 桩/辅助
pub mod rest_llm;

// 6. Mock (测试专用)
pub mod mock;
```

#### lib.rs re-export 顺序

```rust
// Re-exports 顺序:
// 1. 上游 crate 类型 (carpai-internal traits & types)
// 2. 本 crate 配置
// 3. 本 crate 基础设施实现 (infra, sidecar)
// 4. 本 crate 业务逻辑
```

---

## 四、现有代码不一致问题清单

> 以下为已知问题，需按优先级逐步修复。新增代码必须严格遵循本规范。

### 4.1 ✅ 已修复：carpai-core 文件组织结构

| 目录 | 文件 | 结构体 | Trait |
|------|------|--------|-------|
| `infra/` | `store.rs` | `LocalFileSessionStore` | `SessionStore` |
| `infra/` | `exec.rs` | `LocalToolExec` | `ToolExecutor` |
| `infra/` | `fs.rs` | `LocalFileSys` | `VirtualFileSystem` |
| `infra/` | `mem.rs` | `LocalMemBackend` | `MemoryBackend` |
| `infra/` | `bus.rs` | `InProcEventBus` | `EventBus` |
| `sidecar/` | `infer.rs` | `SidecarInferBackend` | `InferenceBackend` |

### 4.2 ✅ 已修复：carpai-server grpc 路径重复

| 旧路径 | 新路径 | 状态 |
|--------|--------|------|
| `grpc/grpc/utils.rs` | `grpc/grpc_utils.rs` | ✅ 已修复 |

### 4.3 🔴 待修复：carpai-server 路径重复

| 当前路径 | 问题 | 建议 |
|----------|------|------|
| `enterprise/enterprise/` | 路径段重复 | 合并到 `enterprise/` 目录下 |
| `rest/rest/` | 路径段重复 | 合并到 `rest/` 目录下 |

### 4.4 🔴 待修复：需重命名的 Config 类型

| 当前 | 位置 | 问题 | 建议 |
|------|------|------|------|
| `ServerConfig` | `src/ws/collab.rs` | 与 carpai-server 的 ServerConfig 冲突 | `CollabServerConfig` |
| `VerifyConfig` | `src/verify/mod.rs` | 太泛 | `RefactorVerifyConfig` |

### 4.5 🔴 待修复：`*_impl` 模式

| 当前 | 问题 | 建议 |
|------|------|------|
| `carpai-server/src/server_impl.rs` | 命名模糊 | → `app_state.rs` |

### 4.6 🔴 待修复：结构体名需同步缩写

| 当前结构体名 | 新结构体名 | 文件 | 状态 |
|------------|-----------|------|------|
| `LocalToolExecutor` | `LocalToolExec` | `infra/exec.rs` | 🔴 待重命名 |
| `LocalFileSystem` | `LocalFileSys` | `infra/fs.rs` | 🔴 待重命名 |
| `LocalMemoryBackend` | `LocalMemBackend` | `infra/mem.rs` | 🔴 待重命名 |
| `InProcessEventBus` | `InProcEventBus` | `infra/bus.rs` | 🔴 待重命名 |
| `SidecarInferenceBackend` | `SidecarInferBackend` | `sidecar/infer.rs` | 🔴 待重命名 |

---

## 五、Crate 架构与依赖方向

### 5.1 分层架构

```
┌─────────────────────────────────────────────┐
│  Layer 2: Products                          │
│  carpai-cli · carpai-server · carpai-sdk    │
└──────────────────────┬──────────────────────┘
                       │ depends on
┌──────────────────────▼──────────────────────┐
│  Layer 1: Business Logic                    │
│  carpai-core                                │
│  ├── infra/   (本地基础设施实现)              │
│  └── sidecar/ (sidecar 推理后端)             │
└──────────────────────┬──────────────────────┘
                       │ depends on
┌──────────────────────▼──────────────────────┐
│  Layer 0: Pure Traits                       │
│  carpai-internal                            │
└─────────────────────────────────────────────┘
```

### 5.2 依赖方向规则

| 规则 | 说明 |
|------|------|
| Layer 2 → Layer 1 → Layer 0 | 单向依赖，禁止反向 |
| `carpai-core` 不可依赖 `carpai-cli` / `carpai-server` | 防止循环依赖 |
| `jcode-*` crates 之间尽量避免互相依赖 | 通过 `carpai-internal` trait 解耦 |
| 同一功能的不同实现放同一子目录 | `infra/`, `sidecar/`, `mock/` |

---

## 六、CI 强制规则

以下规则通过 CI 自动检查，**违反即拦截**:

1. `cargo fmt --check` — 格式检查
2. `cargo clippy -- -D warnings` — lint 检查
3. 依赖方向检查 (carpai-core 不可依赖 carpai-cli/server)
4. Crate 命名前缀检查 (carpai-* / jcode-*)
5. `*_impl.rs` 文件名检查 (新文件禁止使用此后缀)
6. 路径重复检查 (禁止 `foo/foo/` 模式)
7. 禁止缩写词检查 (§2.5 中的 ❌ 禁止缩写列表)

---

## 七、规范变更日志

| 版本 | 日期 | 变更 |
|------|------|------|
| v2.1 | 2026-05-25 | 混合命名规则 (§2.3/§2.5): 通用缩写 + 领域完整；完整缩写词库；禁止缩写列表；结构体同步缩写任务 (§4.6) |
| v2.0 | 2026-05-25 | 引入子目录分组 + 缩写文件名 (§2.5)；路径重复禁令 (§2.8)；Crate 分层架构图 (§五) |
| v1.0 | 2026-05-25 | 初始版本 |
