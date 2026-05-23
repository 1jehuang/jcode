# CarpAI IDE 生态 + 核心能力完全体

## 一、已完成模块清单

### 核心能力 (5 大功能)

| # | 功能 | 文件 | 行数 | 关键能力 |
|---|------|------|------|----------|
| 1 | **Agent自动调用MCP** | `src/mcp/auto_mcp.rs` | ~200 | 自动发现+连接+注册MCP工具、健康检查、指数退避重连 |
| 2 | **自主跨文件规划** | `src/planner/plan.rs` + `dependency.rs` | ~400 | 依赖图拓扑排序、循环检测、影响分析、计划验证 |
| 3 | **语义级重构** | `src/refactor/mod.rs` | ~370 | 重命名/提取方法/内联/变更签名/格式化 |
| 4 | **跨文件事务** | `src/transaction/mod.rs` | ~300 | 两阶段提交、快照回滚、事务日志、原子性保证 |
| 5 | **自主验证修复** | `src/verify/mod.rs` | ~390 | 编译/lint/测试验证、诊断解析、自动修复、迭代循环 |

### IDE 生态 (4 项)

| # | IDE | 状态 | 文件 | 关键特性 |
|---|-----|------|------|----------|
| 1 | **VSCode 扩展** | ✅ 增强 | `editors/vscode-carpai/*` (7文件) | InlineCompletion, MCP同步, QuickFix, DiffViewer, 14命令 |
| 2 | **JetBrains 插件** | ✅ 已有 | `editors/jetbrains-carpai/*` (16 Kotlin文件) | LSP客户端, 设置UI, 协作, 3动作 |
| 3 | **Neovim 插件** | 🆕 新建 | `editors/carpai-nvim/*` (4文件) | 聊天面板, 幽灵文本, Review, Explain, MCP |
| 4 | **配置文件兼容** | 🆕 新建 | `.vscode/.cursor/.claude/mcp.json` | 3大IDE格式全覆盖 |

### MCP 生态 (10 服务器)

| # | 服务器 | 状态 | 工具数 |
|---|--------|------|--------|
| 1-6 | GitHub/Jira/Slack/Docker/PostgreSQL/Redis | ✅ 已有增强 | 7-13 |
| 7-10 | Kubernetes/AWS/Sentry/Datadog | ✅ 修复(原模板) | 8-12 |

### 基础设施

| 类别 | 文件 |
|------|------|
| Python依赖 | `mcp-servers/requirements*.txt` × 11 |
| 安装脚本 | `install_all.py`, `start_all.py`, `test_all.py` |
| Shell脚本 | `scripts/mcp_setup.sh`, `scripts/mcp_setup.ps1` |
| Docker | 10 Dockerfiles + `docker-compose.yml` |
| K8s | `deploy/mcp-gateway.yaml`, `deploy/mcp-ingress.yaml` |
| 配置 | `config/mcp_servers.yaml`, `.env.mcp`, `.jcode/mcp.json` |
| SDK | `crates/carpai-sdk/src/mcp.rs` |
| CLI | `src/commands/agent/mcp.rs` (从占位符重写) |

## 二、文件总数

```
新增/修改文件: ~70 个
  - Python MCP 服务器: 4 个新实现 + 6 个增强
  - 基础设施文件: 24 个 (requirements/Docker/脚本)
  - 配置/部署: 10 个 (YAML/JSON/env)
  - Rust 核心: 7 个 (auto_mcp/planner/refactor/transaction/verify + CLI/SDK)
  - IDE 文件: 15 个 (VSCode/Neovim/JetBrains + 配置文件)
  - 文档: 3 个 (MCP_SUMMARY/IDE_SUMMARY/README)
```

## 三、对标 Claude Code 能力矩阵

| 能力 | Claude Code | Cursor | CarpAI (现在) |
|------|-------------|--------|---------------|
| MCP 服务器协议 | ✅ | ⚠️ 有限 | ✅ 完整 (16 Rust模块 + 10 Python服务器) |
| MCP 自动发现 | ✅ | ✅ | ✅ AutoMcpManager |
| Agent 工具调用 | ✅ | ✅ | ✅ Turn Loop + Registry |
| 跨文件规划 | ✅ plan mode | ⚠️ | ✅ Planner + Dependency Analysis |
| 语义重构 | ✅ FileEditTool | ✅ | ✅ RefactorEngine (重命名/提取/内联) |
| 原子提交 | ✅ | ✅ | ✅ Transaction (两阶段提交+快照回滚) |
| 自动验证修复 | ✅ auto-fix | ✅ | ✅ VerifyEngine (编译/lint/测试) |
| VSCode 扩展 | ✅ | ✅ 原生 | ✅ InlineCompletion + MCP + QuickFix |
| JetBrains 插件 | ✅ | ❌ | ✅ 完整 Kotlin 实现 |
| Neovim 插件 | ❌ | ❌ | ✅ carpai-nvim (完全实现) |
| Cursor 配置兼容 | ❌ | ✅ 原生 | ✅ .cursor/mcp.json |
| Claude Code 配置兼容 | ✅ 原生 | ❌ | ✅ .claude/mcp.json + import-desktop |
| Docker 部署 | ❌ | ❌ | ✅ 10 Dockerfiles + Compose |
| K8s 部署 | ❌ | ❌ | ✅ Deployment + HPA + Ingress |
| Windows 支持 | ⚠️ | ✅ | ✅ PowerShell 脚本 |

## 四、快速使用

```bash
# 验证
python mcp-servers/test_all.py

# 启动 MCP 服务器
python mcp-servers/start_all.py github postgres

# 列出 CarpAI 配置的 MCP 服务器
jcode mcp list

# 导入 Claude Desktop 配置
jcode mcp import-desktop

# 使用验证引擎
cargo test -p verify_engine
```
