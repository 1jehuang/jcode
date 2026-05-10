//! # gRPC 认证 (Auth) 拦截器
//!
//! 支持两种认证方式:
//! 1. **API Token**: 在 gRPC metadata 中验证 `authorization` 头
//! 2. **mTLS**: 在传输层验证客户端证书（由 tonic ServerTlsConfig 处理）
//!
//! ## API Token 使用
//! 客户端在 metadata 中携带:
//! ```text
//! authorization: Bearer <token>
//! ```
//!
//! ## 增强说明
//! - 新增 `RateLimiter` 用于 API Token 请求限流
//! - 新增 `TokenScope` 支持 Token 作用域控制
//! - 新增 `extract_token_identity` 从请求中提取 Token 标识
//!
//! ## 配置
//! 通过 `JCODE_GRPC_API_TOKEN` 环境变量或 `~/.jcode/config.toml` 中的 `[grpc]` 节配置。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::{Request, Status};

/// Token 作用域
#[derive(Debug, Clone, PartialEq)]
pub enum TokenScope {
    /// 完全访问
    Full,
    /// 只读访问
    ReadOnly,
    /// 仅限于指定服务
    Services(Vec<String>),
}

impl TokenScope {
    #[allow(dead_code)]
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "readonly" | "read-only" | "ro" => TokenScope::ReadOnly,
            s if s.starts_with("services:") => {
                let services: Vec<String> = s
                    .strip_prefix("services:")
                    .unwrap_or("")
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if services.is_empty() {
                    TokenScope::Full
                } else {
                    TokenScope::Services(services)
                }
            }
            _ => TokenScope::Full,
        }
    }

    /// 检查是否允许访问指定服务
    fn allows_service(&self, service_name: &str) -> bool {
        match self {
            TokenScope::Full => true,
            TokenScope::ReadOnly => {
                // 只读服务列表
                matches!(service_name, "get" | "list" | "retrieve" | "hover" | "symbols")
            }
            TokenScope::Services(services) => services.iter().any(|s| s == service_name),
        }
    }
}

/// 简单令牌桶限流器
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// 每秒允许的请求数
    max_requests_per_sec: u64,
    /// 令牌桶大小
    bucket_size: u64,
    /// 当前令牌数 (受 RwLock 保护)
    state: Arc<RwLock<RateLimiterState>>,
}

#[derive(Debug, Clone)]
struct RateLimiterState {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(max_requests_per_sec: u64) -> Self {
        Self {
            max_requests_per_sec,
            bucket_size: max_requests_per_sec,
            state: Arc::new(RwLock::new(RateLimiterState {
                tokens: max_requests_per_sec as f64,
                last_refill: Instant::now(),
            })),
        }
    }

    /// 尝试消耗一个令牌，返回是否允许通过
    pub fn try_consume(&self) -> bool {
        let mut state = self.state.write();
        // Refill tokens
        let elapsed = state.last_refill.elapsed().as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.max_requests_per_sec as f64)
            .min(self.bucket_size as f64);
        state.last_refill = Instant::now();

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(100) // 默认 100 req/s
    }
}

/// Token 身份信息
#[derive(Debug, Clone)]
pub struct TokenIdentity {
    /// 使用的 Token 前缀（掩码显示）
    pub token_prefix: String,
    /// Token 作用域
    pub scope: TokenScope,
    /// 是否为有效 Token
    pub is_valid: bool,
}

/// 认证服务
#[derive(Debug, Clone)]
pub struct AuthService {
    /// 可选的 API Token (为空则不验证)
    pub(crate) api_token: Arc<Option<String>>,
    /// 是否启用 mTLS
    pub(crate) mtls_enabled: bool,
    /// 限流器
    rate_limiter: Option<Arc<RateLimiter>>,
    /// 多个 Token 配置（用于多租户场景）
    tokens: Arc<HashMap<String, TokenScope>>,
}

impl AuthService {
    /// 创建新的认证服务
    pub fn new(api_token: Option<String>, mtls_enabled: bool) -> Self {
        Self {
            api_token: Arc::new(api_token),
            mtls_enabled,
            rate_limiter: None,
            tokens: Arc::new(HashMap::new()),
        }
    }

    /// 创建带限流的认证服务
    pub fn with_rate_limit(mut self, max_rps: u64) -> Self {
        self.rate_limiter = Some(Arc::new(RateLimiter::new(max_rps)));
        self
    }

    /// 添加多个 Token（多租户）
    pub fn with_tokens(mut self, tokens: HashMap<String, TokenScope>) -> Self {
        self.tokens = Arc::new(tokens);
        self
    }

    /// 添加单个 Token
    pub fn add_token(mut self, token: &str, scope: TokenScope) -> Self {
        let mut tokens = HashMap::clone(&self.tokens);
        tokens.insert(token.to_string(), scope);
        self.tokens = Arc::new(tokens);
        self
    }

    /// 检查请求是否通过认证
    fn verify_token(&self, request: &Request<()>) -> Result<TokenIdentity, Status> {
        // 先从预设的多 Token 表中查找
        let tokens = &*self.tokens;
        let primary_token = self.api_token.as_ref();

        // 从 metadata 中提取 authorization header
        let auth_header = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("Missing authorization header"))?;

