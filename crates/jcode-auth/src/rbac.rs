//! Role-Based Access Control (RBAC) system
//!
//! Provides fine-grained permission management with roles, permissions,
//! and hierarchical access control.

use bitflags::bitflags;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RbacError {
    #[error("Role not found: {0}")]
    RoleNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Invalid role hierarchy")]
    InvalidHierarchy,

    #[error("User not authorized")]
    NotAuthorized,
}

pub type Result<T> = std::result::Result<T, RbacError>;

/// Permission categories for CarpAI operations
bitflags! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PermissionFlags: u64 {
        // File operations
        const FILE_READ = 1 << 0;
        const FILE_WRITE = 1 << 1;
        const FILE_DELETE = 1 << 2;
        const FILE_EXECUTE = 1 << 3;

        // Code operations
        const CODE_COMPLETE = 1 << 4;
        const CODE_REFACTOR = 1 << 5;
        const CODE_ANALYZE = 1 << 6;
        const CODE_GENERATE = 1 << 7;

        // System operations
        const SYSTEM_CONFIG = 1 << 8;
        const SYSTEM_ADMIN = 1 << 9;
        const SYSTEM_MONITOR = 1 << 10;

        // Collaboration
        const COLLAB_READ = 1 << 11;
        const COLLAB_WRITE = 1 << 12;
        const COLLAB_SHARE = 1 << 13;

        // AI/LLM operations
        const AI_QUERY = 1 << 14;
        const AI_TRAIN = 1 << 15;
        const AI_DEPLOY = 1 << 16;

        // Audit & Compliance
        const AUDIT_VIEW = 1 << 17;
        const AUDIT_EXPORT = 1 << 18;
        const GDPR_MANAGE = 1 << 19;

        // All permissions (admin)
        const ALL = u64::MAX;
    }
}

/// Permission context with resource scope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub action: String,
    pub conditions: HashMap<String, serde_json::Value>,
}

impl PermissionContext {
    pub fn new(resource_type: &str, action: &str) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            resource_id: None,
            action: action.to_string(),
            conditions: HashMap::new(),
        }
    }

    pub fn with_resource_id(mut self, id: &str) -> Self {
        self.resource_id = Some(id.to_string());
        self
    }

    pub fn with_condition(mut self, key: &str, value: serde_json::Value) -> Self {
        self.conditions.insert(key.to_string(), value);
        self
    }
}

/// Role definition with permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: PermissionFlags,
    pub parent_roles: Vec<String>, // For role inheritance
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Role {
    pub fn new(id: &str, name: &str, description: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            permissions: PermissionFlags::empty(),
            parent_roles: vec![],
            metadata: HashMap::new(),
        }
    }

    /// Add permissions to role
    pub fn add_permissions(&mut self, flags: PermissionFlags) {
        self.permissions |= flags;
    }

    /// Check if role has specific permission
    pub fn has_permission(&self, flag: PermissionFlags) -> bool {
        self.permissions.contains(flag)
    }

    /// Add parent role for inheritance
    pub fn add_parent_role(&mut self, role_id: &str) {
        if !self.parent_roles.contains(&role_id.to_string()) {
            self.parent_roles.push(role_id.to_string());
        }
    }
}

/// Predefined system roles
pub mod predefined_roles {
    use super::*;

    /// Administrator role with all permissions
    pub fn admin() -> Role {
        let mut role = Role::new("admin", "Administrator", "Full system access");
        role.add_permissions(PermissionFlags::ALL);
        role
    }

    /// Developer role with code and file permissions
    pub fn developer() -> Role {
        let mut role = Role::new("developer", "Developer", "Code development access");
        role.add_permissions(
            PermissionFlags::FILE_READ
                | PermissionFlags::FILE_WRITE
                | PermissionFlags::CODE_COMPLETE
                | PermissionFlags::CODE_REFACTOR
                | PermissionFlags::CODE_ANALYZE
                | PermissionFlags::CODE_GENERATE
                | PermissionFlags::COLLAB_READ
                | PermissionFlags::COLLAB_WRITE
                | PermissionFlags::AI_QUERY,
        );
        role
    }

