# solo-Turbo 任务清单 — CarpAI 重构 v3.0

> **团队**: solo-Turbo (架构协调 + 核心实现)
> **总工作量**: ~24 人天 / 12 周
> **核心产出**: carpai-core + SDK 增强 + 最终联调 + 性能基准
> **基于文档**: THREE_TEAM_REFACTOR_PLAN_V3_FINAL.md

---

## 📋 任务总览（按阶段）

| Phase | 周数 | 任务数 | 人天 | 关键交付物 | 状态 |
|-------|------|--------|------|-----------|------|
| **Phase 1A** | Wk1 | 12 | 3d | carpai-core crate + 6 Local impls | ⏳ 待开始 |
| **Phase 1B** | Wk2-3 | 15 | 4d | Agent 系统 (~12 模块) | ⏳ |
| **Phase 1C** | Wk3-4 | 20 | 4d | 记忆+会话 (~19 模块) | ⏳ |
| **Phase 1D** | Wk4-5 | 10 | 2d | 工具+补全 (~8 模块) | ⏳ |
| **Phase 1E** | Wk5 | 35 | 5d | 重构+AST+Git+错误 (~29 模块) | ⏳ |
| **清理** | Wk5-6 | 25 | 2d | 死代码处置 + 编译基线 | ⏳ |
| **接口契约** | Wk3 | 8 | 1d | API 文档冻结 | ⏳ |
| **性能模块** | Wk6-7 | 11 | 2d | perf/cache/concurrency | ⏳ |
| **Mock 支持** | Wk6-8 | 6 | 2d | MockAgentContext 等 | ⏳ |
| **SDK 增强** | Wk9-10 | 16 | 4d | OpenAI 兼容 + Session CRUD | ⏳ |
| **联调** | Wk9-10 | 15 | 3d | workspace 全编译 + E2E | ⏳ |
| **收尾** | Wk11-12 | 18 | 2d | 性能基准 + 部署文档 | ⏳ |
| **总计** | **12周** | **~191** | **~24d** | | |

---

## 🔴 Phase 1A: carpai-core 初始化 (Week 1, 3 天)

### 目标
创建 `crates/carpai-core/` crate 骨架，迁移 6 个 Local 实现，定义 CoreConfig

### Day 1: Crate 创建 + Local 实现迁移 (8h)

#### 任务 1.1: 创建 Cargo.toml (30min)
```toml
# crates/carpai-core/Cargo.toml
[package]
name = "carpai-core"
version = "0.1.0"
edition = "2024"

[dependencies]
carpai-internal = { path = "../carpai-internal" }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
toml = "0.8"

[dev-dependencies]
tokio-test = "0.4"
tempfile = "3"
```

**验收标准**:
- [ ] `cargo init` 或手动创建 Cargo.toml
- [ ] 所有依赖版本与 workspace 一致
- [ ] `edition = "2024"`

---

#### 任务 1.2: 创建 lib.rs 框架 (30min)
```rust
// crates/carpai-core/src/lib.rs
//! CarpAI Core - Business Logic Layer
//!
//! This crate contains all business logic implementations for the CarpAI system.
//! It depends on `carpai-internal` for trait definitions and provides concrete
//! implementations using local storage and execution.

pub mod config;
pub mod session_impl;
pub mod tool_executor_impl;
pub mod inference_impl;
pub mod filesystem_impl;
pub mod event_bus_impl;
pub mod memory_impl;
pub mod agent_loop;

// Re-export key types from carpai-internal for convenience
pub use carpai_internal::{
    AgentContext,
    AgentContextBuilder,
    AppConfig,
    AppMode,
    ExecutionMode,
    SessionId,
    SessionMeta,
    StoredMessage,
    ToolCallInfo,
    TokenUsage,
    // Traits
    SessionStore,
    ToolExecutor,
    InferenceBackend,
    VirtualFileSystem,
    EventBus,
    MemoryBackend,
};

// Re-export core types
pub use config::CoreConfig;

// Re-export local implementations
pub use session_impl::LocalFileSessionStore;
pub use tool_executor_impl::LocalToolExecutor;
pub use inference_impl::SidecarInferenceBackend;
pub use filesystem_impl::LocalFileSystem;
pub use event_bus_impl::InProcessEventBus;
pub use memory_impl::LocalMemoryBackend;

// Re-public API
pub use agent_loop::{execute_agent_turn, AgentTurnOutput, build_local_agent_context};
```

**验收标准**:
- [ ] 所有模块声明正确
- [ ] re-export 路径可编译
- [ ] 文档注释完整

---

#### 任务 1.3: 迁移 LocalFileSessionStore (2h)
**源文件**: `crates/carpai-internal/src/local_file_session_store.rs` (或类似路径)  
**目标**: `crates/carpai-core/src/session_impl.rs`

**关键改动**:
- 改手动 Stream impl（不使用 async-stream）
- 更新 import 路径: `use carpai_internal::*`
- 添加架构注释: `/// Layer 1: Local implementation of SessionStore trait`

