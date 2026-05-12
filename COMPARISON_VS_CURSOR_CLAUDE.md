# jcode vs Cursor vs Claude Code — 单机编程能力对标

> 数据来源：jcode 完整源代码阅读 + Claude Code CLI 1500+ 文件源码分析 + Cursor 公开文档/2026 评测

## 一、总览热力图

```
                                    Cursor          Claude Code     jcode           最大差距
代码理解 (LSP级)       ████████████  ████████████  ████░░░░░░░░    jcode 落后 ≈66%
代码编辑 (AST级)       ████████████  ████████████  ███░░░░░░░░░    jcode 落后 ≈75%
Agent 能力             ████████████  ████████████  █████████░░░    jcode 落后 ≈12%
调试与测试             ████████████  ████████████  ████████░░░░    jcode 落后 ≈25%
Git 工作流             ████████████  ████████████  ████████░░░░    jcode 落后 ≈25%
终端与 Shell           ████████████  ████████████  ███████████░    jcode 落后 ≈5%
记忆与上下文           ████████████  ██████████░░  ██████████░░    jcode ≈ Claude Code
多模型编排             ██████░░░░░░  ████░░░░░░░░  ███████████░    jcode > Cursor 大
MCP 生态               ████████████  ████████████  ████████░░░░    jcode 落后 ≈20%
迁移与打包             ████████████  ██████░░░░░░  ████████░░░░    jcode > Claude Code
国产化兼容             ░░░░░░░░░░░░  ░░░░░░░░░░░░  ████████░░░░    jcode 唯一优势
```

---

## 二、6 大维度逐项对比

### 维度 1：代码理解 (LSP 级符号操作)

| 能力 | Cursor | Claude Code | jcode |
|------|--------|-------------|-------|
| Go to Definition | ✅ IDE 原生 LSP | ✅ 通过 Claude MCP LSP | ⚠️ LSP client 框架就绪，stdio 刚实现 |
| Find References | ✅ IDE 原生 | ✅ | ✅（同） |
| Hover | ✅ 实时悬浮 | ✅ 通过 /ask | ✅ `lsp_provider.rs` 有真实 stdio |
| Completion | ✅ Supermaven 实时补全 | ❌ 无 | ❌ `lsp_provider.rs` 有实现但未接入主流程 |
| Document Symbols | ✅ IDE 原生 | ✅ | ⚠️ 框架就绪 |
| Workspace Symbols | ✅ | ✅ | ⚠️ 框架就绪 |
| Diagnostics inline | ✅ 编辑器内直接显示 | ❌ 只有终端输出 | ❌ LspDiagnosticRegistry 有框架 |
| Rename Symbol (refactor) | ✅ IDE 原生 | ⚠️ 通过模型理解 | ⚠️ `client.rs` 有实现但 `lsp` tool 未暴露 |
| Call Hierarchy | ✅ | ⚠️ | ⚠️ 框架就绪 |
| 多语言支持 | 不限 (VS Code 生态) | TypeScript/JS/Rust/Python/Go/Cpp/Java | 同 Claude Code (12 种) |
| **综合评分** | **9.5/10** | **7/10** | **4/10** |

**jcode 差距**: 框架完整但 40 个 gRPC LSP RPC 和 `src/tool/lsp.rs` 仍是 stub，尚未连接到真实 LSP server。

---

### 维度 2：代码编辑 (AST 级变换)

| 能力 | Cursor | Claude Code | jcode |
|------|--------|-------------|-------|
| 文本替换编辑 | ✅ IDE 原生 | ✅ Edit tool (string replace) | ✅ edit, multiedit, patch 工具 |
| 正则感知替换 | ✅ | ✅ | ✅（同 Claude Code） |
| Unified diff 应用 | ✅ | ✅ patch tool | ✅ patch, apply_patch |
| 多文件编辑 | ✅ Composer | ✅ 通过 Agent 批量 | ✅ multi-file-edit 引擎 (原子提交) |
| **提取函数** | ✅ | ✅ | ❌ 无 |
| **内联函数** | ✅ | ✅ | ❌ 无 |
| **重命名符号 (AST)** | ✅ IDE 原生 | ⚠️ rename tool | ⚠️ client.rs 有 rename_symbol 但未暴露 |
| **移动符号** | ✅ | ❌ | ❌ |
| **安全插入导入** | ✅ 自动 | ⚠️ 模型生成 | ⚠️ cross-file-repair 定义了 AddImport |
| **类型感知重构** | ✅ | ⚠️ | ❌ |
| 跨文件修复（自修正） | ❌ | ⚠️ | ✅ cross-file-repair 引擎 + 3 轮修正循环 |
| **真实 AST 解析器** | `tree-sitter` by VS Code | tree-sitter | ❌ **只有正则模拟** |
| **综合评分** | **9/10** | **7/10** | **3/10** |

