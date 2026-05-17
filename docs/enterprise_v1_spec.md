# CarpAI 企业版 v1.0 — 产品规格书

## 1. 产品概述

**产品名称**：CarpAI Enterprise Edition v1.0  
**目标用户**：200 人以内软件开发团队  
**部署模式**：私有化部署（单机 Docker / 混合集群）  
**定位**：从个人生产力工具 → 团队协作开发平台

---

## 1a. 核心差异化特性

### 异构集群分工（Heterogeneous Cluster）

**零额外硬件投入**，将公司现有的台式机、笔记本、闲置设备转化为 AI 推理集群。

| 节点类型 | 配置 | 用途 |
|---------|------|------|
| 推理节点 | 128GB ×5 | 72B 大模型（Qwen3） |
| 轻量节点 | 32GB | 32B 模型（DeepSeek-R1） |
| 边缘节点 | 16GB | 9B 模型 + 工具执行 |

技术：mDNS 自动发现 + 心跳、节点离线摘除、mmap KV Cache → 虚拟内存

### Parallax — 分布式推理调度

```
请求 → Parallax Router → 代码生成 → DeepSeek-R1 (轻量)
                         → 架构分析 → Qwen3-72B (推理节点)
                         → 日常对话 → GLM-9B (边缘节点)
                         → 批量任务 → Ruflo 流水线并行
```

### Ruflo — 任务规划引擎

Rust 任务流水线调度，集成 UnifiedScheduler（注水/DP/流水线并行），按任务类型 + 节点负载自动路由。

---

## 2. 功能规格

### 2.1 核心功能（从现有版本保留）

| 功能 | 说明 | 优先级 |
|------|------|--------|
| AI Agent 对话 | 多轮对话 + 上下文管理 | P0 |
| 多 Provider 支持 | Claude/GPT/Gemini/DeepSeek/Kimi | P0 |
| 30+ 内置工具 | 文件操作、代码编辑、Shell、Git、Browser | P0 |
| Tree-sitter 代码智能 | AST 解析、符号查找、代码导航 | P0 |
| 代码补全引擎 | LSP 补全 + 内联建议 | P0 |
| SSH 远程连接 | 远程开发、文件传输 | P0 |
| 语义记忆 | 跨 session 上下文保持 | P1 |

### 2.2 新增功能

#### 2.2.1 认证与权限（P0）

| 模块 | 功能 |
|------|------|
| LDAP/OIDC 集成 | 对接企业 AD/LDAP/Okta/Azure AD |
| 本地账号 | 邮箱+密码注册/登录 |
| RBAC 权限模型 | `admin` / `developer` / `viewer` 三级 |
| API Token | 个人 Access Token + 权限范围 |

#### 2.2.2 团队协作（P0）

| 模块 | 功能 |
|------|------|
| Workspace 管理 | 创建/切换/归档 workspace |
| Session 共享 | 团队可见会话、断点续传 |
| 项目隔离 | 每个 workspace 独立配置、独立记忆 |
| 团队知识库 | 共享 prompt 模板、tool 配置、代码规范 |

#### 2.2.3 Web 管理控制台（P1）

| 模块 | 功能 |
|------|------|
| 仪表盘 | 团队用量统计、活跃度、性能 |
| 用户管理 | 邀请、禁用、角色分配 |
| 系统设置 | Provider 密钥管理、全局配置 |
| 审计日志 | 操作记录、搜索、导出 |

#### 2.2.4 可观测性（P1）

| 模块 | 功能 |
|------|------|
| Prometheus Metrics | 请求量、延迟、错误率 |
| 结构化日志 | JSON 格式、日志级别、可搜索 |
| 健康检查 | `/health` `/ready` 端点 |

#### 2.2.5 安全加固（P1）

| 模块 | 功能 |
|------|------|
| TLS | 全链路 HTTPS + 自动证书 |
| 数据加密 | 敏感字段 AES-256 加密存储 |
| 沙箱执行 | 工具执行隔离（容器级别） |
| SQL 注入防护 | 参数化查询 + ORM |

---

## 3. 技术规格

### 3.1 系统架构（单体 → 三服务拆分）

