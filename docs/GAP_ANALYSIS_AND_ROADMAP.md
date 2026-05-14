# CarpAI 深度差距分析与开发任务清单

> 生成日期: 2026-05-14
> 对比对象: Claude Code CLI / Cursor IDE

---

## 第一部分：CarpAI vs Claude Code CLI 深度差距分析

### 1.1 命令体系数量对比

| 维度 | CarpAI | Claude Code | 差距 |
|------|--------|-------------|------|
| 顶层命令 | **33** | ~100+ | ❌ 缺 70+ |
| 命令实现文件 | 1 (`commands.rs`) | 189 文件 (`src/commands/`) | 单体 vs 模块化 |
| 工具 (Tool) 实现 | 30+ | 227+ | ❌ 缺 ~200 |
| 钩子 (Hook) 文件 | 0 | 104 | ❌ 全缺 |
| 服务 (Service) 文件 | 少量 | 150 | ❌ 缺大部分 |
| 工具函数 (Utils) | 少量 | 500+ | ❌ 缺大部分 |

### 1.2 命令级缺失对比

#### CarpAI 已有 (33 个)
```
serve, connect, run, login, repl, update, version, usage, selfdev,
debug, auth, provider, memory, session, ambient, pair, permissions,
transcript, dictate, setup-hotkey, setup-launcher, browser, replay,
model, auth-test, build, mcp, doctor, init, code-value, skills,
workflows, tasks, git, config, commit, session-mgmt, rethink,
compact, fork, completion
```

#### Claude Code 有而 CarpAI 无的关键命令 (不完全列表)

| 缺失命令 | Claude 文件数 | 功能价值 | 优先级 |
|---------|-------------|---------|--------|
| `review` | 多个源文件 | 代码审查（AI 驱动） | P0 |
| `security-review` | 1 | 安全审查 | P1 |
| `commit-push-pr` | 多个 | 一键提交+推送+PR | P1 |
| `pr_comments` / `autofix-pr` | 多个 | PR 评论与自动修复 | P1 |
| `install-github-app` | 13 文件 | GitHub App 集成 | P2 |
| `install-slack-app` | - | Slack 集成 | P2 |
| `files` / `add-dir` / `rename` / `copy` | 多个 | 文件管理命令 | P1 |
| `env` | 多个 | 环境变量管理 | P2 |
| `effort` / `fast` / `passes` | 多个 | 执行策略控制 | P1 |
| `rate-limit-options` | - | 速率限制配置 | P3 |
| `insights` | 1 (113KB) | 会话分析报告 | P1 |
| `tag` / `summary` | 多个 | 会话标签/摘要 | P2 |
| `buddy` | 目录 | 结对编程模式 | P2 |
| `voice` | 目录 | 语音模式 | P3 |
| `moreRight` | 目录 | MoreRight 集成 | P3 |
| `logout` / `upgrade` | - | 登出/升级 | P2 |
| `export` (session) | 1 | 会话导出增强 | P1 |

### 1.3 工具 (Tool) 体系差距

CarpAI 已有 30+ 工具:
```
read, write, edit, multiedit, patch, apply_patch, glob, grep, bash,
browser, open, webfetch, websearch, codesearch, subagent, mcp,
side_panel, agentgrep, (MCP-proxied tools) ...
```

**关键缺失工具** (Cursor/Claude Code 有而 CarpAI 无):

| 缺失工具 | 功能 | 优先级 |
|---------|------|--------|
| **`multi_file_edit`** | 跨文件批量编辑（感知依赖） | **P0** |
| **`ast_search`** | AST 级别代码搜索（非文本 grep） | **P0** |
| **`symbol_refactor`** | 符号重命名/提取/移动 | **P0** |
| **`debug_integration`** | 调试器集成（断点/单步/变量） | **P0** |
| **`file_diff`** | 文件差异可视化 | P1 |
| **`git_commit_tool`** | 带 diff 预览的提交工具 | P1 |
| **`test_runner`** | 测试运行与结果解析 | P1 |
| **`project_map`** | 项目结构地图 | P1 |
| **`dependency_graph`** | 依赖关系图 | P2 |
| **`code_explain`** | 代码解释（LLM+AST） | P2 |
| **`performance_profile`** | 性能分析 | P3 |

### 1.4 架构差距

