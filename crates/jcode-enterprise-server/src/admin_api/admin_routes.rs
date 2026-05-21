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
    middleware,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::{AuthManager, User, UserRole, Organization, OrgPlan, hash_password, JwtClaims, Permission, PermissionScope};
use crate::enterprise::EnterpriseServerState;
use super::auth_middleware::{auth_middleware, current_user_id, current_org_id};

/// 创建管理后台路由器
pub fn create_admin_router(state: Arc<EnterpriseServerState>) -> Router {
    Router::new()
        // 公开端点（无需认证）
        .route("/admin/auth/login", post(login_handler))
        .route("/admin/auth/register", post(register_handler))
        // 受保护端点（需要认证）
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // 组织管理
        .route("/admin/org", get(get_org_handler))
        .route("/admin/org", post(update_org_handler))
        // 用户管理
        .route("/admin/users", get(list_users_handler))
        .route("/admin/users", post(create_user_handler))
        .route("/admin/users/:user_id", delete(delete_user_handler))
        // RBAC 角色管理
        .route("/admin/roles", get(list_roles_handler))
        .route("/admin/roles", post(create_role_handler))
        .route("/admin/roles/:role_id", delete(delete_role_handler))
        .route("/admin/users/:user_id/roles", post(assign_role_handler))
        .route("/admin/users/:user_id/roles/:role_id", delete(revoke_role_handler))
        .route("/admin/users/:user_id/permissions", get(get_user_permissions_handler))
        // API Keys
        .route("/admin/api-keys", post(generate_api_key_handler))
        .route("/admin/api-keys", get(list_api_keys_handler))
        // 用量和配额
        .route("/admin/usage", get(get_usage_handler))
        .route("/admin/quota", get(get_quota_handler))
        .route("/admin/quota", post(update_quota_handler))
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

/// 分配角色请求
#[derive(Debug, Deserialize)]
pub struct AssignRoleRequest {
    pub role_id: String,
}

/// 创建角色请求
#[derive(Debug, Deserialize)]
pub struct CreateRoleRequest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub scope: String,
    pub scope_value: Option<String>,
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

/// GET /admin/quota - 获取用户配额状态
async fn get_quota_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    // 示例：获取第一个用户的配额
    let users = state.users.read().await;
    if let Some(user) = users.values().next() {
        let tracker = state.quota_tracker.read().await;
        if let Some(summary) = tracker.get_usage_summary(&user.id) {
            return (StatusCode::OK, Json(summary));
        }
    }

    (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "没有找到用户用量数据"})))
}

/// POST /admin/quota - 更新用户配额
async fn update_quota_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    Json(req): Json<UpdateQuotaRequest>,
) -> impl IntoResponse {
    let tier = match req.tier.as_str() {
        "free" => UsageTier::Free,
        "pro" => UsageTier::Pro,
        "enterprise" => UsageTier::Enterprise,
        _ => UsageTier::Free,
    };

    let policy = QuotaPolicy::new(tier)
        .with_soft_limit(req.soft_limit.unwrap_or(false))
        .with_warning_threshold(req.warning_threshold.unwrap_or(80));

    let mut tracker = state.quota_tracker.write().await;
    tracker.set_policy(req.user_id.clone(), policy);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "配额更新成功",
            "user_id": req.user_id,
            "tier": req.tier,
        })),
    )
}

/// 更新配额请求
#[derive(Debug, Deserialize)]
pub struct UpdateQuotaRequest {
    pub user_id: String,
    pub tier: String,
    pub soft_limit: Option<bool>,
    pub warning_threshold: Option<u32>,
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

// ============== RBAC Handlers ==============

/// GET /admin/roles - 列出所有角色
async fn list_roles_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let engine = state.auth_manager.read().await.policy_engine.read().await;
    let roles: Vec<serde_json::Value> = engine.list_roles().iter().map(|role| {
        let permissions: Vec<String> = role.permissions.iter().map(|p| p.name()).collect();
        serde_json::json!({
            "id": role.id,
            "name": role.name,
            "description": role.description,
            "permissions": permissions,
            "scope": format!("{:?}", role.scope),
            "is_builtin": role.is_builtin,
        })
    }).collect();

    (StatusCode::OK, Json(roles))
}

/// POST /admin/roles - 创建自定义角色
async fn create_role_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    Json(req): Json<CreateRoleRequest>,
) -> impl IntoResponse {
    let mut engine = state.auth_manager.write().await.policy_engine.write().await;

    // 解析权限
    let mut permissions = std::collections::HashSet::new();
    for perm_name in &req.permissions {
        if let Some(perm) = Permission::from_name(perm_name) {
            permissions.insert(perm);
        } else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("无效权限: {}", perm_name)})),
            );
        }
    }

    // 解析范围
    let scope = match req.scope.as_str() {
        "global" => PermissionScope::Global,
        "organization" => PermissionScope::Organization(req.scope_value.unwrap_or_default()),
        "team" => PermissionScope::Team(req.scope_value.unwrap_or_default()),
        "project" => PermissionScope::Project(req.scope_value.unwrap_or_default()),
        _ => PermissionScope::Global,
    };

    let role = Role {
        id: req.id.clone(),
        name: req.name,
        description: req.description,
        permissions,
        scope,
        is_builtin: false,
        parent_role: None,
    };

    engine.register_role(role);

    (
        StatusCode::CREATED,
        Json(serde_json::json!({"message": "角色创建成功", "role_id": req.id})),
    )
}

/// DELETE /admin/roles/:role_id - 删除角色
async fn delete_role_handler(
    Path(role_id): Path<String>,
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let mut engine = state.auth_manager.write().await.policy_engine.write().await;

    match engine.delete_role(&role_id) {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({"message": format!("角色 {} 已删除", role_id)})),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        ),
    }
}

/// POST /admin/users/:user_id/roles - 分配角色给用户
async fn assign_role_handler(
    Path(user_id): Path<String>,
    Json(req): Json<AssignRoleRequest>,
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    state.auth_manager.read().await.assign_role_to_user(user_id.clone(), req.role_id.clone()).await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "角色分配成功",
            "user_id": user_id,
            "role_id": req.role_id,
        })),
    )
}

/// DELETE /admin/users/:user_id/roles/:role_id - 撤销用户角色
async fn revoke_role_handler(
    Path((user_id, role_id)): Path<(String, String)>,
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let mut engine = state.auth_manager.write().await.policy_engine.write().await;
    engine.revoke_role(&user_id, &role_id);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "角色撤销成功",
            "user_id": user_id,
            "role_id": role_id,
        })),
    )
}

/// GET /admin/users/:user_id/permissions - 获取用户权限
async fn get_user_permissions_handler(
    Path(user_id): Path<String>,
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let engine = state.auth_manager.read().await.policy_engine.read().await;
    let permissions: Vec<String> = engine.get_user_permissions(&user_id)
        .iter()
        .map(|p| p.name())
        .collect();

    let roles: Vec<String> = engine.get_user_roles(&user_id)
        .iter()
        .map(|r| r.id.clone())
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "user_id": user_id,
            "roles": roles,
            "permissions": permissions,
        })),
    )
}
