# CarpAI 开发环境搭建指南

本文档说明如何设置 CarpAI 企业级功能开发所需的 PostgreSQL 和 Redis 环境。

---

## 📋 前置要求

### 必需软件

1. **Docker Desktop**
   - Windows: [下载 Docker Desktop for Windows](https://www.docker.com/products/docker-desktop/)
   - macOS: [下载 Docker Desktop for Mac](https://www.docker.com/products/docker-desktop/)
   - Linux: 安装 Docker Engine 和 Docker Compose Plugin

2. **Git** (版本控制)

### 推荐软件

- **psql** (PostgreSQL 命令行客户端)
- **Redis CLI** (Redis 命令行客户端)
- **TablePlus** 或 **pgAdmin** (数据库 GUI 工具)
- **Another Redis Desktop Manager** (Redis GUI 工具)

---

## 🚀 快速开始

### 方法 1: 使用自动化脚本（推荐）

#### Windows (PowerShell)

```powershell
# 1. 克隆仓库（如果还没有）
git clone https://github.com/1jehuang/jcode.git
cd jcode

# 2. 运行设置脚本
.\scripts\dev_setup.ps1 start

# 3. 测试连接
.\scripts\test_db_connection.sh
```

#### macOS / Linux (Bash)

```bash
# 1. 克隆仓库（如果还没有）
git clone https://github.com/1jehuang/jcode.git
cd jcode

# 2. 赋予执行权限
chmod +x scripts/dev_setup.sh scripts/test_db_connection.sh

# 3. 运行设置脚本
./scripts/dev_setup.sh start

# 4. 测试连接
./scripts/test_db_connection.sh
```

### 方法 2: 手动使用 Docker Compose

```bash
# 启动 PostgreSQL 和 Redis
docker compose --profile dev up -d postgres redis

# 查看服务状态
docker compose --profile dev ps

# 查看日志
docker compose --profile dev logs -f postgres redis
```

---

## 🔧 配置说明

### 环境变量配置

1. **复制示例配置文件**

```bash
cp .env.example .env
```

2. **编辑 `.env` 文件**，至少修改以下关键配置：

```bash
# ⚠️ 重要：生成安全的 JWT 密钥
# macOS/Linux: openssl rand -base64 32
# Windows PowerShell: [Convert]::ToBase64String((1..32 | ForEach-Object { Get-Random -Minimum 0 -Maximum 255 }))

JWT_SECRET=your_secure_random_string_here_at_least_32_chars

# OAuth2 配置（如需测试 OAuth 登录）
OAUTH_CLIENT_ID=your_oauth_client_id
OAUTH_CLIENT_SECRET=your_oauth_client_secret
```

### 默认连接信息

#### PostgreSQL

| 参数 | 值 |
|------|-----|
| Host | localhost |
| Port | 5432 |
| Database | carpai |
| User | carpai |
| Password | carpai_dev_password |
| URL | `postgresql://carpai:carpai_dev_password@localhost:5432/carpai` |

#### Redis

| 参数 | 值 |
|------|-----|
| Host | localhost |
| Port | 6379 |
| URL | `redis://localhost:6379` |

---

## 📊 数据库结构

### 自动应用的迁移

启动时会自动应用以下迁移脚本（位于 `migrations/` 目录）：

1. **001_create_audit_log.sql** - 审计日志表
   - `audit_logs` - 存储所有安全和合规事件

2. **002_create_users_and_roles.sql** - 用户和角色表
   - `users` - 用户账户
   - `roles` - RBAC 角色定义
   - `user_roles` - 用户角色分配
   - `api_tokens` - API 认证令牌
   - `oauth_sessions` - OAuth2 会话
   - `gdpr_consents` - GDPR 同意记录

3. **003_create_sessions_and_cache.sql** - 会话和协作表
   - `sessions` - 活跃用户会话
   - `collab_rooms` - 协作编辑房间
   - `collab_participants` - 协作者参与者
   - `document_operations` - 文档操作日志（CRDT/OT）

### 查看数据库表

```bash
# 连接到 PostgreSQL
docker exec -it carpai-postgres psql -U carpai -d carpai

# 列出所有表
\dt

# 查看表结构
\d audit_logs
\d users
\d roles

# 退出
\q
```

---

## 🛠️ 常用命令

### 服务管理

```bash
# 启动服务
./scripts/dev_setup.sh start          # Linux/macOS
.\scripts\dev_setup.ps1 start         # Windows

# 停止服务
./scripts/dev_setup.sh stop
.\scripts\dev_setup.ps1 stop

# 重启服务
./scripts/dev_setup.sh restart
.\scripts\dev_setup.ps1 restart

# 查看状态
./scripts/dev_setup.sh status
.\scripts\dev_setup.ps1 status

# 清理所有数据（危险操作！）
./scripts/dev_setup.sh clean
.\scripts\dev_setup.ps1 clean
```

### 数据库操作

```bash
# 连接到 PostgreSQL
docker exec -it carpai-postgres psql -U carpai -d carpai

# 执行 SQL 查询
docker exec carpai-postgres psql -U carpai -d carpai -c "SELECT count(*) FROM users;"

# 备份数据库
docker exec carpai-postgres pg_dump -U carpai carpai > backup.sql

# 恢复数据库
cat backup.sql | docker exec -i carpai-postgres psql -U carpai carpai

# 查看审计日志
docker exec carpai-postgres psql -U carpai -d carpai -c "SELECT * FROM audit_logs ORDER BY timestamp DESC LIMIT 10;"
```

### Redis 操作

```bash
# 连接到 Redis
docker exec -it carpai-redis redis-cli

# 测试连接
PING

# 查看所有键
KEYS *

# 查看特定键的值
GET session:abc123

# 清空所有数据（开发环境用）
FLUSHDB

# 退出
EXIT
```

### 日志查看

```bash
# 实时查看所有服务日志
docker compose --profile dev logs -f

# 仅查看 PostgreSQL 日志
docker compose --profile dev logs -f postgres

# 仅查看 Redis 日志
docker compose --profile dev logs -f redis

# 查看最近 100 行日志
docker compose --profile dev logs --tail=100
```

---

## 🧪 测试连接

运行测试脚本验证数据库连接：

```bash
./scripts/test_db_connection.sh
```

预期输出：

```
🧪 Testing Database Connections...

1️⃣  Testing PostgreSQL connection...
   ✅ PostgreSQL is ready
   ✅ Database tables created: 10 tables

   📋 Database tables:
    public | audit_logs
    public | users
    public | roles
    ...

2️⃣  Testing Redis connection...
   ✅ Redis is responding
   📊 Memory usage: 1.23M

✅ All database connections successful!
```

---

## 🔍 故障排除

### 问题 1: Docker 无法启动

**症状**: `docker compose up` 失败

**解决方案**:
1. 确保 Docker Desktop 正在运行
2. Windows: 检查 WSL 2 是否启用
3. 重启 Docker Desktop
4. 检查端口冲突（5432, 6379）

```bash
# 检查端口占用
netstat -an | grep 5432  # Windows
lsof -i :5432            # macOS/Linux
```

### 问题 2: PostgreSQL 健康检查失败

**症状**: 服务启动但健康检查一直显示 `starting`

**解决方案**:
```bash
# 查看 PostgreSQL 日志
docker logs carpai-postgres

# 手动检查数据库
docker exec carpai-postgres pg_isready -U carpai -d carpai

# 重新启动
docker compose --profile dev restart postgres
```

### 问题 3: 迁移未自动应用

**症状**: 数据库中没有表

**解决方案**:
```bash
# 检查 migrations 目录是否正确挂载
docker exec carpai-postgres ls -la /docker-entrypoint-initdb.d/

# 手动应用迁移
docker exec -i carpai-postgres psql -U carpai -d carpai < migrations/001_create_audit_log.sql
docker exec -i carpai-postgres psql -U carpai -d carpai < migrations/002_create_users_and_roles.sql
docker exec -i carpai-postgres psql -U carpai -d carpai < migrations/003_create_sessions_and_cache.sql
```

### 问题 4: Redis 连接拒绝

**症状**: `redis-cli ping` 返回错误

**解决方案**:
```bash
# 检查 Redis 日志
docker logs carpai-redis

# 重启 Redis
docker compose --profile dev restart redis

# 测试连接
docker exec carpai-redis redis-cli ping
```

### 问题 5: 端口冲突

**症状**: 端口 5432 或 6379 已被占用

**解决方案**: 修改 `docker-compose.yml` 中的端口映射

```yaml
# 例如将 PostgreSQL 改为 5433
ports:
  - "5433:5432"

# 更新 .env 文件
POSTGRES_PORT=5433
DATABASE_URL=postgresql://carpai:carpai_dev_password@localhost:5433/carpai
```

---

## 📚 下一步

开发环境设置完成后，继续实施优化路线图中的任务：

1. **[TASK-001]** - 在 enterprise-server Cargo.toml 中添加 jcode-auth 依赖
2. **[TASK-002]** - 创建 EnterpriseAuthMiddleware 包装器
3. **[TASK-009]** - 实现 PostgresAuditStorage

参考文档：
- [优化路线图](carpai-optimization-roadmap.md)
- [任务跟踪](OPTIMIZATION_TRACKING.md)
- [企业认证指南](ENTERPRISE_AUTH_SETUP.md)

---

## 🔗 相关资源

- **PostgreSQL 文档**: https://www.postgresql.org/docs/15/
- **Redis 文档**: https://redis.io/documentation
- **Docker Compose 文档**: https://docs.docker.com/compose/
- **SQLx 文档** (Rust PostgreSQL 客户端): https://docs.rs/sqlx/latest/sqlx/

---

**最后更新**: 2026-05-21

**维护者**: CarpAI Core Team