| 维度 | CarpAI | Claude Code | 影响 |
|------|--------|-------------|------|
| 命令注册 | 单一 `commands.rs` (138KB) | 每个命令独立文件 | **CarpAI 可维护性差** |
| 工具注册 | 单一 `tool/mod.rs` | `tools.ts` + 227 独立文件 | 同上 |
| Hook 系统 | ❌ 无 | 104 文件 | 缺少 IDE 事件响应 |
| LSP 集成 | ⚠️ 基础 (`jcode-lsp`) | 深度集成 | 缺少智能提示 |
| IDE 桥接 | ⚠️ 骨架 | `src/hooks/*` 全实现 | 缺少 IDE 上下文感知 |

### 1.5 性能差距 (估算)

| 场景 | CarpAI | Claude Code | 差距 |
|------|--------|-------------|------|
| CLI 启动时间 | ~50ms | ~200ms (TS) | **CarpAI 更快** 🏆 |
| 工具执行延迟 | ~5ms (Rust) | ~10ms (TS) | **CarpAI 更快** 🏆 |
| LLM 推理 | 相同 API | 相同 API | 持平 |
| 大文件处理 | 流式读写 | 流式读写 | 持平 |
| AST 操作 | ❌ 无 | ⚠️ 基础 | **CarpAI 缺失** |
| 多文件重构 | 逐个文件 | 批量+感知 | **CarpAI 缺失** |

---

## 第二部分：CarpAI vs Cursor 功能差距分析

### 2.1 Cursor 核心能力概述

Cursor 作为 IDE 的核心能力集中在以下 5 个领域：

| 能力 | Cursor 实现方式 | CarpAI 状态 |
|------|----------------|------------|
| **多文件重构效率** | Tab 补全 + Multi-cursor + AI Edit (Ctrl+K) | ❌ 仅单文件 edit |
| **代码 Debug 效率** | 内置调试器 + AI 断点建议 + 变量解释 | ❌ 无 |
| **AST 代码索引** | tree-sitter + LSP 全文索引 | ⚠️ 基础 grep |
| **引用跳转** | LSP Go-to-Definition + Find References | ❌ 无 |
| **大规模重构** | Rename Symbol + Extract Method + Move File | ❌ 无 |

### 2.2 逐项差距详情

#### 2.2.1 多文件重构效率

| 特性 | Cursor | CarpAI | 差距 |
|------|--------|--------|------|
| 多文件同时编辑 | ✅ Ctrl+Shift+Enter 多光标 | ❌ 单文件串行 | **根本差距** |
| 跨文件感知 | ✅ 知道引入依赖 | ❌ 无感知 | **根本差距** |
| AI 批量编辑 | ✅ "Chat → Apply" 流 | ⚠️ `multiedit` 工具 | **半成品** |
| 重构预览 | ✅ Diff 视图 + Accept/Reject | ❌ 直接写入 | **风险差距** |

#### 2.2.2 代码 Debug 效率

| 特性 | Cursor | CarpAI | 差距 |
|------|--------|--------|------|
| 断点设置 | ✅ UI 点击 | ❌ 无 | **全缺失** |
| 单步执行 | ✅ | ❌ | **全缺失** |
| 变量查看 | ✅ Hover | ❌ | **全缺失** |
| AI 断点建议 | ✅ | ❌ | **全缺失** |
| 运行时日志 | ❌ (外部工具) | ✅ shell tool | 间接 |

#### 2.2.3 AST 级别代码索引

| 特性 | Cursor | CarpAI | 差距 |
|------|--------|--------|------|
| 语法树索引 | ✅ tree-sitter | ❌ 无 | **全缺失** |
| 符号搜索 | ✅ `@` 符号搜索 | ❌ 仅文本 grep | **全缺失** |
| 语义理解 | ✅ 类型感知 | ❌ 无 | **全缺失** |
| 代码高亮 | ✅ | ❌ CLI 模式 | N/A |
| 错误提示 | ✅ LSP 诊断 | ❌ 外部 cargo check | **体验差距** |

#### 2.2.4 引用跳转

| 特性 | Cursor | CarpAI | 差距 |
|------|--------|--------|------|
| Go-to-Definition | ✅ Ctrl+Click | ❌ 无 | **全缺失** |
| Find References | ✅ Shift+F12 | ⚠️ `grep` 文本搜索 | **体验差距** |
| 类型跳转 | ✅ | ❌ | **全缺失** |
| 调用层级 | ✅ | ❌ | **全缺失** |

#### 2.2.5 大规模重构

