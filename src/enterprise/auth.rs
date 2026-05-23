//! 企业级认证 — 底层复用 crates/jcode-auth 的 JWT + RBAC
//! 企业独有：多租户、API Key、密码认证

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use chrono::{DateTime, Utc};

// 复用 CarpAI 核心 auth crate 的 JWT + RBAC
pub use jcode_auth::jwt::{JwtManager, JwtClaims};
pub use jcode_auth::rbac::{RbacEngine, Permission, PermissionContext};

// ============ 企业独有类型（CarpAI 核心 auth 没有的）============

/// 用户角色（企业多租户专用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserRole {
    SuperAdmin,
    OrgAdmin,
    DepartmentHead,
    Developer,
    Viewer,
}

impl UserRole {
    pub fn level(&self) -> u8 {
        match self {
            Self::SuperAdmin => 100,
            Self::OrgAdmin => 80,
            Self::DepartmentHead => 60,
            Self::Developer => 40,
            Self::Viewer => 20,
        }
    }
}

/// 组织
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub id: String,
    pub name: String,
    pub plan: OrgPlan,
    pub created_at: DateTime<Utc>,
    pub max_users: u32,
    pub daily_token_limit: u64,
    pub concurrent_limit: u32,
    pub is_active: bool,
}

/// 组织套餐
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrgPlan { Free, Enterprise }

/// 企业用户
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub org_id: String,
    pub email: String,
    pub name: String,
    pub role: UserRole,
    pub password_hash: String,
    pub api_key_hash: Option<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
}

impl User {
    pub fn verify_password(&self, password: &str) -> bool {
        hash_password(password) == self.password_hash
    }

    pub fn verify_api_key(&self, api_key: &str) -> bool {
        match &self.api_key_hash {
            Some(hash) => hash_api_key(api_key) == *hash,
            None => false,
        }
    }
}

// ============ 企业独有认证方法 ============

/// 密码哈希（加盐 SHA256）
pub fn hash_password(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.update(b"carpai_enterprise_salt_2026");
    hex::encode(hasher.finalize())
}

/// API Key 哈希
pub fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

/// 生成 API Key
pub fn generate_api_key(user_id: &str) -> String {
    use rand::Rng;
    let key: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(32).map(char::from).collect();
    format!("carpai_{}_{}", user_id, key)
}
