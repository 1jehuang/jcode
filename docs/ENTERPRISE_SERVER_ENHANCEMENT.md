# CarpAI Enterprise Server 增强计划

## 对标分析：Cursor & Claude Code 服务端能力

### Cursor Enterprise 核心能力
1. **多租户管理**: 组织、团队、用户层级权限
2. **SSO集成**: SAML/OIDC/LDAP
3. **审计日志**: 完整的操作追溯
4. **用量统计**: Token消耗、API调用统计
5. **私有部署**: VPC内网部署
6. **安全合规**: SOC2, GDPR, HIPAA
7. **SLA保障**: 99.9%可用性承诺
8. **专属支持**: 技术客户经理

### Claude Code Enterprise 核心能力
1. **Anthropic API企业版**: 高吞吐量、低延迟
2. **数据隐私**: 训练数据隔离、不用于模型训练
3. **自定义策略**: 内容过滤、安全策略
4. **批量处理**: 大规模代码库分析
5. **团队协作**: 共享上下文、知识沉淀
6. **集成生态**: GitHub/GitLab/Jira等深度集成

## CarpAI 当前状态评估

### ✅ 已实现功能
- [x] 基础企业服务器架构 (jcode-enterprise-server)
- [x] OpenAI兼容API (`/v1/chat/completions`)
- [x] 管理员API (`/admin/orgs`, `/admin/users`, `/admin/usage`)
- [x] 认证系统 (JWT + API Key)
- [x] 多租户基础 (Organization + User)
- [x] 用量追踪 (UsageManager)
- [x] CPU推理引擎 (llama.cpp集成)
- [x] 分布式推理调度
- [x] 节点发现 (mDNS)
- [x] 统一调度器 (Ruflo-Parallax)
- [x] 虚拟内存管理
- [x] 数据库支持 (SQLite/PostgreSQL)

### ⚠️ 待完善功能
- [ ] 完整RBAC权限系统
- [ ] SSO/SAML/OIDC集成
- [ ] 审计日志系统
- [ ] 用量配额和限制
- [ ] 高级监控和告警
- [ ] 高可用部署方案
- [ ] 备份和恢复机制
- [ ] 性能基准测试
- [ ] 客户端SDK
- [ ] 管理控制台UI

## 增强路线图

### Phase 1: 核心企业功能 (Week 1-4)

#### 1.1 RBAC权限系统强化

**目标**: 实现细粒度的基于角色的访问控制

**实现清单**:
```rust
// crates/jcode-enterprise-server/src/auth/rbac.rs

pub enum Permission {
    // 组织级别
    OrgCreate,
    OrgRead,
    OrgUpdate,
    OrgDelete,
    OrgAdmin,

    // 用户级别
    UserCreate,
    UserRead,
    UserUpdate,
    UserDelete,

    // 会话级别
    SessionCreate,
    SessionRead,
    SessionUpdate,
    SessionDelete,

    // 模型级别
    ModelUse(String),  // model name
    ModelDeploy,

    // 资源级别
    ResourceRead(String),  // resource path
    ResourceWrite(String),

    // 管理级别
    AdminViewAuditLog,
    AdminManageBilling,
    AdminConfigureSSO,
}

pub struct Role {
    pub id: String,
    pub name: String,
    pub permissions: HashSet<Permission>,
    pub scope: PermissionScope,
}

pub struct PolicyEngine {
    roles: HashMap<String, Role>,
    user_roles: HashMap<String, Vec<String>>,  // user_id -> role_ids
}

impl PolicyEngine {
    pub fn check_permission(
        &self,
        user_id: &str,
        permission: &Permission,
        resource: Option<&str>,
    ) -> bool;

    pub fn add_role(&mut self, user_id: &str, role_id: &str);
    pub fn remove_role(&mut self, user_id: &str, role_id: &str);
}
```

**内置角色**:
- `super_admin` - 超级管理员（所有权限）
- `org_admin` - 组织管理员
- `team_lead` - 团队负责人
- `developer` - 开发者
- `viewer` - 只读观察者
- `billing_admin` - 账单管理员

**验收标准**:
- [ ] 单元测试覆盖率 > 90%
- [ ] 权限检查延迟 < 1ms
- [ ] 支持动态角色创建
- [ ] 支持资源级权限过滤

#### 1.2 审计日志系统

**目标**: 记录所有关键操作，满足合规要求

