# CarpAI 架构现状分析 & 三产品维护策略

> **版本**: v1.0 | **日期**: 2026-05-25  
> **基于**: 实际代码库扫描 (`src/lib.rs`, 各 crate `Cargo.toml`, 模块结构)  
> **状态**: 🔶 过渡态 — 新旧架构并存

---

## 一、核心结论：是真正的服务端架构了吗？

### 答案：**骨架已成，但尚未完成拆分。当前是"大仓库 + 新 crate 并存"的过渡态。**

#### ✅ 已完成（架构正确性验证）

| 维度 | 状态 | 证据 |
|------|------|------|
| **Layer 0: Trait 抽象层** | ✅ 完成 | `carpai-internal` 编译通过 (0 error), 7 个核心 trait 全部定义 |
| **Layer 1: 业务逻辑层** | ⚠️ 骨架 | `carpai-core` crate 已创建, 6 个 Local 实现已写入, 但编译待通过 |
| **Layer 2a: 企业服务端** | ⚠️ 骨架 | `carpai-server` crate 已创建, gRPC/REST/WS/Auth/多租户模块已搭建 |
| **Layer 2b: CLI 客户端** | ⚠️ 骨架 | `carpai-cli` crate 已创建, TUI/AgentBridge/命令骨架已搭建 |
| **Layer 2c: IDE SDK** | ✅ 可用 | `carpai-sdk` 已存在且功能完整 (v1.1.0-dev), WASM 支持就绪 |
| **依赖方向铁律** | ✅ 设计正确 | server/cli/sdk → core → internal，无反向依赖 |
| **Feature Gate 分离** | ⚠️ 部分 | 根 `Cargo.toml` 有 `server`/`cli` feature，但 `src/` 仍是单体 |

#### ❌ 尚未完成（关键差距）

| 差距项 | 严重度 | 说明 |
|--------|--------|------|
| **`src/` 过渡区未清空** | 🔴 高 | `src/lib.rs` 声明了 **170+ 个模块**，包含全部历史代码。新 crate 只是"新增"，未替代旧代码 |
| **根 crate 仍是入口** | 🔴 高 | `main.rs` 在根目录，`carpai` 是主 binary。server/cli/sdk 的 `main.rs` 未激活为独立产物 |
| **代码重复** | 🟡 中 | `agent_loop.rs`(core) 和 `src/agent.rs`(根) 存在功能重叠；`config.rs` 三层各自定义 |
| **编译链路** | 🟡 中 | `cargo check -p carpai-server`/`carpai-cli`/`carpai-core` 尚未全部通过 |
| **独立发布** | 🔴 高 | 无法单独发布 `carpai-server` 二进制而不带 CLI 的 ratatui/crossterm 依赖 |

---

## 二、当前架构全景图（实际代码映射）

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Cargo.toml (workspace root)                       │
│  name = "carpai" (edition 2024)                                     │
│  default features: ["server", "cli", "pdf"]                         │
│  workspace members: 100+ crates                                      │
└───────────────────────────────────┬─────────────────────────────────┘
                                    │
        ┌───────────────────────────┼───────────────────────────┐
        │                           │                           │
        ▼                           ▼                           ▼
┌───────────────┐          ┌──────────────┐            ┌──────────────┐
│ carpai-internal│          │  carpai-core  │            │   src/       │
│  (Layer 0)    │          │  (Layer 1)    │            │ (过渡区❗)    │
│               │          │               │            │              │
│ ✅ 7 Traits   │───✅依赖──▶│ ⚠6 LocalImpls│            │ 170+ modules │
│ AgentContext  │          │ agent_loop.rs │            │ agent.rs     │
│ AppConfig     │          │ CoreConfig.rs │            │ tui/         │
│ 0 error       │          │ ⏳编译中...   │            │ server/      │
└───────────────┘          └──────┬────────┘            │ provider/    │
                                  │                     │ completion/  │
                    ┌─────────────┼─────────────┐      │ ...全部历史   │
                    ▼             ▼              ▼      └──────┬───────┘
            ┌────────────┐ ┌──────────┐ ┌──────────┐           │
            │carpai-server│ │carpai-cli│ │carpai-sdk │           │
            │(Layer 2a)  │ │(Layer 2b)│ │(Layer 2c) │           │
            │            │ │          │ │          │           │
            │⚠gRPC/REST │ │⚠TUI骨架  │ │✅可用    │◀──────────┘
            │⚴Auth/RBAC │ │⚠Bridge   │ │WASM支持  │  ← src/ 仍是实际运行入口
            │⚴多租户    │ │⚴Commands │ │HTTP/gRPC │     新 crate 是"并行新建"
            └────────────┘ └──────────┘ └──────────┘
