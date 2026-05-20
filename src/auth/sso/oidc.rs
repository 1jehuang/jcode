//! OIDC (OpenID Connect) 支持模块

use super::{OidcDiscoveryResponse, SsoError, SsoProviderConfig, SsoUserInfo};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// OIDC 提供商元数据缓存
pub struct OidcMetadataCache {
    cache: Arc<RwLock<HashMap<String, OidcDiscoveryResponse>>>,
}

impl OidcMetadataCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 获取提供商元数据
    pub async fn get_metadata(&self, discovery_url: &str) -> Result<OidcDiscoveryResponse, SsoError> {
        // 检查缓存
        {
            let cache = self.cache.read().await;
            if let Some(metadata) = cache.get(discovery_url) {
                return Ok(metadata.clone());
            }
        }

        // 从网络获取
        let metadata = self.fetch_metadata(discovery_url).await?;

        // 缓存结果
        {
            let mut cache = self.cache.write().await;
            cache.insert(discovery_url.to_string(), metadata.clone());
        }

        Ok(metadata)
    }

    /// 获取 OIDC 发现文档
    async fn fetch_metadata(&self, discovery_url: &str) -> Result<OidcDiscoveryResponse, SsoError> {
        let response = reqwest::get(discovery_url)
            .await
            .map_err(|e| SsoError::NetworkError(e.to_string()))?;
        
        let metadata: OidcDiscoveryResponse = response
            .json()
            .await
            .map_err(|e| SsoError::InvalidResponse(e.to_string()))?;

        Ok(metadata)
    }

    /// 清除缓存
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }
}

impl Default for OidcMetadataCache {
    fn default() -> Self {
        Self::new()
    }
}

/// OIDC Claims 结构
#[derive(Debug, Clone, serde::Deserialize)]
struct OidcClaims {
    /// Issuer
    iss: String,
    /// Subject (用户 ID)
    sub: String,
    /// Audience
    #[serde(default)]
    aud: Vec<String>,
    /// 过期时间
    exp: i64,
    /// 签发时间
    iat: i64,
    /// 邮箱
    #[serde(default)]
    email: Option<String>,
    /// 邮箱已验证
    #[serde(default)]
    email_verified: Option<bool>,
    /// 姓名
    #[serde(default)]
    name: Option<String>,
    /// 昵称
    #[serde(default)]
    nickname: Option<String>,
    /// 头像
    #[serde(default)]
    picture: Option<String>,
    /// 用户名
    #[serde(default)]
    preferred_username: Option<String>,
    /// 租户 ID
    #[serde(rename = "tenantId", default)]
    tenant_id: Option<String>,
    /// 用户组
    #[serde(default)]
    groups: Vec<String>,
    /// 角色
    #[serde(default)]
    roles: Vec<String>,
}

/// 验证 ID Token
///
/// ID Token 是 JWT 格式，包含用户声明
pub async fn validate_id_token(
    id_token: &str,
    provider: &SsoProviderConfig,
) -> Result<SsoUserInfo, SsoError> {
    // 解析 JWT
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return Err(SsoError::TokenValidationFailed(
            "Invalid JWT format".to_string(),
        ));
    }

    // 解码 payload
    let payload_bytes = base64_url_decode(parts[1])
        .map_err(|e| SsoError::TokenValidationFailed(format!("Base64 decode failed: {}", e)))?;
    
    let claims: OidcClaims = serde_json::from_slice(&payload_bytes)
        .map_err(|e| SsoError::TokenValidationFailed(format!("JSON parse failed: {}", e)))?;

    // 验证 issuer
    if let Some(expected_issuer) = &provider.issuer_url {
        if claims.iss != *expected_issuer {
            return Err(SsoError::TokenValidationFailed(format!(
                "Invalid issuer: expected {}, got {}",
                expected_issuer, claims.iss
            )));
        }
    }

    // 验证 audience
    let has_valid_aud = if claims.aud.is_empty() {
        // 如果 aud 为空数组，尝试检查是否单值
        false
    } else {
        claims.aud.contains(&provider.client_id)
    };

    if !has_valid_aud {
        return Err(SsoError::TokenValidationFailed(
            "Invalid audience: token not intended for this client".to_string(),
        ));
    }

    // 验证过期时间
    let now = chrono::Utc::now().timestamp();
    if claims.exp < now {
        return Err(SsoError::TokenValidationFailed("Token expired".to_string()));
    }

    // 构建用户信息
    let user_info = SsoUserInfo {
        sub: claims.sub,
        email: claims.email,
        email_verified: claims.email_verified.unwrap_or(false),
        name: claims.name.or(claims.preferred_username.clone()),
        nickname: claims.nickname,
        picture: claims.picture,
        tenant_id: claims.tenant_id.or_else(|| {
            claims.groups.get(0).cloned()
        }),
        groups: claims.groups,
        roles: claims.roles,
        claims: HashMap::new(),
    };

    Ok(user_info)
}

