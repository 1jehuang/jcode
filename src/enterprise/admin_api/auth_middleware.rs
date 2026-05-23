//! 认证与授权中间件
//!
//! 提供：
//! - JWT Token认证
//! - API Key认证
//! - RBAC权限检查

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::enterprise::auth::JwtClaims;
use crate::enterprise::auth::Permission;
use crate::enterprise::enterprise::EnterpriseServerState;

/// 认证上下文（注入到请求扩展中）
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: String,
    pub org_id: Option<String>,
    pub claims: JwtClaims,
}

/// 完整认证中间件 - 支持JWT和API Key
pub async fn auth_middleware(
    state: axum::extract::State<Arc<EnterpriseServerState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let headers = req.headers();

    // 健康检查和公开端点不需要认证
    let path = req.uri().path();
    if path == "/health" || path == "/metrics" || path.starts_with("/admin/auth/") {
        return Ok(next.run(req).await);
    }

    // 尝试从Authorization头获取JWT Token
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(auth_value) = auth_header {
        if let Some(token) = auth_value.strip_prefix("Bearer ") {
            let auth = state.jwt_manager.read().await;
            match auth.validate_token(token) {
                Ok(validation) if validation.is_valid => {
                    let sub = validation.claims.sub.clone();
                    let ctx = AuthContext {
                        user_id: sub,
                        org_id: validation.claims.custom.get("org_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                        claims: validation.claims,
                    };
                    req.extensions_mut().insert(ctx);
                    drop(auth);
                    return Ok(next.run(req).await);
                }
                Err(e) | Ok(_) => {
                    tracing::warn!("JWT验证失败: {:?}", e);
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
        }
    }

    // 尝试从X-API-Key头获取API Key
    if let Some(api_key) = headers
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
    {
        let users = state.users.read().await;
        for user in users.values() {
            if user.verify_api_key(api_key) {
                use crate::enterprise::auth::UserRole;
                let now = chrono::Utc::now();
                let claims = JwtClaims::new(
                    user.id.clone(),
                    "carpai-enterprise".to_string(),
                    24,
                )
                .with_claim("org_id", serde_json::json!(user.org_id))
                .with_claim("role", serde_json::json!(format!("{}", user.role)));
                let ctx = AuthContext {
                    user_id: user.id.clone(),
                    org_id: user.org_id.clone(),
                    claims,
                };
                req.extensions_mut().insert(ctx);
                drop(users);
                return Ok(next.run(req).await);
            }
        }
        return Err(StatusCode::UNAUTHORIZED);
    }

    Err(StatusCode::UNAUTHORIZED)
}

/// 创建需要特定权限的路由保护
pub struct PermissionGuard {
    permission: Permission,
    scope: Option<String>,

}

impl PermissionGuard {
    pub fn new(permission: Permission) -> Self {
        Self {
            permission,
            scope: None,
        }
    }

    pub fn with_scope(mut self, scope: String) -> Self {

        self.scope = Some(scope);
        self
    }

    pub async fn check(
        &self,
        state: &Arc<EnterpriseServerState>,
        user_id: &str,
    ) -> bool {
        let auth = state.jwt_manager.read().await;
        auth.check_permission(user_id, &self.permission, self.scope.as_ref()).await
    }
}

/// 从请求中提取当前用户ID
pub fn current_user_id(req: &Request) -> Option<String> {
    req.extensions()
        .get::<AuthContext>()
        .map(|ctx| ctx.user_id.clone())
}

/// 从请求中提取当前组织ID
pub fn current_org_id(req: &Request) -> Option<String> {
    req.extensions()
        .get::<AuthContext>()
        .and_then(|ctx| ctx.org_id.clone())
}

/// 认证响应
#[derive(Debug, Serialize, Deserialize)]
pub struct AuthResponse {
    pub user_id: String,
    pub org_id: String,
    pub role: String,
}

/// 简化的API Key中间件（用于公开API）
pub async fn api_key_middleware(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // 健康检查端点不需要认证
    if req.uri().path() == "/health" {
        return Ok(next.run(req).await);
    }

    // 检查是否有API Key或Bearer Token
    let has_auth = req.headers().get("X-API-Key").is_some()
        || req.headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.starts_with("Bearer ") || s.starts_with("carpai_"))
            .unwrap_or(false);

    if has_auth {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
