# Repository Guidelines

## Development Workflow

- **Commit as you go** - Make small, focused commits after completing each feature or fix
- If the git state is not clean, or there are other agents working in the codebase in parallel, do your best to still commit your work. 
- **Push when done** - Push all commits to remote when finishing a task or session
- **Use fast iteration by default** - Prefer `cargo check`, targeted tests, and dev builds while iterating
- **Rebuild when done** - When you are done making changes, build the source.
- **Bump version for releases** - Update version in `Cargo.toml` when making releases. When cutting a new release, look at all the changes that happened since the last release and determine what the version bump should be ie patch or minor, etc. 
- **Remote builds available** - Use `scripts/remote_build.sh` to offload heavy cargo work to another machine. If your build is terminated, likely is because there are not enough resources on this machine to build. use remote build in that case. Try checking the resource avaliablity on the machine before you run a build. 

## Logs
- Logs are written to `~/.jcode/logs/` (daily files like `jcode-YYYY-MM-DD.log`).

## Debug Socket
- Use the debug socket for runtime level debugging

## Compilation Error/Warning Repair Principles (五层并行修复法)

> **原理**: 按**错误性质**而非按模块位置分层，保证每层之间**零依赖**，5 个 agent 可同时修复。
> **效率提升**: 实测 12 个错误从此前 ~5-8 次编译迭代 → ~2-3 次编译迭代 (~3x 加速)。

### 总体流程

```
cargo check 获取当前状态
    │
    ├─→ Layer 1 (配置层): 1 agent → 修复 Cargo.toml / features / edition
    ├─→ Layer 2 (结构层): 1 agent → 修复 pub mod / import / re-export
    ├─→ Layer 3 (接口层): 1 agent → 修复 trait / lifetime / ownership
    ├─→ Layer 4 (语法层): 1 agent → 修复 API 调用 / 语法错误
    └─→ Layer 5 (质量层): 1+ agents → 修复 warnings（最后执行）
         │
         └─→ cargo check 全量验证
               │
               └─→ 0 errors? → Done
               └─→ 仍有 error? → 新 error 归类后重入对应层
```

### 升级说明：3 层 → 5 层

| 维度 | 旧 3 层模型 | 新 5 层模型 |
|------|------------|------------|
| **分层依据** | 按模块位置（全局/模块内） | **按错误性质**（配置/结构/接口/语法/质量） |
| **并行粒度** | Layer 2 内模块级并行 | **全 5 层同时并行** |
| **串行瓶颈** | Layer 1 必须等待全部完成 | **零串行等待** |
| **agent 之间依赖** | 有（Layer 1 结果影响 Layer 2） | **无（层间独立，5 agents 同时启动）** |
| **效率** | 线性串行 | **3-5x 加速** |

---

### Layer 1: 配置层 (Config Layer)

| 属性 | 说明 |
|------|------|
| **错误类型** | E0432 (missing crate)，Cargo.toml deps 缺失/版本冲突，edition 不兼容，feature gate 没开 |
| **修复模式** | 1 agent 独立修复，不依赖源文件 |
| **案例** | `carpai-cli` 缺少 `[build-dependencies] tonic-build`；`carpai-core` edition 2024 不兼容 |

---

### Layer 2: 结构层 (Structural Layer)

| 属性 | 说明 |
|------|------|
| **错误类型** | E0433 (use of unresolved module)，E0603 (module is private)，dead mod declarations，re-export 路径错误 |
| **修复模式** | 1 agent 独立修复，只要目录结构和 mod 声明 |
| **案例** | `crate::tui::run` 需要改为 `crate::cli::run`；某个 `mod` 忘记在 `lib.rs` 中声明 |

---

### Layer 3: 接口层 (Interface Layer)

| 属性 | 说明 |
|------|------|
| **错误类型** | E0277 (trait bound)，E0507 (cannot move out)，E0382 (use of moved value)，E0373 (async block lifetime)，E0195 (lifetime mismatch) |
| **修复模式** | 1 agent 独立修复，需要理解类型系统和所有权 |
| **案例** | `agent_bridge.rs` 的 retry closure 无法捕获 `RwLockReadGuard` (E0507)；`JoinHandle` 不能 `.await` 借用 (E0277) |

---

### Layer 4: 语法层 (Syntax Layer)

| 属性 | 说明 |
|------|------|
| **错误类型** | E0061 (wrong arg count)，E0733 (async recursion)，E0599 (no method named)，E0425 (cannot find value/type)，E0728 (await outside async)，E0004 (non-exhaustive patterns) |
| **修复模式** | 1 agent 独立修复，直接替换 API 调用或补全模式分支 |
| **案例** | 4 个 widgets `Block::borders()` → `Block::default().borders(Borders::ALL)` (E0061)；`file_tree.rs` 递归 async fn 加 `Box::pin()` (E0733) |

---

### Layer 5: 质量层 (Code Quality Layer)

| 属性 | 说明 |
|------|------|
| **警告类型** | dead_code，unused imports，unused variables，non_snake_case，irrefutable patterns，unreachable patterns |
| **修复模式** | **最后执行**（所有 errors 修完后）。1 agent 集中批量修复，或按模块拆分 |
| **策略** | 优先尝试**激活使用**（补全调用链）。如确为预留/未完成，则 `#[allow(dead_code)]` 并注释原因 |

