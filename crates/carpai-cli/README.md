# CarpAI CLI

**TUI-based AI programming assistant** — standalone CLI client for the CarpAI monorepo.

```bash
cargo install --path crates/carpai-cli
carpai chat
```

## Features

| 命令 | 用途 |
|------|------|
| `carpai chat` | 交互式 TUI 聊天会话 (默认模式) |
| `carpai ask <question>` | 一次性问答后退出 |
| `carpai complete <file> <line> <col>` | 代码补全 |
| `carpai serve` | 启动 CarpAI 服务器 (子进程) |

## Architecture

```
┌─────────────────────────────────────────────┐
│              carpai-cli                     │  ← THIS CRATE: TUI + CLI commands
├─────────────────────────────────────────────┤
│              carpai-core                    │  ← Business logic (execute_agent_turn)
├─────────────────────────────────────────────┤
│            carpai-internal                  │  ← Trait definitions + DI container
└─────────────────────────────────────────────┘
```

**核心设计原则**: TUI 是纯渲染层，零业务逻辑。所有 Agent 调用通过 `agent_bridge.rs` → `carpai_core::execute_agent_turn()` 完成。

## Quick Start

### 1. 安装

```bash
# 从源码安装
cargo install --path crates/carpai-cli

# 验证安装
carpai --version
```

### 2. 运行 TUI 模式

```bash
carpai chat
```

### 3. 配置

首次运行会自动创建默认配置 `~/.carpai/config.toml`。也可手动创建:

```toml
# ~/.carpai/config.toml
mode = "cli"
working_dir = "/home/user/projects"
default_model = "claude-sonnet-4-20250514"

[theme]
syntax_theme = "base16-dark"

[keybinds]
send_message = "Enter"
interrupt = "Escape"
```

### 4. 远程模式

```bash
# 连接到远程 CarpAI 服务器
CARPAI_REMOTE_URL=https://carpai.example.com:8080 carpai chat
```

## Modes

| 模式 | 描述 | 配置 |
|------|------|------|
| **Local** (默认) | 本地推理，所有数据存储在本地 | `~/.carpai/` |
| **Remote** | 连接到 carpai-server | `CARPAI_REMOTE_URL` 环境变量 |

## Configuration

### 配置文件优先级

1. 硬编码默认值
2. TOML 配置文件 (`~/.carpai/config.toml`)
3. 环境变量覆盖 (`CARPAI_*` 前缀)

### 环境变量参考

| 变量 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `CARPAI_REMOTE_URL` | string | — | 远程服务器 URL |
| `CARPAI_DATA_DIR` | string | `~/.carpai` | 数据存储目录 |
| `CARPAI_DEFAULT_MODEL` | string | `default` | 默认推理模型 |
| `CARPAI_LOG_LEVEL` | string | `info` | 日志级别 (trace/debug/info/warn/error) |
| `CARPAI_CORE__DATA_DIR` | string | `~/.carpai` | 核心数据目录 |
| `CARPAI_CORE__MAX_CONCURRENT_TOOLS` | int | 5 | 最大并发工具数 |

### TUI 快捷键

| 快捷键 | 功能 |
|--------|------|
| `Enter` | 发送消息 |
| `Ctrl-C` | 退出 |
| `Ctrl-F` | 切换文件树面板 |
| `?` / `F1` | 显示帮助 |
| `↑/↓` 或 `j/k` | 文件树导航 (文件树打开时) |

## Development

```bash
# 构建
cargo build -p carpai-cli

# 运行
cargo run -p carpai-cli -- chat

# 测试
cargo test -p carpai-cli
```

## Integration Tests

```bash
# 运行所有集成测试
cargo test -p carpai-cli --tests

# 运行特定测试
cargo test -p carpai-cli --test config_test
cargo test -p carpai-cli --test ambient_test
cargo test -p carpai-cli --test bridge_test
cargo test -p carpai-cli --test notifications_test
cargo test -p carpai-cli --test e2e_test -- --ignored
```

## Dependencies

- **TUI**: ratatui 0.29 + crossterm 0.28
- **Async**: tokio (full) + tokio-util
- **CLI**: clap 4
- **gRPC**: tonic 0.12 + prost 0.13
- **Serialization**: serde + toml
