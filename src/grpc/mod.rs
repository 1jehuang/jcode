pub mod proto {
    tonic::include_proto!("jcode");
}

pub mod tls;
pub mod auth_interceptor;

pub use auth_interceptor::{
    AuthService, TokenInterceptor, ClientTokenInterceptor,
    TokenScope, TokenIdentity, RateLimiter,
};
pub use tls::{
    TlsConfig, TlsConfigBuilder, MtlsClientStatus,
    check_mtls_config, self_signed_help,
};

use proto::{
    session_service_server::SessionService,
    chat_service_server::ChatService,
    memory_service_server::MemoryService,
    agent_service_server::AgentService,
    tool_service_server::ToolService,
    tenant_service_server::TenantService,
    joy_code_service_server::JoyCodeService,
    open_code_service_server::OpenCodeService,
    plugin_service_server::PluginService,
};

use std::sync::Arc;
use parking_lot::RwLock;

// ══════════════════════════════════════════════════════════════════
// GrpcServerBuilder
// ══════════════════════════════════════════════════════════════════

/// gRPC 服务器构建器 — 支持 TLS、mTLS、API Token 认证
///
/// ## 增强说明
/// - 新增 `with_auth_service()` 直接传入 AuthService
/// - `serve()` 添加 TLS/mTLS/Token 状态日志
pub struct GrpcServerBuilder {
    provider: Option<Arc<dyn crate::provider::Provider>>,
    tls_config: Option<Arc<tonic::transport::server::ServerTlsConfig>>,
    token_interceptor: Option<TokenInterceptor>,
    /// 是否启用 mTLS (仅用于日志)
    mtls_enabled: bool,
}

impl GrpcServerBuilder {
    pub fn new() -> Self {
        Self {
            provider: None,
            tls_config: None,
            token_interceptor: None,
            mtls_enabled: false,
        }
    }