**代码模板**:
```rust
// crates/carpai-core/src/session_impl.rs
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::Result;
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use futures::stream::{self, Stream, StreamExt};
use carpai_internal::*;

/// Layer 1: Local file-based session store
///
/// Stores session data as JSONL files in the specified directory.
/// Each session is a separate file named `{session_id}.jsonl`.
pub struct LocalFileSessionStore {
    base_path: PathBuf,
}

impl LocalFileSessionStore {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn session_path(&self, id: &SessionId) -> PathBuf {
        self.base_path.join(format!("{}.jsonl", id))
    }
}

#[async_trait::async_trait]
impl SessionStore for LocalFileSessionStore {
    async fn create_session(&self, meta: SessionMeta) -> Result<SessionId> {
        let id = SessionId::new_v4();
        let path = self.session_path(&id);
        // ... 实现
        Ok(id)
    }

    async fn append_message(&self, session_id: &SessionId, msg: StoredMessage) -> Result<()> {
        let path = self.session_path(session_id);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let line = serde_json::to_string(&msg)? + "\n";
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    async fn load_session(&self, id: &SessionId) -> Result<LoadedSession> {
        let path = self.session_path(id);
        let content = tokio::fs::read_to_string(&path).await?;
        let messages: Vec<StoredMessage> = content
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        // ... 构建 LoadedSession
    }

    // 手动 Stream impl (不使用 async-stream)
    async fn list_sessions(&self) -> Result<Box<dyn Stream<Item = Result<SessionSummary>> + Send + '_>> {
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;
        let mut summaries = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                // 解析 session metadata
                if let Some(stem) = path.file_stem() {
                    if let Ok(id) = uuid::Uuid::parse_str(stem.to_str().unwrap_or("")) {
                        summaries.push(Ok(SessionSummary {
                            id: SessionId::from(id),
                            // ...
                        }));
                    }
                }
            }
        }

        Ok(Box::new(stream::iter(summaries)))
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        let path = self.session_path(id);
        if path.exists() {
            tokio::fs::remove_file(path).await?;
        }
        Ok(())
    }
}
```

**验收标准**:
- [ ] `impl SessionStore for LocalFileSessionStore` 编译通过
- [ ] 不依赖 async-stream crate
- [ ] 单元测试: create → append → load → delete 全流程

---

#### 任务 1.4: 迁移其余 5 个 Local 实现 (3h)

按相同模式迁移:

| # | 文件名 | Trait | 关键注意点 |
|---|--------|-------|-----------|
| 1.4.1 | `tool_executor_impl.rs` | ToolExecutor | 最大并发数配置化 |
| 1.4.2 | `inference_impl.rs` | InferenceBackend | LogProbs 类型补充 (I2) |
| 1.4.3 | `filesystem_impl.rs` | VirtualFileSystem | VFS root 可选 |
| 1.4.4 | `event_bus_impl.rs` | EventBus | 使用 clone_box() 非 Clone |
| 1.4.5 | `memory_impl.rs` | MemoryBackend | JSONL 持久化 |

**每个实现的通用检查清单**:
- [ ] import 路径更新为 `carpai_internal::*`
- [ ] 构造函数接受 `CoreConfig` 相关参数
- [ ] 架构注释: `/// Layer 1: Local implementation of {TraitName}`
- [ ] 错误类型统一使用 `anyhow::Result`
- [ ] 异步函数使用 `#[async_trait::async_trait]`

---

#### 任务 1.5: 更新 carpai-internal (30min)
- [ ] 移除 `local_impls/` 目录或相关 re-exports
- [ ] 确保 `cargo check -p carpai-internal` 通过
- [ ] 更新 carpai-internal 的 README 说明 Local 实现已迁移到 carpai-core

---

### Day 2: CoreConfig + AgentContext 组装器 (6h)

#### 任务 2.1: 实现 CoreConfig (2h)
**文件**: `crates/carpai-core/src/config.rs`

完整代码见 V3 FINAL §3.3，关键要点:
- [ ] `serde(flatten)` 继承 AppConfig
- [ ] 三级加载: 默认值 → TOML → 环境变量 (`CARPAI_*` 前缀)
- [ ] `session_store_path()` / `memory_store_path()` 便捷方法
- [ ] ProviderConfig 子结构

**测试用例**:
```rust
#[test]
fn test_config_load_from_toml() {
    let config = CoreConfig::load(Path::new("test_config.toml")).unwrap();
    assert_eq!(config.data_dir, PathBuf::from("~/.carpai"));
}

#[test]
fn test_config_env_override() {
    std::env::set_var("CARPAI_DATA_DIR", "/custom/path");
    let config = CoreConfig::load(Path::new("nonexistent.toml")).unwrap();
    assert_eq!(config.data_dir, PathBuf::from("/custom/path"));
}
```

---

