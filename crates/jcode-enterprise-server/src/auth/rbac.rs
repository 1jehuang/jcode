//! RBAC (Role-Based Access Control) 权限系统
//!
//! 提供细粒度的基于角色的访问控制，支持：
//! - 自定义角色和权限
//! - 资源级权限控制
//! - 动态策略评估
//! - 权限继承

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

/// 权限类型枚举
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    // ===== 组织管理 =====
    /// 创建组织
    OrgCreate,
    /// 读取组织信息
    OrgRead,
    /// 更新组织信息
    OrgUpdate,
    /// 删除组织
    OrgDelete,
    /// 管理组织配置
    OrgAdmin,

    // ===== 用户管理 =====
    /// 创建用户
    UserCreate,
    /// 读取用户信息
    UserRead,
    /// 更新用户信息
    UserUpdate,
    /// 删除用户
    UserDelete,
    /// 分配角色
    UserRoleAssign,
    /// 撤销角色
    UserRoleRevoke,

    // ===== 会话管理 =====
    /// 创建会话
    SessionCreate,
    /// 读取会话
    SessionRead,
    /// 更新会话
    SessionUpdate,
    /// 删除会话
    SessionDelete,
    /// 查看所有会话（管理员）
    SessionListAll,

    // ===== 模型使用 =====
    /// 使用指定模型
    ModelUse(String),  // model_name
    /// 部署新模型
    ModelDeploy,
    /// 管理模型配置
    ModelAdmin,

    // ===== 代码库管理 =====
    /// 索引代码库
    CodebaseIndex,
    /// 搜索代码库
    CodebaseSearch,
    /// 管理代码库
    CodebaseAdmin,

    // ===== 资源访问 =====
    /// 读取资源（文件、目录等）
    ResourceRead(String),  // resource_path pattern
    /// 写入资源
    ResourceWrite(String),
    /// 执行资源
    ResourceExecute(String),

    // ===== 审计和管理 =====
    /// 查看审计日志
    AuditLogView,
    /// 导出审计日志
    AuditLogExport,
    /// 管理账单
    BillingManage,
    /// 配置SSO
    SSOConfigure,
    /// 查看用量统计
    UsageView,
    /// 管理配额
    QuotaManage,

    // ===== 系统管理 =====
    /// 查看系统指标
    MetricsView,
    /// 管理系统配置
    SystemConfig,
    /// 管理节点
    NodeManage,
}

impl Permission {
    /// 获取权限的名称标识
    pub fn name(&self) -> String {
        match self {
            Self::OrgCreate => "org:create".to_string(),
            Self::OrgRead => "org:read".to_string(),
            Self::OrgUpdate => "org:update".to_string(),
            Self::OrgDelete => "org:delete".to_string(),
            Self::OrgAdmin => "org:admin".to_string(),
            Self::UserCreate => "user:create".to_string(),
            Self::UserRead => "user:read".to_string(),
            Self::UserUpdate => "user:update".to_string(),
            Self::UserDelete => "user:delete".to_string(),
            Self::UserRoleAssign => "user:role:assign".to_string(),
            Self::UserRoleRevoke => "user:role:revoke".to_string(),
            Self::SessionCreate => "session:create".to_string(),
            Self::SessionRead => "session:read".to_string(),
            Self::SessionUpdate => "session:update".to_string(),
            Self::SessionDelete => "session:delete".to_string(),
            Self::SessionListAll => "session:list:all".to_string(),
            Self::ModelUse(model) => format!("model:use:{}", model),
            Self::ModelDeploy => "model:deploy".to_string(),
            Self::ModelAdmin => "model:admin".to_string(),
            Self::CodebaseIndex => "codebase:index".to_string(),
            Self::CodebaseSearch => "codebase:search".to_string(),
            Self::CodebaseAdmin => "codebase:admin".to_string(),
            Self::ResourceRead(path) => format!("resource:read:{}", path),
            Self::ResourceWrite(path) => format!("resource:write:{}", path),
            Self::ResourceExecute(path) => format!("resource:execute:{}", path),
            Self::AuditLogView => "audit:view".to_string(),
            Self::AuditLogExport => "audit:export".to_string(),
            Self::BillingManage => "billing:manage".to_string(),
            Self::SSOConfigure => "sso:configure".to_string(),
            Self::UsageView => "usage:view".to_string(),
            Self::QuotaManage => "quota:manage".to_string(),
            Self::MetricsView => "metrics:view".to_string(),
            Self::SystemConfig => "system:config".to_string(),
            Self::NodeManage => "node:manage".to_string(),
        }
    }

