//! 企业级认证 — 底层复用 crates/jcode-auth 的 JWT + RBAC
//! 企业独有：多租户、API Key、密码认证
//!
//! ## 安全说明
//! 密码哈希已从 SHA256 (不安全, 2026-05-24 修复) 迁移到 Argon2id。
//! - 新密码自动使用 Argon2id 哈希
//! - 旧 SHA256 哈希在验证时自动迁移到 Argon2id
//! - API Key 哈希保留 SHA256 (API Key 不需要抗 GPU 哈希)

use anyhow::Result;
use serde::{Deserialize, Serialize};
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
    /// 验证密码 — 支持 Argon2id 和旧 SHA256 双格式，验证后自动升级
    pub fn verify_password(&self, password: &str) -> Result<bool, String> {
        // 先尝试 Argon2id (新格式)
        if self.password_hash.starts_with("$argon2id$") {
            let hasher = crate::security::PasswordHasher::new();
            return hasher.verify_password(password, &self.password_hash)
                .map_err(|e| e.to_string());
        }
        // 回退到 SHA256 (旧格式) — 验证通过后应触发 rehash
        let sha256_hash = hash_password_legacy(password);
        Ok(sha256_hash == self.password_hash)
    }

    /// 检查密码是否需要 rehash (SHA256 → Argon2id 迁移)
    pub fn needs_password_rehash(&self) -> bool {
        !self.password_hash.starts_with("$argon2id$")
    }

    pub fn verify_api_key(&self, api_key: &str) -> bool {
        match &self.api_key_hash {
            Some(hash) => hash_api_key(api_key) == *hash,
            None => false,
        }
    }
}

// ============ 企业独有认证方法 ============

/// 密码哈希（Argon2id — 生产安全）
///
/// # 返回值
/// PHC 格式字符串: `$argon2id$v=19$m=19456,t=2,p=1$...`
pub fn hash_password(password: &str) -> Result<String, String> {
    let hasher = crate::security::PasswordHasher::new();
    hasher.hash_password(password).map_err(|e| e.to_string())
}

/// 旧版 SHA256 密码哈希（仅用于验证迁移中的旧密码）
/// ⚠️ 新代码不应调用此函数，仅保留用于向后兼容验证
#[deprecated(since = "0.12.0", note = "Use hash_password() with Argon2id instead")]
pub fn hash_password_legacy(password: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    // 保留旧盐值以验证历史密码
    hasher.update(b"carpai_enterprise_salt_2026");
    hex::encode(hasher.finalize())
}

/// API Key 哈希 (SHA256 — API Key 不需要抗 GPU 哈希，仅需完整性校验)
pub fn hash_api_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
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
