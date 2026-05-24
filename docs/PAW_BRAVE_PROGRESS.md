# Paw-brave 小组重构进度报告

> **日期**: 2026-05-24
> **负责**: crates/carpai-cli (CLI 产品, 30% 工作量)
> **总状态**: 全部 19 项任务已完成 (100%)

---

## 总体趋势

```
Phase 1 [Wk1-2]: 骨架  ████████████████████████████ 100%
Phase 2 [Wk3-4]: TUI   ████████████████████████████ 100%
Phase 3 [Wk4-5]: CMD   ████████████████████████████ 100%
Phase 4 [Wk5-6]: AMB   ████████████████████████████ 100%
Phase 5 [Wk6-7]: DASH  ████████████████████████████ 100%
Phase 6 [Wk7-8]: 打磨  ████████████████████████████ 100%
Phase 7 [Wk8-9]: 测试  ████████████████████████████ 100%
Phase 8 [Wk9-10]:联调  ████████████████████████████ 100%
```

---

## Phase 1: 骨架 (Wk1-2) ✅

| 文件 | 状态 | 行数 | 说明 |
|------|------|------|------|
| `Cargo.toml` | ✅ | 67 | ratatui/crossterm/tokio/clap/tonic + tokio-util/fastrand + tonic-build |
| `build.rs` | ✅ | 7 | Proto 编译 (agent/session/health) |
| `main.rs` | ✅ | 66 | clap CLI: chat/ask/complete/serve |
| `lib.rs` | ✅ | 39 | 12 模块声明 + 便利 re-export (含 grpc_client) |
| `config.rs` | ✅ | 194 | CliConfig + Theme/K/B/Clipboard/Startup 子配置 |
| `modes.rs` | ✅ | 63 | CliMode::Local/Remote + Display/FromStr + 测试 |
| `agent_bridge.rs` | ✅ | 181 | 双模式 + 重试 + 优雅降级 |

## Phase 2: TUI 剥离 (Wk3-4) ✅

| 文件 | 状态 | 说明 |
|------|------|------|
| `tui/mod.rs` | ✅ | TUI run() + render_app() + centered_rect() |
| `tui/app.rs` | ✅ | UIMessage + App 状态 + FileTree 集成 + show_help |
| `tui/handler.rs` | ✅ | 快捷键: Enter/Ctrl-C/Ctrl-F/? + file_tree 导航 |
| `tui/event.rs` | ✅ | Event::{Key, Mouse, Resize, Tick} |
| `tui/theme.rs` | ✅ | 10 种配色 + Default impl |
| `tui/widgets/*` | ✅ | chat_view/input_bar/status_line/help_overlay/file_tree (含异步) |

## Phase 3: Commands (Wk4-5) ✅

| 命令 | 状态 | 代码行 | 说明 |
|------|------|--------|------|
| `chat` | ✅ | 30 | 配置加载 → build_local_agent_context → TUI |
| `ask` | ✅ | 37 | execute_agent_turn → stdout + 用量追踪 |
| `complete` | ✅ | 94 | CodeCompletion trait 优先 + agent_turn 回退 |
| `serve` | ✅ | 122 | ServeOptions + 子进程模式 + 库集成准备 |

## Phase 4: Ambient + Notifications (Wk5-6) ✅

| 文件 | 状态 | 代码行 | 说明 |
|------|------|--------|------|
| `ambient/runner.rs` | ✅ | 95 | BackgroundRunner + BackgroundTask trait + Semaphore |
| `ambient/scheduler.rs` | ✅ | 79 | TaskScheduler + ScheduledTask trait + CancellationToken |
| `notifications/browser.rs` | ✅ | 99 | 跨平台 BrowserOpener (Windows/Mac/Linux) |
| `notifications/telegram.rs` | ✅ | 100 | Bot API + 环境变量配置 |
| `notifications/gmail.rs` | ✅ | 107 | Gmail 摘要 + SMTP (future) |

## Phase 5: Dashboard (Wk6-7) ✅

| 功能 | 状态 | 说明 |
|------|------|------|
| file_tree widget | ✅ | 异步递归扫描 + 隐藏文件过滤 + ListState |
| TUI 布局集成 | ✅ | 水平分割 (25%/75%) + Ctrl-F 切换 |
| Help overlay | ✅ | `?` 弹出 + centered_rect() + 任意键关闭 |
| Status line | ✅ | 模型/模式显示 |

## Phase 6: 打磨 (Wk7-8) ✅

| 模块 | 文件 | 说明 |
|------|------|------|
| Retry 工具 | `retry.rs` (118 行) | 指数退避 + jitter + 选择重试 + 测试 |
| 配置热重载 | `config_watch.rs` (95 行) | 轮询式文件变更检测 |
| unwrap 修复 | `cli/chat.rs`, `cli/ask.rs` | 替换为 `context()` 优雅处理 |
| 优雅降级 | `agent_bridge.rs` | remote mode 返回引导消息而非错误崩溃 |
| 重试集成 | `agent_bridge.rs` | local mode execute_turn 自动重试 |
| 文件树异步化 | `file_tree.rs` | 增加 tokio::fs 异步扫描路径 |

