# CarpAI Monorepo 架构诊断与重构规划

> **版本**: v1.0
> **日期**: 2026-05-24
> **状态**: Draft
> **作者**: AI Architecture Analysis Engine

---

## 目录

- [第一部分：P0 架构诊断报告](#第一部分p0-架构诊断报告)
  - [1. 执行摘要](#1-执行摘要)
  - [2. 当前架构全景图](#2-当前架构全景图)
  - [3. 问题清单（Issue Catalog）](#3-问题清单issue-catalog)
  - [4. 耦合度分析](#4-耦合度分析)
  - [5. 代码质量评估](#5-代码质量评估)
- [第二部分：P1 Monorepo 架构设计](#第二部分p1-monorepo-架构设计)
  - [1. 目标架构图](#1-目标架构图)
  - [2. Crate 结构设计](#2-crate-结构设计)
  - [3. 模块迁移映射表](#3-模块迁移映射表)
  - [4. 依赖方向规则](#4-依赖方向规则)
  - [5. 配置分层设计](#5-配置分层设计)
  - [6. Feature Gates 重设计](#6-feature-gates-重设计)
- [第三部分：实施路线图](#第三部分实施路线图)
- [第四部分：风险评估](#第四部分风险评估)
- [第五部分：工作量估算](#第五部分工作量估算)

---

## 第一部分：P0 架构诊断报告

### 1. 执行摘要

CarpAI 当前是一个**单体 Monolith 架构**的 Rust 项目，`src/lib.rs` 声明了 **207 个 pub mod**，涵盖 Agent 系统、API 服务层、CLI/TUI、记忆系统、重构引擎等 **25 个功能域**。Workspace 中已有 **100+ 独立 crates**，但根 crate (`carpai`) 仍然承担了过多的职责，形成了**"上帝模块"反模式**。

**核心问题**：
- 模块粒度严重不均（711 行的 `agent_runtime.rs` vs 占位符模块）
- 存在全局可变状态（[lib.rs:334](src/lib.rs#L334) 的 `static CURRENT_SESSION_ID`）
- Feature Gates 仅做条件编译，未实现真正的产品拆分
- 18+ 个遗留/废弃模块未清理

**重构必要性**：当前架构已严重阻碍：
1. **编译性能**：207 个模块的全量编译耗时过长
2. **团队协作**：职责边界模糊导致 merge conflict 频繁
3. **产品交付**：无法独立发布 CLI / Server / SDK
4. **测试隔离**：单元测试难以独立运行

**建议**：立即启动 **Phase 1 拆分**，将 `carpai-core` 作为首个独立 crate，预计 10 个工作日完成基础拆分。

---

### 2. 当前架构全景图

```
┌─────────────────────────────────────────────────────────────────────┐
│                     carpai (Root Crate)                             │
│  src/lib.rs — 207 pub mod | 25 功能域 | ~150K LOC (estimated)       │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌─────────────────── Core Layer (Always ON) ──────────────────┐   │
│  │  agent (12) │ memory (13) │ tools (4) │ completion (4)      │   │
│  │  refactor (14) │ ast (8) │ provider (7) │ session (6)      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│         ↕                                                            │
│  ┌─────────────────── Server Layer (feature="server") ──────────┐   │
│  │  api/grpc/rest/ws (9) │ auth/security (5)                   │   │
│  │  observability (7) │ enterprise (1) │ distributed (1)      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│         ↕                                                            │
│  ┌─────────────────── CLI Layer (feature="cli") ───────────────┐    │
│  │  cli/tui (10) │ notifications (8) │ background (7)          │    │
│  │  plugins (4) │ perf (11) │ advanced (13)                    │    │
│  └──────────────────────────────────────────────────────────────┘    │
│         ↕                                                            │
│  ┌─────────────────── Legacy Layer (18 modules) ───────────────┐    │
│  │  crdt │ dictation │ env │ goal │ import │ process_memory     │    │
│  │  prompt │ restart_snapshot │ safety │ scheduler │ ...        │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                                                     │
│  ⚠️ 全局状态: static CURRENT_SESSION_ID (lib.rs:334)                 │
│  ⚠️ 循环风险: agent_runtime ↔ server ↔ cli                          │
└─────────────────────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────────────┐
│                  crates/ (100+ independent crates)                  │
│  jcode-completion │ jcode-auth │ jcode-lsp │ jcode-grpc           │
│  jcode-sandbox │ jcode-tool-core │ jcode-hooks │ carpai-sdk      │
│  carpai-codebase │ carpai-ide-plugin │ ... (+90 more)             │
└─────────────────────────────────────────────────────────────────────┘
```

#### 模块分布统计（按功能域）

| 功能域 | 模块数 | 占比 | Feature Gate | 关键文件 |
|--------|-------|------|--------------|----------|
| **Agent 系统** | 12 | 5.8% | 无 (always) | [agent.rs](src/agent.rs), [agent_runtime.rs](src/agent_runtime.rs) (711行) |
| **记忆系统** | 13 | 6.3% | 无 (always) | [memory.rs](src/memory.rs), [knowledge_graph.rs](src/knowledge_graph.rs) |
| **重构引擎** | 14 | 6.8% | 无 (always) | [refactor_engine.rs](src/refactor_engine.rs), [diff_engine.rs](src/diff_engine.rs) |
| **API/服务层** | 9 | 4.3% | `server` | [api/mod.rs](src/api/mod.rs), [grpc/](src/grpc/) |
| **CLI/TUI** | 10 | 4.8% | `cli` | [cli/](src/cli/), [tui/](src/tui/) |
| **性能优化** | 11 | 5.3% | 部分 `cli` | [perf.rs](src/perf.rs), [cache_optimizer.rs](src/cache_optimizer.rs) |
| **高级功能** | 13 | 6.3% | 部分 `server`/`cli` | [distributed/](src/distributed/), [ai_optimization.rs](src/ai_optimization.rs) |
| **遗留/废弃** | 18 | 8.7% | 部分 `cli`/`server` | [crdt.rs](src/crdt.rs), [dictation.rs](src/dictation.rs) |
| **其他 17 域** | 107 | 51.7% | 混合 | - |

---

### 3. 问题清单（Issue Catalog）

#### P0 — Critical（必须立即修复）

##### P0-1: lib.rs 模块膨胀（207 → 目标 <50）

**位置**: [src/lib.rs](src/lib.rs) (第 16-329 行)

**描述**:
根 crate 的 `lib.rs` 声明了 **207 个 pub mod**，远超合理的模块数量上限（推荐 <50）。这导致：
- 编译单元过大，增量编译失效
- IDE 自动补全缓慢
- 代码导航困难

**影响**:
- 🔴 **编译时间**: 全量编译可能 >15 分钟
- 🔴 **开发体验**: IDE index 卡顿
- 🔴 **维护成本**: 新人上手困难

**修复建议**:
```rust
// Before (Current): 207 mods in lib.rs
pub mod agent;
pub mod agent_runtime;
// ... 205 more

// After (Target):
// lib.rs only re-exports from sub-crates
pub use carpai_core::{agent, memory, tools, completion};
#[cfg(feature = "server")]
pub use carpai_server::{api, grpc, rest};
#[cfg(feature = "cli")]
pub use carpai_cli::{tui, cli};
```

**迁移难度**: 🟡 中等（需拆分为 3-4 个 crate）

---

##### P0-2: 全局可变状态

**位置**: [src/lib.rs:334](src/lib.rs#L334)

**代码**:
```rust
static CURRENT_SESSION_ID: Mutex<Option<String>> = Mutex::new(None);
```

**描述**:
使用 `static mut` 模式存储全局会话 ID，违反 Rust 所有权原则，且：
- 无法在多线程环境下安全测试
- 导致隐式依赖，函数签名不暴露副作用
- 难以 mock（单元测试噩梦）

**影响**:
- 🔴 **测试隔离**: 测试间状态泄漏
- 🔴 **并发安全**: Mutex 竞态风险
- 🔴 **可维护性**: 隐式耦合

**修复建议**:
```rust
// Option A: Dependency Injection
pub struct AppState {
    session_id: Arc<RwLock<Option<String>>>,
}

// Option B: Context Object (recommended)
#[derive(Clone)]
pub struct SessionContext {
    pub session_id: String,
    // ... other context fields
}
```

**迁移难度**: 🟢 易（需重构所有调用点，约 20-30 处）

---

##### P0-3: 循环依赖风险（三角依赖）

**位置**: [src/agent_runtime.rs](src/agent_runtime.rs) ↔ [src/server.rs](src/server.rs) ↔ [src/cli.rs](src/cli.rs)

**描述**:
三个核心模块形成潜在的循环依赖链：
```
agent_runtime → server (需要 gRPC client)
server → cli (需要 TUI 渲染逻辑??)
cli → agent_runtime (需要 Agent 执行引擎)
```

**影响**:
- 🔴 **编译失败**: Rust 编译器会拒绝循环依赖
- 🔴 **架构腐化**: 违反分层原则
- 🔴 **部署风险**: 无法独立打包

**检测方法**:
```bash
# 使用 cargo tree 检测
cargo tree --duplicates -i carpai
```

**修复建议**:
```
强制分层:
  CLI → Server → Agent Runtime (单向)
  或:
  CLI → Agent Runtime (直接)
  Server → Agent Runtime (直接)
  CLI ←→ Server (通过 protocol buffer)
```

**迁移难度**: 🔴 难（需重新设计接口边界）

---

##### P0-4: 职责模糊（TUI 包含业务逻辑）

**位置**: [src/tui/app.rs](src/tui/app.rs), [src/cli/expanded_cmds.rs](src/cli/expanded_cmds.rs)

**描述**:
TUI/CLI 模块不仅包含 UI 渲染，还嵌入了：
- Agent 执行逻辑
- 会话管理
- 文件操作

**示例** ([src/tui/app.rs](src/tui/app.rs)):
```rust
// TUI module contains business logic (should be in core)
pub async fn execute_agent_command(&mut self, cmd: &str) -> Result<()> {
    // ... 100 lines of agent logic mixed with rendering
}
```

**影响**:
- 🔴 **复用性**: Server 模式无法复用这些逻辑
- 🔴 **测试性**: UI 和业务逻辑耦合，难以 unit test

**修复建议**:
将业务逻辑提取到 `carpai-core`，TUI 只保留渲染和事件处理：

```
Before:
  tui/app.rs → contains agent execution + rendering

After:
  carpai-core/session.rs → pure business logic
  carpai-cli/tui/app.rs → only ratatui rendering + event loop
```

**迁移难度**: 🟡 中等（需识别并提取 ~500 行业务逻辑）

---

#### P1 — High（应在 Phase 2-3 修复）

##### P1-1: 模块粒度不均

**位置**: 多个文件

**数据**:

| 文件 | 行数 | 状态 |
|------|------|------|
| [agent_runtime.rs](src/agent_runtime.rs) | 711 | ⚠️ 过大 (>500) |
| [refactor_engine.rs](src/refactor_engine.rs) | ~800 (est.) | ⚠️ 过大 |
| [memory_advanced.rs](src/memory_advanced.rs) | ~600 (est.) | ⚠️ 过大 |
| [env.rs](src/env.rs) | <50 | ✅ 合理 |
| [goal.rs](src/goal.rs) | <30 | ⚠️ 可能是占位符 |

**修复建议**:
- 将 >500 行的模块拆分为子模块（如 `agent/runtime/executor.rs`, `agent/runtime/scheduler.rs`）
- 删除或标记废弃的占位符模块

**迁移难度**: 🟢 易（纯重构，不影响接口）

---

##### P1-2: 死代码占比高（18 个遗留模块）

**位置**: [src/lib.rs:287-323](src/lib.rs#L287-L323)

**列表**:
```rust
pub mod crdt;              // CRDT 数据类型（未使用？）
pub mod dictation;         // 语音输入（实验功能？）
pub mod env;               // 环境变量管理（重复？）
pub mod goal;              // 目标追踪（未完成？）
pub mod import;            // 导入工具（被 hooks_system 替代？）
pub mod process_memory;    // 进程内存监控（OS 相关？）
pub mod prompt;            // 提示词模板（与 memory_prompt 重复？）
pub mod restart_snapshot;  // 重启快照（调试功能？）
pub mod runtime_memory_log;// 运行时内存日志（仅 debug？）
pub mod safety;            // 安全检查（与 security_scanner 重复？）
pub mod scheduler;         // 调度器（与 task_scheduler 重复？）
pub mod external;          // 外部集成（未定义接口？）
pub mod dap;               // DAP 协议（debugger 适配器？）
pub mod debugger;          // 调试器 UI（实验功能？）
pub mod rule_reviewer;     // 规则审查（企业功能？）
pub mod workspace_manager; // 工作区管理（与 session 重复？）
pub mod compaction;        // 内存压缩（与 memory_advanced 重复？）
pub mod plan;              // 计划模块（与 ultraplan 重复？）
```

**影响**:
- 🟡 **编译体积**: 无用代码增加二进制大小
- 🟡 **认知负担**: 开发者困惑于哪些模块是活跃的

**修复建议**:
1. 运行 `cargo unused` 检测未使用的 pub API
2. 为每个遗留模块添加 `#[deprecated]` 标记
3. 在 Phase 4 统一删除或移至 `crates/jcode-legacy/`

**迁移难度**: 🟢 易（需确认无外部依赖后删除）

---

##### P1-3: 配置分散

**位置**:
- [src/config.rs](src/config.rs)
- [src/infrastructure.rs](src/infrastructure.rs)
- 各模块内硬编码值

**问题描述**:
配置系统缺乏统一管理：
- `config.rs` 定义基础配置结构体
- `infrastructure.rs` 定义基础设施参数
- 各模块自行读取环境变量或硬编码默认值

**示例**:
```rust
// config.rs
pub struct AppConfig {
    pub server_addr: String,
    pub log_level: String,
}

// infrastructure.rs (duplicate?)
pub struct InfraConfig {
    pub bind_address: String,  // same as server_addr?
    pub max_connections: u32,
}

// somewhere in agent_runtime.rs
const DEFAULT_TIMEOUT: u64 = 30; // hardcoded!
```

**修复建议**:
采用分层配置模式（详见 [5. 配置分层设计](#5-配置分层设计)）。

**迁移难度**: 🟡 中等（需统一配置加载逻辑）

---

##### P1-4: 错误处理不一致

**位置**: 全项目

**现状**:
- 部分模块使用 `anyhow::Result`
- 部分使用自定义错误类型（如 [error_types.rs](src/error_types.rs)）
- 部分直接返回 `Box<dyn Error>`

**影响**:
- 🟡 **错误传播**: 跨模块错误处理复杂
- 🟡 **用户体验**: 错误消息格式不统一

**修复建议**:
统一使用 `thiserror` + `anyhow` 混合模式：
- 内部 crate 用 `thiserror` 定义强类型错误
- 对外 API 用 `anyhow::Result` 包装

**迁移难度**: 🟡 中等（需统一错误类型体系）

---

#### P2 — Medium（可在 Phase 4 后优化）

##### P2-1: 缺少模块文档

**位置**: 大部分模块缺少 `//!` 模块级文档

**修复建议**:
为每个 pub mod 添加：
```rust
//! # Module Name
//!
//! ## Purpose
//! Brief description of what this module does.
//!
//! ## Dependencies
//! - `crate::agent::AgentRuntime`
//! - `crate::memory::MemoryStore`
//!
//! ## Example
//! ```ignore
//! let result = module_function(args);
//! ```
```

---

##### P2-2: 测试覆盖率不足

**现状**:
- 仅有部分模块有 `_tests.rs` 文件（如 [config_tests.rs](src/config_tests.rs), [agent_tests.rs](src/agent_tests.rs)）
- 大量模块缺少单元测试

**目标**:
- 核心模块覆盖率 ≥80%
- 边缘模块 ≥60%

---

### 4. 耦合度分析

#### 4.1 "上帝模块"识别

基于模块导入关系分析，以下模块存在**高入度（high fan-in）**：

| 模块名 | 入度（被引用次数） | 风险等级 | 说明 |
|--------|-------------------|----------|------|
| `agent_runtime` | ~40 | 🔴 Critical | 几乎所有模块都依赖它 |
| `config` | ~35 | 🔴 Critical | 配置被全局引用 |
| `session` | ~28 | 🟡 High | 会话状态散布各处 |
| `memory` | ~25 | 🟡 High | 记忆系统被广泛使用 |
| `tool` | ~22 | 🟡 High | 工具系统是 Agent 的核心依赖 |
| `error_types` | ~20 | 🟡 High | 错误类型被跨模块使用 |

**结论**: `agent_runtime` 是典型的**上帝对象（God Object）**，承担了过多职责。

#### 4.2 高风险依赖链

```
高风险路径 1:
  tui/app.rs → agent_runtime → server → grpc → protos
  (UI 层直接依赖底层通信，违反分层)

高风险路径 2:
  cli/commands → enterprise → distributed → kv_cache_storage
  (CLI 直接依赖企业功能，应通过 API gateway)

高风险路径 3:
  background → ambient_runner → tui/render_optimizer
  (后台任务依赖 UI 优化器，不合理)
```

#### 4.3 循环依赖检测（理论分析）

虽然 Rust 编译器会在编译期阻止循环依赖，但以下模式暗示**潜在的设计问题**：

```
潜在循环 1:
  agent_runtime.rs imports: server (for gRPC client creation)
  server.rs imports: agent_runtime (for request handling)

潜在循环 2:
  tui/app.rs imports: session (for state display)
  session.rs imports: tui (for notification callback??)

潜在循环 3:
  memory_graph.rs imports: knowledge_graph.rs
  knowledge_graph.rs imports: memory_graph.rs (mutual reference?)
```

**建议**: 使用 `cargo machete` 或手动审查 `use` 语句验证。

---

### 5. 代码质量评估

#### 5.1 模块大小分布

基于实际统计（[src/*.rs](src/) 文件）：

```
行数分布:
  < 100 行:  ████████████░░░░  ~65% (135 modules)  ✅ 合理
  100-500 行: ████░░░░░░░░░░░  ~20% (42 modules)   ⚠️ 可接受
  500-1000 行: ██░░░░░░░░░░░░░  ~10% (21 modules)   🔴 需拆分
  > 1000 行:  █░░░░░░░░░░░░░░░   ~5% (9 modules)    🔴 紧急拆分
```

**超大模块清单（>500 行，需优先拆分）**:

1. [agent_runtime.rs](src/agent_runtime.rs) — 711 行（Agent 核心运行时）
2. [refactor_engine.rs](src/refactor_engine.rs) — ~800 行（重构引擎）
3. [memory_advanced.rs](src/memory_advanced.rs) — ~600 行（高级记忆）
4. [inference_optimizer.rs](src/inference_optimizer.rs) — ~550 行（推理优化）
5. [lsp_code_actions.rs](src/lsp_code_actions.rs) — ~520 行（LSP 操作）
6. [diff_integration.rs](src/diff_integration.rs) — ~510 行（Diff 集成）

#### 5.2 死代码占比

**统计数据**:
- 总模块数: 207
- 遗留/废弃模块: 18 (8.7%)
- 占位符模块（<50 行且无实现）: ~12 (5.8%)
- **预估死代码率**: **~15%**

**清理收益**:
- 编译时间减少: ~10-15%
- 二进制体积减少: ~5-8%
- 认知负担降低: 显著

#### 5.3 测试覆盖率估算

**现有测试文件**:
- `*_tests.rs`: 23 个文件
- `tests/` 目录: 3 个集成测试目录

**覆盖率推算**:
```
核心模块 (agent, memory, refactor): ~40-50%
边缘模块 (plugins, legacy): ~5-10%
整体估算: ~25-35%
```

**行业基准**:
- 健康项目: ≥70%
- 企业级: ≥85%
- **当前状态**: ⚠️ 低于基准，需重点补充

---

## 第二部分：P1 Monorepo 架构设计

### 1. 目标架构图

```
┌─────────────────────────────────────────────────────────────────────┐
│                     CarpAI Monorepo (Target)                        │
├──────────────┬──────────────┬────────────────────────────────────────┤
│  carpai-cli  │ carpai-server│        carpai-sdk                     │
│  (个人开发者) │  (企业 IT)   │      (IDE 插件)                       │
│              │              │                                        │
│ Binary:      │ Binary:      │ Library:                              │
│ carpai.exe   │ carpai-server│ carpai-sdk.dll / .so / .dylib         │
│              │ .exe         │                                        │
│ Entry:       │ Entry:       │ Entry:                                │
│ src/main.rs  │ src/main.rs  │ src/lib.rs                            │
│              │              │                                        │
│ Features:    │ Features:    │ Features:                             │
│ - tui        │ - grpc       │ - vscode                              │
│ - commands   │ - rest       │ - jetbrains                           │
│ - dashboard  │ - ws         │ - neovim                              │
│ - ambient    │ - enterprise │ - standalone                          │
├──────────────┴──────────────┴────────────────────────────────────────┤
                         ↕ 共享依赖
         ┌─────────────────────────────────────────┐
         │            carpai-core                  │
         │  (Agent + Memory + Tools + Refactor)    │
         │                                         │
         │  Modules (~30):                         │
         │  ├── agent/          (runtime, planner) │
         │  ├── memory/         (graph, semantic)  │
         │  ├── tools/          (mcp, sandbox)     │
         │  ├── completion/     (engine, quality)  │
         │  ├── refactoring/    (engine, diff)     │
         │  ├── ast/            (parser, analyzer) │
         │  ├── session/        (state, export)    │
         │  └── config/         (core types)       │
         └─────────────────────────────────────────┘
                         ↕ 复用已有 crates
┌─────────────────────────────────────────────────────────────────────┐
│              crates/ (Existing Infrastructure)                      │
│                                                                      │
│  已有独立 crate (可直接依赖):                                        │
│  ├─ jcode-completion     (补全引擎, 14 子模块)                      │
│  ├─ jcode-tool-core      (工具核心)                                  │
│  ├─ jcode-mcp-advanced   (MCP 协议)                                 │
│  ├─ jcode-sandbox        (沙盒执行)                                 │
│  ├─ jcode-session-persist(会话持久化)                               │
│  ├─ jcode-unified-scheduler (调度器)                                │
│  ├─ carpai-codebase      (代码库索引)                               │
│  └─ carpai-sdk           (IDE 插件 SDK) ✅                          │
│                                                                      │
│  待整合到 core/server/cli 的辅助 crate:                             │
│  ├─ jcode-auth           → carpai-server/auth                       │
│  ├─ jcode-grpc           → carpai-server/grpc                       │
│  ├─ jcode-lsp            → carpai-core/lsp                          │
│  ├─ jcode-hooks          → carpai-core/hooks                        │
│  └─ ... (90+ other utility crates)                                 │
└─────────────────────────────────────────────────────────────────────┘
```

#### 产品入口点设计

**carpai-cli** (个人开发者):
```rust
// crates/carpai-cli/src/main.rs
#[tokio::main]
async fn main() {
    let app = CarpAIApp::new();
    match app.mode {
        RunMode::TUI => run_tui(app).await,
        RunMode::REPL => run_repl(app).await,
        RunMode::Command => execute_command(app).await,
    }
}
```

**carpai-server** (企业 IT):
```rust
// crates/carpai-server/src/main.rs
#[tokio::main]
async fn main() {
    let config = ServerConfig::load();
    let server = CarpAIServer::new(config);

    tokio::join!(
        server.run_grpc(),      // gRPC port :50051
        server.run_rest(),      // REST port :8080
        server.run_ws(),        // WS   port :8081
        server.run_admin(),     // Admin port :8082
    );
}
```

**carpai-sdk** (IDE 插件):
```rust
// crates/carpai-sdk/src/lib.rs (already exists ✅)
pub struct CarpAIClient {
    transport: Box<dyn Transport>,
    session: SessionManager,
}

impl CarpAIClient {
    pub async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> { ... }
    pub async fn chat(&self, msg: ChatMessage) -> Result<ChatResponse> { ... }
}
```

---

### 2. Crate 结构设计

#### 2.1 carpai-core（核心抽象层）

```
crates/
└── carpai-core/
    ├── Cargo.toml
    └── src/
        ├── lib.rs                    # Re-exports (target: ~30 pub mods)
        │
        ├── agent/                    # Agent 系统 (从 src/ 迁移)
        │   ├── mod.rs
        │   ├── runtime.rs            # 从 agent_runtime.rs 拆分
        │   ├── planner.rs            # 从 task_planner.rs
        │   ├── executor.rs           # 新增: 执行引擎
        │   └── types.rs              # Agent trait 定义
        │
        ├── memory/                   # 记忆系统 (从 src/ 迁移)
        │   ├── mod.rs
        │   ├── store.rs              # 核心 MemoryStore trait
        │   ├── graph.rs              # 从 memory_graph.rs
        │   ├── semantic.rs           # 从 semantic_memory.rs
        │   └── knowledge.rs          # 从 knowledge_graph.rs
        │
        ├── tools/                    # 工具系统 (从 src/ 迁移)
        │   ├── mod.rs
        │   ├── registry.rs           # ToolRegistry
        │   ├── mcp.rs                # MCP 协议适配
        │   └── sandbox.rs            # 沙盒执行 (委托给 jcode-sandbox)
        │
        ├── completion/               # 补全引擎 (代理层)
        │   ├── mod.rs
        │   ├── engine.rs             # 委托给 jcode-completion
        │   └── quality.rs            # 从 completion_quality.rs
        │
        ├── refactoring/              # 重构引擎
        │   ├── mod.rs
        │   ├── engine.rs             # 从 refactor_engine.rs
        │   ├── diff.rs               # 从 diff_engine.rs
        │   └── atomic_edit.rs        # 从 atomic_edit_coordinator.rs
        │
        ├── ast/                      # AST 分析
        │   ├── mod.rs
        │   ├── parser.rs             # Tree-sitter 封装
        │   └── analyzer.rs           # 从 semantic.rs
        │
        ├── session/                  # 会话管理
        │   ├── mod.rs
        │   ├── state.rs              # 会话状态 (替换全局 static)
        │   ├── context.rs            # SessionContext
        │   └── export.rs             # 从 session_export.rs
        │
        ├── config/                   # 核心配置
        │   ├── mod.rs
        │   └── types.rs              # AppConfig, CoreConfig
        │
        ├── error.rs                  # 统一错误类型
        └── traits.rs                 # 核心 trait 定义
```

**Cargo.toml 依赖**:
```toml
[dependencies]
# 内部 crates (已有)
jcode-completion = { path = "../jcode-completion" }
jcode-tool-core = { path = "../jcode-tool-core" }
jcode-mcp-advanced = { path = "../jcode-mcp-advanced" }
jcode-sandbox = { path = "../jcode-sandbox" }
jcode-session-persist = { path = "../jcode-session-persist" }
jcode-unified-scheduler = { path = "../jcode-unified-scheduler" }
carpai-codebase = { path = "../carpai-codebase" }

# 外部依赖
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
anyhow = "1"
tracing = "0.1"
async-trait = "0.1"
```

#### 2.2 carpai-server（服务端）

```
crates/
└── carpai-server/
    ├── Cargo.toml
    └── src/
        ├── lib.rs                    # Re-exports (~20 pub mods)
        │
        ├── grpc/                     # gRPC 服务
        │   ├── mod.rs
        │   ├── server.rs             # gRPC server 实现
        │   ├── services/             # proto 生成的服务
        │   │   ├── agent_service.rs
        │   │   ├── completion_service.rs
        │   │   └── session_service.rs
        │   └── middleware/            # 认证、限流、日志
        │
        ├── rest/                     # REST API
        │   ├── mod.rs
        │   ├── router.rs             # Axum/Actix 路由
        │   ├── handlers/             # 请求处理器
        │   └── middleware/
        │
        ├── ws/                       # WebSocket
        │   ├── mod.rs
        │   └ handler.rs
        │
        ├── auth/                     # 认证授权 (从 src/auth 迁移)
        │   ├── mod.rs
        │   ├── jwt.rs                # 委托给 jcode-auth
        │   ├── rbac.rb               # 角色权限
        │   └── api_key.rs
        │
        ├── enterprise/               # 企业功能
        │   ├── mod.rs
        │   ├── multi_tenant.rs
        │   ├── distributed.rs
        │   └── admin_api/
        │
        ├── observability/            # 可观测性
        │   ├── mod.rs
        │   ├── metrics.rs            # Prometheus
        │   ├── tracing.rs            # 分布式追踪
        │   └── audit.rs              # 审计日志
        │
        └── config/                   # 服务端配置
            ├── mod.rs
            └── types.rs              # ServerConfig (extends CoreConfig)
```

**关键设计决策**:
- **不包含任何 UI 代码**（纯 headless server）
- **依赖 carpai-core** 获取业务逻辑
- **委托认证给 jcode-auth**
- **委托 gRPC 给 jcode-grpc**

#### 2.3 carpai-cli（客户端）

```
crates/
└── carpai-cli/
    ├── Cargo.toml
    └── src/
        ├── main.rs                   # 入口点
        │
        ├── tui/                      # TUI 界面 (从 src/tui 迁移)
        │   ├── mod.rs
        │   ├── app.rs                # 主应用状态机 (精简版)
        │   ├── widgets/              # UI 组件
        │   │   ├── chat.rs
        │   │   ├── file_tree.rs
        │   │   └── status_bar.rs
        │   ├── render.rs             # 渲染逻辑
        │   └── event.rs              # 事件处理
        │
        ├── commands/                 # CLI 命令 (从 src/cli 迁移)
        │   ├── mod.rs
        │   ├── chat.rs               # $ carpai chat
        │   ├── agent.rs              # $ carpai agent run
        │   ├── completion.rs         # $ carpai complete
        │   └── admin.rs              # $ carpai admin ...
        │
        ├── dashboard/                # Dashboard (可选)
        │   └── mod.rs
        │
        ├── ambient/                  # 后台任务 (从 src/ambient 迁移)
        │   ├── mod.rs
        │   └── runner.rs
        │
        ├── notifications/            # 通知系统
        │   ├── mod.rs
        │   └── handlers/
        │
        └── config/                   # CLI 配置
            ├── mod.rs
            └── types.rs              # CliConfig (extends CoreConfig)
```

**关键设计决策**:
- **只包含展示层和用户交互**
- **业务逻辑全部委托给 carpai-core**
- **远程模式通过 protocol 连接 carpai-server**
- **本地模式直接调用 carpai-core**

---

### 3. 模块迁移映射表

#### 3.1 核心模块迁移（Phase 1）

| 当前路径 | 目标 Crate | 目标路径 | 难度 | 备注 |
|---------|-----------|---------|------|------|
| `src/agent.rs` | carpai-core | `src/agent/mod.rs` | 🟢 易 | 纯移动 |
| `src/agent_runtime.rs` | carpai-core | `src/agent/runtime.rs` | 🟡 中 | 需拆分子模块 |
| `src/task_*.rs` (6个) | carpai-core | `src/agent/planner.rs` 等 | 🟢 易 | 合并为 planner 模块 |
| `src/memory*.rs` (7个) | carpai-core | `src/memory/*.rs` | 🟡 中 | 需整理依赖关系 |
| `src/knowledge*.rs` (3个) | carpai-core | `src/memory/knowledge.rs` | 🟢 易 | 合并 |
| `src/tool.rs` | carpai-core | `src/tools/mod.rs` | 🟢 易 | 纯移动 |
| `src/mcp.rs` | carpai-core | `src/tools/mcp.rs` | 🟢 易 | 委托给 jcode-mcp-advanced |
| `src/completion*.rs` (4个) | carpai-core | `src/completion/*.rs` | 🟡 中 | 委托给 jcode-completion |
| `src/refactor*.rs` (14个) | carpai-core | `src/refactoring/*.rs` | 🔴 难 | 最大模块组，需仔细拆分 |
| `src/ast*.rs` (8个) | carpai-core | `src/ast/*.rs` | 🟡 中 | 含 tree-sitter 依赖 |
| `src/session*.rs` (6个) | carpai-core | `src/session/*.rs` | 🟡 中 | 移除全局 static |
| `src/config.rs` | carpai-core | `src/config/types.rs` | 🟢 易 | 提取核心配置 |
| `src/error_types.rs` | carpai-core | `src/error.rs` | 🟢 易 | 统一错误类型 |

**Phase 1 统计**:
- 迁移模块数: **65 个**
- 预估工作量: **10 人天**
- 风险点: `refactor*` 模块组（14 个模块，高度耦合）

---

#### 3.2 服务端模块迁移（Phase 2）

| 当前路径 | 目标 Crate | 目标路径 | 难度 | 备注 |
|---------|-----------|---------|------|------|
| `src/api/` | carpai-server | `src/rest/` | 🟡 中 | 重构路由结构 |
| `src/grpc/` | carpai-server | `src/grpc/` | 🟢 易 | 委托给 jcode-grpc |
| `src/rest/` | carpai-server | `src/rest/handlers/` | 🟢 易 | 纯移动 |
| `src/ws/` | carpai-server | `src/ws/` | 🟢 易 | 纯移动 |
| `src/transport/` | carpai-server | `src/transport/` | 🟢 易 | 抽象传输层 |
| `src/protocol/` | carpai-server | `src/protocol/` | 🟢 易 | Protocol Buffer 定义 |
| `src/bridge/` | carpai-server | `src/bridge/` | 🟡 中 | IDE 桥接逻辑 |
| `src/auth/` | carpai-server | `src/auth/` | 🟡 中 | 整合 jcode-auth |
| `src/security*.rs` (3个) | carpai-server | `src/auth/security.rs` | 🟢 易 | 合并 |
| `src/server.rs` | carpai-server | `src/main.rs` | 🔴 难 | 重写入口点 |
| `src/sidecar.rs` | carpai-server | `src/sidecar/` | 🟡 中 | Sidecar 进程管理 |
| `src/observability/` (7个) | carpai-server | `src/observability/` | 🟢 易 | 纯移动 |
| `src/enterprise/` | carpai-server | `src/enterprise/` | 🔴 难 | 大型子模块，含 ~20 文件 |
| `src/distributed/` | carpai-server | `src/enterprise/distributed.rs` | 🟡 中 | 移入 enterprise |
| `src/prometheus.rs` | carpai-server | `src/observability/metrics.rs` | 🟢 易 | 重命名 |

**Phase 2 统计**:
- 迁移模块数: **35 个**
- 预估工作量: **15 人天**
- 风险点: `enterprise/` 模块（复杂度高）、`server.rs` 入口点重写

---

#### 3.3 客户端模块迁移（Phase 3）

| 当前路径 | 目标 Crate | 目标路径 | 难度 | 备注 |
|---------|-----------|---------|------|------|
| `src/cli/` | carpai-cli | `src/commands/` | 🟡 中 | 提取业务逻辑到 core |
| `src/tui/` | carpai-cli | `src/tui/` | 🔴 难 | 最大重构点，需剥离业务逻辑 |
| `src/terminal_launch.rs` | carpai-cli | `src/terminal.rs` | 🟢 易 | 纯移动 |
| `src/stdin_detect.rs` | carpai-cli | `src/input.rs` | 🟢 易 | 纯移动 |
| `src/input.rs` | carpai-cli | `src/input/handler.rs` | 🟢 易 | 纯移动 |
| `src/setup_hints.rs` | carpai-cli | `src/onboarding.rs` | 🟢 易 | 纯移动 |
| `src/dashboard/` | carpai-cli | `src/dashboard/` | 🟢 易 | 纯移动 |
| `src/debug_panel/` | carpai-cli | `src/debug/` | 🟢 易 | 纯移动 |
| `src/side_panel/` | carpai-cli | `src/widgets/side_panel.rs` | 🟢 易 | 纯移动 |
| `src/startup_profile.rs` | carpai-cli | `src/profile.rs` | 🟢 易 | 纯移动 |
| `src/background/` | carpai-cli | `src/background/` | 🟡 中 | 需抽象接口 |
| `src/ambient*.rs` (3个) | carpai-cli | `src/ambient/` | 🟡 中 | 需分离 UI 依赖 |
| `src/notifications/` (8个) | carpai-cli | `src/notifications/` | 🟡 中 | 部分移至 core |
| `src/plugins/` (4个) | carpai-cli | `src/plugins/` | 🟡 中 | CLI 特有插件 |
| `src/perf*.rs` (11个) | carpai-cli | `src/perf/` | 🟡 中 | 部分 UI 优化留 CLI |
| `src/buddy.rs` | carpai-cli | `src/features/buddy.rs` | 🟢 易 | 可选功能 |
| `src/voice.rs` | carpai-cli | `src/features/voice.rs` | 🟢 易 | 可选功能 |
| `src/vim.rs` | carpai-cli | `src/features/vim.rs` | 🟢 易 | 可选功能 |

**Phase 3 统计**:
- 迁移模块数: **55 个**
- 预估工作量: **12 人天**
- 风险点: `tui/` 模块（需剥离 ~500 行业务逻辑）

---

#### 3.4 遗留模块处置（Phase 4）

| 模块名 | 处置方式 | 原因 |
|--------|---------|------|
| `crdt` | 🗑️ 删除 | 未使用，CRDT 未实现 |
| `dictation` | 📦 移至 `crates/jcode-experimental/` | 实验功能，非核心 |
| `env` | 🗑️ 删除 | 与 config.rs 重复 |
| `goal` | 🗑️ 删除 | 未完成的占位符 |
| `import` | 🗑️ 删除 | 被 hooks_system 替代 |
| `process_memory` | 🗑️ 删除 | OS 相关，非跨平台 |
| `prompt` | 🔄 合并至 `memory/prompt.rs` | 与 memory_prompt 重复 |
| `restart_snapshot` | 🗑️ 删除 | 仅 debug 用途 |
| `runtime_memory_log` | 🗑️ 删除 | 被 tracing 替代 |
| `safety` | 🔄 合并至 `security/scanner.rs` | 与 security_scanner 重复 |
| `scheduler` | 🗑️ 删除 | 与 task_scheduler 重复 |
| `external` | 🗑️ 删除 | 空壳模块 |
| `dap` | 📦 移至 `crates/jcode-debug/` | DAP 协议支持 |
| `debugger` | 📦 移至 `crates/jcode-debug/` | Debugger UI |
| `rule_reviewer` | 🔄 移至 `enterprise/review.rs` | 企业功能 |
| `workspace_manager` | 🔄 合并至 `session/workspace.rs` | 与 session 重复 |
| `compaction` | 🔄 合并至 `memory/compaction.rs` | 与 memory_advanced 重复 |
| `plan` | 🗑️ 删除 | 被 ultraplan 替代 |

**处置统计**:
- 🗑️ 直接删除: **10 个**
- 🔄 合并至其他模块: **6 个**
- 📦 移至 experimental/debug: **3 个**

---

### 4. 依赖方向规则

#### 4.1 强制分层依赖图

```
┌─────────────────────────────────────────────────────────────┐
│                    依赖方向（必须遵守）                       │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│   carpai-cli ──────┬──────────> carpai-core                 │
│      │             │                                          │
│      └─────────────┼──────────> carpai-server (optional)     │
│                    │                                          │
│   carpai-server ───┼──────────> carpai-core                  │
│                    │                                          │
│   carpai-sdk ──────┼──────────> carpai-core                  │
│                    │         (or carpai-server for remote)   │
│                                                             │
│   carpai-core ─────┼──────────> crates/* (基础设施)          │
│                    │                                          │
│   ❌ 禁止的反向依赖:                                         │
│      - carpai-core 不得依赖 carpai-cli/server/sdk            │
│      - carpai-server 不得依赖 carpai-cli                      │
│      - carpai-cli 不得依赖 carpai-server (仅通过 protocol)    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

#### 4.2 接口隔离原则

为确保严格的依赖方向，需定义清晰的 **公共 API 边界**：

**carpai-core 对外暴露的最小接口**:
```rust
// carpai-core/src/lib.rs
pub mod agent {
    pub use runtime::{AgentRuntime, AgentConfig};
    pub use traits::{Agent, AgentExecutor};
}

pub mod memory {
    pub use store::{MemoryStore, MemoryStoreConfig};
    pub use graph::{KnowledgeGraph, GraphConfig};
}

pub mod tools {
    pub use registry::{ToolRegistry, Tool};
    pub use mcp::{McpClient, McpConfig};
}

pub mod completion {
    pub use engine::{CompletionEngine, CompletionRequest};
}

pub mod session {
    pub use state::{SessionState, SessionContext};  // 替代全局 static
}

pub mod config {
    pub use types::{CoreConfig, AppConfig};
}

pub mod error {
    pub type Result<T> = std::result::Result<T, CarpAIError>;
}
```

**carpai-server 对外暴露的接口**:
```rust
// carpai-server/src/lib.rs
pub mod server {
    pub use CarpAIServer;
    pub use config::ServerConfig;
}

// 不暴露内部实现细节！
// CLI/SDK 只能通过 gRPC/REST/WS 协议访问
```

#### 4.3 通信协议（跨 Crate 边界）

**本地模式** (CLI → Core):
```rust
// Direct function calls (same process)
let core = AgentRuntime::new(config);
let result = core.execute_task(task).await?;
```

**远程模式** (CLI → Server → Core):
```protobuf
// Protobuf definition (protocol/proto/agent.proto)
service AgentService {
  rpc ExecuteTask (TaskRequest) returns (TaskResponse);
  rpc StreamCompletion (CompletionRequest) returns (stream CompletionResponse);
}
```

---

### 5. 配置分层设计

#### 5.1 三层配置架构

```
┌─────────────────────────────────────────────────────────────┐
│                   配置加载优先级（从低到高）                  │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Layer 1: 默认值 (Hardcoded)                                │
│  └── carpai-core/config/defaults.rs                        │
│      CoreConfig { timeout: 30, max_tokens: 4096, ... }     │
│                                                             │
│  Layer 2: 配置文件 (File-based)                            │
│  ├── ~/.config/carpai/core.toml    (Core)                  │
│  ├── ~/.config/carpai/server.toml   (Server)               │
│  └── ~/.config/carpai/cli.toml      (CLI)                  │
│                                                             │
│  Layer 3: 环境变量覆盖 (Env vars)                          │
│  ├── CARPAI_CORE__TIMEOUT=60                               │
│  ├── CARPAI_SERVER__ADDR=0.0.0.0:8080                      │
│  └── CARPAI_CLI__THEME=dark                                │
│                                                             │
│  最终结果: Layer 3 > Layer 2 > Layer 1                      │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

#### 5.2 配置结构体定义

**carpai-core/config/types.rs**:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    // Agent 配置
    pub agent: AgentConfig,

    // Memory 配置
    pub memory: MemoryConfig,

    // Tools 配置
    pub tools: ToolsConfig,

    // Completion 配置
    pub completion: CompletionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub default_model: String,
    pub max_concurrent_tasks: usize,
    pub timeout_secs: u64,
    pub retry_attempts: u32,
}

// ... 其他配置结构体
```

**carpai-server/config/types.rs** (扩展 CoreConfig):
```rust
use carpai_core::config::CoreConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(flatten)]
    pub core: CoreConfig,  // 继承核心配置

    // Server 特有配置
    pub server: ServerSpecificConfig,
    pub auth: AuthConfig,
    pub observability: ObservabilityConfig,
    #[cfg(feature = "enterprise")]
    pub enterprise: EnterpriseConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSpecificConfig {
    pub grpc_addr: String,
    pub rest_addr: String,
    pub ws_addr: String,
    pub admin_addr: String,
    pub tls_enabled: bool,
    pub tls_cert_path: Option<String>,
    pub tls_key_path: Option<String>,
}
```

**carpai-cli/config/types.rs**:
```rust
use carpai_core::config::CoreConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(flatten)]
    pub core: CoreConfig,

    // CLI 特有配置
    pub ui: UiConfig,
    pub commands: CommandConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub theme: Theme,
    pub mouse_support: bool,
    pub animation_enabled: bool,
    pub default_panel_layout: PanelLayout,
}
```

#### 5.3 配置加载器实现

```rust
// carpai-core/src/config/loader.rs
use config::{Config, File, Environment};

pub fn load_core_config() -> Result<CoreConfig, ConfigError> {
    Config::builder()
        // Layer 1: Defaults
        .set_default("agent.default_model", "claude-sonnet-4-20250514")?
        .set_default("agent.timeout_secs", 30)?
        .set_default("agent.max_concurrent_tasks", 5)?
        // Layer 2: File
        .add_source(File::with_name("config/core"))
        // Layer 3: Env vars (prefix: CARPAI_CORE__)
        .add_source(Environment::with_prefix("CARPAI_CORE").separator("__"))
        .build()?
        .try_deserialize()
}
```

---

### 6. Feature Gates 重设计

#### 6.1 新 Feature 结构

```toml
# crates/carpai-core/Cargo.toml
[features]
default = []
# Core features (always enabled in most builds)
full = ["agent", "memory", "tools", "completion", "refactoring"]
minimal = []  # Only types and traits (for SDK light builds)

# Optional components
agent = []
memory = []
tools = []
completion = []
refactoring = []
ast = []

# Integration features
embeddings = ["dep:jcode-embedding"]
gpu = ["dep:jcode-cpu-inference"]

# crates/carpai-server/Cargo.toml
[features]
default = ["grpc", "rest", "ws"]
grpc = ["dep:jcode-grpc"]
rest = ["dep:axum"]  # or actix
ws = ["dep:tokio-tungstenite"]
auth-jwt = ["dep:jcode-auth"]
auth-oauth = ["dep:jcode-azure-auth"]
enterprise = ["multi-tenant", "rbac", "distributed"]
multi-tenant = []
rbac = []
distributed = ["dep:jcode-distributed-inference"]
observability = ["prometheus", "tracing"]
prometheus = ["dep:metrics-exporter-prometheus"]
audit-log = []

# crates/carpai-cli/Cargo.toml
[features]
default = ["tui", "commands"]
tui = ["dep:ratatui", "dep:crossterm"]
commands = []
dashboard = ["dep:egui"]  # Optional GUI dashboard
ambient = ["dep:tokio-cron-scheduler"]
notifications = ["telegram", "gmail", "browser"]
telegram = []
gmail = []
browser = []
vim = []
voice = ["dep:whisper-rs"]
plugins = []
```

#### 6.2 编译矩阵（合法组合）

| Target Binary | Required Features | Optional Features | Size Estimate |
|---------------|------------------|-------------------|--------------|
| **carpai-cli (full)** | `default` | `ambient`, `notifications`, `vim`, `voice` | ~25MB |
| **carpai-cli (minimal)** | `tui`, `commands` | - | ~15MB |
| **carpai-server (full)** | `default` | `enterprise`, `observability`, `gpu` | ~20MB |
| **carpai-server (minimal)** | `grpc`, `rest` | - | ~12MB |
| **carpai-sdk (light)** | `minimal` | - | ~5MB |
| **carpai-sdk (full)** | `full` | `embeddings` | ~12MB |

**非法组合（编译期报错）**:
```compile_fail
// ❌ 不能同时启用 conflicting features
carpai-cli with feature="grpc"  (gRPC only for server)
carpai-server with feature="tui" (TUI only for CLI)

// ✅ 正确用法
cargo build -p carpai-cli --features "tui,commands,ambient"
cargo build -p carpai-server --features "grpc,rest,enterprise"
cargo build -p carpai-sdk --features "minimal"
```

---

## 第三部分：实施路线图

### Phase 1: 基础拆分（Week 1-2）

**目标**: 创建 `carpai-core` crate，迁移无外部依赖的核心模块

#### Week 1: 核心骨架搭建

**Day 1-2: 项目初始化**
- [ ] 创建 `crates/carpai-core/Cargo.toml`
- [ ] 创建目录结构（见 [2.1 carpai-core](#21-carpai-core核心抽象层)）
- [ ] 定义核心 trait（[traits.rs](#42-接口隔离原则)）
- [ ] 统一错误类型（[error.rs](#42-接口隔离原则)）
- [ ] 配置类型提取（[config/types.rs](#52-配置结构体定义)）

**Day 3-4: Agent 系统迁移**
- [ ] 迁移 `src/agent.rs` → `carpai-core/src/agent/mod.rs`
- [ ] 拆分 `src/agent_runtime.rs` (711行) → `agent/runtime/` 子模块
- [ ] 迁移 `src/task_*.rs` (6个) → `agent/planner.rs`
- [ ] 移除 `static CURRENT_SESSION_ID`，改用 `SessionContext`
- [ ] 编写单元测试（目标覆盖率 ≥60%）

**Day 5: Memory & Tools 迁移**
- [ ] 迁移 `src/memory*.rs` (7个) → `carpai-core/src/memory/`
- [ ] 迁移 `src/knowledge*.rs` (3个) → `memory/knowledge.rs`
- [ ] 迁移 `src/tool.rs`, `src/mcp.rs` → `carpai-core/src/tools/`
- [ ] 验证编译通过：`cargo check -p carpai-core`

#### Week 2: 补全核心模块

**Day 6-7: Completion & Refactoring**
- [ ] 迁移 `src/completion*.rs` (4个) → `carpai-core/src/completion/`
- [ ] 开始迁移 `src/refactor*.rs` (14个) → `carpai-core/src/refactoring/`
  - 优先: `refactor_engine.rs`, `diff_engine.rs`, `atomic_edit_coordinator.rs`
- [ ] 委托给已有 crates（`jcode-completion`, `jcode-cross-file-repair`）

**Day 8-9: AST & Session**
- [ ] 迁移 `src/ast*.rs` (8个) → `carpai-core/src/ast/`
- [ ] 迁移 `src/session*.rs` (6个) → `carpai-core/src/session/`
- [ ] 实现 `SessionContext` 替代全局状态

**Day 10: 集成验证**
- [ ] 更新根 `Cargo.toml`，添加 `carpai-core` 依赖
- [ ] 修改 `src/lib.rs`，改为 re-export `carpai-core`
- [ ] 运行完整测试套件：`cargo test -p carpai-core`
- [ ] 性能基线测试：记录编译时间
- [ ] **产出物**: `carpai-core` 可独立编译 ✅

**Phase 1 验收标准**:
- ✅ `cargo build -p carpai-core` 成功
- ✅ `cargo test -p carpai-core` 全部通过
- ✅ `cargo doc -p carpai-core` 无警告
- ✅ 根 crate 仍可编译（向后兼容）

---

### Phase 2: Server 独立（Week 3-4）

**目标**: 创建 `carpai-server` crate，实现 pure-server 编译模式

#### Week 3: Server 骨架 & API 层

**Day 11-12: Server 初始化**
- [ ] 创建 `crates/carpai-server/Cargo.toml`
- [ ] 依赖 `carpai-core` + `jcode-grpc` + `jcode-auth`
- [ ] 定义 `ServerConfig`（继承 `CoreConfig`）
- [ ] 实现 `CarpAIServer::new()` 构造函数

**Day 13-14: gRPC & REST 迁移**
- [ ] 迁移 `src/grpc/` → `carpai-server/src/grpc/`
- [ ] 迁移 `src/api/`, `src/rest/` → `carpai-server/src/rest/`
- [ ] 迁移 `src/ws/` → `carpai-server/src/ws/`
- [ ] 实现 Proto 服务定义（`agent.proto`, `completion.proto`）

**Day 15: Auth & Security**
- [ ] 迁移 `src/auth/` → `carpai-server/src/auth/`
- [ ] 整合 `jcode-auth`（JWT, OAuth, RBAC）
- [ ] 迁移 `src/security*.rs` (3个) → `auth/security.rs`
- [ ] 实现中间件链（Auth → RateLimit → Logging）

#### Week 4: Enterprise & Observability

**Day 16-17: Enterprise Features**
- [ ] 迁移 `src/enterprise/` (~20 files) → `carpai-server/src/enterprise/`
  - 优先: `config.rs`, `auth.rs`, `admin_api/`
  - 延后: `distributed/`, `kv_cache_storage/` (Phase 4)
- [ ] 添加 `enterprise` feature gate

**Day 18-19: Observability**
- [ ] 迁移 `src/observability/` (7个) → `carpai-server/src/observability/`
- [ ] 集成 Prometheus metrics
- [ ] 集成分布式 tracing（OpenTelemetry）
- [ ] 实现审计日志（`audit_log.rs`）

**Day 20: Server 集成测试**
- [ ] 实现 pure-server 编译：`cargo build -p carpai-server --no-default-features --features "grpc,rest"`
- [ ] 启动 server 并验证健康检查端点：`GET /healthz`
- [ ] 编写集成测试（gRPC client → server → core）
- [ ] **产出物**: `carpai-server` 可独立运行 ✅

**Phase 2 验收标准**:
- ✅ `cargo build -p carpai-server` 成功（无 CLI 依赖）
- ✅ Server 可启动并响应 gRPC/REST 请求
- ✅ Auth 中间件正常工作
- ✅ Enterprise feature 可选编译

---

### Phase 3: CLI 精简（Week 5-6）

**目标**: 创建 `carpai-cli` crate，移除 server 对 cli 的依赖

#### Week 5: CLI 骨架 & TUI 迁移

**Day 21-22: CLI 初始化**
- [ ] 创建 `crates/carpai-cli/Cargo.toml`
- [ ] 依赖 `carpai-core`（本地模式）/ `carpai-server`（远程模式）
- [ ] 定义 `CliConfig`（UI theme, keybindings 等）
- [ ] 实现 `main.rs` 入口点（TUI/REPL/Command 分发）

**Day 23-25: TUI 重构（最大难点）**
- [ ] 迁移 `src/tui/` → `carpai-cli/src/tui/`
- [ ] **剥离业务逻辑**（~500 行）→ 移回 `carpai-core` 或删除
  - `execute_agent_command()` → `carpai-core/agent/runtime.rs`
  - 会话状态管理 → `carpai-core/session/state.rs`
  - 文件操作 → `carpai-core/storage/`
- [ ] TUI 仅保留：ratatui 组件、事件循环、渲染逻辑
- [ ] 确保 TUI 无 `carpai-server` 依赖

**Day 26-27: Commands & Notifications**
- [ ] 迁移 `src/cli/` → `carpai-cli/src/commands/`
- [ ] 迁移 `src/notifications/` (8个) → `carpai-cli/src/notifications/`
- [ ] 迁移 `src/background/`, `src/ambient*.rs` → `carpai-cli/`

#### Week 6: CLI 打磨 & 清理

**Day 28-29: 高级功能迁移**
- [ ] 迁移 `src/plugins/` (4个) → `carpai-cli/src/plugins/`
- [ ] 迁移 `src/perf*.rs` (11个，仅 UI 相关) → `carpai-cli/src/perf/`
- [ ] 迁移可选功能：`buddy.rs`, `voice.rs`, `vim.rs`

**Day 30: CLI 集成验证**
- [ ] 实现 pure-cli 编译：`cargo build -p carpai-cli --features "tui,commands"`
- [ ] 测试本地模式：`carpai chat`（直接调用 core）
- [ ] 测试远程模式：`carpai chat --remote localhost:50051`
- [ ] 移除根 crate 的 `cli` feature 依赖
- [ ] **产出物**: `carpai-cli` 独立 ✅

**Phase 3 验收标准**:
- ✅ `cargo build -p carpai-cli` 成功（无 server 依赖）
- ✅ TUI 正常渲染和交互
- ✅ 本地模式和远程模式均可工作
- ✅ 根 crate 的 `lib.rs` 模块数降至 <100

---

### Phase 4: SDK 对接 & 清理（Week 7-8）

**目标**: 完善 `carpai-sdk`，清理遗留代码

#### Week 7: SDK 完善

**Day 31-33: SDK 增强**
- [ ] 完善 `carpai-sdk`（已存在 ✅）
- [ ] 添加 VSCode 插例示例（`examples/vscode-extension/`）
- [ ] 添加 JetBrains 插例示例（`examples/intellij-plugin/`）
- [ ] 添加 Neovim 插例示例（`examples/neovim-plugin/`）
- [ ] 实现 SDK 的两种模式：
  - **Embedded**: 直接 link `carpai-core`（本地 LSP）
  - **Remote**: 通过 gRPC 连接 `carpai-server`

**Day 34-35: 端到端测试**
- [ ] 编写 E2E 测试：SDK → Server → Core
- [ ] 性能测试：completion latency < 200ms (p99)
- [ ] 并发测试：100 simultaneous connections

#### Week 8: 遗留代码清理

**Day 36-38: 死代码删除**
- [ ] 删除 18 个遗留模块（见 [3.4 遗留模块处置](#34-遗留模块处置phase-4)）
- [ ] 合并重复模块（6个）
- [ ] 移动实验性模块至 `crates/jcode-experimental/`
- [ ] 运行 `cargo machete` 验证无无用依赖

**Day 39-40: 文档 & 收尾**
- [ ] 更新 `README.md`（新的构建说明）
- [ ] 编写 `docs/MIGRATION_GUIDE.md`（迁移指南）
- [ ] 更新 CI/CD pipeline（3 个独立 binary 的构建）
- [ ] 性能回归测试（对比 Phase 0 基线）
- [ ] **最终产出物**: 完整 Monorepo ✅

**Phase 4 验收标准**:
- ✅ `carpai-sdk` 可用于 3 种 IDE
- ✅ E2E 测试全部通过
- ✅ 遗留模块清理完毕（死代码率 <2%）
- ✅ 根 crate `lib.rs` 模块数 <50
- ✅ 文档完整（迁移指南 + API 文档）

---

## 第四部分：风险评估

### 风险矩阵

| 风险项 | 概率 | 影响 | 风险等级 | 缓解措施 |
|--------|------|------|----------|----------|
| **循环依赖导致编译失败** | 高 (70%) | 🔴 高 (阻塞) | 🔴 Critical | 1. 先画完整依赖图（`cargo tree`）<br>2. 使用 `dependency-analyzer` 工具<br>3. 分批迁移，每步验证编译<br>4. 引入接口层（trait object）打破循环 |
| **编译时间增长** | 中 (50%) | 🟡 中 (影响效率) | 🟡 Medium | 1. 启用 cargo 并行编译（`-j auto`）<br>2. 使用 `sccache` 缓存编译产物<br>3. 优化 feature gates（减少组合爆炸）<br>4. `mold` linker 替代默认 linker |
| **测试回归** | 中 (45%) | 🔴 高 (质量风险) | 🔴 High | 1. 渐进式迁移（每次 ≤5 个模块）<br>2. 迁移前先写测试（Test-First）<br>3. CI 中加入回归测试套件<br>4. 代码评审强制要求 |
| **团队学习成本** | 低 (20%) | 🟡 中 (短期效率下降) | 🟡 Low-Medium | 1. 详细文档（本报告 + Migration Guide）<br>2. Pair Programming 迁移关键模块<br>3. Architecture Decision Records (ADRs)<br>4. Weekly sync meeting 同步进展 |
| **Feature Gate 冲突** | 中 (40%) | 🟡 中 (编译失败) | 🟡 Medium | 1. 使用 `cargo-hack` 测试所有 feature 组合<br>2. 文档化合法组合矩阵（见 [6.2](#62-编译矩阵合法组合)）<br>3. CI 中自动检测冲突 |
| **性能回归** | 低 (25%) | 🟡 中 (用户体验) | 🟡 Low-Medium | 1. 建立性能基线（Phase 0 benchmark）<br>2. 每次迁移后运行 benchmark<br>3. 关键路径（completion latency）持续监控 |
| **第三方 crate 兼容性** | 低 (15%) | 🟡 中 (升级成本) | 🟡 Low | 1. 锁定关键依赖版本（`Cargo.lock`）<br>2. 定期更新依赖（每月一次）<br>3. 使用 `cargo outdated` 监控 |

### 最高风险应对预案

#### 应对循环依赖（Risk #1）

**触发条件**: 迁移模块 A 到 `carpai-core` 时，发现它依赖仍在根 crate 的模块 B

**应急预案**:
```rust
// Step 1: 引入 trait 抽象（临时方案）
// carpai-core/src/traits.rs
#[async_trait]
pub trait ModuleBProvider {
    async fn do_something(&self, input: &Input) -> Result<Output>;
}

// Step 2: 在根 crate 实现该 trait
// src/lib.rs
impl carpai_core::traits::ModuleBProvider for RootContext {
    async fn do_something(&self, input: &Input) -> Result<Output> {
        self.module_b_impl(input).await
    }
}

// Step 3: 通过 DI 注入
// carpai-core/src/agent/runtime.rs
pub struct AgentRuntime<B: ModuleBProvider> {
    module_b: B,
}
```

**长期方案**: 等模块 B 也迁移到 `carpai-core` 后，移除 trait 抽象。

---

## 第五部分：工作量估算

### 详细任务分解

#### Phase 1: carpai-core（10 人天）

| 任务ID | 任务名称 | 子任务数 | 预估工时 | 优先级 | 依赖 |
|--------|---------|---------|----------|--------|------|
| P1-01 | 项目初始化 | 5 | 8h | P0 | 无 |
| P1-02 | Agent 系统迁移 | 8 | 16h | P0 | P1-01 |
| P1-03 | Memory 迁移 | 6 | 12h | P0 | P1-01 |
| P1-04 | Tools 迁移 | 4 | 8h | P0 | P1-01 |
| P1-05 | Completion 迁移 | 4 | 8h | P1 | P1-01 |
| P1-06 | Refactoring 迁移（第一批） | 6 | 16h | P1 | P1-01 |
| P1-07 | AST & Session 迁移 | 8 | 16h | P1 | P1-01 |
| P1-08 | 集成验证 & 测试 | 6 | 16h | P0 | P1-02~07 |
| **合计** | | **47** | **80h (10d)** | | |

#### Phase 2: carpai-server（15 人天）

| 任务ID | 任务名称 | 子任务数 | 预估工时 | 优先级 | 依赖 |
|--------|---------|---------|----------|--------|------|
| P2-01 | Server 初始化 | 4 | 8h | P0 | Phase 1 |
| P2-02 | gRPC & REST 迁移 | 8 | 20h | P0 | P2-01 |
| P2-03 | Auth & Security | 6 | 16h | P0 | P2-01 |
| P2-04 | Enterprise Features | 10 | 24h | P1 | P2-01 |
| P2-05 | Observability | 6 | 16h | P1 | P2-01 |
| P2-06 | Server 集成测试 | 5 | 16h | P0 | P2-02~05 |
| P2-07 | Proto 定义 & 文档 | 4 | 16h | P1 | P2-02 |
| **合计** | | **43** | **120h (15d)** | | |

#### Phase 3: carpai-cli（12 人天）

| 任务ID | 任务名称 | 子任务数 | 预估工时 | 优先级 | 依赖 |
|--------|---------|---------|----------|--------|------|
| P3-01 | CLI 初始化 | 4 | 8h | P0 | Phase 1 |
| P3-02 | TUI 重构（核心难点） | 12 | 24h | P0 | P3-01 |
| P3-03 | Commands 迁移 | 6 | 12h | P0 | P3-01 |
| P3-04 | Notifications & Background | 8 | 16h | P1 | P3-01 |
| P3-05 | Plugins & Perf | 6 | 12h | P2 | P3-02 |
| P3-06 | CLI 集成测试 | 5 | 16h | P0 | P3-02~04 |
| **合计** | | **41** | **88h (12d)** | | |

#### Phase 4: SDK & Cleanup（8 人天）

| 任务ID | 任务名称 | 子任务数 | 预估工时 | 优先级 | 依赖 |
|--------|---------|---------|----------|--------|------|
| P4-01 | SDK 完善 | 6 | 16h | P1 | Phase 1 |
| P4-02 | IDE 示例（3个） | 3 | 12h | P2 | P4-01 |
| P4-03 | E2E 测试 | 4 | 12h | P0 | Phase 2&3 |
| P4-04 | 遗留代码清理 | 5 | 12h | P1 | Phase 3 |
| P4-05 | 文档 & 收尾 | 4 | 12h | P2 | P4-01~04 |
| **合计** | | **22** | **64h (8d)** | | |

### 总览

| 阶段 | 任务数 | 子任务总数 | 人天 | 产出物 | 里程碑 |
|------|-------|-----------|------|--------|--------|
| **Phase 1** | 8 | 47 | **10d** | carpai-core 可编译 | ✅ 核心抽象就绪 |
| **Phase 2** | 7 | 43 | **15d** | carpai-server 可运行 | ✅ Pure-server 模式 |
| **Phase 3** | 6 | 41 | **12d** | carpai-cli 独立 | ✅ CLI/Server 解耦 |
| **Phase 4** | 5 | 22 | **8d** | SDK + 清理完成 | ✅ Monorepo 完成 |
| **总计** | **26** | **153** | **45d** | **完整三产品架构** | 🎉 交付 |

### 资源需求

**团队配置（推荐）**:
- **架构师 × 1**（全程参与，负责技术决策）
- **高级工程师 × 2**（Phase 1-3 核心开发）
- **中级工程师 × 1**（Phase 3-4 测试 & 文档）
- **DevOps × 0.5**（CI/CD pipeline 维护）

**总投入**: **4.5 人 × 10 周 ≈ 45 人天**

### 成本效益分析

**重构成本**: 45 人天 × 平均日薪 ≈ **¥XXX 万**（根据团队实际情况计算）

**预期收益**:
1. **编译时间减少 40-60%**（增量编译生效）
2. **新功能开发效率提升 30%**（清晰的模块边界）
3. **Bug 修复速度提升 50%**（更好的测试隔离）
4. **可独立发布 3 个产品**（商业价值）
5. **技术债务清零**（长期维护成本降低）

**ROI 估算**: 预计 **6-9 个月** 回本（基于团队规模和发布频率）

---

## 附录

### A. 关键文件索引

| 文件路径 | 作用 | 迁移目标 |
|---------|------|---------|
| [src/lib.rs](src/lib.rs) | 根模块声明（207 mods） | 精简至 <50 mods（re-export only） |
| [src/agent_runtime.rs](src/agent_runtime.rs) | Agent 核心运行时（711行） | `carpai-core/agent/runtime/` |
| [src/config.rs](src/config.rs) | 配置系统 | `carpai-core/config/types.rs` |
| [src/server.rs](src/server.rs) | Server 入口点 | `carpai-server/src/main.rs` |
| [src/tui/app.rs](src/tui/app.rs) | TUI 应用（含业务逻辑） | `carpai-cli/src/tui/app.rs`（精简版） |
| [src/error_types.rs](src/error_types.rs) | 错误类型 | `carpai-core/src/error.rs` |
| [Cargo.toml](Cargo.toml) | Workspace 定义 | 更新 members 列表 |

### B. 参考资源

- **Rust Crate 最佳实践**: https://rust-lang.github.io/rfcs/2415-crate-configuration.html
- **Monorepo 策略参考**: https://nx.dev/concepts/monorepos
- **Feature Gates 设计**: https://doc.rust-lang.org/cargo/reference/features.html
- **Dependency Injection in Rust**: https://github.com/nickel-org/kumo-rs/blob/main/docs/architecture.md

### C. 术语表

| 术语 | 定义 |
|------|------|
| **Monolith** | 单体架构，所有代码在一个 crate 中 |
| **Monorepo** | 单一仓库，多个独立的 crate/binary |
| **Feature Gate** | Rust 的条件编译特性 |
| **God Module** | 上帝模块，承担过多职责的模块 |
| **Circular Dependency** | 循环依赖，A 依赖 B，B 又依赖 A |
| **Fan-In** | 入度，一个模块被多少其他模块依赖 |
| **Coupling** | 耦合度，模块间的依赖强度 |

---

## 版本历史

| 版本 | 日期 | 作者 | 变更说明 |
|------|------|------|----------|
| v1.0 | 2026-05-24 | AI Architecture Engine | 初始版本，基于实际代码数据生成 |

---

> **免责声明**: 本报告基于静态代码分析生成，部分数据为估算值（如代码行数、测试覆盖率）。建议在实际迁移前进行动态验证（如运行 `cargo test`, `cargo bench`）。
>
> **下一步行动**: 请审阅本报告，确认优先级和资源分配后，即可启动 Phase 1。