/// Base64 URL 解码
fn base64_url_decode(input: &str) -> Result<Vec<u8>, SsoError> {
    // 替换 URL-safe 字符
    let mut base64 = input.replace('-', "+").replace('_', "/");
    
    // 填充
    let padding = (4 - base64.len() % 4) % 4;
    base64.extend(std::iter::repeat('=').take(padding));
    
    // 解码
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &base64)
        .map_err(|e| SsoError::TokenValidationFailed(format!("Base64 decode error: {}", e)))?;
    
    Ok(decoded)
}

/// 生成 OIDC 授权 URL
pub fn build_authorization_url(
    provider: &SsoProviderConfig,
    state: &str,
    nonce: &str,
    redirect_uri: &str,
) -> Result<String, SsoError> {
    let discovery_url = provider.discovery_url.as_ref()
        .ok_or_else(|| SsoError::ConfigurationError("Missing discovery URL".to_string()))?;

    let metadata_cache = OidcMetadataCache::new();
    
    // 异步获取元数据
    let rt = tokio::runtime::Handle::try_current()
        .map_err(|_| SsoError::ConfigurationError("No tokio runtime available".to_string()))?;
    
    let metadata = rt.block_on(async {
        metadata_cache.get_metadata(discovery_url).await
    })?;

    let mut url = url::Url::parse(&metadata.authorization_endpoint)
        .map_err(|e| SsoError::ConfigurationError(format!("Invalid auth URL: {}", e)))?;

    url.query_pairs_mut()
        .append_pair("client_id", &provider.client_id)
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email profile")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("nonce", nonce);

    Ok(url.to_string())
}

/// OIDC Token 响应
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OidcTokenResponse {
    /// 访问令牌
    pub access_token: String,
    /// 令牌类型
    #[serde(default)]
    pub token_type: Option<String>,
    /// 刷新令牌
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// ID 令牌
    #[serde(default)]
    pub id_token: Option<String>,
    /// 过期时间（秒）
    pub expires_in: Option<i64>,
    /// 范围
    #[serde(default)]
    pub scope: Option<String>,
}

/// 交换授权码获取 Token
pub async fn exchange_code_for_tokens(
    provider: &SsoProviderConfig,
    code: &str,
    redirect_uri: &str,
) -> Result<OidcTokenResponse, SsoError> {
    let discovery_url = provider.discovery_url.as_ref()
        .ok_or_else(|| SsoError::ConfigurationError("Missing discovery URL".to_string()))?;

    let metadata_cache = OidcMetadataCache::new();
    let metadata = metadata_cache.get_metadata(discovery_url).await?;

    let client = reqwest::Client::new();
    
    let mut params = HashMap::new();
    params.insert("grant_type", "authorization_code");
    params.insert("code", code);
    params.insert("redirect_uri", redirect_uri);
    params.insert("client_id", &provider.client_id);
    
    if let Some(client_secret) = &provider.client_secret {
        params.insert("client_secret", client_secret);
    }

    let response = client
        .post(&metadata.token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| SsoError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(SsoError::TokenValidationFailed(format!(
            "Token exchange failed: {}",
            error_text
        )));
    }

    let token_response: OidcTokenResponse = response
        .json()
        .await
        .map_err(|e| SsoError::InvalidResponse(format!("Token response parse failed: {}", e)))?;

    Ok(token_response)
}

/// 获取用户信息
pub async fn fetch_user_info(
    metadata: &OidcDiscoveryResponse,
    access_token: &str,
) -> Result<SsoUserInfo, SsoError> {
    let userinfo_endpoint = metadata.userinfo_endpoint.as_ref()
        .ok_or_else(|| SsoError::InvalidResponse("No userinfo endpoint".to_string()))?;

    let client = reqwest::Client::new();
    let response = client
        .get(userinfo_endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| SsoError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(SsoError::TokenValidationFailed(
            "Failed to fetch user info".to_string(),
        ));
    }

    let user_info: SsoUserInfo = response
        .json()
        .await
        .map_err(|e| SsoError::InvalidResponse(format!("User info parse failed: {}", e)))?;

    Ok(user_info)
}