| 特性 | Cursor | CarpAI | 差距 |
|------|--------|--------|------|
| Rename Symbol | ✅ 自动更新所有引用 | ❌ 手动 | **全缺失** |
| Extract Method | ✅ | ❌ | **全缺失** |
| Move File | ✅ 自动更新导入 | ❌ | **全缺失** |
| 安全保证 | ✅ 编译检查 + 预览 | ❌ 直接写入 | **风险差距** |

---

## 第三部分：开发任务清单

### P0 — 必须优先实现（对标 Cursor 核心能力）

#### [P0.1] CLI 代码导航命令（基于已有 LSP 基础设施）

```
状态：jcode-lsp 基础设施已存在（tree-sitter + LSP client + AST操作）
      src/tool/lsp.rs — LSP 工具已注册到 Agent
需要新增：CLI 命令层，让用户可以直接在终端中使用代码导航

新增命令：
  carpai code-nav goto-def <file>:<line>:<col>
  carpai code-nav find-refs <file>:<line>:<col>
  carpai code-nav hover <file>:<line>:<col>
  carpai code-nav symbols [<file>]
  carpai code-nav search <query>
  carpai code-nav impl <file>:<line>:<col>
  carpai code-nav callers <file>:<line>:<col>
  carpai code-nav callees <file>:<line>:<col>

实现组件：
  └── src/commands/code_nav.rs    ─ 代码导航 CLI 命令（包装 jcode_lsp API）

工作量估算：3-5 天（已有基础设施，只需加 CLI 命令）
依赖：jcode-lsp crate (已存在)
```

#### [P0.2] 重构 CLI 命令（基于已有 AST 操作基础设施）

```
状态：jcode-lsp 已有 ast_operations.rs (37KB) 包含:
      - RenameSymbolParams → 符号重命名
      - ExtractMethodParams → 提取方法
      - InlineFunctionParams → 内联函数
      - CodeEditResult → 编辑结果
      - FormatCodeEngine → 代码格式化
      AST Operations 已通过 LspOperations trait 可用
需要新增：CLI 命令层 + 安全应用机制（diff preview + accept/reject）

新增命令：
  carpai refactor rename <old-name> <new-name> [--file <path>] [--dry-run]
  carpai refactor extract-method <file>:<start>-<end> --name <method-name>
  carpai refactor inline <file>:<line>:<col>
  carpai refactor format [<file>...] [--check]
  carpai refactor diff <file>          ─ 显示重构预览差异

实现组件：
  ├── src/commands/refactor.rs       ─ 重构 CLI 命令
  └── src/tool/refactor_preview.rs   ─ Diff 预览 + 安全确认机制

工作量估算：1-2 周（已有 AST 操作，只需加 CLI 层）
依赖：jcode-lsp crate (已存在)
```

#### [P0.3] 调试器集成

```
目标：在 CLI 中提供基本调试能力（断点/单步/LOCAL 变量）

实现组件：
  ├── crates/jcode-debugger/
  │   ├── Cargo.toml               ─ DAP (Debug Adapter Protocol) 客户端
  │   ├── src/lib.rs
  │   ├── src/dap_client.rs        ─ DAP 协议客户端
  │   ├── src/breakpoint.rs        ─ 断点管理
  │   ├── src/stack_trace.rs       ─ 堆栈追踪
  │   └── src/variable_view.rs     ─ 变量查看
  ├── src/tool/debug.rs            ─ Tool: debug (设置/跳过/继续)
  └── src/commands/debug_cli.rs    ─ CLI 调试命令 (暂不实现 TUI)

工作方式：
  - 启动项目自带的调试配置
  - DAP over stdio 连接调试器
  - CLI 中显示堆栈/变量

工作量估算：3-4 周
依赖：P0.1 (用于断点位置解析)
```

---

### P1 — 重要功能（对标 Claude Code 核心命令）

#### [P1.1] 代码审查系统

```
新命令：carpai review [--staged] [--diff <ref>] [--security]
       carpai security-review

实现：
  ├── src/commands/review.rs       ─ review / security-review 命令
  └── src/commands/pr_comments.rs  ─ PR 评论集成

功能：
  - 分析 git diff / staged 变更
  - 按文件/严重程度分类问题
  - 安全审查专用模式
  - 输出格式：tabular / JSON

工作量估算：1-2 周
```

#### [P1.2] 一键 PR 工作流

```
新命令：carpai commit-push-pr

实现：
  ├── src/commands/commit_push_pr.rs

功能：
  - git add → git commit (AI message) → git push → gh pr create
  - 交互式确认每个步骤
  - 支持 PR 模板

工作量估算：1 周
```