**实现**:
```rust
// crates/jcode-enterprise-server/src/audit/mod.rs

pub enum AuditAction {
    // 认证相关
    LoginSuccess,
    LoginFailure,
    Logout,
    ApiKeyCreated,
    ApiKeyRevoked,

    // 用户管理
    UserCreated,
    UserUpdated,
    UserDeleted,
    RoleAssigned,
    RoleRevoked,

    // 会话相关
    SessionStarted,
    SessionEnded,
    MessageSent,
    ToolExecuted,

    // 数据访问
    FileRead,
    FileWritten,
    CodebaseIndexed,

    // 配置变更
    ConfigUpdated,
    PolicyChanged,

    // 系统事件
    NodeJoined,
    NodeLeft,
    ModelDeployed,
}

pub struct AuditLog {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub actor_id: String,           // 执行者
    pub actor_type: ActorType,       // User/Service/System
    pub action: AuditAction,
    pub target_id: Option<String>,   // 目标对象
    pub target_type: Option<String>,
    pub metadata: serde_json::Value,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub result: AuditResult,
}

pub struct AuditLogger {
    writer: Box<dyn AuditWriter>,
    buffer: Vec<AuditLog>,
}

trait AuditWriter {
    async fn write_batch(&mut self, logs: &[AuditLog]) -> Result<()>;
    async fn flush(&mut self) -> Result<()>;
}

// 支持多种存储后端
struct DatabaseAuditWriter { db: Arc<DatabaseManager> }
struct FileAuditWriter { path: PathBuf }
struct CloudAuditWriter { client: CloudStorageClient }
```

**API端点**:
```
GET /admin/audit/logs?start_date=&end_date=&user_id=&action=
GET /admin/audit/logs/:id
POST /admin/audit/export  # 导出为CSV/JSON
```

**验收标准**:
- [ ] 所有敏感操作都有审计日志
- [ ] 日志不可篡改（可选区块链哈希链）
- [ ] 支持实时日志流
- [ ] 日志保留策略可配置
- [ ] 查询延迟 < 100ms

#### 1.3 用量配额和限制

**目标**: 防止资源滥用，支持分级定价

**实现**:
```rust
// crates/jcode-enterprise-server/src/quota/mod.rs

pub struct QuotaPolicy {
    pub tier: UsageTier,  // free/pro/enterprise
    pub limits: QuotaLimits,
    pub reset_period: ResetPeriod,  // daily/monthly
}

pub struct QuotaLimits {
    pub max_tokens_per_month: u64,
    pub max_requests_per_hour: u64,
    pub max_concurrent_sessions: u32,
    pub max_file_size_mb: u64,
    pub max_codebase_size_gb: u64,
    pub allowed_models: Vec<String>,
    pub max_context_length: u32,
    pub rate_limit_rpm: u32,  // requests per minute
}

pub struct UsageTracker {
    current_usage: HashMap<String, UsageRecord>,
    policies: HashMap<String, QuotaPolicy>,
}

impl UsageTracker {
    pub fn check_quota(&self, user_id: &str, request: &Request) -> Result<(), QuotaError>;
    pub fn record_usage(&mut self, user_id: &str, usage: UsageRecord);
    pub fn get_usage_summary(&self, user_id: &str) -> UsageSummary;
}
```

**API端点**:
```
GET /api/v1/usage/current      # 当前用量
GET /api/v1/usage/history      # 历史用量
GET /api/v1/quota/policy       # 配额策略
POST /admin/quota/update       # 更新配额
```

**验收标准**:
- [ ] 实时用量追踪
- [ ] 超额自动拒绝
- [ ] 用量预警通知
- [ ] 支持软限制和硬限制

### Phase 2: 身份认证增强 (Week 5-8)

#### 2.1 SSO集成

**支持的协议**:
- OAuth 2.0 / OIDC
- SAML 2.0
- LDAP / Active Directory

**实现**:
```rust
// crates/jcode-enterprise-server/src/auth/sso.rs

pub trait SSOProvider {
    fn name(&self) -> &str;
    async fn initiate_login(&self, redirect_url: &str) -> Result<LoginUrl>;
    async fn handle_callback(&self, code: &str) -> Result<SSOToken>;
    async fn validate_token(&self, token: &SSOToken) -> Result<UserInfo>;
    async fn logout(&self, session_id: &str) -> Result<()>;
}

pub struct OIDCProvider {
    config: OIDCConfig,
    client: reqwest::Client,
}

pub struct SAMLProvider {
    config: SAMLConfig,
    idp_metadata: IdpMetadata,
}

pub struct LDAPProvider {
    config: LDAPConfig,
    pool: ldap3::LdapPool,
}
```

**配置示例**:
```toml
[auth.sso]
enabled = true
providers = ["oidc", "saml"]

[auth.sso.oidc]
issuer = "https://accounts.google.com"
client_id = "${GOOGLE_CLIENT_ID}"
client_secret_env = "GOOGLE_CLIENT_SECRET"
scopes = ["openid", "profile", "email"]

[auth.sso.saml]
idp_metadata_url = "https://your-idp.com/metadata"
sp_entity_id = "carpai-enterprise"
acs_url = "https://carpai.example.com/sso/saml/acs"
```

