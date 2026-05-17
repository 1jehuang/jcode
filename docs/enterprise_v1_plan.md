# CarpAI Enterprise v1.0 — 开发计划

## 1. 项目时间线（6 个月 · 5 人团队）

```
M1 ───────────────────── M2 ───────────────────── M3 ───────────────────── M4 ───────────────────── M5 ───────────────────── M6
│                        │                        │                        │                        │                        │
├─ Auth + RBAC ──────────┤                        │                        │                        │                        │
├─ DB Schema + migration ┤                        │                        │                        │                        │
├─ Service 拆分 ─────────┤                        │                        │                        │                        │
│                        ├─ Web Console MVP ──────┤                        │                        │                        │
│                        ├─ Workspace 管理 ───────┤                        │                        │                        │
│                        ├─ Session 共享 ─────────┤                        │                        │                        │
│                        │                        ├─ 可观测性 ─────────────┤                        │                        │
│                        │                        ├─ 安全加固 ─────────────┤                        │                        │
│                        │                        ├─ 沙箱执行 ─────────────┤                        │                        │
│                        │                        │                        ├─ Team KB ──────────────┤                        │
│                        │                        │                        ├─ Audit Log ────────────┤                        │
│                        │                        │                        ├─ 性能优化 ─────────────┤                        │
│                        │                        │                        │                        ├─ E2E 测试 ─────────────┤
│                        │                        │                        │                        ├─ Docker 构建 ──────────┤
│                        │                        │                        │                        ├─ 文档 ─────────────────┤
│                        │                        │                        │                        ├─ 安全审计 ─────────────┤
│                        │                        │                        │                        │                        ├─ RC1 ─ RC2 ─ Release
```

## 2. 里程碑

| 里程碑 | 时间 | 交付物 |
|--------|------|--------|
| M1：架构落地 | 第 4 周 | 三服务拆分可运行、Auth 可用、DB schema 到位 |
| M2：MVP | 第 8 周 | Web 控制台可登录、Workspace 管理、Session 共享 |
| M3：核心完备 | 第 12 周 | 可观测性、安全加固、沙箱执行 |
| M4：功能完备 | 第 16 周 | 团队知识库、审计日志、性能达标 |
| M5：质量加固 | 第 20 周 | 测试覆盖 > 70%、安全审计通过 |
| RC1 | 第 22 周 | 功能冻结、仅修 bug |
| GA | 第 24 周 | 正式发布 |

---

## 3. 开发任务列表

### Sprint 1-4: 架构拆分 + Auth（第 1-4 周）

| ID | 任务 | 估算(人天) | 负责人 | 依赖 |
|----|------|-----------|--------|------|
| A-01 | 从 CLI 中提取 Server 核心逻辑 | 5 | SRE | — |
| A-02 | 设计并实现 auth-service 框架（Axum） | 3 | Backend | — |
| A-03 | 设计 DB schema 并编写 migration | 2 | Backend | — |
| A-04 | 实现用户注册/登录 API | 4 | Backend | A-02, A-03 |
| A-05 | 实现 JWT 签发/验证中间件 | 2 | Backend | A-04 |
| A-06 | 集成 LDAP/OIDC  | 5 | Backend | A-02 |
| A-07 | 实现 RBAC 权限检查 | 3 | Backend | A-04 |
| A-08 | 实现 API Token 管理 | 2 | Backend | A-07 |
| A-09 | 将 agent-service 从 cli 拆分为独立进程 | 5 | SRE | A-01 |
| A-10 | 服务间通信协议定义（gRPC/HTTP） | 2 | Backend | A-02, A-09 |
| A-11 | Docker Compose 编排 | 1 | SRE | A-02, A-09 |
| A-12 | CI 基础建设（cargo check + test） | 1 | SRE | — |

### Sprint 5-8: Web Console + Workspace（第 5-8 周）

| ID | 任务 | 估算(人天) | 负责人 | 依赖 |
|----|------|-----------|--------|------|
| B-01 | Web 服务框架搭建（Axum + Tera/HTMX） | 3 | Frontend | A-02 |
| B-02 | 登录页面 + 认证流程 | 2 | Frontend | B-01, A-04 |
| B-03 | 用户管理页面（CRUD + 角色分配） | 3 | Frontend | B-01, A-07 |
| B-04 | Dashboard 仪表盘（用量/活跃度） | 4 | Frontend | B-01 |
| B-05 | Workspace CRUD API | 2 | Backend | A-03 |
| B-06 | Workspace 管理页面 | 2 | Frontend | B-05 |
| B-07 | Session 共享机制设计 | 3 | Backend | A-09 |
| B-08 | Session 持久化到 Redis | 2 | Backend | B-07 |
| B-09 | 团队 Session 列表页面 | 3 | Frontend | B-08 |
| B-10 | Provider 密钥管理 API + 页面 | 3 | Backend | A-07 |
| B-11 | 系统设置页面 | 2 | Frontend | B-10 |

