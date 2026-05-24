# CarpAI 架构重构计划

**版本**: v0.13.0  
**启动日期**: 2026-05-24  
**目标完成**: 2026-06-21 (4周)  
**状态**: 🟡 规划中

---

## 🎯 重构目标

将CarpAI从"功能完整的单机助手"转型为"生产就绪的企业级AI Server"，达到与 Cursor Enterprise / Claude Code Server 同等的架构成熟度。

### 成功标准

| 指标 | 当前值 | 目标值 | 验收方式 |
|------|--------|--------|---------|
| 顶层模块数 | 247个 | ≤20个 | `grep "^pub mod" src/lib.rs \| wc -l` |
| 编译时间(cargo check) | ~10分钟 | <5分钟 | `time cargo check` |
| 租户隔离覆盖率 | 0% | 100% | 所有DB查询包含org_id过滤 |
| 审计日志覆盖率 | 0% | 关键路径100% | 审计测试用例通过 |
| API错误码统一率 | 30% | 100% | 所有端点返回CarpAiError |
| RBAC拦截率 | 0% | 100% | 跨租户访问被拒绝 |

---

## 📐 目标架构

### 模块重组方案

```
src/
├── lib.rs                    # 仅导出15个领域模块
│
├── core/                     # 核心类型和trait (原 id, util, platform等)
│   ├── types.rs             # 通用类型定义
│   ├── traits.rs            # 基础trait
│   └── error.rs             # 统一错误层次 (新增)
│
├── agent/                    # Agent执行引擎 (保持)
│   ├── runtime.rs           # Agent生命周期
│   ├── turn_execution.rs    # 单轮执行
│   ├── tools.rs             # 工具调用
│   └── memory.rs            # 会话记忆
│
├── api/                      # API层 (原 grpc, api, ws合并)
│   ├── grpc/                # gRPC服务
│   │   ├── session.rs
│   │   ├── chat.rs
│   │   └── ...
│   ├── rest/                # REST API
│   │   ├── completions.rs
│   │   ├── chat.rs
│   │   └── ...
│   ├── websocket/           # WebSocket
│   └── middleware/          # 中间件 (新增)
│       ├── auth.rs          # JWT验证
│       ├── tenant.rs        # 租户上下文 (新增)
│       ├── rate_limit.rs    # 限流
│       └── audit.rs         # 审计日志 (新增)
│
├── auth/                     # 认证授权 (原 auth + enterprise/auth)
│   ├── jwt.rs               # JWT令牌
│   ├── oauth.rs             # OAuth流程
│   ├── sso.rs               # SAML/OIDC (启用enterprise feature)
│   └── rbac.rs              # 基于角色的访问控制
│
├── completion/               # 代码补全 (原 completion_engine)
│   ├── engine.rs
│   ├── providers.rs
│   └── ranking.rs
│
├── memory/                   # 记忆系统 (原 memory, memory_graph等)
│   ├── store.rs             # MemoryStore trait实现
│   ├── tantivy_index.rs     # 全文检索
│   ├── sqlite_store.rs      # SQLite持久化
│   └── pgvector_store.rs    # 向量数据库
│
├── tools/                    # 工具注册和执行 (原 tool, mcp)
│   ├── registry.rs          # ToolRegistry trait实现
│   ├── builtin/             # 内置工具
│   │   ├── shell.rs
│   │   ├── file_ops.rs
│   │   └── ...
│   └── mcp/                 # MCP协议工具
│       ├── client.rs
│       ├── server.rs
│       ├── discovery.rs     # 工具发现
│       ├── orchestration.rs # 工作流编排
│       └── audit_log.rs     # 审计日志
│
├── enterprise/               # 企业功能 (精简)
│   ├── config.rs            # 企业配置
│   ├── quota.rs             # 用量配额
│   ├── usage.rs             # 用量追踪
│   ├── compliance/          # 合规模块
│   │   ├── gdpr.rs
│   │   └── hipaa.rs
│   └── distributed/         # 分布式推理
│       ├── raft.rs
│       └── crdt.rs
│
├── observability/            # 可观测性 (已存在)
│   ├── tracer.rs
│   ├── metrics.rs
│   ├── logger.rs
│   └── config.rs
│
├── config/                   # 配置管理 (整合分散配置)
│   ├── app_config.rs        # 统一配置结构
│   ├── validation.rs        # 配置验证
│   └── env_loader.rs        # 环境变量加载
│
├── infrastructure/           # 基础设施 (新增聚合模块)
│   ├── database.rs          # DB连接池
│   ├── cache.rs             # Redis缓存
│   ├── storage.rs           # 对象存储
│   └── http_client.rs       # HTTP客户端池
│
├── tui/                      # 终端界面 (保持)
├── cli/                      # 命令行 (保持)
└── bin/                      # 二进制入口 (保持)
```

