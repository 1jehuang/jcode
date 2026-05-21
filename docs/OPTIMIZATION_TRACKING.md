# CarpAI 优化项目跟踪

本文档追踪 CarpAI 全面优化路线图的实施进度，包含所有 Epic 和子任务的详细列表。

**最后更新**: 2026-05-21

---

## 🎯 Epic 总览

| Epic ID | 标题 | 优先级 | 状态 | 完成度 | 负责人 | 目标完成日期 |
|---------|------|--------|------|--------|--------|-------------|
| EPIC-001 | 企业功能集成到主流程 | P0 | 🔴 未开始 | 0% | TBD | 2026-06-18 |
| EPIC-002 | WebSocket 协作编辑连接 | P0 | 🔴 未开始 | 0% | TBD | 2026-07-16 |
| EPIC-003 | CRDT/OT 算法补齐 | P0 | 🔴 未开始 | 0% | TBD | 2026-08-13 |
| EPIC-004 | 代码质量提升 - 大文件拆分 | P1 | 🔴 未开始 | 0% | TBD | 2026-09-10 |
| EPIC-005 | TUI/Web 协作 UI 实现 | P1 | 🔴 未开始 | 0% | TBD | 2026-10-08 |
| EPIC-006 | 测试覆盖率提升到 70% | P1 | 🔴 未开始 | 0% | TBD | 2026-11-05 |
| EPIC-007 | AER 自动错误修复系统 | P2 | 🔴 未开始 | 0% | TBD | 2027-01-30 |
| EPIC-008 | 分布式集群生产化 | P2 | 🔴 未开始 | 0% | TBD | 2027-02-27 |
| EPIC-009 | SSO/LDAP 完整集成 | P2 | 🔴 未开始 | 0% | TBD | 2027-03-27 |

---

## 📦 EPIC-001: 企业功能集成到主流程

**优先级**: P0 (立即执行)
**预估工作量**: 4 周
**状态**: 🔴 未开始

### 业务价值
激活已实现的 `jcode-auth` crate，使 Enterprise Server 具备完整的 OAuth2/JWT/RBAC/审计/GDPR 功能，达到生产就绪状态。

### 验收标准
- ✅ 所有认证请求经过 jcode-auth 的 JwtManager
- ✅ RBAC 权限检查覆盖 100% 敏感 API
- ✅ 审计日志持久化到 PostgreSQL
- ✅ GDPR 同意记录可查询和管理
- ✅ Admin API 返回真实数据而非空数组

### 子任务列表

#### Week 1: jcode-auth 集成准备
- [ ] **TASK-001** - 在 enterprise-server Cargo.toml 中添加 jcode-auth 依赖
  - 文件: `crates/jcode-enterprise-server/Cargo.toml`
  - 预估: 0.5 人天
  - 标签: `type: task`, `priority: P0`, `component: auth`

- [ ] **TASK-002** - 创建 EnterpriseAuthMiddleware 包装器
  - 文件: `crates/jcode-enterprise-server/src/middleware/auth.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: auth`

- [ ] **TASK-003** - 编写 JWT 验证集成测试
  - 文件: `crates/jcode-enterprise-server/tests/auth_integration.rs` (新建)
  - 预估: 1.5 人天
  - 标签: `type: test`, `priority: P0`, `component: auth`

- [ ] **TASK-004** - 更新环境变量配置文档
  - 文件: `docs/ENTERPRISE_ENV_CONFIG.md` (新建)
  - 预估: 0.5 人天
  - 标签: `type: docs`, `priority: P0`, `component: auth`

#### Week 2: RBAC 中间件实现
- [ ] **TASK-005** - 实现 require_permission() Axum 中间件
  - 文件: `crates/jcode-enterprise-server/src/middleware/rbac.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: auth`