**jcode 差距**: 没有集成 `syn` (Rust) 或 `tree-sitter` (多语言)。`AstAdapter` trait 定义了结构但没有实现。所有"AST 操作"是正则模拟。

---

### 维度 3：Agent 能力

| 能力 | Cursor | Claude Code | jcode |
|------|--------|-------------|-------|
| 基础 ReAct 循环 | ✅ Agent Mode | ✅ | ✅ `jcode-agent-advanced` |
| 并行工具执行 | ✅ | ✅ streaming executor | ✅ `StreamingToolExecutor` |
| 子 Agent 编排 | ✅ Agent spawn | ✅ Agent tool / SendMessage | ✅ `SubAgentPool` |
| **多 Agent 群组** | ✅ Agents Window (3.0) | ✅ Agent Teams / Swarm | ⚠️ `jcode-swarm-core` 但缺少 Team 接口 |
| **Coordinator 模式** | ❌ | ✅ Coordinator Mode | ⚠️ 从 Claude Code 移植了核心类型但未集成 |
| 错误恢复 | ✅ | ✅ 指数退避 + 模型降级 | ✅ 移植完毕 |
| 模型降级 | ⚠️ | ✅ FallbackChain | ✅ 移植完毕 |
| 任务队列/调度 | ✅ | ✅ `/loop` cron 任务 | ⚠️ `scheduler.rs` 但缺 cron 解析 |
| **Worktree 隔离** | ✅ 3.0 `/worktree` | ✅ | ❌ |
| 自动修复循环 | ✅ | ✅ | ✅ SelfCorrectionLoop (3 轮) |
| 人工产物 | ✅ Background Agent | ✅ | ✅ `SubAgentResult` |
| **综合评分** | **8/10** | **9/10** | **7.5/10** |

**jcode 差距**: 大部分 Agent 能力已从 Claude Code 移植，但 **Agent Teams** / **Coordinator 模式集成** / **cron 调度** 尚未完成。

---

### 维度 4：调试与测试

| 能力 | Cursor | Claude Code | jcode |
|------|--------|-------------|-------|
| Terminal Debugger | ✅ 内嵌调试器 | ❌ 仅 CLI | ❌ |
| 断点/单步 | ✅ VS Code debug | ❌ | ❌ |
| **测试生成** | ✅ | ✅ `generate_tests` RPC | ✅ `JoyCodeService::generate_tests` |
| **测试运行** | ✅ 编辑器内 | ✅ Bash tool `cargo test` | ✅ `run_tests` tool |
| Test Explorer | ✅ | ❌ | ❌ |
| 代码覆盖率 | ✅ 集成 | ⚠️ 通过第三方 | ⚠️ jcode-telemetry 有资源监控 |
| Lint 内联 | ✅ 实时 | ❌ | ❌ |
| **本地 CI (pre-push)** | ❌ | ❌ | ✅ `jcode-micro-ci` (AST + type + AI 三层) |
| AI 修复建议 | ✅ | ✅ | ✅ `SelfCorrectionLoop` |
| **综合评分** | **8/10** | **6/10** | **6.5/10** |

**jcode 亮点**: `jcode-micro-ci` 是本地 pre-push CI，Cursor 和 Claude Code 都不具备。

---

### 维度 5：Git 工作流

