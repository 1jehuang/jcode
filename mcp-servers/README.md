# CarpAI MCP Servers
**10 个企业级 MCP (Model Context Protocol) 服务器实现**，对标 Claude Code/Cursor 的 MCP 生态，用于扩展 CarpAI 的企业集成能力。

## 状态：✅ 全部实现

| # | 服务器 | 状态 | 工具数 | 功能 |
|---|--------|------|--------|------|
| 1 | **GitHub** | ✅ 完整实现 | 13 | PR审查、Issue管理、代码浏览、Merge |
| 2 | **Jira** | ✅ 完整实现 | 7 | JQL搜索、Issue CRUD、评论、状态变更 |
| 3 | **Slack** | ✅ 完整实现 | 6 | 消息发送、频道管理、历史查询、文件上传 |
| 4 | **Docker** | ✅ 完整实现 | 10 | 容器控制、镜像管理、系统信息、Prune |
| 5 | **PostgreSQL** | ✅ 完整实现 | 12 | 查询、Schema管理、EXPLAIN、备份、**SSL支持**、**SQLite离线回退** |
| 6 | **Redis** | ✅ 完整实现 | 10 | 键值CRUD、数据结构操作、TTL、Flush |
| 7 | **Kubernetes** | ✅ 完整实现 | 12 | Pod管理、Deployment操作、集群检查 |
| 8 | **AWS** | ✅ 完整实现 | 10 | EC2、S3、Lambda、CloudWatch、RDS、ECS |
| 9 | **Sentry** | ✅ 完整实现 | 8 | Issue管理、Release追踪、事件查看 |
| 10 | **Datadog** | ✅ 完整实现 | 8 | 指标查询、监控管理、日志搜索、仪表板 |

## 快速开始

### 安装依赖

```bash
# 安装所有服务器依赖（需要 Python 3.10+）
python mcp-servers/install_all.py

# 或使用安装脚本
# Linux/Mac:
bash scripts/mcp_setup.sh
# Windows:
.\scripts\mcp_setup.ps1
```

### 配置凭据

```bash
# 复制环境变量模板
cp .env.mcp .env

# 编辑 .env 文件，填入你的 API Key
# 然后启动服务
```

### 启动服务器

```bash
# 启动所有已配置的 MCP 服务器
python mcp-servers/start_all.py

# 启动指定服务器
python mcp-servers/start_all.py github postgres redis
```

### 使用 Docker 部署

```bash
# 构建并启动所有服务
docker-compose -f mcp-servers/docker-compose.yml up -d

# 启动指定服务
docker-compose -f mcp-servers/docker-compose.yml up -d mcp-github mcp-postgres
```

### CarpAI 集成

```bash
# 添加 MCP 服务器到 CarpAI 配置
jcode mcp add github "python" "mcp-servers/github/src/server.py" --scope local

# 从 Claude Desktop 导入配置
jcode mcp import-desktop --scope local

# 列出已配置的服务器
jcode mcp list

# 启动 CarpAI MCP Bridge（同时作为 MCP Server 和 Client）
jcode mcp bridge --status
```

## 连接方式

所有 MCP 服务器支持三种传输模式：

| 模式 | 说明 | 端口 |
|------|------|------|
| **stdio** | 标准输入输出（默认，适合 CarpAI Agent 调用） | - |
| **SSE** | Server-Sent Events (HTTP) | 8001-8010 |
| **Streamable HTTP** | HTTP JSON-RPC | 8001-8010 |

## SSL 配置（PostgreSQL）

PostgreSQL MCP 服务器支持完整的 SSL/TLS 配置：

```bash
# SSL 模式
PG_SSL_MODE=require          # 强制 SSL
PG_SSL_MODE=verify-ca        # 验证服务器证书的 CA
PG_SSL_MODE=verify-full      # 验证证书和主机名
PG_SSL_ROOT_CERT=/path/to/ca.pem  # CA 证书路径

# 离线回退（当 PostgreSQL 不可用时自动切换到 SQLite）
PG_OFFLINE_FALLBACK=1
MCP_DB_PATH=./carpai_mcp.db  # SQLite 数据库文件路径
```