- [ ] **TASK-006** - 在所有 Admin API 路由上应用权限检查
  - 文件: `crates/jcode-enterprise-server/src/admin_api/*.rs`
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: auth`

- [ ] **TASK-007** - 实现角色分配管理 API
  - 文件: `crates/jcode-enterprise-server/src/admin_api/roles.rs` (新建)
  - 预估: 1.5 人天
  - 标签: `type: task`, `priority: P0`, `component: auth`

- [ ] **TASK-008** - 添加权限拒绝的审计日志
  - 文件: `crates/jcode-enterprise-server/src/middleware/rbac.rs`
  - 预估: 1 人天
  - 标签: `type: task`, `priority: P0`, `component: audit`

#### Week 3: 审计日志持久化
- [ ] **TASK-009** - 实现 PostgresAuditStorage
  - 文件: `crates/jcode-auth/src/storage/postgres.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: task`, `priority: P0`, `component: audit`

- [ ] **TASK-010** - 创建数据库迁移脚本
  - 文件: `migrations/001_create_audit_log.sql` (新建)
  - 预估: 1 人天
  - 标签: `type: task`, `priority: P0`, `component: audit`

- [ ] **TASK-011** - 实现定时清理任务（保留策略）
  - 文件: `crates/jcode-enterprise-server/src/tasks/audit_cleanup.rs` (新建)
  - 预估: 1.5 人天
  - 标签: `type: task`, `priority: P0`, `component: audit`

- [ ] **TASK-012** - 添加审计日志查询 API
  - 文件: `crates/jcode-enterprise-server/src/admin_api/audit.rs` (新建)
  - 预估: 1.5 人天
  - 标签: `type: task`, `priority: P0`, `component: audit`

#### Week 4: GDPR 合规激活
- [ ] **TASK-013** - 实现同意管理 API
  - 文件: `crates/jcode-enterprise-server/src/api/consent.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: gdpr`

- [ ] **TASK-014** - 添加数据导出 API（JSON/CSV）
  - 文件: `crates/jcode-enterprise-server/src/api/data_export.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: gdpr`

- [ ] **TASK-015** - 实现数据删除请求处理
  - 文件: `crates/jcode-enterprise-server/src/api/deletion_request.rs` (新建)
  - 预估: 1.5 人天
  - 标签: `type: task`, `priority: P0`, `component: gdpr`

- [ ] **TASK-016** - 编写 GDPR 合规模板文档
  - 文件: `docs/GDPR_COMPLIANCE_GUIDE.md` (新建)
  - 预估: 1 人天
  - 标签: `type: docs`, `priority: P0`, `component: gdpr`

---

## 📦 EPIC-002: WebSocket 协作编辑连接

**优先级**: P0 (立即执行)
**预估工作量**: 4 周
**状态**: 🔴 未开始

### 业务价值
激活多人实时协作编辑功能，解决 `src/ws/handlers/collab.rs` 中的 TODO 项，实现真正的协同编辑能力。

### 验收标准
- ✅ 多用户可同时编辑同一文档
- ✅ 编辑操作正确广播到所有协作者
- ✅ 光标位置实时同步（延迟 < 200ms）
- ✅ 重连后能恢复未接收的操作
- ✅ 无数据丢失或冲突

### 子任务列表

#### Week 5: WebSocket Handler 完善
- [ ] **TASK-017** - 实现 handle_edit() 完整逻辑
  - 文件: `src/ws/handlers/collab.rs`
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: collaboration`

- [ ] **TASK-018** - 连接光标广播到 PresenceManager
  - 文件: `src/ws/handlers/collab.rs`, `src/server/collab.rs`
  - 预估: 1.5 人天
  - 标签: `type: task`, `priority: P0`, `component: collaboration`

- [ ] **TASK-019** - 实现操作重放机制（重连场景）
  - 文件: `src/server/collab.rs`
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: collaboration`

- [ ] **TASK-020** - 添加协作编辑集成测试
  - 文件: `tests/collaboration_tests.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P0`, `component: collaboration`

#### Week 6-8: 冲突解决与优化
- [ ] **TASK-021** - 完善 ConflictResolver 的 OT 策略
  - 文件: `src/server/collab.rs`
  - 预估: 3 人天
  - 标签: `type: task`, `priority: P0`, `component: collaboration`

- [ ] **TASK-022** - 添加冲突检测与解决 UI 提示
  - 文件: `src/ws/protocol.rs`, `src/tui/widgets/mod.rs`
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P1`, `component: collaboration`

- [ ] **TASK-023** - 性能优化：操作批处理
  - 文件: `src/server/collab.rs`
  - 预估: 2 人天
  - 标签: `type: perf`, `priority: P1`, `component: collaboration`

