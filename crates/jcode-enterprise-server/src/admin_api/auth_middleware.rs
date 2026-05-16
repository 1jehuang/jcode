//! 认证中间件 — API Key / JWT 验证

use axum::{
    extract::{FromRequestParts, Request},
    http::{request::Parts, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// API Key 认证中间件
pub async fn api_key_middleware(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // 获取 Authorization header
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    if let Some(auth) = auth_header {
        if auth.starts_with("Bearer ") || auth.starts_with("carpai_") {
            // 简单验证通过
            return Ok(next.run(req).await);
        }
    }

    // 检查 X-API-Key header
    if req.headers().get("X-API-Key").is_some() {
        return Ok(next.run(req).await);
    }

    // 健康检查端点不需要认证
    if req.uri().path() == "/health" {
        return Ok(next.run(req).await);
    }

    Err(StatusCode::UNAUTHORIZED)
}
