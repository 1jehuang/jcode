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
    quota::{UsageTracker, SharedUsageTracker, QuotaPolicy, UsageTier},
};
use jcode_llm::{LlmProvider, LlmProviderFactory, OpenAiCompatibleProvider, config::LlmConfig};
use jcode_unified_scheduler::{UnifiedScheduler, SchedulerConfig, NodeHardwareInfo};
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
    /// 配额追踪器
    pub quota_tracker: SharedUsageTracker,
    /// Ruflo-Parallax 统一调度器
    pub scheduler: Arc<UnifiedScheduler>,
    /// 优先级规则引擎
    pub priority_engine: PriorityRuleEngine,
    /// 虚拟内存管理器
    pub vm_manager: Option<Arc<VirtualMemoryManager>>,
    /// 数据库管理器
    pub db: Option<Arc<DatabaseManager>>,
    /// 代码库索引引擎 (Phase 3.1)
    pub codebase_engine: Arc<tokio::sync::Mutex<Option<carpai_codebase::CodebaseEngine>>>,
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

        // 初始化配额追踪器
        let quota_tracker = Arc::new(RwLock::new(UsageTracker::new()));

        // 初始化 Ruflo-Parallax 统一调度器
        let scheduler_config = SchedulerConfig {
            min_bootstrap_nodes: 1,
            enable_goap: true,
            adaptive_scheduling: true,
            ..SchedulerConfig::default()
        };
        let scheduler = Arc::new(UnifiedScheduler::new(scheduler_config).await?);
        info!("✅ Ruflo-Parallax 统一调度器已初始化");

        // 初始化优先级规则引擎
        let priority_engine = PriorityRuleEngine::default();

        // 初始化 CPU 推理引擎
        let cpu_engine = Arc::new(CpuInferenceEngine::new(config.clone()));
        info!("✅ CPU 推理引擎已就绪");

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
            cpu_engine: Some(cpu_engine.clone()),
            providers: Arc::new(RwLock::new(HashMap::new())),
            scheduler: scheduler.clone(),
            distributed_scheduler,
            discovery_manager: discovery_manager.clone(),
            usage_manager,
            quota_tracker,
            priority_engine,
            vm_manager,
            db,
            codebase_engine: Arc::new(tokio::sync::Mutex::new(None)),
            started_at: chrono::Utc::now(),
        });

        // 启动后台代码库索引进程 (Phase 3.1)
        if config.codebase.enable_indexing {
            if let Some(workspace_path) = &config.codebase.workspace_path {
                let engine_ref = state.codebase_engine.clone();
                let workspace_clone = workspace_path.clone();
                tokio::spawn(async move {
                    match carpai_codebase::CodebaseEngine::new(
                        std::path::PathBuf::from("./carpai_index")
                    ) {
                        Ok(mut engine) => {
                            info!("🚀 启动后台代码库索引...");
                            if let Err(e) = engine.index_workspace(&workspace_clone).await {
                                tracing::error!("代码库索引失败: {:?}", e);
                            } else {
                                info!("✅ 代码库索引完成");
                                let mut lock = engine_ref.lock().await;
                                *lock = Some(engine);
                            }
                        }
                        Err(e) => tracing::error!("创建代码库引擎失败: {:?}", e),
                    }
                });
            }
        }

        let server = Self {
            config,
            state: state.clone(),
        };

        // 注册本机到 Ruflo-Parallax 统一调度器
        let total_mem = sys_info::mem_info()
            .map(|m| m.total as f64 / 1024.0 / 1024.0)
            .unwrap_or(16.0);
        let cpu_cores = num_cpus::get_physical() as u32;
        let node_hw = jcode_unified_scheduler::NodeHardwareInfo {
            node_id: uuid::Uuid::new_v4(),
            num_gpus: 0,
            gpu_name: "CPU-only".into(),
            memory_gb: total_mem,
            cpu_cores,
            tflops_fp16: 0.0,
            tflops_fp32: 0.0,
            gpu_bandwidth_gbps: 0.0,
            pcie_bandwidth_gbps: 0.0,
            has_gpu: false,
            vram_gb: 0.0,
            cpu_arch: std::env::consts::ARCH.to_string(),
        };
        let _ = state.scheduler.register_node(node_hw).await;
        info!("✅ 本机已注册到调度器（内存: {:.1}GB, CPU: {}核）", total_mem, cpu_cores);

        // 也注册到旧分布式调度器（保持兼容）
        if let Some(sched) = &state.distributed_scheduler {
            let _ = sched.register_node("本机服务器", total_mem, cpu_cores, false, 0.0).await;
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

        // 1. 创建 Prometheus 指标收集器
        let metrics_collector = Arc::new(crate::metrics::MetricsCollector::new()?);

        // 2. 启动 API 和 Admin 服务
        let api_router = admin_api::create_openai_router()
            .with_state(state.clone());

        let admin_router = admin_api::create_admin_router(state.clone());

        // 合并路由并添加认证中间件
        let app = api_router
            .merge(admin_router)
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                admin_api::auth_middleware,
            ));

        // 添加 /metrics 端点
        let metrics_router = crate::metrics::create_metrics_router(metrics_collector.clone());
        let app = app.merge(metrics_router);

        let api_addr = format!("{}:{}", config.server.bind, config.server.api_port)
            .parse::<std::net::SocketAddr>()?;
        let admin_addr = format!("{}:{}", config.server.bind, config.server.admin_port)
            .parse::<std::net::SocketAddr>()?;

        // 3. 启动 Ruflo-Parallax 调度循环
        let scheduler = state.scheduler.clone();
        tokio::spawn(async move {
            if let Err(e) = scheduler.run().await {
                error!("[UnifiedScheduler] 调度循环异常退出: {:?}", e);
            }
        });
        info!("✅ Ruflo-Parallax 调度循环已启动");

        // 4. 启动心跳检测循环
        let discovery = state.discovery_manager.clone();
        tokio::spawn(async move {
            discovery.heartbeat_check_loop().await;
        });

        // 5. 按需加载预热模型 (CPU 推理引擎)
        if !config.models.warm_up_models.is_empty() {
            info!("🔥 预热模型: {:?}", config.models.warm_up_models);
            let state = state.clone();
            tokio::spawn(async move {
                for model_name in &state.config.models.warm_up_models {
                    if let Some(model_entry) = state.config.models.supported_models
                        .iter()
                        .find(|m| &m.name == model_name)
                    {
                        info!("🔥 启动 CPU 推理引擎: {}", model_name);

                        // 如果启用了虚拟内存管理，先为KV Cache创建mmap区域
                        if let Some(ref vm_mgr) = state.vm_manager {
                            // 根据模型参数量估算KV Cache大小
                            // 7B模型约需8GB, 14B约需16GB, 72B约需80GB
                            let kv_cache_mb = match model_entry.name.to_lowercase().as_str() {
                                name if name.contains("72b") || name.contains("70b") => 80_000,
                                name if name.contains("32b") || name.contains("35b") => 40_000,
                                name if name.contains("14b") || name.contains("13b") => 16_000,
                                name if name.contains("7b") || name.contains("8b") => 8_000,
                                name if name.contains("3b") || name.contains("1.5b") => 4_000,
                                _ => 8_000, // 默认8GB
                            };

                            match vm_mgr.create_kv_cache_mmap(model_name, kv_cache_mb).await {
                                Ok(region) => {
                                    info!(
                                        "✅ [VirtualMemory] KV Cache mmap已创建: model={}, size={}MB",
                                        model_name, kv_cache_mb
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "⚠️ [VirtualMemory] KV Cache mmap创建失败，将使用普通内存: {:?}",
                                        e
                                    );
                                }
                            }
                        }

                        if let Some(ref engine) = state.cpu_engine {
                            match engine.start_model(model_entry).await {
                                Ok(instance) => {
                                    info!("✅ 模型 {} 已启动 (port={})", model_name, instance.port);
                                    let provider = LlmProviderFactory::local_llamacpp(
                                        model_entry.name.clone(), instance.port,
                                    );
                                    state.providers.write().await.insert(
                                        model_entry.name.clone(), provider,
                                    );
                                }
                                Err(e) => warn!("⚠️ 模型 {} 启动失败: {}", model_name, e),
                            }
                        }
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

    /// 评估本地资源是否足以运行指定模型
    ///
    /// 返回 (is_local_sufficient, reason)
    /// - is_local_sufficient: true表示本地资源充足，false需要分布式推理
    /// - reason: 决策原因说明
    pub async fn evaluate_local_capacity(&self, model_name: &str) -> (bool, String) {
        // 1. 获取模型需求
        let model_entry = self.config.models.supported_models
            .iter()
            .find(|m| m.name == model_name);

        let required_memory_gb = match model_entry {
            Some(entry) => entry.min_memory_gb,
            None => {
                // 未知模型，使用默认估算
                if model_name.contains("72b") || model_name.contains("70b") { 80.0 }
                else if model_name.contains("32b") || model_name.contains("35b") { 40.0 }
                else if model_name.contains("14b") || model_name.contains("13b") { 16.0 }
                else if model_name.contains("7b") || model_name.contains("8b") { 8.0 }
                else { 16.0 } // 默认
            }
        };

        // 2. 检查本地可用内存
        let mem_info = sys_info::mem_info();
        let available_memory_gb = match mem_info {
            Ok(info) => info.avail as f64 / 1024.0 / 1024.0, // KB to GB
            Err(_) => 0.0,
        };

        // 3. 检查是否有预热的本地provider
        let has_local_provider = self.find_provider(model_name).await.is_some();

        // 4. 检查虚拟内存支持
        let vm_enabled = self.vm_manager.is_some();
        let vm_stats = if let Some(ref vm_mgr) = self.vm_manager {
            let usage = vm_mgr.get_memory_usage().await;
            usage.swap.available_gb + usage.physical.available_gb
        } else {
            0.0
        };

        // 5. 决策逻辑
        let total_available_gb = available_memory_gb + vm_stats;
        let memory_threshold = 1.2; // 需要20%余量

        if has_local_provider && total_available_gb >= required_memory_gb * memory_threshold {
            (true, format!(
                "本地资源充足: 可用{:.1}GB >= 需要{:.1}GB (含VM)",
                total_available_gb, required_memory_gb
            ))
        } else if !has_local_provider && required_memory_gb > 20.0 {
            // 大模型且未预热，优先使用分布式
            (false, format!(
                "大模型未预热: 需要{:.1}GB，建议分布式推理",
                required_memory_gb
            ))
        } else if total_available_gb < required_memory_gb {
            (false, format!(
                "本地内存不足: 可用{:.1}GB < 需要{:.1}GB",
                total_available_gb, required_memory_gb
            ))
        } else {
            // 资源紧张但勉强可用
            (true, format!(
                "本地资源紧张: 可用{:.1}GB ≈ 需要{:.1}GB，可能影响并发",
                total_available_gb, required_memory_gb
            ))
        }
    }

    /// 获取集群资源概览
    pub async fn get_cluster_resource_summary(&self) -> serde_json::Value {
        let local_mem = sys_info::mem_info().ok();
        let cluster_summary = self.scheduler.get_cluster_summary().await;

        serde_json::json!({
            "local": {
                "available_memory_gb": local_mem.as_ref().map(|m| m.avail as f64 / 1024.0 / 1024.0).unwrap_or(0.0),
                "total_memory_gb": local_mem.as_ref().map(|m| m.total as f64 / 1024.0 / 1024.0).unwrap_or(0.0),
                "cpu_cores": num_cpus::get_physical(),
                "vm_enabled": self.vm_manager.is_some(),
            },
            "cluster": {
                "node_count": cluster_summary.node_count,
                "total_vram_gb": cluster_summary.total_vram_gb,
                "total_tflops": cluster_summary.total_tflops,
                "avg_latency_ms": cluster_summary.avg_latency_ms,
            },
            "scheduler_metrics": {
                "tasks_submitted": {
                    "value": cluster_summary.node_count,
                    "description": "当前活跃节点数"
                }
            }
        })
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