| 能力 | Cursor | Claude Code | jcode |
|------|--------|-------------|-------|
| 差异对比 | ✅ 内嵌 | ✅ Diff | ✅ similar::TextDiff |
| 暂存/提交 | ✅ 原生 | ✅ commit / commit-push-pr | ✅ `commit` tool |
| **分支管理** | ✅ 原生 | ✅ branch tool | ✅ branch tool |
| **PR 创建** | ✅ | ✅ commit-push-pr | ❌ |
| **Code Review** | ✅ 内嵌 | ✅ review tool | ✅ review tool (gRPC) |
| **变更归因** | ❌ | ✅ `commitAttribution.ts` | ✅ `CommitAttributionTracker` 已移植 |
| **回滚 (file history)** | ✅ VS Code 本地历史 | ✅ `fileHistory.ts` | ✅ `FileHistory` 已移植 |
| Worktree 管理 | ✅ 3.0 `/worktree` | ✅ worktree tool | ❌ |
| **综合评分** | **9/10** | **8/10** | **6/10** |

**jcode 差距**: 缺少 PR 创建和 Worktree 管理。

---

### 维度 6：终端与 Shell

| 能力 | Cursor | Claude Code | jcode |
|------|--------|-------------|-------|
| 内嵌终端 | ✅ | ❌ CLI only | ✅ TUI 终端 |
| Shell 执行 | ✅ terminal | ✅ Bash tool | ✅ bash, bg tool |
| Shell 沙箱 | ❌ | ❌ | ✅ `CommandSandbox` |
| **PowerShell 支持** | ✅ | ⚠️ 有限 | ✅ Windows 支持 |
| **SSH 远程** | ✅ Remote SSH | ✅ CCR v2 | ✅ `RemoteSessionManager` |
| **WebSocket 协议** | ❌ | ✅ SSE + Hybrid | ✅ `BridgeMain` |
| 信号处理 (SIGINT) | ✅ | ✅ | ✅ tokio::signal |
| 进程管理 (bg/spawn) | ✅ | ⚠️ | ✅ spawn_detached / bg tool |
| 环境变量管理 | ✅ | ✅ env command | ✅ env command |
| **综合评分** | **8/10** | **8/10** | **8.5/10** |

**jcode 领先**: 终端沙箱（`CommandSandbox` + `SsrfGuard` + `PermissionEngine`）是 Cursor 和 Claude Code 都没有的独有能力。

---

## 三、jcode 最大短板 & 最大优势

### 最大短板的修复优先级

| 短板 | 差距 | 修复工作量 | 建议路线 |
|------|------|-----------|---------|
| **LSP 未连真实 Server** | -66% | 2-3 天 | 给 `client.rs::send_request` 补完 stdio 读写，删除 3 套冗余实现 |
| **无 AST 解析器** | -75% | 1-2 周 | 集成 `syn` (Rust) + `tree-sitter` (多语言) 实现 `AstAdapter` |
| **无 PR/Worktree** | -25% | 3-5 天 | `gh` CLI 封装 |
| **Agent Teams 未集成** | -12% | 3-5 天 | 将 Coordinator Mode 提示词和 Team 接口集成到 `jcode-swarm-core` |

### 最大优势（Curson / Claude Code 没有的）

```
jcode-micro-ci         ✅ 本地三层 pre-push CI（独有）
CommandSandbox         ✅ 命令执行沙箱（独有）
SsrfGuard              ✅ SSRF 防护（独有）
SelfCorrectionLoop     ✅ 跨文件自修正 3 轮循环（独有）
cross-file-repair      ✅ 带依赖传播的跨文件修复（独有）
国产化支持             ✅ KylinOS selinx + .deb/.rpm（独有）
多模型编排             ✅ 9 个 provider 统一抽象（领先）
7:Telemetry            ✅ OTel Metrics + 成本追踪 + 资源监控（领先）
```

## 四、总结雷达图

```
                 代码理解 (LSP)
                   10
                  /  \
                 /    \
          迁移   8      代码编辑 (AST)
          /               \
         /                 \
       6                    \
国产化  ----- jcode -------   Agent
       \    Cursor .......   /
        \   Claude Code --- /
         4                  /
           \              /
            \            /
             MCP      调试与测试
              生态       \
                         \
                          Git 工作流
```

jcode 在 **Agent 能力**、**终端与 Shell**、**多模型编排**、**国产化** 方面追平或领先竞品；但在 **LSP 代码理解** 和 **AST 编辑** 两个核心 IDE 能力上仍有巨大差距，这是接下来最值得投入的方向。