```
┌───────────────┐     ┌───────────────┐     ┌───────────────┐
│  auth-service  │     │  agent-service │     │  web-service   │
│  :8081         │     │  :8082        │     │  :8080         │
│               │     │               │     │               │
│ - LDAP/OIDC   │     │ - Agent Loop  │     │ - Admin UI    │
│ - RBAC        │◄───►│ - Tool Exec   │◄───►│ - Dashboard   │
│ - Token Mgmt  │     │ - Memory      │     │ - Settings    │
│ - Session     │     │ - LLM Router  │     │ - Audit View  │
└───────┬───────┘     └───────┬───────┘     └───────┬───────┘
        │                     │                     │
        └─────────────────────┼─────────────────────┘
                              ▼
                    ┌──────────────────┐
                    │   PostgreSQL      │
                    │   + Redis         │
                    └──────────────────┘
```

### 3.2 依赖变更

| 新增依赖 | 用途 | 版本 |
|----------|------|------|
| `axum` | Web 框架（替换 ratatui 作为管理入口） | 0.8 |
| `sqlx` | 异步 PostgreSQL 驱动 + migrate | 0.8 |
| `fred` / `redis-rs` | Redis 客户端（session/cache） | 8.x |
| `tower-http` | CORS、压缩、鉴权 middleware | 0.6 |
| `opentelemetry` | 分布式追踪 | 0.27 |
| `ldap3` | LDAP 认证 | 0.8 |
| `jsonwebtoken` | JWT 签发/验证 | 9.x |
| `prometheus` | Metrics 采集 | 0.14 |
| `tracing-stackdriver` | 结构化日志 | 0.11 |

### 3.3 数据模型（新增表）

```sql
-- 认证
CREATE TABLE users (id UUID PK, email, password_hash, name, role, created_at, disabled_at);
CREATE TABLE api_tokens (id UUID PK, user_id FK, name, token_hash, scopes, expires_at, last_used_at);

-- 组织
CREATE TABLE organizations (id UUID PK, name, domain, plan, created_at);
CREATE TABLE org_members (org_id FK, user_id FK, role, joined_at);
CREATE TABLE workspaces (id UUID PK, org_id FK, name, settings JSONB, created_at);

-- 审计
CREATE TABLE audit_log (id BIGSERIAL PK, org_id FK, user_id FK, action, resource, detail JSONB, ip, timestamp);
CREATE INDEX idx_audit_org_ts ON audit_log(org_id, timestamp DESC);

-- 用量
CREATE TABLE usage_records (id BIGSERIAL PK, org_id FK, user_id FK, provider, model, input_tokens, output_tokens, cost, timestamp);
```

---

## 4. 部署规格

### 4.1 硬件要求

| 规格 | 最低 | 推荐 |
|------|------|------|
| CPU | 8 核 | 16 核 |
| 内存 | 32 GB | 64 GB |
| 磁盘 | 100 GB SSD | 500 GB NVMe |
| 网络 | 100 Mbps | 1 Gbps |

### 4.2 软件栈

```
Docker 24+  +  Docker Compose V2
├── carpai-auth:8081
├── carpai-agent:8082
├── carpai-web:8080
├── postgres:16
├── redis:7
└── nginx:1.26 (TLS termination + reverse proxy)
```

### 4.3 快速启动

```bash
# 1. 下载
curl -O https://get.carpai.dev/enterprise/docker-compose.yml
curl -O https://get.carpai.dev/enterprise/.env.example

# 2. 配置
cp .env.example .env
# 编辑 .env: 设置 LDAP_SERVER, DB_PASSWORD, JWT_SECRET

# 3. 启动
docker compose up -d

# 4. 访问
open https://carpai.yourcompany.com
```

---

## 5. 质量要求

| 指标 | 目标值 |
|------|--------|
| API P99 延迟 | < 500ms |
| Agent 首次响应 | < 3s |
| 可用性 | 99.5%（月宕机 < 3.6h） |
| 并发用户 | 50 同时在线 |
| 数据备份 | 每日全量 + 实时 WAL |
| 代码测试覆盖率 | > 70% |
| 安全漏洞 | 0 高危 |