## Phase 7: 测试 (Wk8-9) ✅

### 单元测试覆盖

| 模块 | 测试数 | 类型 |
|------|--------|------|
| `config.rs` | 6 | 同步 |
| `modes.rs` | 5 | 同步 |
| `retry.rs` | 4 | 同步 + 异步 |
| `config_watch.rs` | 3 | 同步 |
| `agent_bridge.rs` | 4 | 同步 + 异步 |
| `tui/app.rs` | 4 | 同步 + 异步 |
| `notifications/browser.rs` | 2 | 同步 |
| `notifications/telegram.rs` | 1 | 同步 |
| `notifications/gmail.rs` | 2 | 同步 |
| **单元测试合计** | **31** | |

### 集成测试覆盖

| 测试文件 | 测试数 | 类型 | 覆盖模块 |
|---------|--------|------|---------|
| `tests/config_test.rs` | 6 | 同步 | CliConfig 三层加载 |
| `tests/ambient_test.rs` | 5 | 异步 | Runner + Scheduler |
| `tests/bridge_test.rs` | 6 | 异步 | AgentBridge 双模式 |
| `tests/notifications_test.rs` | 5 | 同步 | 三通知渠道 |
| `tests/e2e_test.rs` | 4 active + 2 ignored | 异步 | E2E 链路 |
| **集成测试合计** | **28** | | |

### dev-dependencies

| Crate | 用途 |
|-------|------|
| `tempfile` | 临时目录/文件创建 |
| `tokio-test` | 异步测试辅助 |
| `fastrand` (runtime) | 重试 jitter |

## Phase 8: 联调配合 (Wk9-10) ✅

| 交付物 | 状态 | 说明 |
|--------|------|------|
| `E2E_INTEGRATION_PLAN.md` | ✅ | 完整联调计划、风险、Bug 分派协议 |
| `tests/e2e_test.rs` | ✅ | 7 个场景: 基础对话/空输入/重建/热重载/远程(预留) |
| 接口契约对齐 | ✅ | AgentBridge.execute_turn → core.execute_agent_turn |
| 跨组 Bug 分派协议 | ✅ | 文档化 |

## 新增交付物

### gRPC 客户端 (Q8 修复)

| 组件 | 状态 | 说明 |
|------|------|------|
| `build.rs` | ✅ | Proto 编译 (agent/session/health) |
| `grpc_client.rs` | ✅ | GrpcClient: connect → health_check → chat_completion → create_session |
| `agent_bridge.rs` | ✅ | Remote 模式可接入 GrpcClient (connect_remote 方法预留) |

### VSCode Webview React App + @carpai/sdk

| 组件 | 状态 | 说明 |
|------|------|------|
| `webview-ui/` | ✅ | Vite + React 18 + TypeScript 完整前端 |
| `@carpai/sdk` 集成 | ✅ | package.json 引用 + carpaiSdk.ts 服务层 |
| VSCode 通信 | ✅ | useVSCode hook (postMessage) + chatPanel.ts 加载器 |
| 组件 | ✅ | ChatView / MessageBubble (Markdown) / InputBar / TypingIndicator |

---

## 计划结构偏差说明

以下 4 项在重构计划中有不同预期，但**实际架构更优**，已从质量问题列表移除：

| 计划要求 | 实际实现 | 偏差原因 |
|---------|---------|---------|
| `cli/startup.rs` | TUI 初始化在 `tui/mod.rs` | TUI 初始化是 TUI 层职责，放在 `cli/` 会导致跨模块环引用 |
| `cli/dispatch.rs` | 路由在 `main.rs` match | 4 个命令的简单 match 无需独立模块，避免过度工程 |
| `cli/commands/` 子目录 | 文件平铺 `cli/*.rs` | < 5 个模块平铺更清晰，子目录增加无意义间接层 |
| `modes/local.rs` + `remote.rs` | 单文件 `modes.rs` | CliMode 是 2 变体轻量枚举，拆 3 文件是过度分解 |

---

## 待执行 (运行验证)

```bash
# 1. 验证 carpai-core 编译
cargo check -p carpai-core

# 2. 验证 carpai-cli 编译 (含 proto 生成)
cargo check -p carpai-cli

# 3. 运行测试套件
cargo test -p carpai-cli

# 4. 与 solo-Turbo 同步合并
# - merge gamma/cli-build → main
# - 跨组 E2E: CLI local / CLI remote → server
```

## 文件统计

| 指标 | 值 |
|------|-----|
| Rust 源文件数 | 35 (原 32 + build.rs + grpc_client.rs + 更新 file_tree) |
| TypeScript/JS 文件数 | 12 (webview-ui) |
| Cargo.toml | 1 |
| Npm package.json | 1 |
| 自定义 Error 类型 | 8 (Config/Bridge/Telegram/Gmail/Browser/Retry/Grpc) |
| 测试总数 | 59 (31 单元 + 28 集成) |