#### 任务 2.2: 实现 build_local_agent_context() (2h)
**文件**: `crates/carpai-core/src/agent_loop.rs` (组装器部分)

```rust
// crates/carpai-core/src/agent_loop.rs
use std::sync::Arc;
use carpai_internal::*;
use crate::config::CoreConfig;
use crate::{
    LocalFileSessionStore,
    LocalToolExecutor,
    SidecarInferenceBackend,
    LocalFileSystem,
    InProcessEventBus,
    LocalMemoryBackend,
};

/// Build a complete AgentContext with all Local* implementations
///
/// This is the primary entry point for CLI/local mode.
/// All trait objects are wired to their local filesystem-backed implementations.
pub fn build_local_agent_context(config: &CoreConfig) -> AgentContext {
    AgentContextBuilder::new()
        .with_config(config.base.clone())
        .with_sessions(Arc::new(LocalFileSessionStore::new(
            config.session_store_path(),
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
        .expect("AgentContext assembly: all components must be valid")
}
```

**验收标准**:
- [ ] 编译通过
- [ ] 集成测试: `build_local_agent_context(&config)` 不 panic
- [ ] 所有 6 个 trait object 已注入

---

#### 任务 2.3: 补充 LogProbs 类型 (1h)
**位置**: `inference_impl.rs` 或新建 `types.rs`

```rust
/// Log probabilities for token-level analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogProbs {
    pub content: Vec<TokenLogProb>,
    pub top_logprobs: Option<Vec<Vec<TokenLogProb>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLogProb {
    pub token: String,
    pub logprob: f64,
    pub bytes: Option<Vec<u8>>,
}
```

---

#### 任务 2.4: execute_agent_turn() 骨架 (1h)
**文件**: `crates/carpai-core/src/agent_loop.rs`

先写骨架，Phase 1B 完成后填充完整逻辑:

```rust
// crates/carpai-core/src/agent_loop.rs (续)

/// Execute one complete agent turn (pure business logic)
///
/// # Flow
/// 1. Append user message to session via SessionStore
/// 2. Call InferenceBackend to generate response
/// 3. If tool_calls present, execute via ToolExecutor
/// 4. Collect results and send back to inference
/// 5. Return final output
///
/// # Arguments
/// * `ctx` - AgentContext containing all trait objects
/// * `user_message` - Raw user input string
///
/// # Returns
/// * `AgentTurnOutput` with text, tool_calls, usage, etc.
pub async fn execute_agent_turn(
    ctx: &AgentContext,
    user_message: &str,
) -> Result<AgentTurnOutput> {
    let start = std::time::Instant::now();

    // TODO: Phase 1B 后实现完整逻辑
    // 目前返回 stub
    Ok(AgentTurnOutput {
        text: format!("[STUB] Received: {}", user_message),
        tool_calls: vec![],
        usage: TokenUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        },
        session_id: ctx.config.default_session_id.clone(),
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Output of a single agent interaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnOutput {
    pub text: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub usage: TokenUsage,
    pub session_id: SessionId,
    pub duration_ms: u64,
}
```

---

### Day 3: 验证 + 文档 (8h)

#### 任务 3.1: 编译验证 (2h)
```bash
# 必须全部通过
cargo check -p carpai-core                    # 0 errors
cargo check -p carpai-internal                 # 未被破坏
cargo test -p carpai-core                      # 数据结构序列化测试
cargo doc -p carpai-core --no-deps             # 无 doc warnings
```

**检查清单**:
- [ ] 0 compilation errors
- [ ] <50 warnings (acceptable for now)
- [ ] 所有 public items 有文档注释
- [ ] no `pub use` 循环依赖

---

#### 任务 3.2: 单元测试 (3h)

**必测模块**:
```rust
// tests/test_local_implements.rs
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_lifecycle() {
        let store = LocalFileSessionStore::new(tempdir().into_path());
        let meta = SessionMeta::default();
        let id = store.create_session(meta).await.unwrap();
        let msg = StoredMessage::user("hello");
        store.append_message(&id, msg).await.unwrap();
        let loaded = store.load_session(&id).await.unwrap();
        assert_eq!(loaded.messages.len(), 1);
        store.delete_session(&id).await.unwrap();
    }

    #[tokio::test]
    async fn test_tool_executor_basic() {
        let executor = LocalToolExecutor::new(2);
        // 测试工具注册和执行
    }

    #[test]
    fn test_core_config_default() {
        let config = CoreConfig::default();
        assert!(config.data_dir.ends_with(".carpai"));
        assert_eq!(config.max_concurrent_tools, 5);
    }

    #[test]
    fn test_agent_context_assembly() {
        let config = CoreConfig::default();
        let ctx = build_local_agent_context(&config);
        assert!(ctx.sessions.is_some());
        assert!(ctx.tools.is_some());
        assert!(ctx.inference.is_some());
    }
}
```

---