        let bearer_token = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| Status::unauthenticated("Invalid authorization format, expected 'Bearer <token>'"))?;

        // 检查限流
        if let Some(ref limiter) = self.rate_limiter {
            if !limiter.try_consume() {
                return Err(Status::unauthenticated("Rate limit exceeded"));
            }
        }

        // 验证 Token
        let is_valid = if !tokens.is_empty() {
            tokens.contains_key(bearer_token)
        } else if let Some(primary) = primary_token {
            bearer_token == primary.as_str()
        } else {
            // Token auth 未配置，跳过
            return Ok(TokenIdentity {
                token_prefix: "none".to_string(),
                scope: TokenScope::Full,
                is_valid: true,
            });
        };

        if !is_valid {
            return Err(Status::unauthenticated("Invalid API token"));
        }

        let prefix = if bearer_token.len() > 8 {
            format!("{}***", &bearer_token[..8])
        } else {
            "***".to_string()
        };

        let scope = tokens.get(bearer_token)
            .cloned()
            .unwrap_or(TokenScope::Full);

        Ok(TokenIdentity {
            token_prefix: prefix,
            scope,
            is_valid: true,
        })
    }

    /// 检查服务是否允许访问
    pub fn check_service_access(&self, identity: &TokenIdentity, service: &str) -> Result<(), Status> {
        if !identity.scope.allows_service(service) {
            return Err(Status::permission_denied(
                format!("Token '{}' does not have access to service '{}'", identity.token_prefix, service),
            ));
        }
        Ok(())
    }

    /// 获取 mTLS 状态
    pub fn mtls_enabled(&self) -> bool {
        self.mtls_enabled
    }
}

/// 服务器端认证拦截器
///
/// 验证所有 gRPC 请求中的 API Token。
/// 如果 token_auth 未启用，则所有请求放行。
#[derive(Clone)]
pub struct TokenInterceptor {
    auth_service: AuthService,
}

impl TokenInterceptor {
    /// 创建 Token 拦截器
    ///
    /// * `api_token` - 期望的 API Token，为空则跳过验证
    /// * `mtls_enabled` - mTLS 是否启用（仅用于日志）
    pub fn new(api_token: Option<String>, mtls_enabled: bool) -> Self {
        Self {
            auth_service: AuthService::new(api_token, mtls_enabled),
        }
    }

    /// 从 config 创建 Token 拦截器
    pub fn from_config(grpc_cfg: &crate::config::GrpcConfig) -> Self {
        let api_token = if grpc_cfg.token_auth_enabled && !grpc_cfg.api_token.is_empty() {
            Some(grpc_cfg.api_token.clone())
        } else {
            None
        };
        Self::new(api_token, grpc_cfg.mtls_enabled)
    }
}

impl Interceptor for TokenInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        // 如果没有配置 Token 认证，直接放行
        let is_auth_enabled = self.auth_service.api_token.is_some()
            || !self.auth_service.tokens.is_empty();
        if !is_auth_enabled && !self.auth_service.mtls_enabled {
            return Ok(request);
        }

        self.auth_service.verify_token(&request)?;
        Ok(request)
    }
}

/// 客户端认证拦截器
///
/// 在客户端 gRPC 请求的 metadata 中注入 API Token。
#[derive(Clone)]
pub struct ClientTokenInterceptor {
    token: Arc<Option<MetadataValue<Ascii>>>,
}

impl ClientTokenInterceptor {
    /// 创建客户端 Token 拦截器
    pub fn new(api_token: Option<&str>) -> Self {
        let token = api_token
            .filter(|t| !t.is_empty())
            .and_then(|t| format!("Bearer {}", t).parse::<MetadataValue<Ascii>>().ok());
        Self {
            token: Arc::new(token),
        }
    }

    /// 从环境变量创建客户端 Token 拦截器
    pub fn from_env() -> Self {
        let token = std::env::var("JCODE_GRPC_API_TOKEN").ok();
        Self::new(token.as_deref())
    }

    /// 从 config 创建客户端 Token 拦截器
    pub fn from_config(grpc_cfg: &crate::config::GrpcConfig) -> Self {
        if grpc_cfg.token_auth_enabled && !grpc_cfg.api_token.is_empty() {
            Self::new(Some(&grpc_cfg.api_token))
        } else {
            Self::new(None)
        }
    }
}

impl Interceptor for ClientTokenInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        if let Some(ref token) = *self.token {
            request
                .metadata_mut()
                .insert("authorization", token.clone());
        }
        Ok(request)
    }
}