```

### 关键发现：`src/` 目录的角色

`src/lib.rs` 当前声明了 **~170 个 pub mod**，分为以下几类：

| 分类 | 模块数 | Feature Gate | 应迁往 |
|------|--------|-------------|--------|
| **Agent 核心** (agent, agent_runtime, task_*, plan_*) | ~15 | always | `carpai-core` |
| **Server 功能** (api, grpc, rest, ws, auth, security, distributed) | ~20 | `server` | `carpai-server` |
| **CLI/TUI** (cli, tui, dashboard, buddy, voice, vim) | ~25 | `cli` | `carpai-cli` |
| **Provider/LLM** (provider, inference_*, embedding, sidecar) | ~12 | always | `carpai-core`(trait) + `carpai-server`(remote impl) |
| **Memory 系统** (memory_*, knowledge_*) | ~15 | always | `carpai-core` |
| **重构引擎** (refactor_*, diff_*, edit_*) | ~15 | always | `carpai-core` 或独立的 `jcode-refactor` |
| **工具/MCP** (tool, mcp, slash_command) | ~5 | always | `carpai-core` |
| **企业特性** (enterprise, audit, quota) | ~4 | `enterprise` | `carpai-server` |
| **基础设施** (config, session, git, storage, file_*) | ~20 | always | 按职责分 |
| **观测性** (observability, metrics, telemetry) | ~8 | `server` | `carpai-server` |

---

## 三、三产品如何维护和发布版本？

### 3.1 发布矩阵

```
┌────────────┬──────────────────┬─────────────────────┬─────────────────────┐
│   产品     │    Binary 名      │   Build 命令         │   目标用户          │
├────────────┼──────────────────┼─────────────────────┼─────────────────────┤
│ 服务端     │ carpai-server    │ cargo build -p       │ 企业 IT / DevOps    │
│            │                  │ carpai-server --release│                   │
├────────────┼──────────────────┼─────────────────────┼─────────────────────┤
│ 单机客户端 │ carpai (或        │ cargo build -p       │ 个人开发者          │
│            │ carpai-cli)      │ carpai-cli --release │                    │
├────────────┼──────────────────┼─────────────────────┼─────────────────────┤
│ IDE SDK    │ (library)        │ cargo publish -p     │ VSCode/JetBrains/   │
│            │                  │ carpai-sdk           │ Neovim 插件开发者   │
└────────────┴──────────────────┴─────────────────────┴─────────────────────┘
```

### 3.2 版本号策略

```
carpai-internal:  0.1.x  ← Trait 层，变动最少，semver 严格
carpai-core:      0.1.x  ← 业务逻辑层，随 internal 变动而变
carpai-server:    0.1.x  ← 服务端产品，独立发版周期
carpai-cli:       0.1.x  ← CLI 产品，独立发版周期
carpai-sdk:       1.1.x  ← SDK 产品（已有用户），保持 API 兼容
```

**规则**：
- `carpai-internal` 变更 → **minor** version bump (新增 trait/方法)
- `carpai-core` 变化 → 跟随 internal，**patch** 为 bugfix
- `carpai-server`/`carpai-cli` → 独立版本，但依赖的 core/internal 必须兼容
- `carpai-sdk` → **最保守**，只能加 API 不能删（已有外部消费者）

### 3.3 当前 vs 目标发布能力对比

| 能力 | 当前状态 | 目标状态 |
|------|---------|---------|
| 单独构建服务端 (无 TUI 依赖) | ❌ 不行 — `default = ["server", "cli"]` 导致 cli 的 ratatui 总被拉入 | ✅ `cargo build -p carpai-server --no-default-features` |
| 单独构建 CLI (轻量分发) | ❌ 不行 — 根 crate 包含所有 server 代码 | ✅ `cargo build -p carpai-cli` |
| SDK 作为 crates.io 发布 | ⚠️ 可以但依赖过重 (reqwest 0.11, tonic) | ✅ 精简依赖，可选 feature |
| 三者共享同一份 Agent 逻辑 | ⚠️ 部分共享 — trait 定义了但实现重复 | ✅ core 统一，server/cli 各自注入不同实现 |

---

## 四、团队分工与代码拆分计划

### 4.1 团队映射（基于 THREE_TEAM_REFACTOR_PLAN_V3_FINAL）

```
┌─────────────────────────────────────────────────────────────────────┐
│                        团队分工矩阵                                  │
├──────────────┬──────────────────┬──────────────────┬────────────────┤
│              │  solo-Turbo      │  Paw-brave       │  ma-guoyang    │
│              │  (服务端核心)     │  (CLI 客户端)     │  (SDK+基础)    │
├──────────────┼──────────────────┼──────────────────┼────────────────┤
│ 负责 Crate   │ carpai-server    │ carpai-cli       │ carpai-sdk     │
│              │ + enterprise 中间件│ + TUI 渲染      │ + carpai-internal│
│              │                  │ + AgentBridge    │ + carpai-core   │
├──────────────┼──────────────────┼──────────────────┼────────────────┤
│ 代码来源     │ src/server/*     │ src/tui/*        │ crates/carpai-* │
│              │ src/api/*        │ src/cli/*        │ src/agent*      │
│              │ src/auth/*       │ src/ambient/*    │ src/provider*   │
│              │ src/enterprise/* │ src/notifications│ src/completion* │
├──────────────┼──────────────────┼──────────────────┼────────────────┤
│ 交付物       │ gRPC/REST/WS     │ TUI 交互式终端   │ Trait 定义      │
│              │ 多租户 + RBAC    │ 双模式(Local/    │ Local 实现      │
│              │ 审计日志         │  Remote)         │ SDK HTTP/gRPC   │
│              │ 分布式推理调度   │ 后台任务+通知    │ IDE 协议适配    │
└──────────────┴──────────────────┴──────────────────┴────────────────┘
```

### 4.2 什么时候拆？—— 分阶段迁移路线图

> **原则**: 不是"大爆炸式"重写，而是**渐进式掏空 `src/`**。

#### Phase 1: 当前阶段（进行中）— ✅ 骨架建立

**目标**: 新 crate 编译通过，`src/` 保持不动

```
Week 1-2 (现在):
  ✅ carpai-internal: 7 traits + AgentContext → 编译通过
  ⏳ carpai-core: 6 Local 实现 + agent_loop → 编译修复中
  ⏳ carpai-server: gRPC/REST 骨架 → 编译修复中
  ⏳ carpai-cli: TUI/Bridge 骨架 → 编译修复中
  ✅ carpai-sdk: 已可用（不动）
```

**产出标准**: `cargo check -p {internal,core,server,cli}` 全部 0 error

#### Phase 2: 双轨运行（建议 Week 3-6）— 🔄 接入但不替换

**目标**: 新 crate 的代码可以被调用，但 `src/` 仍是主入口

```
关键动作:
  1. 根 crate main.rs 增加 dispatch 逻辑:
     - 环境变量 CARPAI_MODE=server → 调用 carpai_server::main()
     - 环境变量 CARPAI_MODE=cli → 调用 carpai_cli::main()
     - 默认 (空) → 保持现有 src/ 行为（向后兼容）
  
  2. carpai-core 的 execute_agent_turn() 被 src/agent.rs 调用:
     - 先平行运行，验证结果一致性
     
  3. carpai-server 的 gRPC handler 替换 src/grpc/:
     - 先 A/B 测试，再全量切换
```

**团队分工**:
| 团队 | 任务 | 交付 |
|------|------|------|
| **ma-guoyang** | `src/agent.rs` → 委托给 `carpai_core::execute_agent_turn` | Agent 逻辑不再重复 |
| **solo-Turbo** | `src/server/` → 委托给 `carpai_server::Application` | 服务启动统一 |
| **Paw-brave** | `src/tui/` + `src/cli/` → 委托给 `carpai_cli` | TUI 渲染层分离 |

#### Phase 3: 物理搬迁（建议 Week 7-10）— 📦 删除 `src/` 对应模块

**目标**: `src/` 中的模块被物理移动到对应 crate，原位置变为 re-export

```
搬迁顺序（按风险从低到高）:

  Batch 1 (低风险 - 无循环依赖):
    src/ambient/*         → crates/carpai-cli/src/ambient/
    src/notifications/*   → crates/carpai-cli/src/notifications/
    src/dictation.rs      → crates/carpai-cli/src/
    src/login_qr.rs       → crates/carpai-cli/src/

  Batch 2 (中风险 - Agent 核心):
    src/agent_runtime.rs  → crates/carpai-core/src/runtime.rs
    src/task_*.rs         → crates/carpai-core/src/task/
    src/plan_*.rs         → crates/carpai-core/src/planning/
    src/skill_system.rs   → crates/carpai-core/src/skill.rs
    src/sub_agents.rs     → crates/carpai-core/src/sub_agent.rs

  Batch 3 (中风险 - Server):
    src/server/*          → crates/carpai-server/src/legacy/ (先合入)
    src/api/*             → crates/carpai-server/src/api/
    src/auth/*            → crates/carpai-server/src/auth/ (已有新版本)
    src/security/*        → crates/carpai-server/src/security/
    src/distributed/*     → crates/carpai-server/src/distributed/
    src/enterprise/*      → crates/carpai-server/src/enterprise/ (已有新版本)

  Batch 4 (高风险 - Provider/Completion):
    src/provider/*        → crates/carpai-core/src/provider/ (trait接口)
                            crates/carpai-server/src/provider/ (远程实现)
    src/completion/*      → crates/carpai-core/src/completion/
    src/completion_engine/* → crates/carpai-core/src/completion_engine/
    src/sidecar.rs        → crates/carpai-core/src/sidecar.rs
    src/embedding.rs      → crates/carpai-core/src/embedding.rs

  Batch 5 (高风险 - Memory/Session):
    src/memory*.rs        → crates/carpai-core/src/memory/
    src/session.rs        → crates/carpai-core/src/session.rs
    src/git/*.rs          → crates/carpai-core/src/git/
    src/storage.rs        → crates/carpai-core/src/storage.rs

  Batch 6 (TUI - 最后搬):
    src/tui/**            → crates/carpai-cli/src/tui/ (合并)
    src/cli/**            → crates/carpai-cli/src/cli/ (合并)
    src/dashboard.rs      → crates/carpai-cli/src/
    src/debug_panel.rs    → crates/carpai-cli/src/
    src/buddy.rs          → crates/carpai-cli/src/
```

**每个 Batch 的验收标准**:
1. `cargo check` 全量通过（0 error）
2. `cargo test` 全量通过（不减少现有测试覆盖）
3. `src/lib.rs` 中对应 `pub mod` 改为 `pub use xxx_crate::module`
4. Git commit 按 Batch 粒度提交（可回滚）

#### Phase 4: 清理收尾（建议 Week 11-12）— 🧹 根 crate 最小化

**目标**: `src/lib.rs` 只剩下一页 re-export

```rust
// Phase 4 目标状态的 src/lib.rs
#![allow(unknown_lints)]

// CarpAI Monorepo Root — Compatibility Re-export Layer
//
// 所有真实代码已搬迁到各 crate。
// 此文件仅保留 re-export 以向后兼容旧的 use carpai::* 路径。

// --- Core (re-export from carpai-core + carpai-internal) ---
pub use carpai_internal::*;
pub use carpai_core::*;

// --- Server (conditional) ---
#[cfg(feature = "server")]
pub use carpai_server::*;

// --- Client (conditional) ---
#[cfg(feature = "cli")]
pub use carpai_cli::*;

// --- Legacy aliases (deprecated, 将在 v2.0 移除) ---
#[deprecated(note = "Use carpai_core::execute_agent_turn instead")]
pub use carpai_core::execute_agent_turn as run_agent;
```

---

## 五、依赖方向铁律（CI 自动拦截）

### 当前依赖图（实际）

```
                ┌─────────────────┐
                │  carpai-internal│  Layer 0: Pure Traits ✅
                └────────┬────────┘
                         │
         ┌───────────────┼───────────────┐
         ▼               ▼               ▼
 ┌──────────────┐ ┌──────────┐ ┌──────────────┐
 │ carpai-core  │ │carpai-sdk│ │   src/ (root) │
 │  (business)  │ │  (IDE)   │ │  (过渡区)     │
 └──────┬───────┘ └────┬─────┘ └──────┬───────┘
        │              │              │
        ▼              ▼              ▼
 ┌──────────────┐ ┌──────────┐ ┌──────────────┐
 │ carpai-server│ │ (独立发布)│ │ carpai-cli    │
 │ (enterprise) │ │          │ │  (TUI client) │
 └──────────────┘ └──────────┘ └──────────────┘
```

### CI 拦截规则（应加入 `.github/workflows/rust.yml`）

```yaml
# 依赖方向检查（使用 cargo deny 或自定义脚本）
- name: Check dependency direction
  run: |
    # 禁止规则
    # 1. carpai-server 不得依赖 carpai-cli
    ! grep -r 'carpai-cli' crates/carpai-server/Cargo.toml
    
    # 2. carpai-cli 不得依赖 carpai-server
    ! grep -r 'carpai-server' crates/carpai-cli/Cargo.toml
    
    # 3. carpai-core 不得依赖 carpai-server 或 carpai-cli
    ! grep -E 'carpai-(server|cli)' crates/carpai-core/Cargo.toml
    
    # 4. carpai-internal 不得依赖任何业务 crate
    ! grep -E 'carpai-(core|server|cli|sdk)' crates/carpai-internal/Cargo.toml
    
    # 5. carpai-sdk 不得依赖 carpai-server
    ! grep -r 'carpai-server' crates/carpai-sdk/Cargo.toml
```

---

## 六、风险与应对

| 风险 | 影响 | 概率 | 应对措施 |
|------|------|------|----------|
| **循环依赖** | 编译失败 | 中 | strict dependency direction CI check; 每次 add dependency 人工 review |
| **`src/` 搬迁破坏现有功能** | 回归 | 高 | 每个 Batch 配套完整测试; keep `src/` as re-export for 2 versions |
| **Trait 接口变更波及三层** | 级联修改 | 中 | internal 的 trait 变更必须经过 RFC 流程; version bump 自动触发下游检查 |
| **团队间的 API 契约不一致** | 集成失败 | 高 | 每周 sync meeting; 共享的 `carpai-internal` trait 是唯一契约 |
| **SDK 发版节奏不匹配** | 用户抱怨 | 低 | SDK 独立版本号; feature gate 控制可选依赖 |

---

## 七、立即行动建议

### 本周（Week 1-2 剩余）:
1. **完成编译修复**: 确保 `carpai-core` + `carpai-cli` + `carpai-server` 分别 `cargo check` 通过
2. **确定根 crate dispatch 逻辑**: 在 `src/main.rs` 中加入 `CARPAI_MODE` 环境变量分支
3. **团队确认分工边界**: 明确哪些 `src/` 子目录属于哪个团队的"领地"

### 下周（Week 3-4）:
1. **开始 Batch 1 搬迁** (ambient/notifications/dictation → carpai-cli): 风险最低，最快见效
2. **建立 CI pipeline**: 加入依赖方向检查 + per-crate 编译检查
3. **输出第一个 alpha release**: `carpai-server` 和 `carpai-cli` 的独立二进制

### Month 2 (Week 5-8):
1. **Batch 2-3 搬迁**: Agent 核心和 Server 功能
2. **集成测试**: 三产品交叉测试（CLI 连 Server、SDK 连 Server）
3. **文档完善**: 每个 crate 的 `README.md` + 架构决策记录 (ADR)

### Month 3 (Week 9-12):
1. **Batch 4-6 搬迁**: Provider/Memory/TUI（最高风险）
2. **Phase 4 清理**: `src/lib.rs` 最小化
3. **v1.0 正式发布**: 三产品独立发布，Monorepo 文档完善

---

*"好的架构不是设计出来的，而是演化出来的。我们现在有正确的骨架，接下来需要的是纪律性的渐进搬迁，而不是一次性的大爆炸。"*