#### 任务 3.3: 接口契约草案 (2h)
输出文件: `docs/INTERFACE_CONTRACT_DRAFT.md`

包含:
- [ ] `execute_agent_turn()` 签名 + 文档
- [ ] `build_local_agent_context()` 参数说明
- [ ] `AgentTurnOutput` 字段含义
- [ ] `CoreConfig` 配置项清单
- [ ] 错误码枚举 (ErrorCode enum)
- [ ] 使用示例代码

---

#### 任务 3.4: 架构注释审查 (1h)
- [ ] 每个 Local impl 有 `/// Layer 1:` 注释
- [ ] lib.rs 有 crate 级文档
- [ ] config.rs 有字段级文档
- [ ] 公共函数有 `# Arguments` / `# Returns` / `# Examples`

---

### Day 1-3 验收标准总结

- [ ] `cargo check -p carpai-core` ✅ 0 errors
- [ ] `cargo check -p carpai-internal` ✅ 未破坏
- [ ] 6 个 Local impl 全部可编译
- [ ] CoreConfig 三级加载可用
- [ ] `build_local_agent_context()` 可调用
- [ ] `execute_agent_turn()` 骨架就绪
- [ ] 接口契约草案已发布给 ma-guoyang/Paw-brave
- [ ] 单元测试 >80% 通过

---

## 🟠 Phase 1B: Agent 系统迁移 (Week 2-3, 4 天)

### 目标
迁移 ~12 个 Agent 相关模块到 `crates/carpai-core/src/agent/`

### 源模块清单
```
src/agent.rs
src/agent_runtime.rs          ← ⚠️ 上帝模块 (711行, fan-in ~40)
src/sub_agents.rs
src/skill_system.rs
src/plan_mode.rs
src/task_planner.rs
src/task_manager.rs
src/task_decomposer.rs
src/task_scheduler.rs
src/plan_verifier.rs
src/ultraplan.rs
+ 新建 src/agent_loop.rs      ← Phase 1A 已创建骨架
```

### Day 4-5: Batch A-1 核心模块 (8h)

#### 任务 4.1: 创建 agent/ 目录结构 (30min)
```
crates/carpai-core/src/agent/
├── mod.rs              # pub mod 声明
├── runtime.rs          # 从 agent_runtime.rs 迁移
├── sub_agents.rs       # 从 sub_agents.rs 迁移
├── skill_system.rs     # 从 skill_system.rs 迁移
├── plan_mode.rs        # 从 plan_mode.rs 迁移
└── task/
    ├── mod.rs
    ├── planner.rs      # 从 task_planner.rs 迁移
    ├── manager.rs      # 从 task_manager.rs 迁移
    ├── decomposer.rs   # 从 task_decomposer.rs 迁移
    ├── scheduler.rs    # 从 task_scheduler.rs 迁移
    └── verifier.rs     # 从 plan_verifier.rs 迁移
```

---

#### 任务 4.2: 迁移 agent.rs → agent/mod.rs (1.5h)
- [ ] 提取公共类型定义 (AgentState, AgentConfig 等)
- [ ] 更新所有 `use crate::xxx` 为 `use crate::agent::xxx` 或 `use carpai_core::xxx`
- [ ] 分离 UI 相关代码 (如果有) 到 Paw-brave 职责

---

#### 任务 4.3: 迁移 agent_runtime.rs → agent/runtime.rs (3h) ⚠️ 高风险

**策略**: 
1. 先读取完整文件，理解依赖关系图
2. 识别 fan-in 来源 (哪些模块依赖它)
3. 将纯逻辑部分提取到 runtime/core.rs
4. 将状态管理部分提取到 runtime/state.rs
5. 保持公共 API 不变

**检查清单**:
- [ ] 列出所有 `use crate::agent_runtime` 的位置
- [ ] 确认无循环依赖引入
- [ ] `cargo check -p carpai-core` 通过
- [ ] 原有功能测试仍通过

---

#### 任务 4.4: 迁移子代理系统 (2h)
- [ ] sub_agents.rs → agent/sub_agents.rs
- [ ] skill_system.rs → agent/skill_system.rs
- [ ] plan_mode.rs → agent/plan_mode.rs
- [ ] 更新相互引用

---

#### 任务 4.5: 中间验证 (1h)
```bash
cargo check -p carpai-core    # 必须通过
cargo test -p carpai-core     # Agent 相关测试
```

---

### Day 6-7: Batch A-2 任务系统 (8h)

#### 任务 6.1: 迁移 task_planner.rs (2h)
- [ ] 创建 agent/task/mod.rs
- [ ] 迁移并重构: 提取 TaskPlan 结构体
- [ ] 添加单元测试

