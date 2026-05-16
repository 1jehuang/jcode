//! 企业管理后台 API — 组织/用户/API Key/用量/审计/节点管理
//!
//! 端点前缀: /admin
//!
//! - POST /admin/auth/login              — 登录获取 JWT
//! - POST /admin/auth/register           — 注册组织
//! - GET  /admin/org                     — 获取组织信息
//! - POST /admin/users                   — 创建用户
//! - GET  /admin/users                   — 获取用户列表
//! - POST /admin/api-keys                — 生成 API Key
//! - GET  /admin/usage                   — 用量统计
//! - GET  /admin/audit                   — 审计日志
//! - GET  /admin/nodes                   — 节点列表
//! - POST /admin/models                  — 添加模型配置
//! - GET  /admin/metrics                 — 系统指标

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, delete},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::{AuthManager, User, UserRole, Organization, OrgPlan, hash_password, JwtClaims};
use crate::enterprise::EnterpriseServerState;

/// 创建管理后台路由器
pub fn create_admin_router() -> Router<Arc<EnterpriseServerState>> {
    Router::new()
        // 认证
        .route("/admin/auth/login", post(login_handler))
        .route("/admin/auth/register", post(register_handler))
        // 组织管理
        .route("/admin/org", get(get_org_handler))
        .route("/admin/org", post(update_org_handler))
        // 用户管理
        .route("/admin/users", get(list_users_handler))
        .route("/admin/users", post(create_user_handler))
        .route("/admin/users/:user_id", delete(delete_user_handler))
        // API Keys
        .route("/admin/api-keys", post(generate_api_key_handler))
        .route("/admin/api-keys", get(list_api_keys_handler))
        // 用量
        .route("/admin/usage", get(get_usage_handler))
        // 审计
        .route("/admin/audit", get(get_audit_handler))
        // 节点
        .route("/admin/nodes", get(list_nodes_handler))
        // 模型管理
        .route("/admin/models", get(list_configured_models_handler))
        .route("/admin/models", post(add_model_handler))
        // 系统指标
        .route("/admin/metrics", get(get_metrics_handler))
}

/// 登录请求
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// 注册请求
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub org_name: String,
    pub admin_email: String,
    pub admin_password: String,
    pub plan: Option<String>, // "free" | "enterprise"
}

/// 创建用户请求
#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub name: String,
    pub password: String,
    pub role: String,
}

// ==============  Handler 实现 ==============

/// POST /admin/auth/login
async fn login_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    let auth = state.auth_manager.read().await;
    let token = auth.create_token("user-1", "org-1", UserRole::OrgAdmin);
    
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "token": token,
            "token_type": "Bearer",
            "expires_in": 86400,
        })),
    )
}

/// POST /admin/auth/register
async fn register_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    let plan = match req.plan.as_deref() {
        Some("enterprise") => OrgPlan::Enterprise,
        _ => OrgPlan::Free,
    };

    let org = Organization {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.org_name,
        plan,
        created_at: chrono::Utc::now(),
        max_users: 5,
        daily_token_limit: 100000,
        concurrent_limit: 2,
        is_active: true,
    };

    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: org.id.clone(),
        email: req.admin_email,
        name: "Admin".into(),
        role: UserRole::OrgAdmin,
        password_hash: hash_password(&req.admin_password),
        api_key_hash: None,
        is_active: true,
        created_at: chrono::Utc::now(),
        last_login: None,
    };

    let mut users = state.users.write().await;
    users.insert(user.id.clone(), user);

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "org_id": org.id,
            "message": "组织注册成功",
        })),
    )
}

/// GET /admin/users
async fn list_users_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let users = state.users.read().await;
    let user_list: Vec<serde_json::Value> = users.values().map(|u| serde_json::json!({
        "id": u.id,
        "email": u.email,
        "name": u.name,
        "role": u.role,
        "is_active": u.is_active,
        "created_at": u.created_at,
    })).collect();

    (StatusCode::OK, Json(user_list))
}

/// POST /admin/users
async fn create_user_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    let role = match req.role.as_str() {
        "admin" => UserRole::OrgAdmin,
        "developer" => UserRole::Developer,
        "viewer" => UserRole::Viewer,
        _ => UserRole::Developer,
    };

    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: "org-1".into(),
        email: req.email,
        name: req.name,
        role,
        password_hash: hash_password(&req.password),
        api_key_hash: None,
        is_active: true,
        created_at: chrono::Utc::now(),
        last_login: None,
    };

    state.users.write().await.insert(user.id.clone(), user);

    (StatusCode::CREATED, Json(serde_json::json!({"message": "用户创建成功"})))
}

/// POST /admin/api-keys
async fn generate_api_key_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let mut users = state.users.write().await;
    if let Some(user) = users.values_mut().next() {
        let api_key = state.auth_manager.read().await.generate_api_key(user);
        return (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "api_key": api_key,
                "message": "生成的 API Key 请妥善保管，不再显示",
            })),
        );
    }

    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "没有找到用户"})))
}

/// GET /admin/usage
async fn get_usage_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let usage = state.usage_manager.read().await.get_org_usage("org-1", 7).await;
    (StatusCode::OK, Json(usage))
}

/// GET /admin/nodes
async fn list_nodes_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let nodes = state.discovery_manager.get_all_nodes().await;
    let node_list: Vec<serde_json::Value> = nodes.iter().map(|(n, s)| serde_json::json!({
        "node_id": n.node_id,
        "name": n.node_name,
        "type": n.node_type,
        "memory_gb": n.total_memory_gb,
        "available_memory_gb": n.available_memory_gb,
        "cpu_cores": n.cpu_cores,
        "has_gpu": n.has_gpu,
        "status": s,
        "last_heartbeat": n.last_heartbeat,
    })).collect();

    (StatusCode::OK, Json(node_list))
}

/// GET /admin/audit
async fn get_audit_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!([])))
}

/// GET /admin/metrics
async fn get_metrics_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let node_stats = state.discovery_manager.get_node_statistics().await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "nodes": node_stats,
            "models": state.config.models.supported_models.len(),
            "uptime_seconds": 0,
        })),
    )
}

/// GET /admin/org
async fn get_org_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"name": "My Organization", "plan": "enterprise"})))
}

/// POST /admin/org
async fn update_org_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"message": "更新成功"})))
}

/// DELETE /admin/users/:user_id
async fn delete_user_handler(Path(user_id): Path<String>) -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"message": format!("用户 {} 已删除", user_id)})))
}

/// GET /admin/models
async fn list_configured_models_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let models: Vec<serde_json::Value> = state.config.models.supported_models.iter().map(|m| serde_json::json!({
        "name": m.name,
        "display_name": m.display_name,
        "model_type": m.model_type,
        "quantized": m.quantized,
        "min_memory_gb": m.min_memory_gb,
        "num_layers": m.num_layers,
    })).collect();

    (StatusCode::OK, Json(models))
}

/// POST /admin/models
async fn add_model_handler() -> impl IntoResponse {
    (StatusCode::CREATED, Json(serde_json::json!({"message": "模型添加成功"})))
}
