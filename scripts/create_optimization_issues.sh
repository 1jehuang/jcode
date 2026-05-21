#!/bin/bash
# CarpAI Optimization Issues Creator
# This script creates GitHub Issues for all tasks in the optimization roadmap
# Requires: GitHub CLI (gh) configured with appropriate permissions

set -e

REPO="1jehuang/jcode"  # Update with actual repo
TRACKING_FILE="docs/OPTIMIZATION_TRACKING.md"

echo "🚀 Starting CarpAI Optimization Issues Creation..."
echo "Repository: $REPO"
echo ""

# Function to create an issue from template
create_issue() {
    local title="$1"
    local body_file="$2"
    local labels="$3"

    echo "Creating issue: $title"
    gh issue create \
        --repo "$REPO" \
        --title "$title" \
        --body-file "$body_file" \
        --label "$labels" \
        --assignee "@me"

    echo "✅ Created: $title"
    echo ""
}

# Create Epic issues first
echo "=== Creating Epic Issues ==="
echo ""

# EPIC-001
cat > /tmp/epic001.md << 'EOF'
# EPIC-001: 企业功能集成到主流程

**优先级**: P0
**预估工作量**: 4 周

激活已实现的 `jcode-auth` crate，使 Enterprise Server 具备完整的 OAuth2/JWT/RBAC/审计/GDPR 功能。

## 子任务
- TASK-001 到 TASK-016 (详见 OPTIMIZATION_TRACKING.md)

## 验收标准
- ✅ 所有认证请求经过 jcode-auth 的 JwtManager
- ✅ RBAC 权限检查覆盖 100% 敏感 API
- ✅ 审计日志持久化到 PostgreSQL
- ✅ GDPR 同意记录可查询和管理
EOF

create_issue "[EPIC-001] 企业功能集成到主流程" "/tmp/epic001.md" "type: epic,priority: P0,component: auth"

# EPIC-002
cat > /tmp/epic002.md << 'EOF'
# EPIC-002: WebSocket 协作编辑连接

**优先级**: P0
**预估工作量**: 4 周

激活多人实时协作编辑功能，解决 src/ws/handlers/collab.rs 中的 TODO 项。

## 子任务
- TASK-017 到 TASK-024 (详见 OPTIMIZATION_TRACKING.md)

## 验收标准
- ✅ 多用户可同时编辑同一文档
- ✅ 编辑操作正确广播到所有协作者
- ✅ 光标位置实时同步（延迟 < 200ms）
EOF

create_issue "[EPIC-002] WebSocket 协作编辑连接" "/tmp/epic002.md" "type: epic,priority: P0,component: collaboration"

# EPIC-003
cat > /tmp/epic003.md << 'EOF'
# EPIC-003: CRDT/OT 算法补齐

**优先级**: P0 (阻塞性)
**预估工作量**: 6 周

实现生产级 CRDT 算法，保证并发编辑的最终一致性。

## 技术方案
推荐集成 `yrs` (Yjs Rust 实现)

## 子任务
- TASK-025 到 TASK-031 (详见 OPTIMIZATION_TRACKING.md)

## 验收标准
- ✅ 集成 yrs 到项目中
- ✅ 支持至少 20 人同时在线编辑
- ✅ 并发冲突率 < 0.1%
EOF

create_issue "[EPIC-003] CRDT/OT 算法补齐" "/tmp/epic003.md" "type: epic,priority: P0,component: crdt"

echo ""
echo "=== Sample Task Issues (First 5 tasks as examples) ==="
echo ""

# TASK-001
cat > /tmp/task001.md << 'EOF'
# TASK-001: 在 enterprise-server Cargo.toml 中添加 jcode-auth 依赖

**所属 Epic**: #EPIC-001
**优先级**: P0
**预估工作量**: 0.5 人天

## 目标
将 jcode-auth crate 添加为 enterprise-server 的依赖，使其可以访问认证、RBAC、审计等功能。

## 实施步骤
1. 编辑 `crates/jcode-enterprise-server/Cargo.toml`
2. 在 [dependencies] 部分添加:
   ```toml
   jcode-auth = { path = "../jcode-auth" }
   ```
3. 运行 `cargo check` 验证编译通过

## 验收标准
- ✅ Cargo.toml 已更新
- ✅ cargo check 通过
- ✅ 无依赖冲突
EOF

create_issue "[TASK-001] 添加 jcode-auth 依赖到 enterprise-server" "/tmp/task001.md" "type: task,priority: P0,component: auth"

# TASK-002
cat > /tmp/task002.md << 'EOF'
# TASK-002: 创建 EnterpriseAuthMiddleware 包装器

**所属 Epic**: #EPIC-001
**优先级**: P0
**预估工作量**: 2 人天

## 目标
创建 Axum 中间件，将 jcode-auth 的 JWT 验证功能集成到 enterprise-server 的请求处理流程中。

## 关键文件
- `crates/jcode-enterprise-server/src/middleware/auth.rs` (新建)
- `crates/jcode-enterprise-server/src/middleware/mod.rs` (更新)

## 技术实现
```rust
pub struct EnterpriseAuthMiddleware {
    jwt_manager: Arc<jcode_auth::JwtManager>,
}

impl<S> tower::Service<Request> for EnterpriseAuthMiddleware {
    // 实现 JWT 验证逻辑
}
```

## 验收标准
- ✅ 中间件正确提取和验证 JWT token
- ✅ 无效 token 返回 401 Unauthorized
- ✅ 有效 token 将 claims 注入 request extensions
- ✅ 单元测试覆盖
EOF

create_issue "[TASK-002] 创建 EnterpriseAuthMiddleware" "/tmp/task002.md" "type: task,priority: P0,component: auth"

echo ""
echo "✅ Issue creation completed!"
echo ""
echo "Next steps:"
echo "1. Review created issues at: https://github.com/$REPO/issues"
echo "2. Assign team members to Epics and Tasks"
echo "3. Start with EPIC-001 TASK-001"
echo ""
echo "Note: This script created sample issues. Run again with full task list when ready."