    /// 从名称字符串解析权限
    pub fn from_name(name: &str) -> Option<Self> {
        let parts: Vec<&str> = name.split(':').collect();
        match parts.as_slice() {
            ["org", "create"] => Some(Self::OrgCreate),
            ["org", "read"] => Some(Self::OrgRead),
            ["org", "update"] => Some(Self::OrgUpdate),
            ["org", "delete"] => Some(Self::OrgDelete),
            ["org", "admin"] => Some(Self::OrgAdmin),
            ["user", "create"] => Some(Self::UserCreate),
            ["user", "read"] => Some(Self::UserRead),
            ["user", "update"] => Some(Self::UserUpdate),
            ["user", "delete"] => Some(Self::UserDelete),
            ["user", "role", "assign"] => Some(Self::UserRoleAssign),
            ["user", "role", "revoke"] => Some(Self::UserRoleRevoke),
            ["session", "create"] => Some(Self::SessionCreate),
            ["session", "read"] => Some(Self::SessionRead),
            ["session", "update"] => Some(Self::SessionUpdate),
            ["session", "delete"] => Some(Self::SessionDelete),
            ["session", "list", "all"] => Some(Self::SessionListAll),
            ["model", "use", model] => Some(Self::ModelUse(model.to_string())),
            ["model", "deploy"] => Some(Self::ModelDeploy),
            ["model", "admin"] => Some(Self::ModelAdmin),
            ["codebase", "index"] => Some(Self::CodebaseIndex),
            ["codebase", "search"] => Some(Self::CodebaseSearch),
            ["codebase", "admin"] => Some(Self::CodebaseAdmin),
            ["resource", "read", path] => Some(Self::ResourceRead(path.to_string())),
            ["resource", "write", path] => Some(Self::ResourceWrite(path.to_string())),
            ["resource", "execute", path] => Some(Self::ResourceExecute(path.to_string())),
            ["audit", "view"] => Some(Self::AuditLogView),
            ["audit", "export"] => Some(Self::AuditLogExport),
            ["billing", "manage"] => Some(Self::BillingManage),
            ["sso", "configure"] => Some(Self::SSOConfigure),
            ["usage", "view"] => Some(Self::UsageView),
            ["quota", "manage"] => Some(Self::QuotaManage),
            ["metrics", "view"] => Some(Self::MetricsView),
            ["system", "config"] => Some(Self::SystemConfig),
            ["node", "manage"] => Some(Self::NodeManage),
            _ => None,
        }
    }
}

/// 权限范围
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionScope {
    /// 全局范围
    Global,
    /// 组织范围
    Organization(String),
    /// 团队范围
    Team(String),
    /// 项目范围
    Project(String),
    /// 资源范围
    Resource(String),
}

impl PermissionScope {
    pub fn matches(&self, other: &PermissionScope) -> bool {
        match (self, other) {
            (Self::Global, _) => true,
            (_, Self::Global) => true,
            (Self::Organization(a), Self::Organization(b)) => a == b || b == "*",
            (Self::Team(a), Self::Team(b)) => a == b || b == "*",
            (Self::Project(a), Self::Project(b)) => a == b || b == "*",
            (Self::Resource(a), Self::Resource(b)) => a == b || b == "*",
            _ => false,
        }
    }
}

/// 角色定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    /// 角色ID
    pub id: String,
    /// 角色名称
    pub name: String,
    /// 角色描述
    pub description: String,
    /// 权限集合
    pub permissions: HashSet<Permission>,
    /// 权限范围
    pub scope: PermissionScope,
    /// 是否内置角色（不可删除）
    pub is_builtin: bool,
    /// 父角色（支持角色继承）
    pub parent_role: Option<String>,
}

impl Role {
    /// 检查角色是否具有指定权限
    pub fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions.contains(permission)
    }

    /// 检查是否具有某个范围的权限
    pub fn has_permission_in_scope(&self, permission: &Permission, scope: &PermissionScope) -> bool {
        self.has_permission(permission) && self.scope.matches(scope)
    }
}