### 关键变更

1. **消除全局状态**:
   ```rust
   // 删除
   static CURRENT_SESSION_ID: Mutex<Option<String>> = Mutex::new(None);

   // 改为依赖注入
   pub struct AgentContext {
       pub session_id: String,
       pub tenant_id: String,
       pub user_id: String,
   }
   ```

2. **统一错误层次**:
   ```rust
   // src/core/error.rs
   #[derive(Debug, thiserror::Error)]
   pub enum CarpAiError {
       #[error("Validation error: {message}")]
       ValidationError { message: String, field: String },

       #[error("Authentication failed: {reason}")]
       AuthError { reason: AuthFailureReason },

       #[error("Resource not found: {resource_type}/{id}")]
       NotFound { resource_type: String, id: String },

       #[error("Rate limit exceeded")]
       RateLimited { retry_after: Duration },

       #[error("Tenant access denied")]
       TenantAccessDenied { requested_tenant: String },

       #[error("Internal server error")]
       Internal { source: anyhow::Error, trace_id: String },
   }

   impl CarpAiError {
       pub fn code(&self) -> &'static str { ... }
       pub fn http_status(&self) -> StatusCode { ... }
       pub fn grpc_status(&self) -> tonic::Status { ... }
   }
   ```

3. **租户上下文中间件**:
   ```rust
   // src/api/middleware/tenant.rs
   #[derive(Clone)]
   pub struct TenantContext {
       pub tenant_id: String,
       pub user_id: String,
       pub permissions: Vec<Permission>,
   }

   pub struct TenantContextMiddleware<S> {
       inner: S,
   }

   impl<S, B> Service<Request<B>> for TenantContextMiddleware<S>
   where
       S: Service<Request<B>, Response = Response>,
   {
       async fn call(&mut self, req: Request<B>) -> Result<Self::Response, Self::Error> {
           // 从JWT提取tenant_id
           let token = extract_jwt(&req)?;
           let claims = verify_jwt(&token)?;

           // 注入上下文
           let ctx = TenantContext {
               tenant_id: claims.org_id,
               user_id: claims.sub,
               permissions: load_permissions(&claims.sub).await?,
           };

           let mut req = req;
           req.extensions_mut().insert(ctx);

           self.inner.call(req).await
       }
   }
   ```

---

## 📅 执行计划（4周）

### Week 1: 模块重组基础

**Day 1-2**: 创建新目录结构
```bash
mkdir -p src/{core,api/middleware,tools/mcp,config,infrastructure}
```

**Day 3-5**: 迁移基础模块
- 移动 `id`, `util`, `platform` → `src/core/`
- 创建 `src/core/error.rs` (统一错误)
- 更新 `src/lib.rs` 导出

**验收**:
- ✅ 顶层模块数降至≤50个
- ✅ `cargo check` 无编译错误

---

### Week 2: 租户隔离实现

**Day 6-7**: 实现TenantContext
- 创建 `src/api/middleware/tenant.rs`
- 集成到Axum router
- 集成到Tonic interceptor

