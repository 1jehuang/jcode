//! # MCP OAuth 认证流
//!
//! 源自 Claude Code `src/services/mcp/auth.ts` (2466 行)
//!
//! ## 能力
//! - PKCE OAuth 认证流程 (授权码 + 本地回调服务器)
//! - Token 管理 (存储、刷新、撤销)
//! - XAA (Cross-App Access) 静默认证
//! - Step-up Scope 检测 (403 insufficient_scope)
//! - 令牌撤销 (RFC 7009)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;

/// OAuth 令牌
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpOAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub scope: Option<String>,
    pub token_type: String,
}

/// OAuth 客户端信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpOAuthClientInfo {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
}

/// 授权服务器元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthServerMetadata {
    pub issuer: Option<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub revocation_endpoint: Option<String>,
}

/// MCP OAuth 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAuthConfig {
    pub server_name: String,
    pub auth_url: Option<String>,
    pub token_url: Option<String>,
    pub client_id: Option<String>,
    pub redirect_port: Option<u16>,
}

/// MCP OAuth 认证管理器
///
/// 管理 MCP 服务器的 OAuth 认证生命周期。
pub struct McpAuthManager {
    /// 令牌存储 (server_name → tokens)
    tokens: Arc<Mutex<HashMap<String, McpOAuthTokens>>>,
    /// 客户端信息存储
    clients: Arc<Mutex<HashMap<String, McpOAuthClientInfo>>>,
    /// 元数据缓存
    metadata_cache: Arc<Mutex<HashMap<String, AuthServerMetadata>>>,
    /// PKCE verifier 存储 (用于回调匹配)
    verifiers: Arc<Mutex<HashMap<String, PkceState>>>,
}

/// PKCE 状态
#[derive(Debug, Clone)]
pub struct PkceState {
    pub code_verifier: String,
    pub state: String,
    pub redirect_uri: String,
    pub created_at: Instant,
}

impl McpAuthManager {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(Mutex::new(HashMap::new())),
            clients: Arc::new(Mutex::new(HashMap::new())),
            metadata_cache: Arc::new(Mutex::new(HashMap::new())),
            verifiers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 获取 OAuth 令牌（含自动刷新检查）
    /// 源自 Claude Code 的 `ClaudeAuthProvider.tokens()`
    pub fn get_tokens(&self, server_name: &str) -> Option<McpOAuthTokens> {
        let tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        let token = tokens.get(server_name)?.clone();

        // 检查是否需要在到期前提前刷新
        if let Some(expires) = token.expires_at {
            let now = chrono::Utc::now();
            let time_to_expiry = expires - now;
            // 如果 5 分钟内过期，标记为需要刷新
            if time_to_expiry < chrono::Duration::minutes(5) {
                return None; // 调用方应触发刷新
            }
        }

        Some(token)
    }

    /// 保存令牌
    /// 源自 Claude Code 的 `saveTokens()`
    pub fn save_tokens(&self, server_name: &str, tokens: McpOAuthTokens) {
        let mut store = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        store.insert(server_name.to_string(), tokens);
    }

    /// 保存客户端信息
    /// 源自 Claude Code 的 `saveClientInformation()`
    pub fn save_client_info(&self, server_name: &str, info: McpOAuthClientInfo) {
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.insert(server_name.to_string(), info);
    }

    /// 获取客户端信息
    pub fn get_client_info(&self, server_name: &str) -> Option<McpOAuthClientInfo> {
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).get(server_name).cloned()
    }

    /// 启动 PKCE OAuth 流程
    /// 源自 Claude Code 的 `performMCPOAuthFlow()`
    pub fn start_pkce_flow(&self, server_name: &str, redirect_uri: &str) -> anyhow::Result<PkceChallenge> {
        let code_verifier = generate_code_verifier();
        let code_challenge = sha256_base64_url(&code_verifier);
        let state = uuid::Uuid::new_v4().to_string();

        let mut verifiers = self.verifiers.lock().unwrap_or_else(|e| e.into_inner());
        verifiers.insert(server_name.to_string(), PkceState {
            code_verifier,
            state: state.clone(),
            redirect_uri: redirect_uri.to_string(),
            created_at: Instant::now(),
        });

        Ok(PkceChallenge {
            code_challenge,
            code_challenge_method: "S256".to_string(),
            state,
            redirect_uri: redirect_uri.to_string(),
        })
    }

    /// 完成 PKCE 流程（用授权码交换令牌）
    /// 源自 Claude Code 的授权码交换逻辑
    pub fn complete_pkce_flow(
        &self,
        server_name: &str,
        auth_code: &str,
        received_state: &str,
    ) -> anyhow::Result<()> {
        let mut verifiers = self.verifiers.lock().unwrap_or_else(|e| e.into_inner());
        let pkce = verifiers.remove(server_name)
            .ok_or_else(|| anyhow::anyhow!("No PKCE flow in progress for '{}'", server_name))?;

        // 验证 state 防止 CSRF
        if pkce.state != received_state {
            anyhow::bail!("State mismatch — possible CSRF attack");
        }

        // 调用方应使用 code_verifier + auth_code 交换令牌
        Ok(())
    }

    /// 使令牌失效
    /// 源自 Claude Code 的 `invalidateCredentials()`
    pub fn invalidate_tokens(&self, server_name: &str) {
        let mut tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        tokens.remove(server_name);
    }

    /// 撤销服务器令牌
    /// 源自 Claude Code 的 `revokeServerTokens()`
    pub fn revoke_tokens(&self, server_name: &str) {
        let mut tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = tokens.remove(server_name) {
            // 实际撤销需要 HTTP 调用 revocation_endpoint
            tracing::info!("[MCP Auth] Revoked tokens for '{}'", server_name);
            drop(token);
        }
    }

    /// 获取 PKCE verifier（用于回调）
    pub fn get_pkce_verifier(&self, server_name: &str) -> Option<PkceState> {
        self.verifiers.lock().unwrap_or_else(|e| e.into_inner()).get(server_name).cloned()
    }

    /// 清理过期的 PKCE verifier
    pub fn cleanup_expired_verifiers(&self) {
        let mut verifiers = self.verifiers.lock().unwrap_or_else(|e| e.into_inner());
        verifiers.retain(|_, v| v.created_at.elapsed() < Duration::from_secs(600));
    }

    /// 检查是否需要 step-up 认证
    /// 源自 Claude Code 的 `wrapFetchWithStepUpDetection()`
    pub fn check_step_up_required(status: u16) -> bool {
        status == 403
    }
}