#### [P1.3] 执行策略控制

```
新命令：carpai effort [auto|conserve|high]
       carpai fast [on|off]
       carpai passes [1-10]

实现：
  └── src/commands/effort.rs + config 扩展

功能：
  - effort: 控制 LLM 推理深度
  - fast: 跳过非关键工具调用
  - passes: 设置自动迭代次数

工作量估算：1 周
```

#### [P1.4] 会话分析报告

```
新命令：carpai insights [session-id]

实现：
  └── src/commands/insights.rs

功能：
  - 分析 Token 消耗趋势
  - 工具使用频率统计
  - 错误模式识别
  - 输出：Markdown / JSON / HTML

工作量估算：1 周
```

---

### P2 — 增强功能

#### [P2.1] 文件管理命令

```
carpai files [--type <ext>] [--modified <since>]
carpai add-dir <path>
carpai rename <old> <new>
carpai copy <src> <dest>

工作量估算：3-5 天
```

#### [P2.2] GitHub/Slack 集成

```
carpai install-github-app
carpai install-slack-app

工作量估算：2 周
```

#### [P2.3] 会话标签与摘要

```
carpai tag <key>=<value>
carpai summary [--export]

工作量估算：3 天
```

#### [P2.4] 结对编程模式

```
carpai buddy [on|off|share]

工作量估算：2 周
```

---

### P3 — 长期演进

| 任务 | 说明 | 优先级 | 估算 |
|------|------|--------|------|
| 语音模式 | 语音输入/输出集成 | P3 | 3 周 |
| Vim 模式 | Vim 键位绑定 | P3 | 1 周 |
| 更多传输层 | MCP WebSocket 完整实现 | P3 | 1 周 |
| 插件市场 | 插件发现/安装/更新 | P3 | 3 周 |
| 远程开发 | SSH 远程会话优化 | P3 | 2 周 |

---

## 第四部分：总体评分

| 维度 | CarpAI | Claude Code | Cursor | 说明 |
|------|--------|-------------|--------|------|
| **CLI 命令广度** | 33/100 | 100+ | 50+ | 缺批量重构、审查|
| **工具体系** | 30+/227 | 227+ | 100+ | 缺 AST 工具 |
| **AST 索引** | 60% | 10% | 90% | ✅ 已有 `jcode-lsp` + tree-sitter + LSP client |
| **多文件重构** | 40% | 30% | 90% | ✅ 已有 `ast_operations.rs`，缺 CLI 命令 |
| **调试器** | 0% | 5% | 85% | ❌ 全缺，DAP 客户端需新建 |
| **LLM 能力** | 95% | 100% | 100% | 使用相同 API |
| **架构优雅性** | 70% | 70% | 60% | 持平 |
| **性能** | 150% | 100% | 80% | **Rust 优势** 🏆 |

---

## 第五部分：执行路线图

```
Q2 2026 (当前 — 已有 jcode-lsp 基础设施)
  ├── [1-2 天] P0.1 代码导航 CLI 命令  carpai code-nav goto-def ...
  ├── [1 周]   P0.2 重构 CLI 命令      carpai refactor rename ...
  └── [1 周]   P1.1 代码审查系统        carpai review ...

Q3 2026
  ├── [3-4 周] P0.3 调试器集成 (DAP 客户端)
  ├── [1 周]   P1.2 一键 PR 工作流
  └── [1 周]   P1.3 执行策略控制

Q4 2026
  ├── P1.4 会话分析报告
  └── P2.x 文件管理/GitHub 集成

2027
  └── P3 长期演进
```

### 关键里程碑

| 里程碑 | 交付物 | 时间估计 | 已有基础 |
|--------|--------|---------|---------|
| M1 - 代码导航 CLI | `carpai code-nav goto-def/find-refs` 等 | **1-2 天** | ✅ `jcode-lsp` + `LspOperations` trait |
| M2 - 重构 CLI | `carpai refactor rename/extract` 等 | **1 周** | ✅ `ast_operations.rs` (37KB) |
| M3 - 代码审查 | `carpai review --staged` | 1 周 | ⚠️ 需新建 |
| M4 - 调试 MVP | 断点设置 + 单步 CLI | 3-4 周 | ❌ DAP 客户端需新建 |
| M5 - 批量编辑 | 多文件 Diff Preview + Apply | 2-3 周 | ⚠️ `multiedit` 工具已有基� |