/// 预定义角色工厂
impl Role {
    /// 超级管理员角色（拥有所有权限）
    pub fn super_admin() -> Self {
        let mut permissions = HashSet::new();
        // 添加所有权限
        permissions.insert(Permission::OrgCreate);
        permissions.insert(Permission::OrgRead);
        permissions.insert(Permission::OrgUpdate);
        permissions.insert(Permission::OrgDelete);
        permissions.insert(Permission::OrgAdmin);
        permissions.insert(Permission::UserCreate);
        permissions.insert(Permission::UserRead);
        permissions.insert(Permission::UserUpdate);
        permissions.insert(Permission::UserDelete);
        permissions.insert(Permission::UserRoleAssign);
        permissions.insert(Permission::UserRoleRevoke);
        permissions.insert(Permission::SessionCreate);
        permissions.insert(Permission::SessionRead);
        permissions.insert(Permission::SessionUpdate);
        permissions.insert(Permission::SessionDelete);
        permissions.insert(Permission::SessionListAll);
        permissions.insert(Permission::ModelDeploy);
        permissions.insert(Permission::ModelAdmin);
        permissions.insert(Permission::CodebaseIndex);
        permissions.insert(Permission::CodebaseSearch);
        permissions.insert(Permission::CodebaseAdmin);
        permissions.insert(Permission::AuditLogView);
        permissions.insert(Permission::AuditLogExport);
        permissions.insert(Permission::BillingManage);
        permissions.insert(Permission::SSOConfigure);
        permissions.insert(Permission::UsageView);
        permissions.insert(Permission::QuotaManage);
        permissions.insert(Permission::MetricsView);
        permissions.insert(Permission::SystemConfig);
        permissions.insert(Permission::NodeManage);

        Self {
            id: "super_admin".to_string(),
            name: "Super Administrator".to_string(),
            description: "Full system access with all permissions".to_string(),
            permissions,
            scope: PermissionScope::Global,
            is_builtin: true,
            parent_role: None,
        }
    }

    /// 组织管理员角色
    pub fn org_admin(org_id: String) -> Self {
        let mut permissions = HashSet::new();
        permissions.insert(Permission::OrgRead);
        permissions.insert(Permission::OrgUpdate);
        permissions.insert(Permission::OrgAdmin);
        permissions.insert(Permission::UserCreate);
        permissions.insert(Permission::UserRead);
        permissions.insert(Permission::UserUpdate);
        permissions.insert(Permission::UserRoleAssign);
        permissions.insert(Permission::SessionRead);
        permissions.insert(Permission::SessionListAll);
        permissions.insert(Permission::AuditLogView);
        permissions.insert(Permission::UsageView);
        permissions.insert(Permission::QuotaManage);

        Self {
            id: "org_admin".to_string(),
            name: "Organization Administrator".to_string(),
            description: "Manage organization and its users".to_string(),
            permissions,
            scope: PermissionScope::Organization(org_id),
            is_builtin: true,
            parent_role: None,
        }
    }

    /// 团队负责人角色
    pub fn team_lead(org_id: String) -> Self {
        let mut permissions = HashSet::new();
        permissions.insert(Permission::UserRead);
        permissions.insert(Permission::SessionCreate);
        permissions.insert(Permission::SessionRead);
        permissions.insert(Permission::SessionUpdate);
        permissions.insert(Permission::CodebaseIndex);
        permissions.insert(Permission::CodebaseSearch);
        permissions.insert(Permission::UsageView);

        Self {
            id: "team_lead".to_string(),
            name: "Team Lead".to_string(),
            description: "Lead a team and manage team resources".to_string(),
            permissions,
            scope: PermissionScope::Organization(org_id),
            is_builtin: true,
            parent_role: None,
        }
    }

    /// 开发者角色
    pub fn developer(org_id: String) -> Self {
        let mut permissions = HashSet::new();
        permissions.insert(Permission::SessionCreate);
        permissions.insert(Permission::SessionRead);
        permissions.insert(Permission::SessionUpdate);
        permissions.insert(Permission::SessionDelete);
        permissions.insert(Permission::CodebaseIndex);
        permissions.insert(Permission::CodebaseSearch);

        // 默认允许使用所有模型
        permissions.insert(Permission::ModelUse("*".to_string()));

        Self {
            id: "developer".to_string(),
            name: "Developer".to_string(),
            description: "Standard developer with session and codebase access".to_string(),
            permissions,
            scope: PermissionScope::Organization(org_id),
            is_builtin: true,
            parent_role: None,
        }
    }

    /// 只读观察者角色
    pub fn viewer(org_id: String) -> Self {
        let mut permissions = HashSet::new();
        permissions.insert(Permission::SessionRead);
        permissions.insert(Permission::CodebaseSearch);

        Self {
            id: "viewer".to_string(),
            name: "Viewer".to_string(),
            description: "Read-only access to sessions and codebase".to_string(),
            permissions,
            scope: PermissionScope::Organization(org_id),
            is_builtin: true,
            parent_role: None,
        }
    }

