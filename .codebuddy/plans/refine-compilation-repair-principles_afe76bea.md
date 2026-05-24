---
name: refine-compilation-repair-principles
overview: 将 AGENTS.md 中 "Compilation Error/Warning Repair Principles" 从 3 层模型升级为 5 层精细化模型，沉淀最近高修复效率的实践为正式指导原则
todos:
  - id: update-agents-md-5-layer
    content: 替换AGENTS.md中"Compilation Error/Warning Repair Principles"章节从3层升级为5层模型，保留其余内容不变
    status: completed
---

## 用户需求

将最近一次 "12 个错误分类成 5 层并行修复" 的高效实战经验，提炼为 CarpAI 修复编译错误/警告的正式指导原则，更新到 `AGENTS.md` 中。

## 当前问题

现有 `AGENTS.md` 第 19-44 行的 "Compilation Error/Warning Repair Principles" 采用 3 层模型，存在以下效率瓶颈：

| 问题 | 表现 |
| --- | --- |
| Layer 1 粒度过粗 | "全局/跨模块"一锅端，但 Cargo.toml 配置完全不依赖其他模块，可以并行 |
| Layer 2 混合不同性质错误 | "类型系统错误"和"语法错误"混在一起，实际互不依赖 |
| 缺少"按错误性质分层"维度 | 导致 agent 分配粒度不够细，串行等待浪费 |


## 5 层模型核心设计

按错误性质而非按模块位置分层，保证每层之间**零依赖**，5 个 agent 可同时修复：

1. **配置层** — Cargo.toml deps/features/edition
2. **结构层** — pub mod/visibility/re-exports
3. **接口层** — trait impl/类型系统/lifetime/ownership
4. **语法层** — API 调用/async 递归/import 缺失
5. **质量层** — 所有 warnings（最后单独处理）

## 修改目标

- 文件：`d:\studying\Codecargo\CarpAI\AGENTS.md` 
- 位置：第 19-44 行，替换 "Compilation Error/Warning Repair Principles (分层分模块修复法)" 章节
- 不动现有 Phase 1 Action Plan 部分（只更新引用格式）

## 技术方案

仅文档修改，不涉及代码变更。直接编辑 AGENTS.md 中的单个章节。

### 章节替换内容设计

#### 1. 升级说明表

| 维度 | 旧 3 层模型 | 新 5 层模型 |
| --- | --- | --- |
| 分层依据 | 按模块位置（全局/模块内） | **按错误性质**（配置/结构/接口/语法/质量） |
| 并行粒度 | Layer 2 内模块级并行 | **全 5 层同时并行** |
| 串行瓶颈 | Layer 1 必须等待全部完成 | **零串行等待** |
| agent 之间依赖 | 有（Layer 1 结果影响 Layer 2） | **无（层间独立，5 agents 同时启动）** |
| 效率提升 | 线性 | **3-5x 加速**（实测 12 个错误 by 1 agent -> 5 agents 并行 = ~3x） |


#### 2. 每层详细定义

**Layer 1: 配置层 (Config Layer)**

- 错误类型：E0432 (missing crate)，Cargo.toml deps 缺失/版本冲突，edition 不兼容，feature gate 没开
- 修复模式：1 agent 独立修复，不用等待其他层
- 案例：`carpai-cli` 缺少 `[build-dependencies] tonic-build`

**Layer 2: 结构层 (Structural Layer)**

- 错误类型：E0433 (use of unresolved module)，E0603 (module is private)，dead mod declarations，re-export 路径错
- 修复模式：1 agent 独立修复，src 目录结构/import 树清理
- 案例：`tui::run` 需要改为 `crate::tui::run`

**Layer 3: 接口层 (Interface Layer)**

- 错误类型：E0277 (trait bound)，E0507 (cannot move out)，E0382 (use of moved value)，E0373 (async block lifetime)，E0195 (lifetime mismatch)
- 修复模式：1 agent 独立修复，理解类型系统后一次性修复
- 案例：`agent_bridge.rs` 的 retry closure + E0507

**Layer 4: 语法层 (Syntax Layer)**

- 错误类型：E0061 (wrong arg count)，E0733 (async recursion)，E0599 (no method)，E0425 (cannot find value/type)
- 修复模式：1 agent 独立修复，直来直去的 API 用法错误
- 案例：4 个 widgets 的 `Block::borders()` API 变更 + 4x E0061

**Layer 5: 质量层 (Code Quality Layer)**

- 警告类型：dead_code, unused imports, unused variables, naming conventions, irrefutable patterns, unreachable patterns
- 修复模式：**最后处理**（在所有 errors 修完后），1 agent 集中批量修复或按模块分 agent
- 策略：优先尝试"激活使用"，否则 `#[allow(dead_code)]` + 注释

#### 3. 总体流程

```
cargo check 获取当前状态
    │
    ├─→ Layer 1 (配置层): 1 agent → 修复 Cargo.toml
    ├─→ Layer 2 (结构层): 1 agent → 修复 mod/import/re-export
    ├─→ Layer 3 (接口层): 1 agent → 修复 trait/lifetime/ownership  
    ├─→ Layer 4 (语法层): 1 agent → 修复 API 调用/语法错误
    └─→ Layer 5 (质量层): 1+ agents → 修复 warnings（最后执行）
         │
         └─→ cargo check 全量验证
               │
               └─→ 0 errors? → Done
               └─→ 仍有 error? → 新 error 归类后重入对应层
```

#### 4. 实战案例

附 carpai-cli 实测数据：

- 按旧 3 层模型：无法并行，串行逐个修复，~5-8 次编译迭代
- 按新 5 层模型：5 agents 同时修改，1 次全体验证，~2-3 次编译
- 效率提升：约 3x

### 文件修改

- 目标：`d:\studying\Codecargo\CarpAI\AGENTS.md` 
- 动作：替换第 19 行 `## Compilation Error/Warning Repair Principles` 到第 44 行的旧 3 层内容，更新为新的 5 层内容
- 保留文档其余部分不变（Phase 1 Action Plan，Install Notes）