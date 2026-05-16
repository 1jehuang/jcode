//! 企业版核心结构体 — 整合所有模块

use crate::{
    admin_api,
    auth::{AuthManager, User},
    config::EnterpriseConfig,
    cpu_inference::CpuInferenceEngine,
    db::DatabaseManager,
    discovery::NodeDiscoveryManager,
    distributed::DistributedInferenceScheduler,
    priority::{EnterprisePriority, PriorityRuleEngine, TaskType},
    usage::UsageManager,
    virtual_memory::VirtualMemoryManager,
};
use jcode_llm::{LlmProvider, LlmProviderFactory, OpenAiCompatibleProvider, config::LlmConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// 企业版服务器状态（所有模块共享）
pub struct EnterpriseServerState {
    /// 配置
    pub config: Arc<EnterpriseConfig>,
    /// 认证管理器
    pub auth_manager: Arc<RwLock<AuthManager>>,
    /// 用户存储
    pub users: Arc<RwLock<HashMap<String, User>>>,
    /// CPU 推理引擎
    pub cpu_engine: Option<Arc<CpuInferenceEngine>>,
    /// LLM Provider 映射 (model_name -> Provider)
    providers: Arc<RwLock<HashMap<String, Arc<dyn LlmProvider>>>>,
    /// 分布式推理调度器
    pub distributed_scheduler: Option<Arc<DistributedInferenceScheduler>>,
    /// 节点发现管理器
    pub discovery_manager: Arc<NodeDiscoveryManager>,
    /// 用量管理器
    pub usage_manager: Arc<RwLock<UsageManager>>,
    /// 优先级规则引擎
    pub priority_engine: PriorityRuleEngine,
    /// 虚拟内存管理器
    pub vm_manager: Option<Arc<VirtualMemoryManager>>,
    /// 数据库管理器
    pub db: Option<Arc<DatabaseManager>>,
    /// 启动时间
    pub started_at: chrono::DateTime<chrono::Utc>,
}

/// 企业版服务器 — 主入口
pub struct EnterpriseServer {
    config: Arc<EnterpriseConfig>,
    state: Arc<EnterpriseServerState>,
}

impl EnterpriseServer {
    /// 创建并启动企业版服务器
    pub async fn new(config_path: Option<String>) -> anyhow::Result<Self> {
        let config = if let Some(path) = config_path {
            EnterpriseConfig::from_file(path)?
        } else {
            EnterpriseConfig::load()
        };
        let config = Arc::new(config);

        // 初始化日志
        crate::enterprise::init_logging(&config.server.log_level, config.server.json_log);

        info!("🚀 CarpAI Enterprise Server v{} 启动中...", env!("CARGO_PKG_VERSION"));
        info!("📋 配置: API={}:{}, Admin={}:{}, Node={}:{}",
            config.server.bind, config.server.api_port,
            config.server.bind, config.server.admin_port,
            config.server.bind, config.server.node_port);

        // 初始化数据库
        let db = match DatabaseManager::new(&config.database).await {
            Ok(db) => {
                info!("✅ 数据库已连接");
                Some(Arc::new(db))
            }
            Err(e) => {
                warn!("⚠️ 数据库连接失败（将使用内存存储）: {}", e);
                None
            }
        };

        // 初始化认证管理器
        let jwt_secret = std::env::var(&config.auth.jwt_secret_env)
            .unwrap_or_else(|_| {
                warn!("JWT secret 未设置，使用默认值（生产环境请设置环境变量 {}）", config.auth.jwt_secret_env);
                "default-jwt-secret-do-not-use-in-production".into()
            });
        let auth_manager = Arc::new(RwLock::new(AuthManager::new(
            jwt_secret,
            config.auth.jwt_expiry_hours,
        )));

        // 初始化节点发现管理器
        let discovery_manager = Arc::new(NodeDiscoveryManager::new(config.clone()));

        // 初始化用量管理器
        let usage_manager = Arc::new(RwLock::new(UsageManager::new()));

        // 初始化优先级规则引擎
        let priority_engine = PriorityRuleEngine::default();

        // 初始化分布式推理调度器（默认启用）
        let distributed_scheduler = Some(Arc::new(
            DistributedInferenceScheduler::new(config.clone())
        ));

        // 初始化虚拟内存管理器
        let vm_manager = if config.scheduling.enable_virtual_memory {
            Some(Arc::new(VirtualMemoryManager::new(
                config.virtual_memory.clone()
            )))
        } else {
            None
        };

        let state = Arc::new(EnterpriseServerState {
            config: config.clone(),
            auth_manager,
            users: Arc::new(RwLock::new(HashMap::new())),
            cpu_engine: None, // 按需启动
            providers: Arc::new(RwLock::new(HashMap::new())),
            distributed_scheduler,
            discovery_manager: discovery_manager.clone(),
            usage_manager,
            priority_engine,
            vm_manager,
            db,
            started_at: chrono::Utc::now(),
        });

        let server = Self {
            config,
            state: state.clone(),
        };

        // 连接已注册的固定节点
        info!("🏗️ 注册本地节点（内存: {}GB, CPU: {}核）",
            crate::cpu_inference::CpuMemoryStatus::map_or(0.0, |s| s.total_gb),
            num_cpus::get_physical()
        );

        // 注册本机作为第一个节点
        if let Some(scheduler) = &state.distributed_scheduler {
            let total_mem = sys_info::mem_info()
                .map(|m| m.total as f64 / 1024.0 / 1024.0)
                .unwrap_or(16.0);
            let cpu_cores = num_cpus::get_physical() as u32;

            let _ = scheduler.register_node(
                "本机服务器",
                total_mem,
                cpu_cores,
                false,
                0.0,
            ).await;
        }

        info!("✅ CarpAI Enterprise Server 初始化完成");
        info!("📌 支持的模型:");
        for model in &server.config.models.supported_models {
            info!("   - {} (需要 {} GB 内存)", model.display_name, model.min_memory_gb);
        }

        Ok(server)
    }

    /// 启动所有服务
    pub async fn serve(&self) -> anyhow::Result<()> {
        let config = self.config.clone();
        let state = self.state.clone();

        // 1. 启动 API 和 Admin 服务
        let api_router = admin_api::create_openai_router()
            .merge(admin_api::create_admin_router())
            .layer(axum::middleware::from_fn(admin_api::auth_middleware::api_key_middleware))
            .with_state(state.clone());

        let api_addr = format!("{}:{}", config.server.bind, config.server.api_port)
            .parse::<std::net::SocketAddr>()?;
        let admin_addr = format!("{}:{}", config.server.bind, config.server.admin_port)
            .parse::<std::net::SocketAddr>()?;

        // 2. 启动心跳检测循环
        let discovery = state.discovery_manager.clone();
        tokio::spawn(async move {
            discovery.heartbeat_check_loop().await;
        });

        // 3. 按需加载预热模型
        if !config.models.warm_up_models.is_empty() {
            info!("🔥 预热模型: {:?}", config.models.warm_up_models);
            let state = state.clone();
            tokio::spawn(async move {
                for model_name in &state.config.models.warm_up_models {
                    if let Some(model_entry) = state.config.models.supported_models
                        .iter()
                        .find(|m| &m.name == model_name)
                    {
                        info!("加载模型: {}", model_name);
                        // 创建 LLM Provider
                        let provider = LlmProviderFactory::local_llamacpp(
                            model_entry.name.clone(),
                            18000, // 默认端口
                        );
                        state.providers.write().await.insert(
                            model_entry.name.clone(),
                            provider,
                        );
                    }
                }
                info!("✅ 模型预热完成");
            });
        }

        // 4. 启动 HTTP 服务（API + Admin 共享同一端口简化部署）
        info!("🌐 Admin API: http://{}", admin_addr);
        info!("🌐 OpenAI API: http://{}", api_addr);

        // API 端口
        let state_clone = state.clone();
        let api_listener = tokio::net::TcpListener::bind(api_addr).await?;
        tokio::spawn(async move {
            axum::serve(api_listener, api_router)
                .with_graceful_shutdown(shutdown_signal())
                .await
                .unwrap_or_else(|e| error!("API server error: {}", e));
        });

        // Admin 端口（可选，也可合并到 API 端口）
        let admin_router = admin_api::create_admin_router()
            .with_state(state.clone());
        let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
        tokio::spawn(async move {
            axum::serve(admin_listener, admin_router)
                .with_graceful_shutdown(shutdown_signal())
                .await
                .unwrap_or_else(|e| error!("Admin server error: {}", e));
        });

        info!("✅ CarpAI Enterprise Server 已启动，按 Ctrl+C 停止");

        // 保持主进程运行
        tokio::signal::ctrl_c().await?;
        info!("正在停止服务...");

        Ok(())
    }

    /// 获取服务器状态引用
    pub fn state(&self) -> Arc<EnterpriseServerState> {
        self.state.clone()
    }
}

// EnterpriseServerState 的方法
impl EnterpriseServerState {
    /// 查找模型对应的 Provider
    pub async fn find_provider(&self, model_name: &str) -> Option<Arc<dyn LlmProvider>> {
        let providers = self.providers.read().await;
        providers.get(model_name).cloned()
    }

    /// 获取可用模型列表
    pub async fn list_available_models(&self) -> Vec<serde_json::Value> {
        self.config.models.supported_models.iter().map(|m| {
            serde_json::json!({
                "id": m.name,
                "object": "model",
                "created": 0,
                "owned_by": "carpai",
                "permission": [],
            })
        }).collect()
    }

    /// 健康检查
    pub async fn health_check(&self) -> bool {
        true
    }

    /// 获取当前节点数
    pub async fn node_count(&self) -> usize {
        self.discovery_manager.get_online_nodes().await.len()
    }
}

/// 优雅关机信号
async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.expect("无法安装 Ctrl+C 信号处理器");
    info!("收到 Ctrl+C 信号，正在优雅关闭...");
}

/// 初始化日志系统
pub fn init_logging(level: &str, json: bool) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::builder()
        .parse(format!("carpai_enterprise_server={},jcode_llm={},{}",
            level, level,
            if level == &"debug" { "debug" } else { "warn" }
        ))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .with_thread_ids(true)
            .init();
    }
}
