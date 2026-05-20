//! SSO (Single Sign-On) 认证模块
//!
//! 支持 OIDC (OpenID Connect) 和 SAML 2.0 企业认证

pub mod oidc;
pub mod session;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};

pub use oidc::{
    build_authorization_url, exchange_code_for_tokens, fetch_user_info, OidcMetadataCache,
    OidcTokenResponse,
};
pub use session::{
    SessionState, SessionStats, SsoSessionExt, SsoSessionStore,
};

/// SSO 提供商类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SsoProviderType {
    /// OIDC (OpenID Connect)
    Oidc,
    /// SAML 2.0
    Saml,
    /// Okta (OIDC)
    Okta,
    /// Azure AD (OIDC)
    AzureAd,
    /// Google Workspace (OIDC)
    GoogleWorkspace,
    /// Auth0 (OIDC)
    Auth0,
    /// Keycloak (OIDC)
    Keycloak,
    /// LDAP (直接目录访问)
    Ldap,
}

/// SSO 提供商配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsoProviderConfig {
    /// 提供商 ID
    pub id: String,
    /// 提供商显示名称
    pub name: String,
    /// 提供商类型
    pub provider_type: SsoProviderType,
    /// 发行者 URL (OIDC)
    pub issuer_url: Option<String>,
    /// 客户端 ID
    pub client_id: String,
    /// 客户端密钥
    pub client_secret: Option<String>,
    /// OIDC 发现文档 URL
    pub discovery_url: Option<String>,
    /// SAML 元数据 URL 或内联 XML
    pub saml_metadata_url: Option<String>,
    /// SAML SSO URL
    pub saml_sso_url: Option<String>,
    /// SAML 证书指纹
    pub saml_cert_fingerprint: Option<String>,
    /// LDAP 服务器地址
    pub ldap_server: Option<String>,
    /// LDAP Base DN
    pub ldap_base_dn: Option<String>,
    /// 要求的 email 域名
    pub allowed_domains: Vec<String>,
    /// 是否启用
    pub enabled: bool,
    /// 自动创建账户
    pub auto_create_account: bool,
    /// 默认角色
    pub default_role: String,
    /// SSO callback URL
    pub callback_url: String,
}

impl SsoProviderConfig {
    pub fn new_oidc(
        id: &str,
        name: &str,
        client_id: &str,
        issuer_url: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            provider_type: SsoProviderType::Oidc,
            issuer_url: Some(issuer_url.to_string()),
            client_id: client_id.to_string(),
            client_secret: None,
            discovery_url: Some(format!("{}/.well-known/openid-configuration", issuer_url)),
            saml_metadata_url: None,
            saml_sso_url: None,
            saml_cert_fingerprint: None,
            ldap_server: None,
            ldap_base_dn: None,
            allowed_domains: Vec::new(),
            enabled: true,
            auto_create_account: true,
            default_role: "developer".to_string(),
            callback_url: String::new(),
        }
    }

    pub fn discovery_endpoint(&self) -> Option<String> {
        self.discovery_url.clone()
    }
}

/// SSO 会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsoSession {
    /// 会话 ID
    pub session_id: String,
    /// 用户 ID
    pub user_id: String,
    /// 提供商 ID
    pub provider_id: String,
    /// 提供商类型
    pub provider_type: SsoProviderType,
    /// 访问令牌
    pub access_token: String,
    /// 刷新令牌
    pub refresh_token: Option<String>,
    /// ID 令牌 (OIDC)
    pub id_token: Option<String>,
    /// 过期时间
    pub expires_at: DateTime<Utc>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 用户信息
    pub user_info: SsoUserInfo,
}

impl SsoSession {
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

/// SSO 用户信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsoUserInfo {
    /// 用户 ID (来自 IdP)
    pub sub: String,
    /// 邮箱
    pub email: Option<String>,
    /// 邮箱已验证
    pub email_verified: bool,
    /// 姓名
    pub name: Option<String>,
    /// 昵称
    pub nickname: Option<String>,
    /// 头像 URL
    pub picture: Option<String>,
    /// 组织/租户 ID
    pub tenant_id: Option<String>,
    /// 所属组
    pub groups: Vec<String>,
    /// 角色
    pub roles: Vec<String>,
    /// 额外声明
    #[serde(flatten)]
    pub claims: HashMap<String, String>,
}

/// SSO 提供商管理器
pub struct SsoProviderManager {
    providers: Arc<RwLock<HashMap<String, SsoProviderConfig>>>,
    sessions: Arc<RwLock<HashMap<String, SsoSession>>>,
    default_provider: Arc<RwLock<Option<String>>>,
}