    /// Viewer role with read-only access
    pub fn viewer() -> Role {
        let mut role = Role::new("viewer", "Viewer", "Read-only access");
        role.add_permissions(
            PermissionFlags::FILE_READ
                | PermissionFlags::CODE_ANALYZE
                | PermissionFlags::COLLAB_READ
                | PermissionFlags::SYSTEM_MONITOR,
        );
        role
    }

    /// Auditor role with audit and compliance permissions
    pub fn auditor() -> Role {
        let mut role = Role::new("auditor", "Auditor", "Audit and compliance access");
        role.add_permissions(
            PermissionFlags::AUDIT_VIEW
                | PermissionFlags::AUDIT_EXPORT
                | PermissionFlags::FILE_READ
                | PermissionFlags::SYSTEM_MONITOR,
        );
        role
    }
}

/// User-role assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRole {
    pub user_id: String,
    pub role_id: String,
    pub assigned_at: chrono::DateTime<chrono::Utc>,
    pub assigned_by: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// RBAC engine for permission checking
pub struct RbacEngine {
    roles: DashMap<String, Arc<Role>>,
    user_roles: DashMap<String, Vec<UserRole>>,
    role_hierarchy: DashMap<String, Vec<String>>,
}

impl RbacEngine {
    pub fn new() -> Self {
        let engine = Self {
            roles: DashMap::new(),
            user_roles: DashMap::new(),
            role_hierarchy: DashMap::new(),
        };

        // Register predefined roles
        engine.register_role(predefined_roles::admin());
        engine.register_role(predefined_roles::developer());
        engine.register_role(predefined_roles::viewer());
        engine.register_role(predefined_roles::auditor());

        engine
    }

    /// Register a new role
    pub fn register_role(&self, role: Role) {
        let role_id = role.id.clone();
        self.roles.insert(role_id, Arc::new(role));
    }

    /// Assign role to user
    pub fn assign_role(&self, user_id: &str, role_id: &str, assigned_by: Option<String>) -> Result<()> {
        if !self.roles.contains_key(role_id) {
            return Err(RbacError::RoleNotFound(role_id.to_string()));
        }

        let user_role = UserRole {
            user_id: user_id.to_string(),
            role_id: role_id.to_string(),
            assigned_at: chrono::Utc::now(),
            assigned_by,
            expires_at: None,
        };

        self.user_roles
            .entry(user_id.to_string())
            .or_insert_with(Vec::new)
            .push(user_role);

        Ok(())
    }

    /// Remove role from user
    pub fn remove_role(&self, user_id: &str, role_id: &str) -> Result<()> {
        if let mut roles = self.user_roles.get_mut(user_id) {
            roles.retain(|ur| ur.role_id != role_id);
            Ok(())
        } else {
            Err(RbacError::NotAuthorized)
        }
    }

    /// Get all roles for a user
    pub fn get_user_roles(&self, user_id: &str) -> Vec<Arc<Role>> {
        let mut result = Vec::new();

        if let Some(user_roles) = self.user_roles.get(user_id) {
            for ur in user_roles.iter() {
                // Check if role is expired
                if let Some(expires_at) = ur.expires_at {
                    if chrono::Utc::now() > expires_at {
                        continue;
                    }
                }

                if let Some(role) = self.roles.get(&ur.role_id) {
                    result.push(role.clone());

                    // Add inherited roles
                    self.collect_inherited_roles(&ur.role_id, &mut result);
                }
            }
        }

        result
    }