- [ ] **TASK-024** - 编写协作编辑基准测试
  - 文件: `benches/collaboration_bench.rs` (新建)
  - 预估: 1.5 人天
  - 标签: `type: test`, `priority: P1`, `component: collaboration`

---

## 📦 EPIC-003: CRDT/OT 算法补齐

**优先级**: P0 (阻塞性)
**预估工作量**: 6 周
**状态**: 🔴 未开始

### 业务价值
实现生产级 CRDT 算法，保证并发编辑的最终一致性，这是协作编辑的核心技术保障。

### 技术方案选择
**推荐**: 集成 `yrs` (Yjs Rust 实现)
- 成熟稳定，已被 Yjs JavaScript 版本验证
- 活跃的社区和维护
- 良好的性能表现

**备选**: `automerge`
- 更简单的 API
- 但性能较差，内存占用高

### 验收标准
- ✅ 集成 yrs 到项目中
- ✅ 实现 Text CRDT 用于文档编辑
- ✅ 支持至少 20 人同时在线编辑
- ✅ 并发冲突率 < 0.1%
- ✅ 操作应用延迟 < 50ms (P95)

### 子任务列表

#### Week 9-10: CRDT 引擎选型与 PoC
- [ ] **TASK-025** - 评估 yrs vs automerge 性能对比
  - 文件: `crates/jcode-collab/benches/crdt_bench.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: research`, `priority: P0`, `component: crdt`

- [ ] **TASK-026** - 创建 PoC 验证技术方案
  - 文件: `examples/crdt_poc.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: task`, `priority: P0`, `component: crdt`

- [ ] **TASK-027** - 设计 API 适配层
  - 文件: `crates/jcode-collab/src/crdt_adapter.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: task`, `priority: P0`, `component: crdt`

#### Week 11-14: 集成与测试
- [ ] **TASK-028** - 集成 yrs 到 CollaborationServer
  - 文件: `src/server/collab.rs`, `crates/jcode-collab/Cargo.toml`
  - 预估: 4 人天
  - 标签: `type: task`, `priority: P0`, `component: crdt`

- [ ] **TASK-029** - 实现操作转换层
  - 文件: `crates/jcode-collab/src/operation_transform.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: task`, `priority: P0`, `component: crdt`

- [ ] **TASK-030** - 编写 CRDT 一致性测试
  - 文件: `crates/jcode-collab/tests/consistency_tests.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: test`, `priority: P0`, `component: crdt`

- [ ] **TASK-031** - 压力测试：20+ 并发用户
  - 文件: `tests/load_tests/collaboration_load.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P0`, `component: crdt`

---

## 📦 EPIC-004: 代码质量提升 - 大文件拆分

**优先级**: P1 (短期优先)
**预估工作量**: 8 周
**状态**: 🔴 未开始

### 业务价值
改善代码可维护性，降低编译时间，减少 bug 引入风险。

### 目标文件
- `src/server/comm_control.rs` (3228 LOC) → 拆分为 4-5 个子模块
- `src/tool/communicate.rs` (3165 LOC) → 拆分为 4-5 个子模块
- `src/session.rs` (2729 LOC) → 拆分为 3-4 个子模块
- `src/server/client_lifecycle.rs` (2704 LOC) → 拆分为 3-4 个子模块
- `src/provider/openai.rs` (2683 LOC) → 拆分为 4 个子模块

### 验收标准
- ✅ 所有超过 1200 LOC 的文件拆分完成
- ✅ 平均文件大小 < 800 LOC
- ✅ 所有测试通过，无回归 bug
- ✅ 编译时间减少 15%
- ✅ 代码审查通过率提升 30%

### 子任务列表

#### Week 15-16: Server 模块拆分
- [ ] **TASK-032** - 拆分 comm_control.rs
  - 文件: `src/server/comm_control/{mod,connection,router,protocol,rate_limit}.rs`
  - 预估: 3 人天
  - 标签: `type: refactor`, `priority: P1`, `component: server`

- [ ] **TASK-033** - 拆分 client_lifecycle.rs
  - 文件: `src/server/client_lifecycle/{mod,bootstrap,health_check,shutdown}.rs`
  - 预估: 2.5 人天
  - 标签: `type: refactor`, `priority: P1`, `component: server`