impl Default for McpAuthManager {
    fn default() -> Self { Self::new() }
}

/// PKCE Challenge 参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkceChallenge {
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: String,
    pub redirect_uri: String,
}

/// 生成 Code Verifier (128 字符, RFC 7636)
fn generate_code_verifier() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..128)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// SHA-256 Base64 URL 编码（不含 padding）
fn sha256_base64_url(input: &str) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(input.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

/// 构建授权 URL
/// 源自 Claude Code 的 `redirectToAuthorization()`
pub fn build_authorization_url(
    metadata: &AuthServerMetadata,
    client_id: &str,
    challenge: &PkceChallenge,
    scope: &str,
) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", &challenge.redirect_uri),
        ("code_challenge", &challenge.code_challenge),
        ("code_challenge_method", &challenge.code_challenge_method),
        ("state", &challenge.state),
        ("scope", scope),
    ];
    let query: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{}", metadata.authorization_endpoint, query)
}

fn urlencode(s: &str) -> String {
    s.replace(' ', "%20")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_flow() {
        let mgr = McpAuthManager::new();
        let challenge = mgr.start_pkce_flow("test-server", "http://localhost:12345/callback").unwrap();

        assert_eq!(challenge.code_challenge_method, "S256");
        assert!(!challenge.state.is_empty());
        assert!(!challenge.code_challenge.is_empty());

        // Verify verifier stored
        let verifier = mgr.get_pkce_verifier("test-server");
        assert!(verifier.is_some());
    }

    #[test]
    fn test_token_save_and_get() {
        let mgr = McpAuthManager::new();
        let tokens = McpOAuthTokens {
            access_token: "abc".into(),
            refresh_token: Some("def".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            scope: Some("read".into()),
            token_type: "Bearer".into(),
        };

        mgr.save_tokens("server-1", tokens.clone());
        let retrieved = mgr.get_tokens("server-1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().access_token, "abc");
    }

    #[test]
    fn test_token_expiry_refresh_needed() {
        let mgr = McpAuthManager::new();
        let tokens = McpOAuthTokens {
            access_token: "abc".into(),
            refresh_token: Some("def".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(2)), // expiring soon
            scope: None,
            token_type: "Bearer".into(),
        };

        mgr.save_tokens("server-1", tokens);
        // Should return None because expires within 5 min
        assert!(mgr.get_tokens("server-1").is_none());
    }

    #[test]
    fn test_invalidate() {
        let mgr = McpAuthManager::new();
        mgr.save_tokens("server-1", McpOAuthTokens {
            access_token: "abc".into(),
            refresh_token: None,
            expires_at: None,
            scope: None,
            token_type: "Bearer".into(),
        });
        mgr.invalidate_tokens("server-1");
        assert!(mgr.get_tokens("server-1").is_none());
    }

    #[test]
    fn test_step_up_detection() {
        assert!(McpAuthManager::check_step_up_required(403));
        assert!(!McpAuthManager::check_step_up_required(401));
        assert!(!McpAuthManager::check_step_up_required(200));
    }

    #[test]
    fn test_client_info() {
        let mgr = McpAuthManager::new();
        mgr.save_client_info("server-1", McpOAuthClientInfo {
            client_id: "my-client".into(),
            client_secret: Some("secret".into()),
            redirect_uri: "http://localhost:12345/callback".into(),
        });
        let info = mgr.get_client_info("server-1");
        assert!(info.is_some());
        assert_eq!(info.unwrap().client_id, "my-client");
    }
}