    /// 账单管理员角色
    pub fn billing_admin(org_id: String) -> Self {
        let mut permissions = HashSet::new();
        permissions.insert(Permission::BillingManage);
        permissions.insert(Permission::UsageView);
        permissions.insert(Permission::AuditLogView);

        Self {
            id: "billing_admin".to_string(),
            name: "Billing Administrator".to_string(),
            description: "Manage billing and view usage statistics".to_string(),
            permissions,
            scope: PermissionScope::Organization(org_id),
            is_builtin: true,
            parent_role: None,
        }
    }
}

/// 策略引擎 - 用于权限检查和决策
pub struct PolicyEngine {
    roles: HashMap<String, Role>,
    user_roles: HashMap<String, Vec<String>>,  // user_id -> role_ids
}

impl PolicyEngine {
    pub fn new() -> Self {
        let mut engine = Self {
            roles: HashMap::new(),
            user_roles: HashMap::new(),
        };

        // 注册内置角色
        engine.register_role(Role::super_admin());

        engine
    }

    /// 注册角色
    pub fn register_role(&mut self, role: Role) {
        let role_id = role.id.clone();
        self.roles.insert(role_id, role);
    }

    /// 为用户分配角色
    pub fn assign_role(&mut self, user_id: String, role_id: String) {
        self.user_roles
            .entry(user_id)
            .or_insert_with(Vec::new)
            .push(role_id);
        debug!("Assigned role {} to user {}", role_id, user_id);
    }

    /// 从用户撤销角色
    pub fn revoke_role(&mut self, user_id: &str, role_id: &str) {
        if let Some(roles) = self.user_roles.get_mut(user_id) {
            roles.retain(|r| r != role_id);
            debug!("Revoked role {} from user {}", role_id, user_id);
        }
    }

    /// 检查用户是否具有指定权限
    pub fn check_permission(
        &self,
        user_id: &str,
        permission: &Permission,
        scope: Option<&PermissionScope>,
    ) -> bool {
        let role_ids = match self.user_roles.get(user_id) {
            Some(roles) => roles,
            None => {
                debug!("User {} has no roles assigned", user_id);
                return false;
            }
        };

        for role_id in role_ids {
            if let Some(role) = self.roles.get(role_id) {
                // 检查直接权限
                if role.has_permission(permission) {
                    // 如果指定了范围，检查范围匹配
                    if let Some(required_scope) = scope {
                        if role.scope.matches(required_scope) {
                            debug!(
                                "Permission granted: user={}, permission={:?}, role={}",
                                user_id, permission, role_id
                            );
                            return true;
                        }
                    } else {
                        return true;
                    }
                }

                // 检查继承的权限（通过父角色）
                if let Some(parent_role_id) = &role.parent_role {
                    if let Some(parent_role) = self.roles.get(parent_role_id) {
                        if parent_role.has_permission(permission) {
                            if let Some(required_scope) = scope {
                                if parent_role.scope.matches(required_scope) {
                                    return true;
                                }
                            } else {
                                return true;
                            }
                        }
                    }
                }
            }
        }

        debug!(
            "Permission denied: user={}, permission={:?}",
            user_id, permission
        );
        false
    }

    /// 获取用户的所有权限
    pub fn get_user_permissions(&self, user_id: &str) -> HashSet<Permission> {
        let mut permissions = HashSet::new();

        if let Some(role_ids) = self.user_roles.get(user_id) {
            for role_id in role_ids {
                if let Some(role) = self.roles.get(role_id) {
                    permissions.extend(role.permissions.iter().cloned());

                    // 添加父角色的权限
                    if let Some(parent_role_id) = &role.parent_role {
                        if let Some(parent_role) = self.roles.get(parent_role_id) {
                            permissions.extend(parent_role.permissions.iter().cloned());
                        }
                    }
                }
            }
        }

        permissions
    }

    /// 获取用户的所有角色
    pub fn get_user_roles(&self, user_id: &str) -> Vec<&Role> {
        let mut roles = Vec::new();

        if let Some(role_ids) = self.user_roles.get(user_id) {
            for role_id in role_ids {
                if let Some(role) = self.roles.get(role_id) {
                    roles.push(role);
                }
            }
        }

        roles
    }

    /// 列出所有角色
    pub fn list_roles(&self) -> Vec<&Role> {
        self.roles.values().collect()
    }

    /// 获取角色详情
    pub fn get_role(&self, role_id: &str) -> Option<&Role> {
        self.roles.get(role_id)
    }