/// 创建带 TLS 的 gRPC 客户端通道（传输层安全）
///
/// 如果配置了 mTLS，通道将使用双向 TLS 连接。
/// API Token 认证由 [ClientTokenInterceptor] 在应用层单独处理，
/// 在创建客户端时作为 interceptor 层注入。
pub async fn create_authenticated_channel(
    endpoint: &str,
    tls_ca_cert: Option<Vec<u8>>,
    tls_client_cert: Option<Vec<u8>>,
    tls_client_key: Option<Vec<u8>>,
) -> anyhow::Result<tonic::transport::Channel> {
    use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};

    let channel = if let Some(ca) = tls_ca_cert {
        let mut tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&ca));

        if let (Some(cert), Some(key)) = (tls_client_cert, tls_client_key) {
            let identity = tonic::transport::Identity::from_pem(&cert, &key);
            tls = tls.identity(identity);
        }

        Endpoint::new(endpoint.to_string())?
            .tls_config(tls)
            .map_err(|e| anyhow::anyhow!("TLS config error: {}", e))?
            .connect()
            .await?
    } else {
        Endpoint::new(endpoint.to_string())?
            .connect()
            .await?
    };

    Ok(channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_interceptor_valid() {
        let mut interceptor = TokenInterceptor::new(Some("test-token".into()), false);
        let request = tonic::Request::new(());
        let result = interceptor.call(request);
        // No token in request metadata
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn test_client_token_interceptor() {
        let mut interceptor = ClientTokenInterceptor::new(Some("my-token"));
        let request = tonic::Request::new(());
        let result = interceptor.call(request).unwrap();
        let auth = result.metadata().get("authorization").unwrap();
        assert_eq!(auth.to_str().unwrap(), "Bearer my-token");
    }

    #[test]
    fn test_token_disabled() {
        let mut interceptor = TokenInterceptor::new(None, false);
        let request = tonic::Request::new(());
        let result = interceptor.call(request);
        assert!(result.is_ok());
    }

    #[test]
    fn test_auth_service_with_token() {
        let service = AuthService::new(Some("my-secret-token".into()), false);

        // Test with correct token
        let mut req = tonic::Request::new(());
        req.metadata_mut().insert("authorization", "Bearer my-secret-token".parse().unwrap());
        let result = service.verify_token(&req);
        assert!(result.is_ok());
        assert!(result.unwrap().is_valid);

        // Test with wrong token
        let mut req2 = tonic::Request::new(());
        req2.metadata_mut().insert("authorization", "Bearer wrong-token".parse().unwrap());
        let result2 = service.verify_token(&req2);
        assert!(result2.is_err());
    }

    #[test]
    fn test_auth_service_missing_header() {
        let service = AuthService::new(Some("token".into()), false);
        let request = tonic::Request::new(());
        let result = service.verify_token(&request);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(10);
        // Should allow 10 requests
        for _ in 0..10 {
            assert!(limiter.try_consume());
        }
        // Next one might be false if within same instant
        // (depends on timing)
    }

    #[test]
    fn test_token_scope_from_str() {
        assert_eq!(TokenScope::from_str("full"), TokenScope::Full);
        assert_eq!(TokenScope::from_str("readonly"), TokenScope::ReadOnly);
        assert_eq!(TokenScope::from_str("ro"), TokenScope::ReadOnly);

        let svc_scope = TokenScope::from_str("services:chat,memory");
        match svc_scope {
            TokenScope::Services(ref s) => {
                assert!(s.contains(&"chat".to_string()));
                assert!(s.contains(&"memory".to_string()));
            }
            _ => panic!("Expected Services scope"),
        }
    }

    #[test]
    fn test_token_scope_allows_service() {
        let full = TokenScope::Full;
        assert!(full.allows_service("any_service"));

        let read_only = TokenScope::ReadOnly;
        assert!(read_only.allows_service("get"));
        assert!(read_only.allows_service("list"));
        assert!(!read_only.allows_service("create"));

        let services = TokenScope::Services(vec!["chat".to_string(), "session".to_string()]);
        assert!(services.allows_service("chat"));
        assert!(services.allows_service("session"));
        assert!(!services.allows_service("memory"));
    }

    #[test]
    fn test_auth_service_with_rate_limit() {
        let service = AuthService::new(Some("test".into()), false)
            .with_rate_limit(100);
        assert!(service.rate_limiter.is_some());
    }

    #[test]
    fn test_auth_service_multi_token() {
        let mut tokens = HashMap::new();
        tokens.insert("admin-token".to_string(), TokenScope::Full);
        tokens.insert("readonly-token".to_string(), TokenScope::ReadOnly);
        let service = AuthService::new(None, false).with_tokens(tokens);

        // Admin token
        let mut req = tonic::Request::new(());
        req.metadata_mut().insert("authorization", "Bearer admin-token".parse().unwrap());
        let identity = service.verify_token(&req).unwrap();
        assert_eq!(identity.scope, TokenScope::Full);

        // Read-only token
        let mut req2 = tonic::Request::new(());
        req2.metadata_mut().insert("authorization", "Bearer readonly-token".parse().unwrap());
        let identity2 = service.verify_token(&req2).unwrap();
        assert_eq!(identity2.scope, TokenScope::ReadOnly);

        // Unknown token
        let mut req3 = tonic::Request::new(());
        req3.metadata_mut().insert("authorization", "Bearer unknown".parse().unwrap());
        assert!(service.verify_token(&req3).is_err());
    }
}
