# CarpAI Enterprise v1.0 — 测试计划

## 1. 测试策略总览

```
E2E (Playwright)    5%
集成测试 (API)     20%
单元测试 (Rust)    70%
静态分析            5%
```

**目标覆盖率**：代码行 > 70%，核心模块 > 85%

## 2. 单元测试计划

| 模块 | 目标率 | 测试重点 |
|------|--------|---------|
| auth-service | 90% | 注册/登录、JWT、RBAC、LDAP mock |
| agent-service | 85% | 回合循环、工具执行、记忆存储 |
| web-service | 70% | API handler、Session 管理 |
| 基础设施 | — | DB migration、Redis 操作 |

### 测试模式
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_user_registration() {
        let db = TestDb::new();
        let svc = AuthService::new(db.pool());
        let result = svc.register("test@example.com", "Password123!").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().email, "test@example.com");
    }
}
```

## 3. 集成测试

| 场景 | 方法 |
|------|------|
| Agent → Auth Token 验证 | HTTP mock |
| Web → Agent Session 查询 | 测试容器 |
| Agent → DB 审计写入 | 数据库断言 |
| Session 过期清理 | 时间模拟 |

## 4. E2E 测试（Playwright）

| ID | 场景 | 关键断言 |
|----|------|---------|
| E2E-01 | 用户注册流程 | 跳转到 Dashboard |
| E2E-02 | LDAP/SSO 登录 | 回跳+已登录 |
| E2E-03 | 创建 Workspace | 列表中出现 |
| E2E-04 | 团队 Session 可见 | B 用户看到 A 的 session |
| E2E-05 | 审计日志搜索 | 操作记录可查 |
| E2E-06 | 权限越界 | viewer 返回 403 |

## 5. 性能测试

| 场景 | 并发 | 时长 | 标准 |
|------|------|------|------|
| API 基线 | 1 | 5min | P99 < 200ms |
| 并发 Agent | 10 | 10min | P99 < 2s |
| 压力峰值 | 50 | 5min | 错误 < 1% |
| 长时间 | 20 | 8h | 无内存泄漏 |

## 6. 质量门禁

| 检查 | 工具 | 阻断 |
|------|------|------|
| 编译错误 | cargo check | ✅ |
| Clippy warnings | cargo clippy | ✅ |
| 测试失败 | cargo test | ✅ |
| 安全漏洞 | cargo audit | ⚠️ |
| 覆盖率 > 70% | cargo tarpaulin | — |

## 7. 测试环境

| 环境 | 配置 | 数据 |
|------|------|------|
| dev | Docker Compose | mock |
| ci | ephemeral | migration 空库 |
| staging | 类生产 | 脱敏数据 |
| perf | 生产同配 | 合成大规模数据 |