## 架构图

```
┌──────────────────────────────────────────────────────────────────┐
│                        CarpAI MCP Ecosystem                       │
├──────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌──────────┐   │
│  │ GitHub  │ │  Jira   │ │  Slack  │ │ Docker  │ │PostgreSQL │   │
│  │ PR/Issue│ │ Task Mg │ │ Notify  │ │Container│ │   SQL    │   │
│  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘ └────┬─────┘   │
│       │           │           │           │           │          │
│  ┌────┴────┐ ┌────┴────┐ ┌────┴────┐ ┌────┴────┐ ┌────┴─────┐  │
│  │  Redis  │ │   K8s   │ │   AWS   │ │ Sentry  │ │ Datadog  │  │
│  │  Cache  │ │ Cluster │ │  Cloud  │ │ Errors  │ │  Metrics │  │
│  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘ └────┬─────┘  │
│       │           │           │           │           │          │
├───────┴───────────┴───────────┴───────────┴───────────┴─────────┤
│                     MCP Protocol (JSON-RPC)                      │
│          stdio | SSE (HTTP) | Streamable HTTP                    │
├──────────────────────────────────────────────────────────────────┤
│                     CarpAI MCP Bridge / Gateway                   │
├──────────────────────────────────────────────────────────────────┤
│                    IDE Integration (MCP Client)                   │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐            │
│  │ VS Code  │ │JetBrains │ │  Neovim  │ │  Emacs   │            │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘            │
└──────────────────────────────────────────────────────────────────┘
```

## 配置格式

### CarpAI MCP 配置 (`~/.jcode/mcp.json`)

```json
{
  "servers": {
    "github": {
      "command": "python",
      "args": ["mcp-servers/github/src/server.py"],
      "env": { "GITHUB_TOKEN": "ghp_xxx" },
      "shared": true
    }
  }
}
```

### VS Code / Cursor 兼容配置 (`.vscode/mcp.json`)

```json
{
  "servers": {
    "carpai-postgres": {
      "type": "stdio",
      "command": "python",
      "args": ["mcp-servers/postgres/src/server.py"],
      "env": { "DATABASE_URL": "postgresql://..." }
    }
  }
}
```

## 测试

```bash
# 单独测试某个 MCP 服务器
cd mcp-servers/github
python -c "
import asyncio
from src.server import mcp
async def test():
    result = await mcp.call_tool('list_pull_requests', {'repo': 'owner/repo'})
    print(result)
asyncio.run(test())
"
```

## 文件结构

```
mcp-servers/
├── README.md              # 本文档
├── docker-compose.yml     # Docker Compose 配置
├── Dockerfile             # 通用 Dockerfile
├── requirements.txt       # 公共依赖
├── requirements-*.txt     # 各服务器依赖
├── install_all.py         # 自动安装脚本
├── start_all.py           # 自动启动脚本
├── github/src/server.py   # GitHub MCP 服务器
├── jira/src/server.py     # Jira MCP 服务器
├── slack/src/server.py    # Slack MCP 服务器
├── docker/src/server.py   # Docker MCP 服务器
├── postgres/src/server.py # PostgreSQL MCP 服务器 (SSL+离线)
├── redis/src/server.py    # Redis MCP 服务器
├── kubernetes/src/server.py # Kubernetes MCP 服务器
├── aws/src/server.py      # AWS MCP 服务器
├── sentry/src/server.py   # Sentry MCP 服务器
└── datadog/src/server.py  # Datadog MCP 服务器
```

## 安全

1. **Token管理**: 使用环境变量或 `.env` 文件，禁止硬编码
2. **权限最小化**: 为每个服务创建专用 API Key，仅授予必要权限
3. **SQL注入防护**: PostgreSQL 使用参数化查询，SQLite 同样支持
4. **SSL/TLS**: PostgreSQL 支持 `verify-full` 完整证书链验证
5. **确认机制**: 危险操作（删除表、清空数据库）需确认参数