**Day 8-10**: 数据库查询自动过滤
- 创建 `src/infrastructure/database.rs`
- 实现scoped query builder
- 迁移所有SQLite/PostgreSQL查询

**验收**:
- ✅ 所有DB查询包含`WHERE org_id = ?`
- ✅ 跨租户访问测试失败（预期行为）

---

### Week 3: 审计日志与RBAC

**Day 11-12**: 审计日志集成
- 在以下10个关键路径插入审计钩子:
  1. 用户登录/登出
  2. Session创建/删除
  3. 文件读取/写入
  4. 工具执行
  5. LLM推理请求
  6. 记忆检索
  7. 配额检查
  8. 权限变更
  9. 配置修改
  10. 数据导出

**Day 13-15**: RBAC权限拦截
- 创建 `src/auth/rbac.rs` 拦截器
- 集成到gRPC/REST中间件
- 编写权限测试用例

**验收**:
- ✅ 审计日志覆盖10个关键路径
- ✅ 无权限访问返回403 Forbidden

---

### Week 4: API对齐与测试

**Day 16-17**: 统一错误码
- 将所有`anyhow::Result`转换为`Result<(), CarpAiError>`
- 实现`From<CarpAiError>` for `tonic::Status`
- 实现`IntoResponse` for `axum`

**Day 18-19**: gRPC/REST功能对齐
- 实现缺失的REST端点或标记为beta
- 确保gRPC和REST返回相同数据结构

**Day 20**: 集成测试
- 编写端到端测试用例
- 压测对比性能（确保无退化）
- 安全扫描（cargo audit, cargo-deny）

**验收**:
- ✅ 所有API返回结构化错误码
- ✅ 编译时间<5分钟
- ✅ 所有测试通过

---

## 🔍 风险评估

### 高风险项

| 风险 | 缓解策略 | 回滚方案 |
|------|---------|---------|
| 模块重组引入编译错误 | 每次迁移后立即`cargo check` | Git revert单个模块迁移 |
| 租户隔离破坏现有功能 | 保留fallback模式（无tenant_id时使用全局） | Feature flag禁用租户检查 |
| 性能下降 | 审计日志异步批量写入 | 临时关闭审计日志 |

### 监控指标

在重构期间持续监控：
```bash
# 每日运行
cargo check --timings 2>&1 | grep "Finished"
cargo test --workspace 2>&1 | grep "test result"

# 每周运行
cargo audit
cargo outdated
```

---

## 📝 决策记录 (ADR)

### ADR-001: 模块重组策略

**背景**: 247个顶层模块导致维护困难

**决策**: 采用领域驱动设计(DDD)分组为15个领域模块

**理由**:
- 降低认知负荷（新人只需理解15个模块而非247个）
- 减少编译时间（模块间依赖更清晰）
- 便于测试（可按领域独立测试）

**后果**:
- ✅ 正面: 代码导航更容易，PR审查更快
- ⚠️ 负面: 需要更新导入路径（一次性成本）

---

### ADR-002: 租户隔离实现方式

**背景**: 需要防止跨租户数据泄露

**决策**: 采用中间件注入TenantContext + DB查询自动过滤

**理由**:
- 中间件保证所有请求都经过租户验证
- DB层过滤防止应用层遗漏
- 比行级安全(RLS)更灵活（支持复杂权限逻辑）

**后果**:
- ✅ 正面: 强隔离，审计友好
- ⚠️ 负面: 每个查询增加1个JOIN条件（性能影响<5%）

---

## 🚀 下一步行动

**立即执行** (今天):
1. 创建新目录结构
2. 迁移`id`, `util`, `platform`到`src/core/`
3. 创建`src/core/error.rs`骨架

**本周内**:
1. 完成Week 1所有任务
2. 召开团队评审会确认架构方向
3. 更新CI/CD流水线适配新结构

---

**负责人**: AI Engineering Team  
**审核人**: CTO  
**下次更新**: 2026-05-31 (Week 1完成后)
