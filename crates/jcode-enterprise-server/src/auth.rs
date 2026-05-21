//! 企业级认证与多租户管理
//!
//! 支持：JWT 认证、API Key、RBAC 角色权限、组织管理

pub mod rbac;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

pub use rbac::{Permission, PermissionScope, Role, PolicyEngine};

/// 用户角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserRole {
    /// 超级管理员
    SuperAdmin,
    /// 组织管理员
    OrgAdmin,
    /// 部门负责人
    DepartmentHead,
    /// 开发者
    Developer,
    /// 查看者（只读）
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

    pub fn can_manage_org(&self) -> bool {
        matches!(self, Self::SuperAdmin | Self::OrgAdmin)
    }

    pub fn can_manage_users(&self) -> bool {
        matches!(self, Self::SuperAdmin | Self::OrgAdmin | Self::DepartmentHead)
    }
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SuperAdmin => write!(f, "super_admin"),
            Self::OrgAdmin => write!(f, "org_admin"),
            Self::DepartmentHead => write!(f, "department_head"),
            Self::Developer => write!(f, "developer"),
            Self::Viewer => write!(f, "viewer"),
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
pub enum OrgPlan {
    Free,
    Enterprise,
}

/// 用户
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
    /// 验证密码
    pub fn verify_password(&self, password: &str) -> bool {
        let hash = hash_password(password);
        hash == self.password_hash
    }

    /// 验证 API Key
    pub fn verify_api_key(&self, api_key: &str) -> bool {
        match &self.api_key_hash {
            Some(hash) => {
                let hashed = hash_api_key(api_key);
                &hashed == hash
            }
            None => false,
        }
    }
}

/// API Key 信息（返回给用户时使用，包含未哈希的 key）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyInfo {
    pub user_id: String,
    pub key_preview: String, // 只显示前8位
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
}

/// JWT Claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,        // user_id
    pub org_id: String,
    pub role: UserRole,
    pub exp: i64,           // expiration timestamp
    pub iat: i64,           // issued at
}

/// 认证管理器
pub struct AuthManager {
    jwt_secret: String,
    jwt_expiry: chrono::Duration,
    users: Arc<RwLock<HashMap<String, User>>>,
    /// RBAC 策略引擎
    pub policy_engine: Arc<RwLock<PolicyEngine>>,
}

impl AuthManager {
    pub fn new(jwt_secret: String, jwt_expiry_hours: u32) -> Self {
        let mut policy_engine = PolicyEngine::new();
        // 注册内置角色（示例：在创建时动态添加组织范围）
        policy_engine.register_role(Role::super_admin());

        Self {
            jwt_secret,
            jwt_expiry: chrono::Duration::hours(jwt_expiry_hours as i64),
            users: Arc::new(RwLock::new(HashMap::new())),
            policy_engine: Arc::new(RwLock::new(policy_engine)),
        }
    }

    /// 创建 JWT Token
    pub fn create_token(&self, user_id: &str, org_id: &str, role: UserRole) -> String {
        let now = Utc::now();
        let claims = JwtClaims {
            sub: user_id.to_string(),
            org_id: org_id.to_string(),
            role,
            exp: (now + self.jwt_expiry).timestamp(),
            iat: now.timestamp(),
        };

        // 简单的 JWT 编码（生产环境应使用 jsonwebtoken crate）
        let header = base64::encode_config(
            serde_json::json!({"alg": "HS256", "typ": "JWT"}).to_string(),
            base64::URL_SAFE_NO_PAD
        );
        let payload = base64::encode_config(
            serde_json::to_string(&claims).unwrap(),
            base64::URL_SAFE_NO_PAD
        );
        let signature = sign_jwt(&header, &payload, &self.jwt_secret);

        format!("{}.{}.{}", header, payload, signature)
    }

    /// 验证 JWT Token
    pub fn verify_token(&self, token: &str) -> Result<JwtClaims, String> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err("无效的 token 格式".into());
        }

        let expected_sig = sign_jwt(parts[0], parts[1], &self.jwt_secret);
        if parts[2] != expected_sig {
            return Err("token 签名无效".into());
        }

        let payload_bytes = base64::decode_config(parts[1], base64::URL_SAFE_NO_PAD)
            .map_err(|_| "base64 解码失败".to_string())?;
        let claims: JwtClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|_| "JSON 解析失败".to_string())?;

        let now = Utc::now().timestamp();
        if claims.exp < now {
            return Err("token 已过期".into());
        }

        Ok(claims)
    }

    /// 创建 API Key
    pub fn generate_api_key(&self, user: &mut User) -> String {
        use rand::Rng;
        let key: String = rand::rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();

        let full_key = format!("carpai_{}_{}", user.id, key);
        user.api_key_hash = Some(hash_api_key(&full_key));
        full_key
    }

    /// 为用户分配角色（RBAC）
    pub async fn assign_role_to_user(&self, user_id: String, role_id: String) {
        let mut engine = self.policy_engine.write().await;
        engine.assign_role(user_id, role_id);
    }

    /// 检查用户权限（RBAC）
    pub async fn check_permission(
        &self,
        user_id: &str,
        permission: &Permission,
        scope: Option<&PermissionScope>,
    ) -> bool {
        let engine = self.policy_engine.read().await;
        engine.check_permission(user_id, permission, scope)
    }
}

/// 哈希密码
pub fn hash_password(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    // 简单加盐
    hasher.update(b"carpai_enterprise_salt_2026");
    hex::encode(hasher.finalize())
}

/// 哈希 API Key
pub fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

/// 签名 JWT
fn sign_jwt(header: &str, payload: &str, secret: &str) -> String {
    use hmac::{Hmac, Mac};
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC 初始化失败");
    mac.update(format!("{}.{}", header, payload).as_bytes());
    let result = mac.finalize();
    base64::encode_config(result.into_bytes(), base64::URL_SAFE_NO_PAD)
}