#### Week 17-18: Provider 模块拆分
- [ ] **TASK-034** - 拆分 provider/openai.rs
  - 文件: `src/provider/openai/{mod,request,stream,tool,response}.rs`
  - 预估: 3 人天
  - 标签: `type: refactor`, `priority: P1`, `component: provider`

- [ ] **TASK-035** - 拆分 provider/mod.rs
  - 文件: `src/provider/{mod,traits,pricing,routes,helpers}.rs`
  - 预估: 2.5 人天
  - 标签: `type: refactor`, `priority: P1`, `component: provider`

#### Week 19-20: Session 和其他大文件
- [ ] **TASK-036** - 拆分 session.rs
  - 文件: `src/session/{mod,state,persistence,lifecycle}.rs`
  - 预估: 3 人天
  - 标签: `type: refactor`, `priority: P1`, `component: session`

- [ ] **TASK-037** - 拆分 tool/communicate.rs
  - 文件: `src/tool/communicate/{mod,protocol,handler,utils}.rs`
  - 预估: 3 人天
  - 标签: `type: refactor`, `priority: P1`, `component: tool`

#### Week 21-22: 测试与验证
- [ ] **TASK-038** - 补充拆分后的单元测试
  - 文件: 各子模块的 `#[cfg(test)]` 块
  - 预估: 4 人天
  - 标签: `type: test`, `priority: P1`, `component: quality`