    /// Collect inherited roles recursively
    fn collect_inherited_roles(&self, role_id: &str, collected: &mut Vec<Arc<Role>>) {
        if let Some(parent_ids) = self.role_hierarchy.get(role_id) {
            for parent_id in parent_ids.iter() {
                if let Some(parent_role) = self.roles.get(parent_id) {
                    if !collected.iter().any(|r| r.id == *parent_id) {
                        collected.push(parent_role.clone());
                        self.collect_inherited_roles(parent_id, collected);
                    }
                }
            }
        }
    }

    /// Check if user has specific permission
    pub fn check_permission(&self, user_id: &str, required_permission: PermissionFlags) -> Result<bool> {
        let roles = self.get_user_roles(user_id);

        if roles.is_empty() {
            return Err(RbacError::NotAuthorized);
        }

        let has_permission = roles.iter().any(|role| role.has_permission(required_permission));
        Ok(has_permission)
    }

    /// Check if user has permission for specific context
    pub fn check_context_permission(
        &self,
        user_id: &str,
        context: &PermissionContext,
    ) -> Result<bool> {
        // First check basic permission based on action
        let required_flag = match context.action.as_str() {
            "read" => PermissionFlags::FILE_READ,
            "write" => PermissionFlags::FILE_WRITE,
            "delete" => PermissionFlags::FILE_DELETE,
            "execute" => PermissionFlags::FILE_EXECUTE,
            _ => PermissionFlags::empty(),
        };

        if required_flag.is_empty() {
            return Ok(true); // No specific permission required
        }

        self.check_permission(user_id, required_flag)
    }

    /// Get all permissions for a user
    pub fn get_user_permissions(&self, user_id: &str) -> PermissionFlags {
        let roles = self.get_user_roles(user_id);
        let mut combined = PermissionFlags::empty();

        for role in roles {
            combined |= role.permissions;
        }

        combined
    }

    /// List all registered roles
    pub fn list_roles(&self) -> Vec<Arc<Role>> {
        self.roles.iter().map(|entry| entry.value().clone()).collect()
    }

    /// Get role by ID
    pub fn get_role(&self, role_id: &str) -> Option<Arc<Role>> {
        self.roles.get(role_id).map(|entry| entry.value().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rbac_basic_permissions() {
        let engine = RbacEngine::new();

        // Assign developer role to user
        engine.assign_role("user1", "developer", None).unwrap();

        // Check developer permissions
        assert!(engine
            .check_permission("user1", PermissionFlags::FILE_READ)
            .unwrap());
        assert!(engine
            .check_permission("user1", PermissionFlags::CODE_COMPLETE)
            .unwrap());
        assert!(!engine
            .check_permission("user1", PermissionFlags::SYSTEM_ADMIN)
            .unwrap_or(false));
    }

    #[test]
    fn test_rbac_admin_has_all_permissions() {
        let engine = RbacEngine::new();
        engine.assign_role("admin1", "admin", None).unwrap();

        assert!(engine
            .check_permission("admin1", PermissionFlags::ALL)
            .unwrap());
    }

    #[test]
    fn test_rbac_viewer_readonly() {
        let engine = RbacEngine::new();
        engine.assign_role("viewer1", "viewer", None).unwrap();

        assert!(engine
            .check_permission("viewer1", PermissionFlags::FILE_READ)
            .unwrap());
        assert!(!engine
            .check_permission("viewer1", PermissionFlags::FILE_WRITE)
            .unwrap_or(false));
    }

    #[test]
    fn test_rbac_multiple_roles() {
        let engine = RbacEngine::new();
        engine.assign_role("user1", "developer", None).unwrap();
        engine.assign_role("user1", "auditor", None).unwrap();

        let permissions = engine.get_user_permissions("user1");
        assert!(permissions.contains(PermissionFlags::CODE_COMPLETE));
        assert!(permissions.contains(PermissionFlags::AUDIT_VIEW));
    }

    #[test]
    fn test_rbac_role_not_found() {
        let engine = RbacEngine::new();
        let result = engine.assign_role("user1", "nonexistent", None);
        assert!(result.is_err());
    }
}