### Sprint 9-12: 可观测性 + 安全（第 9-12 周）

| ID | 任务 | 估算(人天) | 负责人 | 依赖 |
|----|------|-----------|--------|------|
| C-01 | Prometheus metrics 接入 | 3 | SRE | A-02, A-09 |
| C-02 | 结构化日志改造（JSON + tracing） | 3 | Backend | — |
| C-03 | Health Check API | 1 | Backend | A-02 |
| C-04 | 全链路 TLS（nginx + 服务间 mTLS） | 3 | SRE | A-11 |
| C-05 | 敏感字段加密存储 | 3 | Backend | A-03 |
| C-06 | 工具执行沙箱（bubblewrap/容器） | 5 | Backend | A-09 |
| C-07 | SQL 注入防护审查 | 2 | Backend | — |
| C-08 | 添加速率限制（token bucket） | 2 | Backend | A-02 |
| C-09 | 压力测试（50 并发） | 3 | QA | A-02, A-09 |

### Sprint 13-16: 团队知识库 + 审计（第 13-16 周）

| ID | 任务 | 估算(人天) | 负责人 | 依赖 |
|----|------|-----------|--------|------|
| D-01 | Prompt 模板管理系统 | 4 | Backend | B-05 |
| D-02 | 模板管理页面 | 3 | Frontend | D-01 |
| D-03 | 团队 Tool 配置共享 | 3 | Backend | B-05 |
| D-04 | 代码规范配置 | 2 | Backend | B-05 |
| D-05 | 审计日志 API | 3 | Backend | A-03 |
| D-06 | 审计日志搜索页面 | 3 | Frontend | D-05 |
| D-07 | 使用统计报告（日/周/月） | 4 | Backend | A-03 |
| D-08 | 用量告警配置 | 3 | Backend | C-01 |
| D-09 | 性能基准测试 + 调优 | 5 | SRE | — |

### Sprint 17-20: 质量加固（第 17-20 周）

| ID | 任务 | 估算(人天) | 负责人 | 依赖 |
|----|------|-----------|--------|------|
| E-01 | 单元测试补充（target > 70%） | 15 | All | — |
| E-02 | 集成测试（API 级别） | 10 | QA | — |
| E-03 | E2E 测试（Playwright） | 8 | QA | B-01 |
| E-04 | 安全渗透测试 | 5 | Security | — |
| E-05 | 文档：部署指南 | 3 | SRE | — |
| E-06 | 文档：用户手册 | 5 | Tech Writer | — |
| E-07 | 文档：API 参考 | 3 | Backend | — |
| E-08 | Docker 镜像优化（多阶段构建） | 2 | SRE | — |

### Sprint 21-24: 发布（第 21-24 周）

| ID | 任务 | 估算(人天) | 负责人 | 依赖 |
|----|------|-----------|--------|------|
| F-01 | RC1 发布 + 内部试用 | 5 | All | E 系列 |
| F-02 | Bug 修复迭代 | 10 | All | F-01 |
| F-03 | RC2 发布 | 2 | SRE | F-02 |
| F-04 | 最终安全审计 | 3 | Security | F-03 |
| F-05 | GA Release | 2 | SRE | F-04 |

---

## 4. 质量保证计划

### 4.1 代码质量门禁

| 检查项 | 工具 | 阈值 | 阻断 |
|--------|------|------|------|
| 编译 | `cargo check` | 零错误 | ✅ |
| Lint | `cargo clippy` | 零 warning | ✅ |
| 格式 | `cargo fmt` | diff 为零 | ✅ |
| 测试 | `cargo test` | 全部通过 | ✅ |
| 安全 | `cargo audit` | 零漏洞 | ⚠️ |
| 覆盖率 | `cargo tarpaulin` | > 70% | — |
| 重复代码 | `cargo dinghy` | < 5% | — |

### 4.2 代码审查流程

```
PR 提交 → CI 自动检查 → 至少 1 人 Review → Approve → Merge
                              │
                    ┌─────────┴────────┐
                    │ - 架构合规        │
                    │ - 测试覆盖新增     │
                    │ - 无安全反模式     │
                    └──────────────────┘
```

### 4.3 环境策略

```
dev（开发者本地）→ staging（CI 自动部署）→ rc（手动验证）→ production
```

### 4.4 发布策略

```
版本号: v1.0.0-rc.1 → v1.0.0-rc.2 → v1.0.0
补丁:   v1.0.1 (仅含 bug 修复，无新功能)
热修复: v1.0.1-hotfix.1 (紧急安全/故障修复)
```