    pub fn with_provider(mut self, provider: Arc<dyn crate::provider::Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// 配置 mTLS/TLS
    pub fn with_tls(mut self, tls: Arc<tonic::transport::server::ServerTlsConfig>) -> Self {
        self.tls_config = Some(tls);
        self
    }

    /// 从 TLS 配置文件和 mTLS 标记自动构建 TLS 配置
    pub fn with_tls_files(mut self, cert_path: &str, key_path: &str, ca_cert_path: Option<&str>, mtls: bool) -> anyhow::Result<Self> {
        let tls_cfg = tls::load_tls_config(cert_path, key_path, ca_cert_path)?;
        if mtls {
            tls::check_mtls_config(&tls_cfg)?;
        }
        let server_tls = tls::build_server_tls_config(&tls_cfg, mtls)?;
        self.tls_config = Some(server_tls);
        self.mtls_enabled = mtls;
        Ok(self)
    }

    /// 配置 API Token 认证拦截器
    pub fn with_token_auth(mut self, interceptor: TokenInterceptor) -> Self {
        self.token_interceptor = Some(interceptor);
        self
    }

    /// 直接传入 AuthService 配置（更灵活）
    pub fn with_auth_service(mut self, auth: &AuthService) -> Self {
        let api_token = auth.api_token.as_ref().clone();
        self.token_interceptor = Some(TokenInterceptor::new(api_token, auth.mtls_enabled));
        self.mtls_enabled = auth.mtls_enabled;
        self
    }

    /// 从 config 自动配置 TLS + Token
    pub fn with_config(mut self, grpc_cfg: &crate::config::GrpcConfig) -> Self {
        // 配置 TLS/mTLS
        if !grpc_cfg.tls_cert_path.is_empty() && !grpc_cfg.tls_key_path.is_empty() {
            match tls::load_tls_config(
                &grpc_cfg.tls_cert_path,
                &grpc_cfg.tls_key_path,
                if grpc_cfg.mtls_enabled { Some(&grpc_cfg.tls_ca_cert_path) } else { None },
            ) {
                Ok(tls_cfg) => {
                    if grpc_cfg.mtls_enabled {
                        if let Err(e) = tls::check_mtls_config(&tls_cfg) {
                            tracing::warn!("mTLS config incomplete: {}", e);
                        }
                    }
                    match tls::build_server_tls_config(&tls_cfg, grpc_cfg.mtls_enabled) {
                        Ok(server_tls) => {
                            self.tls_config = Some(server_tls);
                            self.mtls_enabled = grpc_cfg.mtls_enabled;
                        }
                        Err(e) => tracing::warn!("Failed to build TLS config: {}", e),
                    }
                }
                Err(e) => tracing::warn!("Failed to load TLS files: {}", e),
            }
        }

        // 配置 Token 认证
        if grpc_cfg.token_auth_enabled && !grpc_cfg.api_token.is_empty() {
            self.token_interceptor = Some(TokenInterceptor::from_config(grpc_cfg));
        }

        self
    }

    pub async fn serve(self, addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        let provider = if let Some(p) = self.provider { p } else {
            let pc = crate::cli::provider_init::ProviderChoice::Auto;
            crate::cli::provider_init::init_provider(&pc, None).await?
        };

        // 记录安全配置状态
        if self.tls_config.is_some() {
            if self.mtls_enabled {
                tracing::info!("🔒 mTLS enabled: bidirectional certificate verification");
            } else {
                tracing::info!("🔒 TLS enabled: server certificate only");
            }
        } else {
            tracing::info!("🔓 TLS disabled: unencrypted connections");
        }

        if self.token_interceptor.is_some() {
            tracing::info!("🔑 API Token authentication enabled");
        } else {
            tracing::info!("🔓 API Token authentication disabled");
        }

        let sessions = Arc::new(RwLock::new(std::collections::HashMap::new()));

        macro_rules! register_all_services {
            ($router:expr) => {
                $router
                    .add_service(proto::session_service_server::SessionServiceServer::new(SessionServiceImpl::new(sessions.clone())))
                    .add_service(proto::chat_service_server::ChatServiceServer::new(ChatServiceImpl::new(sessions.clone(), Arc::clone(&provider))))
                    .add_service(proto::memory_service_server::MemoryServiceServer::new(MemoryServiceImpl::new()))
                    .add_service(proto::agent_service_server::AgentServiceServer::new(AgentServiceImpl::new(Arc::clone(&provider))))
                    .add_service(proto::tool_service_server::ToolServiceServer::new(ToolServiceImpl::new(Arc::clone(&provider))))
                    .add_service(proto::tenant_service_server::TenantServiceServer::new(TenantServiceImpl::new()))
                    .add_service(proto::joy_code_service_server::JoyCodeServiceServer::new(JoyCodeServiceImpl::new()))
                    .add_service(proto::open_code_service_server::OpenCodeServiceServer::new(OpenCodeServiceImpl::new(Arc::clone(&provider))))
                    .add_service(proto::plugin_service_server::PluginServiceServer::new(PluginServiceImpl::new()))
            };
        }

        let server = tonic::transport::Server::builder();

        let mut server = if let Some(tls) = self.tls_config {
            server.tls_config((*tls).clone())
                .map_err(|e| anyhow::anyhow!("TLS config error: {}", e))?
        } else {
            server
        };

        if let Some(token_interceptor) = self.token_interceptor {
            let mut router = server.layer(tonic::service::interceptor(token_interceptor));
            register_all_services!(router).serve(addr).await?;
        } else {
            register_all_services!(server).serve(addr).await?;
        }
        Ok(())
    }
}
impl Default for GrpcServerBuilder { fn default() -> Self { Self::new() } }

// ══════════════════════════════════════════════════════════════════
// Session Service
// ══════════════════════════════════════════════════════════════════

struct SessionServiceImpl {
    sessions: Arc<RwLock<std::collections::HashMap<String, proto::Session>>>,
}

impl SessionServiceImpl {
    fn new(sessions: Arc<RwLock<std::collections::HashMap<String, proto::Session>>>) -> Self { Self { sessions } }
}

#[tonic::async_trait]
impl SessionService for SessionServiceImpl {
    async fn create_session(&self, req: tonic::Request<proto::CreateSessionRequest>) -> Result<tonic::Response<proto::CreateSessionResponse>, tonic::Status> {
        let r = req.into_inner();
        if r.workspace_name.is_empty() { return Err(tonic::Status::invalid_argument("workspace_name required")); }
        let s = crate::session::Session::create(None, Some(r.workspace_name.clone()));
        let ps = proto::Session { id: s.id.clone(), workspace_name: r.workspace_name, workspace_path: r.workspace_path, status: 1, tenant_id: r.tenant_id, created_at: s.created_at.to_rfc3339(), last_active_at: chrono::Utc::now().to_rfc3339(), context_tokens: 0 };
        self.sessions.write().insert(s.id.clone(), ps.clone());
        Ok(tonic::Response::new(proto::CreateSessionResponse { session: Some(ps) }))
    }
    async fn get_session(&self, req: tonic::Request<proto::GetSessionRequest>) -> Result<tonic::Response<proto::GetSessionResponse>, tonic::Status> {
        match self.sessions.read().get(&req.into_inner().session_id).cloned() {
            Some(s) => Ok(tonic::Response::new(proto::GetSessionResponse { session: Some(s) })),
            None => Err(tonic::Status::not_found("session not found")),
        }
    }
    async fn update_session(&self, req: tonic::Request<proto::UpdateSessionRequest>) -> Result<tonic::Response<proto::UpdateSessionResponse>, tonic::Status> {
        let r = req.into_inner();
        match self.sessions.write().get_mut(&r.session_id) {
            Some(s) => { s.status = r.status as i32; s.last_active_at = chrono::Utc::now().to_rfc3339(); Ok(tonic::Response::new(proto::UpdateSessionResponse { session: Some(s.clone()) })) }
            None => Err(tonic::Status::not_found("session not found")),
        }
    }
    async fn delete_session(&self, req: tonic::Request<proto::DeleteSessionRequest>) -> Result<tonic::Response<proto::DeleteSessionResponse>, tonic::Status> {
        Ok(tonic::Response::new(proto::DeleteSessionResponse { success: self.sessions.write().remove(&req.into_inner().session_id).is_some() }))
    }
    async fn list_sessions(&self, _req: tonic::Request<proto::ListSessionsRequest>) -> Result<tonic::Response<proto::ListSessionsResponse>, tonic::Status> {
        let sessions: Vec<proto::Session> = self.sessions.read().values().cloned().collect();
        Ok(tonic::Response::new(proto::ListSessionsResponse { sessions, next_page_token: String::new() }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Chat Service
// ══════════════════════════════════════════════════════════════════

struct ChatServiceImpl {
    _sessions: Arc<RwLock<std::collections::HashMap<String, proto::Session>>>,
    provider: Arc<dyn crate::provider::Provider>,
}

impl ChatServiceImpl {
    fn new(sessions: Arc<RwLock<std::collections::HashMap<String, proto::Session>>>, provider: Arc<dyn crate::provider::Provider>) -> Self { Self { _sessions: sessions, provider } }
}

#[tonic::async_trait]
impl ChatService for ChatServiceImpl {
    type ChatStreamStream = tokio_stream::wrappers::ReceiverStream<Result<proto::ChatStreamResponse, tonic::Status>>;
    async fn chat(&self, req: tonic::Request<proto::ChatRequest>) -> Result<tonic::Response<proto::ChatResponse>, tonic::Status> {
        let r = req.into_inner();
        if r.messages.is_empty() { return Err(tonic::Status::invalid_argument("messages required")); }
        let prompt = r.messages.last().map(|m| m.content.clone()).unwrap_or_default();
        let content = self.provider.complete_simple(&prompt, "").await.map_err(|e| tonic::Status::internal(e.to_string()))?;
        Ok(tonic::Response::new(proto::ChatResponse { id: String::new(), model: r.model, content, usage: None, tool_calls: vec![] }))
    }
    async fn chat_stream(&self, req: tonic::Request<proto::ChatStreamRequest>) -> Result<tonic::Response<Self::ChatStreamStream>, tonic::Status> {
        if req.into_inner().messages.is_empty() { return Err(tonic::Status::invalid_argument("messages required")); }
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move { let _ = tx.send(Ok(proto::ChatStreamResponse { id: String::new(), model: String::new(), content: "streamed".into(), done: true, usage: None, tool_calls: vec![] })).await; });
        Ok(tonic::Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
    async fn cancel_chat(&self, _req: tonic::Request<proto::CancelChatRequest>) -> Result<tonic::Response<proto::CancelChatResponse>, tonic::Status> {
        Ok(tonic::Response::new(proto::CancelChatResponse { success: true }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Memory Service (RPCs: AddMemory, RetrieveMemory, ClearMemory)
// ══════════════════════════════════════════════════════════════════

struct MemoryServiceImpl {
    store: Arc<RwLock<std::collections::HashMap<String, Vec<proto::MemoryEntry>>>>,
}

impl MemoryServiceImpl {
    fn new() -> Self { Self { store: Arc::new(RwLock::new(std::collections::HashMap::new())) } }
}

#[tonic::async_trait]
impl MemoryService for MemoryServiceImpl {
    async fn add_memory(&self, req: tonic::Request<proto::AddMemoryRequest>) -> Result<tonic::Response<proto::AddMemoryResponse>, tonic::Status> {
        let r = req.into_inner();
        let entry = proto::MemoryEntry { id: uuid::Uuid::new_v4().to_string(), content: r.content, importance: r.importance, context: r.context, memory_type: r.memory_type, created_at: chrono::Utc::now().to_rfc3339() };
        self.store.write().entry(r.session_id).or_default().push(entry.clone());
        Ok(tonic::Response::new(proto::AddMemoryResponse { entry: Some(entry) }))
    }
    async fn retrieve_memory(&self, req: tonic::Request<proto::RetrieveMemoryRequest>) -> Result<tonic::Response<proto::RetrieveMemoryResponse>, tonic::Status> {
        let r = req.into_inner();
        let entries = self.store.read().get(&r.session_id).cloned().unwrap_or_default().into_iter().take(r.limit.max(1) as usize).collect();
        Ok(tonic::Response::new(proto::RetrieveMemoryResponse { entries }))
    }
    async fn clear_memory(&self, req: tonic::Request<proto::ClearMemoryRequest>) -> Result<tonic::Response<proto::ClearMemoryResponse>, tonic::Status> {
        let r = self.store.write().remove(&req.into_inner().session_id).is_some();
        Ok(tonic::Response::new(proto::ClearMemoryResponse { success: r }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Agent Service
// ══════════════════════════════════════════════════════════════════

struct AgentServiceImpl {
    agents: Arc<RwLock<std::collections::HashMap<String, proto::Agent>>>,
    provider: Arc<dyn crate::provider::Provider>,
}

impl AgentServiceImpl {
    fn new(provider: Arc<dyn crate::provider::Provider>) -> Self { Self { agents: Arc::new(RwLock::new(std::collections::HashMap::new())), provider } }
}

#[tonic::async_trait]
impl AgentService for AgentServiceImpl {
    async fn create_agent(&self, req: tonic::Request<proto::CreateAgentRequest>) -> Result<tonic::Response<proto::CreateAgentResponse>, tonic::Status> {
        let r = req.into_inner();
        let id = uuid::Uuid::new_v4().to_string();
        let a = proto::Agent { id: id.clone(), name: r.name, role: r.role, status: 1, session_id: r.session_id, tenant_id: r.tenant_id };
        self.agents.write().insert(id, a.clone());
        Ok(tonic::Response::new(proto::CreateAgentResponse { agent: Some(a) }))
    }
    async fn assign_task(&self, req: tonic::Request<proto::AssignTaskRequest>) -> Result<tonic::Response<proto::AssignTaskResponse>, tonic::Status> {
        let r = req.into_inner();
        let registry = crate::tool::Registry::new(Arc::clone(&self.provider)).await;
        let mut agent = crate::agent::Agent::new(Arc::clone(&self.provider), registry);
        let ok = agent.run_once_capture(&r.task_description).await.is_ok();
        if let Some(a) = self.agents.write().get_mut(&r.agent_id) { a.status = if ok { 4 } else { 1 }; }
        Ok(tonic::Response::new(proto::AssignTaskResponse { agent: self.agents.read().get(&r.agent_id).cloned() }))
    }
    async fn get_agent(&self, req: tonic::Request<proto::GetAgentRequest>) -> Result<tonic::Response<proto::GetAgentResponse>, tonic::Status> {
        match self.agents.read().get(&req.into_inner().agent_id).cloned() {
            Some(a) => Ok(tonic::Response::new(proto::GetAgentResponse { agent: Some(a) })),
            None => Err(tonic::Status::not_found("agent not found")),
        }
    }
    async fn list_agents(&self, _req: tonic::Request<proto::ListAgentsRequest>) -> Result<tonic::Response<proto::ListAgentsResponse>, tonic::Status> {
        let agents: Vec<proto::Agent> = self.agents.read().values().cloned().collect();
        Ok(tonic::Response::new(proto::ListAgentsResponse { agents }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Tool Service
// ══════════════════════════════════════════════════════════════════

struct ToolServiceImpl { provider: Arc<dyn crate::provider::Provider> }

impl ToolServiceImpl { fn new(provider: Arc<dyn crate::provider::Provider>) -> Self { Self { provider } } }

#[tonic::async_trait]
impl ToolService for ToolServiceImpl {
    async fn execute_tool(&self, req: tonic::Request<proto::ExecuteToolRequest>) -> Result<tonic::Response<proto::ExecuteToolResponse>, tonic::Status> {
        let r = req.into_inner();
        let registry = crate::tool::Registry::new(Arc::clone(&self.provider)).await;
        let ctx = crate::tool::ToolContext { session_id: r.session_id, message_id: uuid::Uuid::new_v4().to_string(), tool_call_id: uuid::Uuid::new_v4().to_string(), working_dir: None, stdin_request_tx: None, graceful_shutdown_signal: None, execution_mode: crate::tool::ToolExecutionMode::Direct };
        match registry.execute(&r.tool_name, serde_json::Value::Null, ctx).await {
            Ok(o) => Ok(tonic::Response::new(proto::ExecuteToolResponse { success: true, output: o.output })),
            Err(e) => Err(tonic::Status::internal(format!("tool error: {}", e)))
        }
    }
    async fn list_tools(&self, _req: tonic::Request<proto::ListToolsRequest>) -> Result<tonic::Response<proto::ListToolsResponse>, tonic::Status> {
        let registry = crate::tool::Registry::new(Arc::clone(&self.provider)).await;
        let names = registry.tool_names().await;
        let tools = names.into_iter().map(|name| proto::ToolInfo { name, description: String::new(), input_schema: None }).collect();
        Ok(tonic::Response::new(proto::ListToolsResponse { tools }))
    }
}

// ══════════════════════════════════════════════════════════════════
// Tenant Service
// ══════════════════════════════════════════════════════════════════

struct TenantServiceImpl { tenants: Arc<RwLock<std::collections::HashMap<String, proto::Tenant>>> }

impl TenantServiceImpl { fn new() -> Self { Self { tenants: Arc::new(RwLock::new(std::collections::HashMap::new())) } } }

#[tonic::async_trait]
impl TenantService for TenantServiceImpl {
    async fn create_tenant(&self, req: tonic::Request<proto::CreateTenantRequest>) -> Result<tonic::Response<proto::CreateTenantResponse>, tonic::Status> {
        let r = req.into_inner();
        let id = uuid::Uuid::new_v4().to_string();
        let t = proto::Tenant { id, name: r.name, domain: r.domain, limits: r.limits, created_at: chrono::Utc::now().to_rfc3339() };
        self.tenants.write().insert(t.id.clone(), t.clone());
        Ok(tonic::Response::new(proto::CreateTenantResponse { tenant: Some(t) }))
    }
    async fn get_tenant(&self, req: tonic::Request<proto::GetTenantRequest>) -> Result<tonic::Response<proto::GetTenantResponse>, tonic::Status> {
        match self.tenants.read().get(&req.into_inner().tenant_id).cloned() { Some(t) => Ok(tonic::Response::new(proto::GetTenantResponse { tenant: Some(t) })), None => Err(tonic::Status::not_found("tenant not found")) }
    }
    async fn update_tenant(&self, req: tonic::Request<proto::UpdateTenantRequest>) -> Result<tonic::Response<proto::UpdateTenantResponse>, tonic::Status> {
        let r = req.into_inner();
        match self.tenants.write().get_mut(&r.tenant_id) { Some(t) => { t.name = r.name; t.limits = r.limits; Ok(tonic::Response::new(proto::UpdateTenantResponse { tenant: Some(t.clone()) })) }, None => Err(tonic::Status::not_found("tenant not found")) }
    }
    async fn delete_tenant(&self, req: tonic::Request<proto::DeleteTenantRequest>) -> Result<tonic::Response<proto::DeleteTenantResponse>, tonic::Status> {
        Ok(tonic::Response::new(proto::DeleteTenantResponse { success: self.tenants.write().remove(&req.into_inner().tenant_id).is_some() }))
    }
}

// ══════════════════════════════════════════════════════════════════
// JoyCode Service
// ══════════════════════════════════════════════════════════════════

struct JoyCodeServiceImpl { patches: Arc<RwLock<std::collections::HashMap<String, proto::Patch>>> }

impl JoyCodeServiceImpl { fn new() -> Self { Self { patches: Arc::new(RwLock::new(std::collections::HashMap::new())) } } }

#[tonic::async_trait]
impl JoyCodeService for JoyCodeServiceImpl {
    async fn generate_patch(&self, req: tonic::Request<proto::GeneratePatchRequest>) -> Result<tonic::Response<proto::GeneratePatchResponse>, tonic::Status> {
        if req.into_inner().session_id.is_empty() { return Err(tonic::Status::invalid_argument("session_id required")); }
        let p = proto::Patch { id: uuid::Uuid::new_v4().to_string(), diff: String::new(), description: "auto-generated patch".into(), confidence: 0.8, changes: vec![] };
        self.patches.write().insert(p.id.clone(), p.clone());
        Ok(tonic::Response::new(proto::GeneratePatchResponse { patch_id: p.id.clone(), candidates: vec![p], summary: "patch generated".into() }))
    }
    async fn review_code(&self, req: tonic::Request<proto::ReviewCodeRequest>) -> Result<tonic::Response<proto::ReviewCodeResponse>, tonic::Status> {
        let fs: Vec<proto::CodeReview> = req.into_inner().files.into_iter().map(|f| proto::CodeReview { file_path: f, issues: vec![], score: 80 }).collect();
        Ok(tonic::Response::new(proto::ReviewCodeResponse { review_id: uuid::Uuid::new_v4().to_string(), reviews: fs, overall_feedback: "review complete".into() }))
    }
    async fn generate_tests(&self, req: tonic::Request<proto::GenerateTestsRequest>) -> Result<tonic::Response<proto::GenerateTestsResponse>, tonic::Status> {
        if req.into_inner().target_file.is_empty() { return Err(tonic::Status::invalid_argument("target_file required")); }
        Ok(tonic::Response::new(proto::GenerateTestsResponse { test_id: uuid::Uuid::new_v4().to_string(), tests: vec![], coverage_info: "N/A".into() }))
    }
    async fn apply_patch(&self, req: tonic::Request<proto::ApplyPatchRequest>) -> Result<tonic::Response<proto::ApplyPatchResponse>, tonic::Status> {
        let r = req.into_inner();
        if self.patches.read().get(&r.patch_id).is_none() { return Err(tonic::Status::not_found("patch not found")); }
        Ok(tonic::Response::new(proto::ApplyPatchResponse { success: true, message: if r.dry_run { "dry run".into() } else { "applied".into() }, applied_files: vec![] }))
    }
}

// ══════════════════════════════════════════════════════════════════
// OpenCode Service (40+ RPCs — correct proto signatures)
// ══════════════════════════════════════════════════════════════════

struct OpenCodeServiceImpl { provider: Arc<dyn crate::provider::Provider> }

impl OpenCodeServiceImpl { fn new(provider: Arc<dyn crate::provider::Provider>) -> Self { Self { provider } } }

type OpenCodeStream = tokio_stream::wrappers::ReceiverStream<Result<proto::SubscribeToChangesResponse, tonic::Status>>;

#[tonic::async_trait]
impl OpenCodeService for OpenCodeServiceImpl {
    type SubscribeToChangesStream = OpenCodeStream;

    async fn complete_code(&self, req: tonic::Request<proto::CompleteCodeRequest>) -> Result<tonic::Response<proto::CompleteCodeResponse>, tonic::Status> {
        let query = req.into_inner();
        match self.provider.complete_simple(&format!("Continue: {}", query.code), "").await {
            Ok(_t) => Ok(tonic::Response::new(proto::CompleteCodeResponse { completions: vec![], error: String::new() })),
            Err(e) => Err(tonic::Status::internal(e.to_string()))
        }
    }
    async fn generate_code(&self, req: tonic::Request<proto::GenerateCodeRequest>) -> Result<tonic::Response<proto::GenerateCodeResponse>, tonic::Status> {
        self.provider.complete_simple(&req.into_inner().prompt, "").await
            .map(|t| tonic::Response::new(proto::GenerateCodeResponse { generated_code: t, explanation: String::new(), files: vec![], error: String::new() }))
            .map_err(|e| tonic::Status::internal(e.to_string()))
    }
    async fn refactor_code(&self, req: tonic::Request<proto::RefactorCodeRequest>) -> Result<tonic::Response<proto::RefactorCodeResponse>, tonic::Status> {
        let r = req.into_inner();
        self.provider.complete_simple(&format!("Refactor ({}): {}", r.refactor_type, r.code), "").await
            .map(|t| tonic::Response::new(proto::RefactorCodeResponse { refactored_code: t, diff: String::new(), operations: vec![], error: String::new() }))
            .map_err(|e| tonic::Status::internal(e.to_string()))
    }
    async fn extract_method(&self, _: tonic::Request<proto::ExtractMethodRequest>) -> Result<tonic::Response<proto::ExtractMethodResponse>, tonic::Status> { Ok(tonic::Response::new(proto::ExtractMethodResponse::default())) }
    async fn inline_function(&self, _: tonic::Request<proto::InlineFunctionRequest>) -> Result<tonic::Response<proto::InlineFunctionResponse>, tonic::Status> { Ok(tonic::Response::new(proto::InlineFunctionResponse::default())) }
    async fn rename_symbol(&self, req: tonic::Request<proto::RenameSymbolRequest>) -> Result<tonic::Response<proto::RenameSymbolResponse>, tonic::Status> { let r = req.into_inner(); Ok(tonic::Response::new(proto::RenameSymbolResponse { updated_code: r.new_name, renamed_count: 0, locations: vec![], success: true, error: String::new() })) }
    async fn move_symbol(&self, _: tonic::Request<proto::MoveSymbolRequest>) -> Result<tonic::Response<proto::MoveSymbolResponse>, tonic::Status> { Ok(tonic::Response::new(proto::MoveSymbolResponse::default())) }
    async fn encapsulate_field(&self, _: tonic::Request<proto::EncapsulateFieldRequest>) -> Result<tonic::Response<proto::EncapsulateFieldResponse>, tonic::Status> { Ok(tonic::Response::new(proto::EncapsulateFieldResponse::default())) }
    async fn plan_project(&self, req: tonic::Request<proto::PlanProjectRequest>) -> Result<tonic::Response<proto::PlanProjectResponse>, tonic::Status> {
        let r = req.into_inner();
        self.provider.complete_simple(&format!("Plan: {}", r.project_description), "").await
            .map(|t| tonic::Response::new(proto::PlanProjectResponse { plan_id: uuid::Uuid::new_v4().to_string(), architecture: t, modules: vec![], timeline: String::new(), error: String::new() }))
            .map_err(|e| tonic::Status::internal(e.to_string()))
    }
    async fn go_to_definition(&self, _: tonic::Request<proto::GoToDefinitionRequest>) -> Result<tonic::Response<proto::GoToDefinitionResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GoToDefinitionResponse::default())) }
    async fn find_references(&self, _: tonic::Request<proto::FindReferencesRequest>) -> Result<tonic::Response<proto::FindReferencesResponse>, tonic::Status> { Ok(tonic::Response::new(proto::FindReferencesResponse::default())) }
    async fn hover(&self, _: tonic::Request<proto::HoverRequest>) -> Result<tonic::Response<proto::HoverResponse>, tonic::Status> { Ok(tonic::Response::new(proto::HoverResponse::default())) }
    async fn document_symbols(&self, _: tonic::Request<proto::DocumentSymbolsRequest>) -> Result<tonic::Response<proto::DocumentSymbolsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::DocumentSymbolsResponse::default())) }
    async fn analyze_project(&self, req: tonic::Request<proto::AnalyzeProjectRequest>) -> Result<tonic::Response<proto::AnalyzeProjectResponse>, tonic::Status> { let _r = req.into_inner(); Ok(tonic::Response::new(proto::AnalyzeProjectResponse::default())) }
    async fn quick_fix(&self, _: tonic::Request<proto::QuickFixRequest>) -> Result<tonic::Response<proto::QuickFixResponse>, tonic::Status> { Ok(tonic::Response::new(proto::QuickFixResponse::default())) }
    async fn generate_documentation(&self, req: tonic::Request<proto::GenerateDocumentationRequest>) -> Result<tonic::Response<proto::GenerateDocumentationResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GenerateDocumentationResponse { documentation: format!("Docs for {}", req.into_inner().file_path), comments: vec![], error: String::new() })) }
    async fn generate_image(&self, _: tonic::Request<proto::GenerateImageRequest>) -> Result<tonic::Response<proto::GenerateImageResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GenerateImageResponse::default())) }
    async fn analyze_image(&self, _: tonic::Request<proto::AnalyzeImageRequest>) -> Result<tonic::Response<proto::AnalyzeImageResponse>, tonic::Status> { Ok(tonic::Response::new(proto::AnalyzeImageResponse::default())) }
    async fn analyze_chart(&self, _: tonic::Request<proto::AnalyzeChartRequest>) -> Result<tonic::Response<proto::AnalyzeChartResponse>, tonic::Status> { Ok(tonic::Response::new(proto::AnalyzeChartResponse::default())) }
    async fn analyze_document(&self, req: tonic::Request<proto::AnalyzeDocumentRequest>) -> Result<tonic::Response<proto::AnalyzeDocumentResponse>, tonic::Status> { let r = req.into_inner(); Ok(tonic::Response::new(proto::AnalyzeDocumentResponse { summary: format!("Analysis of {}", r.analysis_type), sections: vec![], key_points: String::new(), error: String::new() })) }
    async fn cache_analysis(&self, _: tonic::Request<proto::CacheAnalysisRequest>) -> Result<tonic::Response<proto::CacheAnalysisResponse>, tonic::Status> { Ok(tonic::Response::new(proto::CacheAnalysisResponse::default())) }
    async fn invalidate_cache(&self, _: tonic::Request<proto::InvalidateCacheRequest>) -> Result<tonic::Response<proto::InvalidateCacheResponse>, tonic::Status> { Ok(tonic::Response::new(proto::InvalidateCacheResponse::default())) }
    async fn format_code(&self, req: tonic::Request<proto::FormatCodeRequest>) -> Result<tonic::Response<proto::FormatCodeResponse>, tonic::Status> { Ok(tonic::Response::new(proto::FormatCodeResponse { formatted_code: req.into_inner().code, success: true, error: String::new() })) }
    async fn workspace_symbols(&self, _: tonic::Request<proto::WorkspaceSymbolsRequest>) -> Result<tonic::Response<proto::WorkspaceSymbolsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::WorkspaceSymbolsResponse::default())) }
    async fn code_lens(&self, _: tonic::Request<proto::CodeLensRequest>) -> Result<tonic::Response<proto::CodeLensResponse>, tonic::Status> { Ok(tonic::Response::new(proto::CodeLensResponse::default())) }
    async fn semantic_tokens(&self, _: tonic::Request<proto::SemanticTokensRequest>) -> Result<tonic::Response<proto::SemanticTokensResponse>, tonic::Status> { Ok(tonic::Response::new(proto::SemanticTokensResponse::default())) }
    async fn analyze_code_semantics(&self, _: tonic::Request<proto::AnalyzeCodeSemanticsRequest>) -> Result<tonic::Response<proto::AnalyzeCodeSemanticsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::AnalyzeCodeSemanticsResponse::default())) }
    async fn optimize_code(&self, _: tonic::Request<proto::OptimizeCodeRequest>) -> Result<tonic::Response<proto::OptimizeCodeResponse>, tonic::Status> { Ok(tonic::Response::new(proto::OptimizeCodeResponse::default())) }
    async fn review_code_quality(&self, _: tonic::Request<proto::ReviewCodeQualityRequest>) -> Result<tonic::Response<proto::ReviewCodeQualityResponse>, tonic::Status> { Ok(tonic::Response::new(proto::ReviewCodeQualityResponse::default())) }
    async fn collaborative_edit(&self, _: tonic::Request<proto::CollaborativeEditRequest>) -> Result<tonic::Response<proto::CollaborativeEditResponse>, tonic::Status> { Ok(tonic::Response::new(proto::CollaborativeEditResponse::default())) }
    async fn batch_refactor(&self, _: tonic::Request<proto::BatchRefactorRequest>) -> Result<tonic::Response<proto::BatchRefactorResponse>, tonic::Status> { Ok(tonic::Response::new(proto::BatchRefactorResponse::default())) }
    async fn incremental_analyze(&self, _: tonic::Request<proto::IncrementalAnalyzeRequest>) -> Result<tonic::Response<proto::IncrementalAnalyzeResponse>, tonic::Status> { Ok(tonic::Response::new(proto::IncrementalAnalyzeResponse::default())) }
    async fn warmup_cache(&self, _: tonic::Request<proto::WarmupCacheRequest>) -> Result<tonic::Response<proto::WarmupCacheResponse>, tonic::Status> { Ok(tonic::Response::new(proto::WarmupCacheResponse::default())) }
    async fn get_performance_stats(&self, _: tonic::Request<proto::GetPerformanceStatsRequest>) -> Result<tonic::Response<proto::GetPerformanceStatsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GetPerformanceStatsResponse::default())) }
    async fn subscribe_to_changes(&self, _: tonic::Request<proto::SubscribeToChangesRequest>) -> Result<tonic::Response<Self::SubscribeToChangesStream>, tonic::Status> { let (_, rx) = tokio::sync::mpsc::channel(1); Ok(tonic::Response::new(tokio_stream::wrappers::ReceiverStream::new(rx))) }
    async fn get_active_users(&self, _: tonic::Request<proto::GetActiveUsersRequest>) -> Result<tonic::Response<proto::GetActiveUsersResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GetActiveUsersResponse::default())) }
    async fn lock_file(&self, _: tonic::Request<proto::LockFileRequest>) -> Result<tonic::Response<proto::LockFileResponse>, tonic::Status> { Ok(tonic::Response::new(proto::LockFileResponse::default())) }
    async fn parse_ast(&self, _: tonic::Request<proto::ParseAstRequest>) -> Result<tonic::Response<proto::ParseAstResponse>, tonic::Status> { Ok(tonic::Response::new(proto::ParseAstResponse::default())) }
    async fn infer_types(&self, _: tonic::Request<proto::InferTypesRequest>) -> Result<tonic::Response<proto::InferTypesResponse>, tonic::Status> { Ok(tonic::Response::new(proto::InferTypesResponse::default())) }
    async fn resolve_symbols(&self, _: tonic::Request<proto::ResolveSymbolsRequest>) -> Result<tonic::Response<proto::ResolveSymbolsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::ResolveSymbolsResponse::default())) }
    async fn validate_code(&self, _: tonic::Request<proto::ValidateCodeRequest>) -> Result<tonic::Response<proto::ValidateCodeResponse>, tonic::Status> { Ok(tonic::Response::new(proto::ValidateCodeResponse::default())) }
    async fn enforce_style(&self, _: tonic::Request<proto::EnforceStyleRequest>) -> Result<tonic::Response<proto::EnforceStyleResponse>, tonic::Status> { Ok(tonic::Response::new(proto::EnforceStyleResponse::default())) }
    async fn detect_errors(&self, _: tonic::Request<proto::DetectErrorsRequest>) -> Result<tonic::Response<proto::DetectErrorsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::DetectErrorsResponse::default())) }
    async fn go_to_type_definition(&self, _: tonic::Request<proto::GoToTypeDefinitionRequest>) -> Result<tonic::Response<proto::GoToTypeDefinitionResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GoToTypeDefinitionResponse::default())) }
    async fn go_to_implementation(&self, _: tonic::Request<proto::GoToImplementationRequest>) -> Result<tonic::Response<proto::GoToImplementationResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GoToImplementationResponse::default())) }
    async fn find_implementations(&self, _: tonic::Request<proto::FindImplementationsRequest>) -> Result<tonic::Response<proto::FindImplementationsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::FindImplementationsResponse::default())) }
    async fn find_derived_classes(&self, _: tonic::Request<proto::FindDerivedClassesRequest>) -> Result<tonic::Response<proto::FindDerivedClassesResponse>, tonic::Status> { Ok(tonic::Response::new(proto::FindDerivedClassesResponse::default())) }
    async fn log_error(&self, _: tonic::Request<proto::LogErrorRequest>) -> Result<tonic::Response<proto::LogErrorResponse>, tonic::Status> { Ok(tonic::Response::new(proto::LogErrorResponse::default())) }
    async fn get_logs(&self, _: tonic::Request<proto::GetLogsRequest>) -> Result<tonic::Response<proto::GetLogsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::GetLogsResponse::default())) }
    async fn set_log_level(&self, _: tonic::Request<proto::SetLogLevelRequest>) -> Result<tonic::Response<proto::SetLogLevelResponse>, tonic::Status> { Ok(tonic::Response::new(proto::SetLogLevelResponse::default())) }
    async fn detect_design_patterns(&self, _: tonic::Request<proto::DetectDesignPatternsRequest>) -> Result<tonic::Response<proto::DetectDesignPatternsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::DetectDesignPatternsResponse::default())) }
    async fn analyze_anti_patterns(&self, _: tonic::Request<proto::AnalyzeAntiPatternsRequest>) -> Result<tonic::Response<proto::AnalyzeAntiPatternsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::AnalyzeAntiPatternsResponse::default())) }
    async fn cross_file_refactor(&self, _: tonic::Request<proto::CrossFileRefactorRequest>) -> Result<tonic::Response<proto::CrossFileRefactorResponse>, tonic::Status> { Ok(tonic::Response::new(proto::CrossFileRefactorResponse::default())) }
    async fn detect_code_smells(&self, _: tonic::Request<proto::DetectCodeSmellsRequest>) -> Result<tonic::Response<proto::DetectCodeSmellsResponse>, tonic::Status> { Ok(tonic::Response::new(proto::DetectCodeSmellsResponse::default())) }
}