**验收标准**:
- [ ] 支持至少3个主流IdP (Google, Azure AD, Okta)
- [ ] SSO登录延迟 < 2秒
- [ ] 支持Just-in-Time用户配置
- [ ] 支持SAML单点登出

#### 2.2 API密钥管理

**功能**:
- 多密钥支持
- 密钥轮换
- 密钥权限范围
- 密钥使用统计

**API端点**:
```
POST /api/v1/api-keys          # 创建密钥
GET  /api/v1/api-keys          # 列出密钥
DELETE /api/v1/api-keys/:id    # 撤销密钥
POST /api/v1/api-keys/:id/rotate  # 轮换密钥
```

### Phase 3: 可观测性 (Week 9-12)

#### 3.1 指标收集

**使用 Prometheus + Grafana**

**关键指标**:
```rust
// crates/jcode-enterprise-server/src/metrics/mod.rs

pub struct MetricsCollector {
    registry: prometheus::Registry,
}

impl MetricsCollector {
    // HTTP指标
    http_requests_total: CounterVec,
    http_request_duration_seconds: HistogramVec,
    http_errors_total: CounterVec,

    // LLM指标
    llm_tokens_processed: CounterVec,
    llm_request_latency: HistogramVec,
    llm_active_sessions: Gauge,
    llm_queue_depth: Gauge,

    // 系统指标
    system_cpu_usage: Gauge,
    system_memory_usage: Gauge,
    system_disk_io: CounterVec,

    // 业务指标
    active_organizations: Gauge,
    active_users: Gauge,
    total_api_keys: Gauge,
}
```

**暴露端点**:
```
GET /metrics  # Prometheus格式
```

#### 3.2 分布式追踪

**使用 OpenTelemetry**

```rust
// crates/jcode-enterprise-server/src/tracing/otel.rs

use opentelemetry::{global, trace::Tracer};

pub fn init_tracing(service_name: &str, endpoint: &str) {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint);

    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();

    global::set_tracer_provider(provider.clone());
}
```

#### 3.3 日志聚合

**支持 ELK Stack / Loki**

```rust
// 结构化日志输出
tracing_subscriber::fmt()
    .json()
    .with_timer(UtcTime::rfc_3339())
    .init();
```

### Phase 4: 高可用和灾备 (Week 13-16)

#### 4.1 集群部署

**架构**:
```
                    ┌─────────────┐
                    │ Load Balancer│
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
     ┌────────▼───┐ ┌─────▼────┐ ┌─────▼────┐
     │ Node 1     │ │ Node 2   │ │ Node 3   │
     │ (Primary)  │ │(Replica) │ │(Replica) │
     └────────┬───┘ └─────┬────┘ └─────┬────┘
              │            │            │
              └────────────┼────────────┘
                           │
                  ┌────────▼────────┐
                  │ Shared Database │
                  │ (PostgreSQL)    │
                  └─────────────────┘
```

**实现要点**:
- Leader选举 (etcd/Consul)
- 数据复制
- 故障转移
- 健康检查

#### 4.2 备份和恢复

```rust
// crates/jcode-enterprise-server/src/backup/mod.rs

pub struct BackupManager {
    config: BackupConfig,
    storage: Box<dyn BackupStorage>,
}

impl BackupManager {
    pub async fn create_backup(&self) -> Result<BackupMetadata>;
    pub async fn restore_backup(&self, backup_id: &str) -> Result<()>;
    pub async fn list_backups(&self) -> Result<Vec<BackupMetadata>>;
    pub async fn schedule_backup(&self, cron_expr: &str) -> Result<()>;
}
```

**备份策略**:
- 每日全量备份
- 每小时增量备份
- 保留30天
- 异地备份 (S3/GCS)

### Phase 5: 客户端SDK和管理工具 (Week 17-20)

#### 5.1 Python SDK

```python
# carpai-sdk-python/carpai/__init__.py

from carpai import Client

client = Client(
    api_key="your-api-key",
    base_url="https://carpai.example.com"
)

# 聊天完成
response = client.chat.completions.create(
    model="qwen-72b",
    messages=[
        {"role": "user", "content": "Hello"}
    ]
)

# 代码库索引
client.codebase.index("/path/to/repo")

# 用量查询
usage = client.usage.get_current()
```

#### 5.2 TypeScript SDK

```typescript
// carpai-sdk-js/src/index.ts

import { CarpAIClient } from '@carpai/sdk';

const client = new CarpAIClient({
  apiKey: 'your-api-key',
  baseURL: 'https://carpai.example.com'
});

const response = await client.chat.completions.create({
  model: 'qwen-72b',
  messages: [{ role: 'user', content: 'Hello' }]
});
```