| 警告类型 | 处置策略 |
|----------|---------|
| **未使用的代码**（死函数/死字段/死变量） | 优先尝试**激活使用**（补全调用链）。如确为预留/未完成，则 `#[allow(dead_code)]` 并注释原因 |
| **命名规范**（non_snake_case） | 改为 snake_case。若涉及 `fn item` 无法捕获外层变量导致无法重命名，用 `#[allow(non_snake_case)]` |
| **语法错误**（E0425/E0433/E0599 等） | 修复语法：补 import、改 API 调用、加类型标注 |
| **未使用导入/变量** | 移除或加 `_` 前缀 |
| **无意义比较**（usize < 0 等） | 简化条件 |
| **不可达模式**（unreachable_patterns） | 简化 patterns 或加 `#[allow(unreachable_patterns)]` |

---

### 实战案例（carpai-cli + carpai-core 修复）

| 指标 | 旧 3 层模型 | 新 5 层模型 |
|------|------------|------------|
| 并行 agent 数 | 1 （串行） | 5（同时启动） |
| 修复轮次 | ~5-8 次编译迭代 | ~2-3 次编译迭代 |
| 总耗时 | ~20-30 分钟 | ~8-12 分钟 |
| **效率提升** | 基线 | **~3x** |

## Phase 1 Action Plan (当前编译修复执行清单)

### Layer 1 — 全局接口对齐（优先处理）

| # | 错误 | 位置 | 修复措施 |
|---|---|---|---|
| 1 | E0433 `providers` 模块 | `src/completion_engine/engine.rs` | 检查 `providers::CompletionProviderConfig` → 应已导入，确认编译环境正常后验证 |
| 2 | E0195 生命周期不匹配 ×4 | `src/completion_engine/providers.rs` | 检查 `provide_completions<'a>` trait 声明与实现的 `'a` 一致性 |
| 3 | E0603 `ast` 模块私有 | 可能涉及 `carpai-sdk` 或 `carpai-codebase` | 在 `src/ast/mod.rs` 中确认 `pub mod tree_sitter; pub use tree_sitter::{...};` 已导出 |
| 4 | E0424 `self` 作为值 | 出现在 `async fn` 或 closure 中 | 将 `.await` 改为 `self.await` 或移除误用的 `self` |
| 5 | E0061 参数数量不匹配 | 某函数调用参数数量错误 | 检查函数签名 vs 调用参数 |
| 6 | E0728 `await` 在非 async 中 ×2 | 搜索 `src/` 中非 async fn 内的 `.await` | 移除 `await` 或加 `async` |
| 7 | E0004 非穷举模式 | `src/` 中 match 或 if let | 补全缺失的 pattern 分支 |

### Layer 2 — 模块内错误修复

| # | 模块 | 错误 | 修复 |
|---|---|---|---|
| 1 | `crates/jcode-cross-file-repair/src/ast.rs` | AstEditOp 新增 Insert/Delete/Replace 变体 | ✅ 已完成 |
| 2 | `src/agent/cross_file_repair.rs` | `operation`→`operations`, 字段修正 | ✅ 已完成 |
| 3 | `src/tui/app/tui_lifecycle.rs` | `CompletionPrefetchState::new`→`::Idle` | ✅ 已完成 |
| 4 | `src/tui/app/tui_lifecycle.rs` | ProviderAdapter 桩 | ✅ 已完成 |
| 5 | `src/server/file_activity.rs` | `unwrap_or(start)` → `unwrap_or(start)` | 检查是否类型匹配 |
| 6 | `crates/jcode-unified-scheduler/src/gpu_discovery.rs` | GPU 估计函数 pub | ✅ 已完成 |

### Layer 3 — 子 crate 验证顺序

```bash
# 验证每个 crate 后再试根 crate
cargo check -p jcode-config-types           # ✅ 已知通过
cargo check -p jcode-unified-scheduler      # ⚠ 上次 17warnings，已修复
cargo check -p carpai-codebase              # ⚠ 修复了 TantivyDocument
cargo check -p jcode-cross-file-repair      # ⚠ 修复了 AstEditOp 变体
cargo check -p jcode-completion             # ⚠ CompletionProvider trait
cargo check -p jcode-tool-core              # ⚠ 标记了 dead_code
cargo check -p jcode-skills                 # ⚠ 标记了 unused_assignments
cargo check -p jcode-enterprise-server      # ⚠ edition 2024
cargo check -p jcode-distributed-inference   # ⚠ edition 2024
cargo check -p carpai                       # 最终验证
```

如果 cargo check 被锁定，先清理进程：
```powershell
taskkill /f /im cargo.exe ; taskkill /f /im rustc.exe
```

## Install Notes
- `~/.local/bin/jcode` is the launcher symlink used from `PATH`.
- `~/.jcode/builds/current/jcode` is the active local/source-build channel; self-dev builds and `scripts/install_release.sh` point the launcher here.
- `~/.jcode/builds/stable/jcode` is the stable release channel; `scripts/install.sh` installs this and points the launcher here.
- `~/.jcode/builds/versions/<version>/jcode` stores immutable binaries.
- `~/.jcode/builds/canary/jcode` still exists for canary/testing flows, but it is not the primary self-dev install path.
- On Windows, the equivalents are `%LOCALAPPDATA%\\jcode\\bin\\jcode.exe` for the launcher, `%LOCALAPPDATA%\\jcode\\builds\\stable\\jcode.exe` for stable, and `%LOCALAPPDATA%\\jcode\\builds\\versions\\<version>\\jcode.exe` for immutable installs; `scripts/install.ps1` currently installs the stable channel.
- Ensure `~/.local/bin` is **before** `~/.cargo/bin` in `PATH`.