- [ ] **TASK-039** - 运行完整回归测试套件
  - 文件: N/A
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P1`, `component: quality`

- [ ] **TASK-040** - 性能基准对比（编译时间）
  - 文件: `scripts/bench_compile.sh`
  - 预估: 1 人天
  - 标签: `type: perf`, `priority: P1`, `component: quality`

---

## 📦 EPIC-005: TUI/Web 协作 UI 实现

**优先级**: P1 (短期优先)
**预估工作量**: 6 周
**状态**: 🔴 未开始

### 业务价值
提供用户可见的协作体验，达到 Claude Code 水平的可视化协作界面。

### 验收标准
- ✅ TUI 中显示远程光标（不同颜色标识）
- ✅ 协作者列表面板实时更新
- ✅ 打字指示器显示
- ✅ 冲突提示和解决界面
- ✅ 光标同步延迟 < 100ms (P95)

### 子任务列表

#### Week 23-24: TUI 协作组件
- [ ] **TASK-041** - 实现 CollaborationPanel widget
  - 文件: `src/tui/widgets/collaboration_panel.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: feature`, `priority: P1`, `component: tui`

- [ ] **TASK-042** - 添加远程光标渲染
  - 文件: `src/tui/widgets/remote_cursor.rs` (新建)
  - 预估: 2.5 人天
  - 标签: `type: feature`, `priority: P1`, `component: tui`

- [ ] **TASK-043** - 实现协作者列表侧边栏
  - 文件: `src/tui/widgets/collaborator_list.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: feature`, `priority: P1`, `component: tui`

#### Week 25-26: 交互与优化
- [ ] **TASK-044** - 添加打字指示器显示
  - 文件: `src/tui/widgets/typing_indicator.rs` (新建)
  - 预估: 1.5 人天
  - 标签: `type: feature`, `priority: P1`, `component: tui`

- [ ] **TASK-045** - 实现冲突提示 UI
  - 文件: `src/tui/widgets/conflict_dialog.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: feature`, `priority: P1`, `component: tui`

- [ ] **TASK-046** - 性能优化：减少 UI 刷新频率
  - 文件: `src/tui/app.rs`
  - 预估: 1.5 人天
  - 标签: `type: perf`, `priority: P1`, `component: tui`

#### Week 27-28: 测试与文档
- [ ] **TASK-047** - 编写 TUI 协作组件测试
  - 文件: `src/tui/widgets/tests/collaboration_tests.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P1`, `component: tui`

- [ ] **TASK-048** - 更新用户文档
  - 文件: `docs/TUI_COLLABORATION_GUIDE.md` (新建)
  - 预估: 1 人天
  - 标签: `type: docs`, `priority: P1`, `component: tui`

---

## 📦 EPIC-006: 测试覆盖率提升到 70%

**优先级**: P1 (短期优先)
**预估工作量**: 6 周
**状态**: 🔴 未开始

### 业务价值
显著提升代码可靠性，减少回归 bug，增强重构信心。

### 验收标准
- ✅ 整体项目测试覆盖率 ≥ 70%
- ✅ jcode-auth 覆盖率 ≥ 85%
- ✅ jcode-completion 覆盖率 ≥ 75%
- ✅ src/server/* 覆盖率 ≥ 70%
- ✅ CI 中集成覆盖率检查

### 子任务列表

#### Week 29-30: 基础设施搭建
- [ ] **TASK-049** - 集成 cargo-tarpaulin 到 CI
  - 文件: `.github/workflows/ci.yml`, `Cargo.toml`
  - 预估: 1 人天
  - 标签: `type: ci`, `priority: P1`, `component: testing`

- [ ] **TASK-050** - 配置 Codecov 集成
  - 文件: `.github/workflows/ci.yml`
  - 预估: 0.5 人天
  - 标签: `type: ci`, `priority: P1`, `component: testing`

- [ ] **TASK-051** - 建立覆盖率基线报告
  - 文件: `docs/TEST_COVERAGE_BASELINE.md` (新建)
  - 预估: 1 人天
  - 标签: `type: docs`, `priority: P1`, `component: testing`

#### Week 31-34: 核心模块测试补充
- [ ] **TASK-052** - jcode-auth 单元测试补充
  - 文件: `crates/jcode-auth/src/*/tests.rs`
  - 预估: 4 人天
  - 标签: `type: test`, `priority: P1`, `component: auth`

- [ ] **TASK-053** - jcode-completion 测试补充
  - 文件: `crates/jcode-completion/tests/*.rs`
  - 预估: 4 人天
  - 标签: `type: test`, `priority: P1`, `component: completion`

- [ ] **TASK-054** - server 模块测试补充
  - 文件: `src/server/*/tests.rs`
  - 预估: 5 人天
  - 标签: `type: test`, `priority: P1`, `component: server`

#### Week 35-36: 集成测试与 E2E
- [ ] **TASK-055** - 添加关键路径集成测试
  - 文件: `tests/integration/*.rs`
  - 预估: 3 人天
  - 标签: `type: test`, `priority: P1`, `component: testing`

- [ ] **TASK-056** - 实现 Mock Provider 用于 E2E 测试
  - 文件: `tests/e2e/mock_provider_enhanced.rs`
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P1`, `component: testing`

- [ ] **TASK-057** - 生成最终覆盖率报告
  - 文件: `docs/TEST_COVERAGE_REPORT_FINAL.md` (新建)
  - 预估: 0.5 人天
  - 标签: `type: docs`, `priority: P1`, `component: testing`

---

## 📦 EPIC-007: AER 自动错误修复系统

**优先级**: P2 (中期规划)
**预估工作量**: 12 周
**状态**: 🔴 未开始

### 业务价值
实现 AI 驱动的错误检测和修复，提升开发者效率 30%，类似 GitHub Copilot Fix。

### 验收标准
- ✅ 基于 LSP 的错误检测引擎完成
- ✅ 常见错误模式规则库（50+ 规则）
- ✅ 修复建议准确率 > 85%
- ✅ 沙箱执行验证修复安全性
- ✅ 用户反馈循环实现

### 子任务列表

#### Phase 1: 基础架构（Week 37-40）
- [ ] **TASK-058** - 设计 AER 系统架构
  - 文件: `docs/AER_ARCHITECTURE.md` (新建)
  - 预估: 2 人天
  - 标签: `type: design`, `priority: P2`, `component: aer`

- [ ] **TASK-059** - 实现 LSP 错误检测引擎
  - 文件: `crates/jcode-aer/src/lsp_detector.rs` (新建)
  - 预估: 4 人天
  - 标签: `type: feature`, `priority: P2`, `component: aer`

- [ ] **TASK-060** - 构建错误模式规则库（50+ 规则）
  - 文件: `crates/jcode-aer/src/rules/*.rs` (新建)
  - 预估: 5 人天
  - 标签: `type: feature`, `priority: P2`, `component: aer`

#### Phase 2: 修复生成（Week 41-44）
- [ ] **TASK-061** - 实现修复建议生成器
  - 文件: `crates/jcode-aer/src/fix_generator.rs` (新建)
  - 预估: 4 人天
  - 标签: `type: feature`, `priority: P2`, `component: aer`

- [ ] **TASK-062** - 集成小型 LLM 用于智能修复
  - 文件: `crates/jcode-aer/src/llm_fixer.rs` (新建)
  - 预估: 5 人天
  - 标签: `type: feature`, `priority: P2`, `component: aer`

- [ ] **TASK-063** - 实现沙箱执行验证
  - 文件: `crates/jcode-aer/src/sandbox.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: feature`, `priority: P2`, `component: aer`

#### Phase 3: 反馈与优化（Week 45-48）
- [ ] **TASK-064** - 实现用户反馈收集
  - 文件: `crates/jcode-aer/src/feedback.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: feature`, `priority: P2`, `component: aer`

- [ ] **TASK-065** - 强化学习优化修复建议
  - 文件: `crates/jcode-aer/src/optimizer.rs` (新建)
  - 预估: 4 人天
  - 标签: `type: feature`, `priority: P2`, `component: aer`

- [ ] **TASK-066** - AER 系统集成测试
  - 文件: `crates/jcode-aer/tests/e2e_tests.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: test`, `priority: P2`, `component: aer`

---

## 📦 EPIC-008: 分布式集群生产化

**优先级**: P2 (中期规划)
**预估工作量**: 10 周
**状态**: 🔴 未开始

### 业务价值
支持大规模部署，提供高可用性保障，实现真正的分布式集群能力。

### 验收标准
- ✅ 集成 openraft 共识算法
- ✅ 实现真实的 gRPC 通信层
- ✅ 节点故障恢复时间 < 30s
- ✅ 支持动态节点添加/移除
- ✅ 负载均衡策略完整实现

### 子任务列表

#### Week 49-52: Raft 集成
- [ ] **TASK-067** - 集成 openraft 到项目
  - 文件: `crates/jcode-raft/Cargo.toml` (新建)
  - 预估: 2 人天
  - 标签: `type: feature`, `priority: P2`, `component: distributed`

- [ ] **TASK-068** - 实现 Raft 状态机
  - 文件: `crates/jcode-raft/src/state_machine.rs` (新建)
  - 预估: 4 人天
  - 标签: `type: feature`, `priority: P2`, `component: distributed`

- [ ] **TASK-069** - 替换模拟 RPC 为真实 gRPC
  - 文件: `src/distributed/election.rs`, `src/distributed/rpc.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: feature`, `priority: P2`, `component: distributed`

#### Week 53-56: 服务发现与故障转移
- [ ] **TASK-070** - 实现节点自动发现（mDNS/Consul）
  - 文件: `src/distributed/discovery.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: feature`, `priority: P2`, `component: distributed`

- [ ] **TASK-071** - 实现故障转移机制
  - 文件: `src/distributed/failover.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: feature`, `priority: P2`, `component: distributed`

- [ ] **TASK-072** - 完善负载均衡策略
  - 文件: `src/distributed/load_balancer.rs`
  - 预估: 2 人天
  - 标签: `type: feature`, `priority: P2`, `component: distributed`

#### Week 57-58: 测试与优化
- [ ] **TASK-073** - 分布式一致性测试
  - 文件: `src/distributed/tests/consistency_tests.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: test`, `priority: P2`, `component: distributed`

- [ ] **TASK-074** - 故障注入测试
  - 文件: `src/distributed/tests/fault_injection.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P2`, `component: distributed`

---

## 📦 EPIC-009: SSO/LDAP 完整集成

**优先级**: P2 (中期规划)
**预估工作量**: 8 周
**状态**: 🔴 未开始

### 业务价值
企业客户必备功能，支持 Active Directory 等企业身份提供商，扩大目标市场。

### 验收标准
- ✅ LDAP provider 完整实现
- ✅ SAML provider 完整实现
- ✅ 用户目录同步功能
- ✅ 单点登出（SLO）支持
- ✅ SSO 登录成功率 > 99%

### 子任务列表

#### Week 59-62: LDAP 集成
- [ ] **TASK-075** - 实现 LDAP provider
  - 文件: `src/auth/sso/ldap.rs` (新建)
  - 预估: 4 人天
  - 标签: `type: feature`, `priority: P2`, `component: sso`

- [ ] **TASK-076** - 实现用户目录同步
  - 文件: `src/auth/sso/sync.rs` (新建)
  - 预估: 3 人天
  - 标签: `type: feature`, `priority: P2`, `component: sso`

- [ ] **TASK-077** - LDAP 集成测试
  - 文件: `src/auth/sso/tests/ldap_tests.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P2`, `component: sso`

#### Week 63-66: SAML 集成
- [ ] **TASK-078** - 实现 SAML provider
  - 文件: `src/auth/sso/saml.rs` (新建)
  - 预估: 4 人天
  - 标签: `type: feature`, `priority: P2`, `component: sso`

- [ ] **TASK-079** - 实现单点登出（SLO）
  - 文件: `src/auth/sso/slo.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: feature`, `priority: P2`, `component: sso`

- [ ] **TASK-080** - SAML 集成测试
  - 文件: `src/auth/sso/tests/saml_tests.rs` (新建)
  - 预估: 2 人天
  - 标签: `type: test`, `priority: P2`, `component: sso`

#### Week 67-68: 文档与部署
- [ ] **TASK-081** - 编写 SSO 配置指南
  - 文件: `docs/SSO_CONFIGURATION_GUIDE.md` (新建)
  - 预估: 1.5 人天
  - 标签: `type: docs`, `priority: P2`, `component: sso`

- [ ] **TASK-082** - 企业部署文档更新
  - 文件: `docs/ENTERPRISE_DEPLOYMENT.md` (更新)
  - 预估: 1 人天
  - 标签: `type: docs`, `priority: P2`, `component: sso`

---

## 📊 总体进度仪表板

### 按优先级统计

| 优先级 | Epic 数量 | 总任务数 | 已完成 | 进行中 | 未开始 | 完成度 |
|--------|----------|---------|--------|--------|--------|--------|
| P0 | 3 | 31 | 0 | 0 | 31 | 0% |
| P1 | 3 | 26 | 0 | 0 | 26 | 0% |
| P2 | 3 | 25 | 0 | 0 | 25 | 0% |
| **总计** | **9** | **82** | **0** | **0** | **82** | **0%** |

### 按组件统计

| 组件 | 任务数 | 完成度 |
|------|--------|--------|
| Auth | 16 | 0% |
| Collaboration | 14 | 0% |
| CRDT | 7 | 0% |
| Quality | 13 | 0% |
| TUI | 8 | 0% |
| Testing | 11 | 0% |
| AER | 9 | 0% |
| Distributed | 8 | 0% |
| SSO | 8 | 0% |

---

## 🔗 快速链接

- **[优化路线图](carpai-optimization-roadmap.md)** - 详细的实施计划和资源需求
- **[代码质量待办](CODE_QUALITY_TODO.md)** - 代码质量提升的具体任务
- **[企业认证指南](ENTERPRISE_AUTH_SETUP.md)** - jcode-auth 集成文档
- **[企业版计划](enterprise_v1_plan.md)** - 企业版开发时间线

---

## 📝 使用说明

### 创建新 Issue

1. 从模板创建: 使用 `.github/ISSUE_TEMPLATE/` 中的模板
2. 添加标签: 确保添加正确的优先级和组件标签
3. 关联 Epic: 在 Issue 描述中引用所属 Epic (#EPIC-XXX)
4. 指派负责人: 根据团队分工指派具体负责人

### 更新进度

1. 完成任务后勾选对应的 checkbox
2. 更新 Epic 的进度表格
3. 在 Issue 中添加完成日期和 PR 链接
4. 更新总体进度仪表板

### 标签规范

- **类型**: `type: task`, `type: test`, `type: docs`, `type: refactor`, `type: feature`, `type: perf`, `type: ci`, `type: design`, `type: research`
- **优先级**: `priority: P0`, `priority: P1`, `priority: P2`, `priority: P3`
- **组件**: `component: auth`, `component: collaboration`, `component: crdt`, `component: server`, `component: tui`, `component: testing`, `component: aer`, `component: distributed`, `component: sso`, `component: quality`, `component: provider`, `component: session`, `component: tool`, `component: gdpr`, `component: audit`
- **状态**: `status: todo`, `status: in-progress`, `status: review`, `status: done`
