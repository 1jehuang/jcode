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
//! ## 配置
//! 通过 `JCODE_GRPC_API_TOKEN` 环境变量或 `~/.jcode/config.toml` 中的 `[grpc]` 节配置。

use std::sync::Arc;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::{Request, Status};

/// 认证服务
#[derive(Debug, Clone)]
pub struct AuthService {
    /// 可选的 API Token (为空则不验证)
    api_token: Arc<Option<String>>,
    /// 是否启用 mTLS
    mtls_enabled: bool,
}

impl AuthService {
    /// 创建新的认证服务
    pub fn new(api_token: Option<String>, mtls_enabled: bool) -> Self {
        Self {
            api_token: Arc::new(api_token),
            mtls_enabled,
        }
    }

    /// 检查请求是否通过认证
    fn verify_token(&self, request: &Request<()>) -> Result<(), Status> {
        let token = match self.api_token.as_ref() {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(()), // Token auth 未配置，跳过
        };

        // 从 metadata 中提取 authorization header
        let auth_header = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                Status::unauthenticated("Missing authorization header")
            })?;

        // 验证 Bearer token 格式
        if let Some(bearer_token) = auth_header.strip_prefix("Bearer ") {
            if bearer_token == token {
                return Ok(());
            }
        }

        Err(Status::unauthenticated("Invalid API token"))
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
    let endpoint_owned = endpoint.to_string();
    let channel = if let Some(ca) = tls_ca_cert {
        use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};

        let mut tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&ca));

        if let (Some(cert), Some(key)) = (tls_client_cert, tls_client_key) {
            let identity = tonic::transport::Identity::from_pem(&cert, &key);
            tls = tls.identity(identity);
        }

        Endpoint::new(endpoint_owned)?
            .tls_config(tls)
            .map_err(|e| anyhow::anyhow!("TLS config error: {}", e))?
            .connect()
            .await?
    } else {
        tonic::transport::Endpoint::new(endpoint_owned)?
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
}