    /// 删除角色（不能删除内置角色）
    pub fn delete_role(&mut self, role_id: &str) -> Result<(), String> {
        if let Some(role) = self.roles.get(role_id) {
            if role.is_builtin {
                return Err("Cannot delete builtin role".to_string());
            }
        }

        self.roles.remove(role_id);

        // 从所有用户中移除此角色
        for roles in self.user_roles.values_mut() {
            roles.retain(|r| r != role_id);
        }

        Ok(())
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// 权限检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionCheckResult {
    pub allowed: bool,
    pub user_id: String,
    pub permission: String,
    pub scope: Option<String>,
    pub matched_role: Option<String>,
    pub reason: String,
}

/// 批量权限请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPermissionRequest {
    pub user_id: String,
    pub permissions: Vec<(Permission, Option<PermissionScope>)>,
}

/// 批量权限响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPermissionResponse {
    pub results: Vec<PermissionCheckResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_super_admin_has_all_permissions() {
        let mut engine = PolicyEngine::new();
        let admin_role = Role::super_admin();
        let role_id = admin_role.id.clone();
        engine.register_role(admin_role);
        engine.assign_role("user1".to_string(), role_id);

        assert!(engine.check_permission("user1", &Permission::OrgCreate, None));
        assert!(engine.check_permission("user1", &Permission::UserDelete, None));
        assert!(engine.check_permission("user1", &Permission::SystemConfig, None));
    }

    #[test]
    fn test_developer_cannot_delete_users() {
        let mut engine = PolicyEngine::new();
        let dev_role = Role::developer("org1".to_string());
        let role_id = dev_role.id.clone();
        engine.register_role(dev_role);
        engine.assign_role("user1".to_string(), role_id);

        assert!(engine.check_permission("user1", &Permission::SessionCreate, None));
        assert!(!engine.check_permission("user1", &Permission::UserDelete, None));
        assert!(!engine.check_permission("user1", &Permission::SystemConfig, None));
    }

    #[test]
    fn test_viewer_readonly() {
        let mut engine = PolicyEngine::new();
        let viewer_role = Role::viewer("org1".to_string());
        let role_id = viewer_role.id.clone();
        engine.register_role(viewer_role);
        engine.assign_role("user1".to_string(), role_id);

        assert!(engine.check_permission("user1", &Permission::SessionRead, None));
        assert!(!engine.check_permission("user1", &Permission::SessionCreate, None));
        assert!(!engine.check_permission("user1", &Permission::SessionUpdate, None));
    }

    #[test]
    fn test_permission_scoping() {
        let mut engine = PolicyEngine::new();
        let org_admin = Role::org_admin("org1".to_string());
        let role_id = org_admin.id.clone();
        engine.register_role(org_admin);
        engine.assign_role("user1".to_string(), role_id);

        // 在org1范围内应该有权限
        let org1_scope = PermissionScope::Organization("org1".to_string());
        assert!(engine.check_permission("user1", &Permission::UserRead, Some(&org1_scope)));

        // 在其他组织范围内不应该有权限
        let org2_scope = PermissionScope::Organization("org2".to_string());
        assert!(!engine.check_permission("user1", &Permission::UserRead, Some(&org2_scope)));
    }

    #[test]
    fn test_role_inheritance() {
        let mut engine = PolicyEngine::new();

        // 创建一个自定义角色，继承自developer
        let mut custom_role = Role {
            id: "senior_developer".to_string(),
            name: "Senior Developer".to_string(),
            description: "Senior developer with additional permissions".to_string(),
            permissions: HashSet::new(),
            scope: PermissionScope::Organization("org1".to_string()),
            is_builtin: false,
            parent_role: Some("developer".to_string()),
        };

        // 添加额外权限
        custom_role.permissions.insert(Permission::CodebaseAdmin);
        custom_role.permissions.insert(Permission::QuotaManage);

        engine.register_role(Role::developer("org1".to_string()));
        engine.register_role(custom_role);
        engine.assign_role("user1".to_string(), "senior_developer".to_string());

        // 应该同时拥有自己的权限和父角色的权限
        assert!(engine.check_permission("user1", &Permission::SessionCreate, None));
        assert!(engine.check_permission("user1", &Permission::CodebaseAdmin, None));
        assert!(engine.check_permission("user1", &Permission::QuotaManage, None));
    }

    #[test]
    fn test_permission_name_roundtrip() {
        let perms = vec![
            Permission::OrgCreate,
            Permission::UserRead,
            Permission::SessionDelete,
            Permission::ModelUse("gpt-4".to_string()),
            Permission::ResourceRead("/src".to_string()),
        ];

        for perm in perms {
            let name = perm.name();
            let parsed = Permission::from_name(&name);
            assert_eq!(Some(perm), parsed, "Failed for permission: {:?}", perm);
        }
    }
}