#### 5.3 CLI管理工具

```bash
# 安装
pip install carpai-admin

# 使用
carpai-admin org create --name "Acme Corp"
carpai-admin user invite --email dev@example.com
carpai-admin quota set --user user123 --tokens 1000000
carpai-admin audit export --start 2026-01-01 --end 2026-01-31
```

## API端点完整列表

### OpenAI兼容API
```
POST /v1/chat/completions
POST /v1/embeddings
GET  /v1/models
POST /v1/completions  (legacy)
```

### 管理员API
```
# 组织管理
POST   /admin/orgs
GET    /admin/orgs
GET    /admin/orgs/:id
PUT    /admin/orgs/:id
DELETE /admin/orgs/:id

# 用户管理
POST   /admin/users
GET    /admin/users
GET    /admin/users/:id
PUT    /admin/users/:id
DELETE /admin/users/:id

# 角色管理
POST   /admin/roles
GET    /admin/roles
PUT    /admin/roles/:id
DELETE /admin/roles/:id

# 用量统计
GET    /admin/usage/summary
GET    /admin/usage/by-org
GET    /admin/usage/by-user
POST   /admin/usage/reset

# 审计日志
GET    /admin/audit/logs
GET    /admin/audit/logs/:id
POST   /admin/audit/export

# 系统配置
GET    /admin/config
PUT    /admin/config
GET    /admin/health
GET    /admin/metrics
```

### 用户API
```
# 认证
POST   /api/v1/auth/login
POST   /api/v1/auth/logout
POST   /api/v1/auth/refresh
POST   /api/v1/auth/sso/:provider

# API密钥
POST   /api/v1/api-keys
GET    /api/v1/api-keys
DELETE /api/v1/api-keys/:id

# 会话管理
POST   /api/v1/sessions
GET    /api/v1/sessions
GET    /api/v1/sessions/:id
DELETE /api/v1/sessions/:id

# 用量查询
GET    /api/v1/usage/current
GET    /api/v1/usage/history

# 代码库
POST   /api/v1/codebase/index
GET    /api/v1/codebase/status
POST   /api/v1/codebase/search
```

## 性能目标

| 指标 | 目标值 |
|------|--------|
| API响应时间 (P50) | < 100ms |
| API响应时间 (P99) | < 500ms |
| 并发会话数 | > 1000 |
| 每秒请求数 | > 100 RPS |
| 可用性 | 99.9% |
| 数据持久性 | 99.999% |
| 故障恢复时间 | < 30秒 |

## 安全合规

### 安全措施
- [ ] TLS 1.3强制启用
- [ ] 数据加密 (AES-256)
- [ ] 密钥轮换 (每90天)
- [ ] DDoS防护
- [ ] WAF规则
- [ ] 速率限制
- [ ] IP白名单
- [ ] 安全扫描 (SAST/DAST)

### 合规认证
- [ ] SOC 2 Type II
- [ ] ISO 27001
- [ ] GDPR
- [ ] HIPAA (如需要)

## 文档和培训

### 技术文档
- [ ] API参考文档 (OpenAPI/Swagger)
- [ ] 部署指南
- [ ] 运维手册
- [ ] 故障排查指南
- [ ] 最佳实践

### 客户培训
- [ ] 入门教程视频
- [ ] 网络研讨会
- [ ] 案例研究
- [ ] FAQ

## 成功指标

### 技术指标
- [ ] API可用性 > 99.9%
- [ ] P99延迟 < 500ms
- [ ] 零数据丢失
- [ ] 自动化测试覆盖率 > 85%

### 业务指标
- [ ] 支持10+企业客户
- [ ] 月活跃用户 > 1000
- [ ] 客户满意度 > 4.5/5
- [ ] 平均响应时间 < 2小时 (支持工单)

## 风险缓解

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|----------|
| 性能瓶颈 | 高 | 中 | 持续性能测试，水平扩展 |
| 安全漏洞 | 高 | 低 | 定期安全审计，bug bounty |
| 数据丢失 | 高 | 极低 | 多重备份，异地容灾 |
| 依赖中断 | 中 | 低 | 多供应商策略，fallback机制 |

## 总结

通过这20周的增强计划，CarpAI Enterprise Server将达到与Cursor和Claude Code企业版相当的能力水平，同时在以下方面保持优势：

1. **成本效益**: 开源架构，无供应商锁定
2. **灵活性**: 完全可定制，支持私有化部署
3. **性能**: Rust底层，极致优化
4. **本地化**: 中文支持更好，符合国内法规

这将使CarpAI成为中小企业和对数据主权有要求的企业的理想选择。