#### 任务 6.2: 迁移 task_manager + decomposer + scheduler (3h)
- [ ] 注意三者之间的依赖关系
- [ ] manager 依赖 decomposer 的输出
- [cheduler 依赖 manager 的状态
- [ ] 按依赖顺序迁移

#### 任务 6.3: 迁移 plan_verifier + ultraplan (2h)
- [ ] verifier 独立性高，可优先迁移
- [ ] ultraplan 可能依赖 goal (已合并到这里)

#### 任务 6.4: 完成 agent_loop.rs 填充 (1h)
现在 Agent 模块已就绪，可以填充 `execute_agent_turn()` 的真实逻辑:
```rust
pub async fn execute_agent_turn(ctx: &AgentContext, user_msg: &str) -> Result<AgentTurnOutput> {
    // 1. 获取或创建 session
    let session_id = ctx.get_or_create_session().await?;

    // 2. 追加用户消息
    let user_msg = StoredMessage::user(user_msg);
    ctx.sessions.append_message(&session_id, user_msg).await?;

    // 3. 加载会话历史构建 context
    let history = ctx.sessions.load_session(&session_id).await?;

    // 4. 调用 inference
    let response = ctx.inference.generate(&history.messages).await?;

    // 5. 处理 tool calls (如有)
    let mut tool_calls = vec![];
    if !response.tool_calls.is_empty() {
        for tc in &response.tool_calls {
            let result = ctx.tools.execute(&tc.name, &tc.params).await?;
            tool_calls.push(ToolCallInfo {
                name: tc.name.clone(),
                params: tc.params.clone(),
                result: Some(result),
                duration_ms: 0,
            });
        }
    }

    // 6. 追加 assistant 回复
    let assistant_msg = StoredMessage::assistant(&response.text);
    ctx.sessions.append_message(&session_id, assistant_msg).await?;

    Ok(AgentTurnOutput {
        text: response.text,
        tool_calls,
        usage: response.usage,
        session_id,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
```

---

### Phase 1B 验收标准
- [ ] `crates/carpai-core/src/agent/` 包含 12 个模块
- [ ] `cargo check -p carpai-core` 0 errors
- [ ] `execute_agent_turn()` 有完整实现
- [ ] Agent 相关测试 >70% 通过

---

## 🟡 Phase 1C: 记忆+会话系统 (Week 3-4, 4 天)

### 目标
迁移 ~19 个记忆和会话模块

### Day 8-9: 记忆系统 (8h)

#### 模块清单 (13 个)
```
memory.rs              → memory/mod.rs
memory_agent.rs        → memory/agent.rs
memory_graph.rs        → memory/graph.rs
memory_log.rs          → memory/log.rs
memory_types.rs        → memory/types.rs
memory_prompt.rs       → memory/prompt.rs
memory_advanced.rs     → memory/advanced.rs
semantic_memory.rs     → memory/semantic.rs
hierarchical_memory.rs → memory/hierarchical.rs
knowledge_graph.rs     → memory/knowledge_graph.rs
knowledge.rs           → memory/knowledge.rs
knowledge_agents.rs    → memory/knowledge_agents.rs
protocol_memory.rs     → memory/protocol.rs
```

#### 迁移策略
1. **Day 8**: 核心类型 (types, log, graph) — 4h
2. **Day 9**: 高级功能 (semantic, hierarchical, knowledge*) — 4h

**注意事项**:
- [ ] memory 可能依赖 knowledge_graph，注意顺序
- [ ] protocol_memory 可能是独立协议适配器
- [ ] semantic_memory 可能需要 embedding 模型 (feature-gate)

---

### Day 10-11: 会话系统 (8h)

#### 模块清单 (6 个)
```
session.rs              → session/mod.rs
session_export.rs       → session/export.rs
session_cost_tracker.rs → session/cost_tracker.rs
session_gc.rs           → session/gc.rs
runtime_manager.rs      → session/runtime_manager.rs
cgroup_isolation.rs     → session/cgroup_isolation.rs
```

#### 迁移策略
1. **Day 10**: 核心 (mod, export, cost_tracker) — 4h
2. **Day 11**: 管理 (gc, runtime_manager, cgroup) — 4h

**注意事项**:
- [ ] session_gc 依赖 LocalFileSessionStore 的 delete 功能
- [ ] cgroup_isolation 是 Linux 特性，需要 cfg(unix)
- [ ] cost_tracker 可能需要对接 billing 系统 (未来)

---

### Phase 1C 验收标准
- [ ] `crates/carpai-core/src/memory/` 13 模块
- [ ] `crates/carpai-core/src/session/` 6 模块
- [ ] 记忆系统可存储/检索/查询
- [ ] 会话系统可导入/导出/清理

---

## 🔵 Phase 1D: 工具+补全 (Week 4-5, 2 天)

### Day 12: 工具系统 (4h)

#### 模块清单 (4 个)
```
tool.rs           → tools/mod.rs
mcp.rs            → tools/mcp.rs
tools.rs          → tools/registry.rs
slash_command.rs  → tools/slash_command.rs
```

**关键**:
- [ ] MCP 协议实现保持不变
- [ ] 工具注册表支持动态注册
- [ ] slash_command 是 CLI 特殊命令 (可能部分移至 Paw-brave)

---

### Day 13: 补全系统 (4h)

#### 模块清单 (4 个)
```
completion.rs           → completion/mod.rs
completion_engine.rs    → completion/engine.rs  ← jcode-completion 包装层
completion_quality.rs   → completion/quality.rs
auto_fallback.rs        → completion/fallback.rs
```

**关键**:
- [ ] completion_engine 是对 `jcode-completion` crate 的包装/集成
- [ ] 不重复实现补全逻辑，只做桥接
- [ ] quality 和 fallback 是增强功能

---

## 🟣 Phase 1E: 重构+AST+Git+错误 (Week 5, 5 天)

### Day 14-15: 重构引擎 (8h, 14 模块) ⚠️ 最大批次

#### 模块清单
```
refactor.rs                  → refactoring/mod.rs
refactor_engine.rs           → refactoring/engine.rs
orchestrator.rs              → refactoring/orchestrator.rs
precise_edit.rs              → refactoring/precise_edit.rs
atomic_edit_coordinator.rs   → refactoring/atomic_edit.rs
diff_engine.rs               → refactoring/diff_engine.rs
diff_integration.rs          → refactoring/diff_integration.rs
streaming_diff_preview.rs    → refactoring/streaming_preview.rs
compilation_engine.rs        → refactoring/compilation.rs
diagnostics.rs               → refactoring/diagnostics.rs
transaction.rs               → refactoring/transaction.rs
refactor_verify_pipeline.rs   → refactoring/verify_pipeline.rs
delivery_pipeline.rs          → refactoring/delivery_pipeline.rs
```

**策略**:
- [ ] Day 14: 核心 (engine, orchestrator, precise_edit, atomic_edit) — 4h
- [ ] Day 15: diff + verify + delivery — 4h

**高风险**:
- compilation_engine.rs 可能依赖 AST 模块
- diff_engine 可能是外部 crate (similar/diffy)
- streaming_diff_preview 可能依赖 TUI (需解耦)

---

### Day 16: AST/语义分析 (4h, 8 模块)

```
ast.rs                → analysis/ast.rs
classifier.rs         → analysis/classifier.rs
semantic.rs           → analysis/semantic.rs
context_pruner.rs     → analysis/context_pruner.rs
incremental_index.rs  → analysis/incremental_index.rs
proactive_context.rs  → analysis/proactive_context.rs
context.rs            → analysis/context.rs
reasoning.rs          → analysis/reasoning.rs
```

**注意**:
- [ ] ast.rs 可能依赖 tree-sitter (已有独立 crate)
- [ ] incremental_index 对接 carpai-codebase
- [ ] reasoning.rs 可能是新模块

---

### Day 17: Git + 错误处理 (3h, 7 模块)

Git (3 个):
```
git.rs            → git/mod.rs
git_workflow.rs   → git/workflow.rs
version_manager.rs→ git/version.rs
```

错误处理 (4 个):
```
error_recovery.rs  → error/recovery.rs
error_types.rs     → error/types.rs
network_retry.rs   → error/network_retry.rs
allowlist.rs       → error/allowlist.rs
```

---

## 🗑️ 死代码清理 (Week 5-6, 2 天)

### Day 18: 执行清理 (8h)

按 V3 FINAL §4.2 清单执行:

#### A. 归档到 experimental (3 个)
```bash
mkdir -p crates/jcode-experimental/src
mv src/crdt.rs crates/jcode-experimental/src/
mv src/dictation.rs crates/jcode-experimental/src/
# dap, debugger → crates/jcode-debug/
```

#### B. 直接删除 (12 个)
```bash
rm src/env.rs src/import.rs src/login_qr.rs src/process_memory.rs \
   src/process_title.rs src/restart_snapshot.rs src/runtime_memory_log.rs \
   src/scheduler.rs src/external.rs src/subscription_catalog.rs \
   src/todo.rs src/update.rs src/usage.rs src/video_export.rs
```

#### C. 合并到目标模块 (5 个)
```rust
// goal → task_planner (添加 goal planning 方法)
// prompt → memory/prompt.rs (内联单函数)
// safety → security/scanner.rs (合并方法)
// plan → ultraplan (重命名冲突解决)
// workspace_manager → session/workspace.rs (新文件)
// compaction → memory/compaction.rs (新文件)
```

#### D. 移动到 enterprise (1 个)
```bash
mv src/rule_reviewer.rs src/enterprise/review.rs
```

#### 清理后验证
```bash
cargo check -p carpai      # 确保无孤儿引用
grep -r "mod crdt\|mod env\|mod todo" src/  # 确认 lib.rs 已更新
```

---

## 📝 接口契约冻结 (Week 3, 1 天)

### Day 19: 发布接口契约 (8h)

**输出文件**: `docs/INTERFACE_CONTRACT_V1.md`

**内容**:
1. **公共 API 参考**
   - `execute_agent_turn()` 完整签名 + 文档
   - `build_local_agent_context()` 参数说明
   - `AgentTurnOutput` / `ToolCallInfo` 字段含义
   - `CoreConfig` 所有配置项及默认值

2. **类型定义**
   - 所有 public struct/enum 的完整定义
   - 序列化格式示例 (JSON)
   - 错误码枚举及触发条件

3. **使用示例**
   - 最小化 Hello World 示例
   - 带 custom config 的示例
   - 错误处理示例

4. **变更日志模板**
   ```markdown
   ## Changelog
   
   ### v1.0 (Week 3 Frozen)
   - Initial release
   - 6 Local implementations
   - CoreConfig with 3-layer loading
   
   ### v1.1 (Proposed)
   - TBD
   ```

**发布动作**:
- [ ] Push 到 docs/ 目录
- [ ] Notify ma-guoyang/Paw-brave (邮件/Slack/会议)
- [ ] 在 Interface Sync 会议上演示

---

## ⚡ 性能模块迁移 (Week 6-7, 2 天)

### Day 20: 迁移 11 个性能模块 (8h)

```
perf.rs                 → performance/perf.rs
cache_tracker.rs        → performance/cache_tracker.rs
cache_optimizer.rs      → performance/cache_optimizer.rs
cache_integration.rs    → performance/cache_integration.rs
cache_break_detector.rs → performance/cache_break_detector.rs
concurrency_optimizer.rs→ performance/concurrency.rs
compression.rs          → performance/compression.rs
circuit_breaker.rs      → performance/circuit_breaker.rs
backpressure.rs          → performance/backpressure.rs
token_budget.rs          → performance/token_budget.rs
denial_tracking.rs      → performance/denial_tracking.rs
```

**策略**:
- [ ] Day 20 上半场: cache_* 系列 (5 个) — 4h
- [ ] Day 20 下半场: 其余 6 个 — 4h

**注意**:
- [ ] backpressure 可能依赖 tokio sync primitives
- [ ] circuit_breaker 可能是通用模式，考虑提取到 utils

---

## 🧪 Mock 支持 (Week 6-8, 2 天)

### Day 21: 创建 Mock 实现 (8h)

**目的**: 让 ma-guoyang/Paw-brave 在 solo-Turbo 完成前就能开始开发

**文件**: `crates/carpai-core/src/mock/`

```rust
// mock/session_store.rs
pub struct MockSessionStore {
    sessions: Arc<RwLock<HashMap<SessionId, Vec<StoredMessage>>>>,
}

impl SessionStore for MockSessionStore {
    async fn create_session(&self, meta: SessionMeta) -> Result<SessionId> {
        let id = SessionId::new_v4();
        self.sessions.write().await.insert(id, vec![]);
        Ok(id)
    }
    // ... 其他方法返回预定义数据
}

// 同样模式:
// mock/tool_executor.rs  → MockToolExecutor (记录调用，返回固定结果)
// mock/inference.rs      → MockInferenceBackend (返回预设回复)
// mock/filesystem.rs     → MockFileSystem (内存 FS)
// mock/event_bus.rs       → MockEventBus (收集事件，不断言)
// mock/memory.rs          → MockMemoryBackend (内存存储)
```

**组装器**:
```rust
pub fn build_mock_agent_context() -> AgentContext {
    AgentContextBuilder::new()
        .with_config(AppConfig::default())
        .with_sessions(Arc::new(MockSessionStore::new()))
        .with_tools(Arc::new(MockToolExecutor::new()))
        .with_inference(Arc::new(MockInferenceEngine::new()))
        .with_filesystem(Arc::new(MockFileSystem::new()))
        .with_events(Arc::new(MockEventBus::new()))
        .with_memory(Arc::new(MockMemoryBackend::new()))
        .build()
        .unwrap()
}
```

**验收标准**:
- [ ] ma-guoyang 可以用 `MockInferenceEngine` 开发 gRPC handler
- [ ] Paw-brave 可以用 `MockSessionStore` 开发 TUI
- [ ] 所有 Mock 可通过 feature gate 启用: `mock`

---

## 🎯 SDK 增强 (Week 9-10, 4 天)

### Day 22-25: carpai-sdk 增强 (32h)

详见 V3 FINAL §4.2 (SDK 增强)，关键任务:

#### Day 22: OpenAI 兼容类型 (8h)
- [ ] ChatCompletionRequest/Response
- [ ] StreamingChunk (SSE)
- [ ] 复用 inference_backend 已有类型

#### Day 23: Session CRUD API (8h)
- [ ] 5 个 Request/Response 类型
- [ ] 分页支持
- [ ] 过滤器 (by date, by model)

#### Day 24: Client Helpers (8h)
- [ ] CarpaiClient struct
- [ ] HTTP client (reqwest)
- [ ] gRPC client (tonic)
- [ ] 自动重试 + 超时

#### Day 25: OpenAPI spec + 文档 (8h)
- [ ] openapi.yaml 生成
- [ ] examples/ 目录
- [ ] 多语言绑定说明

---

## 🔗 最终联调 (Week 9-10, 3 天)

### Day 26-28: 跨组集成 (24h)

详见 V3 FINAL 第七节，solo-Turbo 主导:

#### Day 26: 合并分支 + 初检 (8h)
- [ ] Merge ma-guoyang's server-build
- [ ] Merge Paw-brave's cli-build
- [ ] Resolve conflicts (<15 expected)
- [ ] `cargo check --workspace` first run

#### Day 27: Bug 分类 + 修复 (8h)
- [ ] 修复 solo-Turbo 自有错误
- [ ] 分配/Review ma-guoyang 的 PR
- [ ] 分配/Review Paw-brave 的 PR
- [ ] 修复跨组交互问题

#### Day 28: E2E 测试 (8h)
- [ ] Test 1: CLI local mode
- [ ] Test 2: Server standalone
- [ ] Test 3: CLI remote mode
- [ ] Test 4: SDK basic flow

---

## 📊 收尾 (Week 11-12, 2 天)

### Day 29-30: 性能基准 + 文档 (16h)

#### Day 29: 性能基准 (8h)
- [ ] 编译时间测量 (`cargo build --release --timings=v2`)
- [ ] 二进制大小检查
- [ ] 内存占用基线
- [ ] Agent turn 延迟 benchmark
- [ ] 并发压力测试
- [ ] 输出: `docs/PERFORMANCE_BASELINE.md`

#### Day 30: 部署文档 + Release (8h)
- [ ] Dockerfile
- [ ] docker-compose.yml
- [ ] systemd unit file
- [ ] production.toml 示例
- [ ] 安全审计 checklist
- [ ] Architecture.md 更新
- [ ] README.md 更新
- [ ] git tag v1.0.0
- [ ] Release notes

---

## 📈 进度追踪

### 每日 Checkpoint

**Week 1 结束时必须完成**:
- [ ] carpai-core crate 存在且可编译
- [ ] 6 个 Local impl 迁移完成
- [ ] CoreConfig 定义完成
- [ ] `build_local_agent_context()` 可用

**Week 4 结束时必须完成**:
- [ ] Agent + Memory + Session + Tools + Completion 全部迁移
- [ ] `cargo check -p carpai-core` 0 errors
- [ ] 接口契约已发布

**Week 8 结束时必须完成**:
- [ ] Refactoring + Analysis + Git + Error 全部迁移
- [ ] 死代码清理完成
- [ ] 性能模块迁移完成
- [ ] Mock 实现可用

**Week 10 结束时必须完成**:
- [ ] SDK 增强完成
- [ ] workspace 全编译通过
- [ ] 4 条 E2E 链路通过

**Week 12 结束时必须完成**:
- [ ] 性能基准报告
- [ ] 部署文档
- [ ] v1.0.0 release

---

## ⚠️ 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 | 应急方案 |
|------|------|------|---------|---------|
| agent_runtime 拆分困难 | 高 | 高 | 先画依赖图；保留原结构，只移动文件 | 暂时作为单个大模块迁入 |
| 循环依赖 (core↔internal) | 中 | 高 | 每次添加 import 前检查方向 | 使用 trait object 打破循环 |
| 编译时间增长 >2x | 中 | 中 | sccache + mold linker | 只 check 不 build |
| ma-guoyang/Paw-brave 进度延迟 | 中 | 中 | Week 6 提供 Mock 解耦 | 他们先用 Mock 开发 |
| 死代码删除导致隐藏依赖 | 低 | 高 | 先 grep 引用再删除 | 保留但标记 deprecated |

---

## 📞 协作节点

### solo-Turbo 需要主动沟通的时间点

| 时间 | 事件 | 对象 | 内容 |
|------|------|------|------|
| **Wk1 Day 3** | 接口契约草案发布 | ma-guoyang + Paw-brave | 发送 draft，收集反馈 |
| **Wk3 Day 1** | Interface Sync 会议 | 全员 | 冻结接口契约 |
| **Wk5 Day 2** | Mock 实现就绪通知 | ma-guoyang + Paw-brave | 提供 Mock 使用指南 |
| **Wk6 Day 3** | 进度同步 | 全员 | 确认各组进度匹配 |
| **Wk8 Day 5** | Integration Prep | 全员 | 确认 merge 策略 |
| **Wk9 Day 1** | 开始合并 | ma-guoyang + Paw-brave | 接收 PR |
| **Wk10 Day 3** | E2E 测试结果 | 全员 | 报告 4 条链路状态 |
| **Wk12 Day 5** | Final Review | 全员 + Stakeholder | 验收演示 |

---

> **文档维护者**: solo-Turbo
>
> **最后更新**: 2026-05-24
>
> **下次更新**: Week 1 Day 1 开始实施时