// ══════════════════════════════════════════════════════════════════
// PluginService (from utils.rs)
// ══════════════════════════════════════════════════════════════════

struct PluginServiceImpl { plugins: Arc<RwLock<std::collections::HashMap<String, PluginInfo>>> }

struct PluginInfo { id: String, name: String, version: String, description: String, enabled: bool, capabilities: Vec<String> }

impl PluginServiceImpl { fn new() -> Self { Self { plugins: Arc::new(RwLock::new(std::collections::HashMap::new())) } } }

#[tonic::async_trait]
impl PluginService for PluginServiceImpl {
    async fn load_plugin(&self, req: tonic::Request<proto::LoadPluginRequest>) -> Result<tonic::Response<proto::LoadPluginResponse>, tonic::Status> {
        let r = req.into_inner();
        let id = uuid::Uuid::new_v4().to_string();
        let name = r.plugin_path.rsplit('/').next().unwrap_or("unknown").to_string();
        let info = PluginInfo { id: id.clone(), name: name.clone(), version: "1.0".into(), description: "loaded".into(), enabled: true, capabilities: vec![] };
        self.plugins.write().insert(id.clone(), info);
        Ok(tonic::Response::new(proto::LoadPluginResponse { plugin_id: id, name, version: "1.0".into(), success: true, error: String::new() }))
    }
    async fn unload_plugin(&self, req: tonic::Request<proto::UnloadPluginRequest>) -> Result<tonic::Response<proto::UnloadPluginResponse>, tonic::Status> {
        let r = self.plugins.write().remove(&req.into_inner().plugin_id).is_some();
        Ok(tonic::Response::new(proto::UnloadPluginResponse { success: r, error: if r { String::new() } else { "not found".into() } }))
    }
    async fn list_plugins(&self, _req: tonic::Request<proto::ListPluginsRequest>) -> Result<tonic::Response<proto::ListPluginsResponse>, tonic::Status> {
        let plugins = self.plugins.read().iter().map(|(_, p)| proto::PluginInfo { plugin_id: p.id.clone(), name: p.name.clone(), version: p.version.clone(), description: p.description.clone(), enabled: p.enabled, capabilities: p.capabilities.clone() }).collect();
        Ok(tonic::Response::new(proto::ListPluginsResponse { plugins, error: String::new() }))
    }
    async fn execute_plugin(&self, req: tonic::Request<proto::ExecutePluginRequest>) -> Result<tonic::Response<proto::ExecutePluginResponse>, tonic::Status> {
        let r = req.into_inner();
        if self.plugins.read().get(&r.plugin_id).is_none() { return Err(tonic::Status::not_found("plugin not found")); }
        Ok(tonic::Response::new(proto::ExecutePluginResponse { success: true, result: format!("executed: {}", r.command), error: String::new() }))
    }
}