impl SsoProviderManager {
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            default_provider: Arc::new(RwLock::new(None)),
        }
    }

    /// 注册 SSO 提供商
    pub async fn register_provider(&self, config: SsoProviderConfig) -> Result<(), SsoError> {
        let id = config.id.clone();
        let mut providers = self.providers.write().await;
        providers.insert(id, config);
        Ok(())
    }

    /// 获取提供商配置
    pub async fn get_provider(&self, id: &str) -> Option<SsoProviderConfig> {
        let providers = self.providers.read().await;
        providers.get(id).cloned()
    }

    /// 列出所有启用的提供商
    pub async fn list_enabled_providers(&self) -> Vec<SsoProviderConfig> {
        let providers = self.providers.read().await;
        providers.values()
            .filter(|p| p.enabled)
            .cloned()
            .collect()
    }

    /// 设置默认提供商
    pub async fn set_default_provider(&self, id: &str) -> Result<(), SsoError> {
        let mut providers = self.providers.read().await;
        if !providers.contains_key(id) {
            return Err(SsoError::ProviderNotFound(id.to_string()));
        }
        drop(providers);
        
        let mut default = self.default_provider.write().await;
        *default = Some(id.to_string());
        Ok(())
    }

    /// 获取默认提供商
    pub async fn get_default_provider(&self) -> Option<SsoProviderConfig> {
        let default = self.default_provider.read().await;
        match &*default {
            Some(id) => {
                let providers = self.providers.read().await;
                providers.get(id).cloned()
            }
            None => {
                let providers = self.providers.read().await;
                providers.values().find(|p| p.enabled).cloned()
            }
        }
    }

    /// 存储会话
    pub async fn store_session(&self, session: SsoSession) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.session_id.clone(), session);
    }

    /// 获取会话
    pub async fn get_session(&self, session_id: &str) -> Option<SsoSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    /// 删除会话
    pub async fn remove_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
    }

    /// 清理过期会话
    pub async fn cleanup_expired_sessions(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| !session.is_expired());
    }

    /// 验证访问令牌
    pub async fn validate_token(&self, provider_id: &str, token: &str) -> Result<SsoUserInfo, SsoError> {
        let provider = self.get_provider(provider_id).await
            .ok_or_else(|| SsoError::ProviderNotFound(provider_id.to_string()))?;
        
        match provider.provider_type {
            SsoProviderType::Oidc | SsoProviderType::Okta | 
            SsoProviderType::AzureAd | SsoProviderType::GoogleWorkspace |
            SsoProviderType::Auth0 | SsoProviderType::Keycloak => {
                self.validate_oidc_token(&provider, token).await
            }
            SsoProviderType::Saml => {
                self.validate_saml_response(token).await
            }
            SsoProviderType::Ldap => {
                Err(SsoError::UnsupportedProvider("LDAP validation not implemented".to_string()))
            }
        }
    }

    /// OIDC 令牌验证
    async fn validate_oidc_token(&self, provider: &SsoProviderConfig, token: &str) -> Result<SsoUserInfo, SsoError> {
        oidc::validate_id_token(token, provider).await
    }

    /// SAML 响应验证
    async fn validate_saml_response(&self, _response: &str) -> Result<SsoUserInfo, SsoError> {
        // SAML 验证实现
        // 注意: 完整的 SAML 实现需要 xmlsec 库
        Err(SsoError::UnsupportedProvider("SAML validation not implemented".to_string()))
    }
}

impl Default for SsoProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

/// SSO 错误类型
#[derive(Debug, thiserror::Error)]
pub enum SsoError {
    #[error("Provider not found: {0}")]
    ProviderNotFound(String),
    
    #[error("Token validation failed: {0}")]
    TokenValidationFailed(String),
    
    #[error("Unsupported provider: {0}")]
    UnsupportedProvider(String),
    
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    
    #[error("Network error: {0}")]
    NetworkError(String),
    
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    
    #[error("User not authorized: {0}")]
    UserNotAuthorized(String),
}

/// SSO 提供商发现响应 (OIDC Discovery)
#[derive(Debug, Clone, Deserialize)]
pub struct OidcDiscoveryResponse {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: Option<String>,
    pub jwks_uri: String,
    pub response_types_supported: Vec<String>,
    pub subject_types_supported: Vec<String>,
    pub id_token_signing_alg_values_supported: Vec<String>,
}

/// SSO 状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SsoState {
    /// 未配置
    NotConfigured,
    /// 已配置但未登录
    Configured,
    /// 已登录
    LoggedIn,
    /// 会话过期
    Expired,
}

/// SSO 认证结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsoAuthResult {
    /// 是否成功
    pub success: bool,
    /// 提供商 ID
    pub provider_id: String,
    /// 会话 ID
    pub session_id: Option<String>,
    /// 用户信息
    pub user_info: Option<SsoUserInfo>,
    /// 错误信息
    pub error: Option<String>,
    /// 错误代码
    pub error_code: Option<String>,
}

impl SsoAuthResult {
    pub fn success(provider_id: String, session: SsoSession) -> Self {
        Self {
            success: true,
            provider_id,
            session_id: Some(session.session_id),
            user_info: Some(session.user_info),
            error: None,
            error_code: None,
        }
    }

    pub fn failure(provider_id: String, error: String, error_code: Option<String>) -> Self {
        Self {
            success: false,
            provider_id,
            session_id: None,
            user_info: None,
            error: Some(error),
            error_code,
        }
    }
}
