//! 企业版服务器配置
//!
//! 从文件或环境变量加载，支持热重载。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 企业版配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseConfig {
    /// 服务端口
    pub server: ServerConfig,
    /// 数据库配置
    pub database: DatabaseConfig,
    /// 模型配置
    pub models: ModelsConfig,
    /// 节点调度配置
    pub scheduling: SchedulingConfig,
    /// 权限认证配置
    pub auth: AuthConfig,
    /// 用量限制配置
    pub limits: UsageLimitsConfig,
    /// 审计日志配置
    pub audit: AuditConfig,
    /// 虚拟内存配置
    pub virtual_memory: VirtualMemoryConfig,
    /// 代码库索引配置
    pub codebase: CodebaseConfig,
}

/// 代码库索引配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodebaseConfig {
    /// 是否启用自动索引
    pub enable_indexing: bool,
    /// 工作区路径
    pub workspace_path: Option<String>,
    /// 索引更新间隔（秒）
    pub refresh_interval_secs: u64,
}

impl Default for CodebaseConfig {
    fn default() -> Self {
        Self {
            enable_indexing: true,
            workspace_path: None,
            refresh_interval_secs: 300, // 5分钟
        }
    }
}

/// 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// 绑定地址
    pub bind: String,
    /// OpenAI 兼容 API 端口
    pub api_port: u16,
    /// 管理后台端口
    pub admin_port: u16,
    /// 节点注册端口
    pub node_port: u16,
    /// 日志级别 (trace/debug/info/warn/error)
    pub log_level: String,
    /// 启用 JSON 日志
    pub json_log: bool,
    /// 最大请求体大小 (MB)
    pub max_body_mb: usize,
    /// 请求超时秒数
    pub request_timeout_secs: u64,
}

/// 数据库配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// 数据库 URL (sqlite:// 或 postgres://)
    pub url: String,
    /// 最大连接池大小
    pub max_connections: u32,
    /// 连接超时秒数
    pub connect_timeout_secs: u64,
    /// 自动运行迁移
    pub auto_migrate: bool,
}

/// 模型配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// 支持的模型列表
    pub supported_models: Vec<ModelEntry>,
    /// 默认模型名
    pub default_model: String,
    /// 模型权重下载路径
    pub model_cache_dir: PathBuf,
    /// 模型后台预热 (启动时自动加载常用模型)
    pub warm_up_models: Vec<String>,
}

/// 单个模型配置项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// 模型标识名 (如 "qwen3-72b-int4")
    pub name: String,
    /// 显示名称
    pub display_name: String,
    /// 模型类型
    pub model_type: ModelType,
    /// 是否量化
    pub quantized: bool,
    /// 量化级别 (如 "Q4_K_M", "INT4", "Q8_0")
    pub quantization: String,
    /// GGUF 模型文件路径
    pub gguf_path: Option<PathBuf>,
    /// 期望的最小内存 (GB) — 用于调度决策
    pub min_memory_gb: f64,
    /// 是否支持分布式
    pub supports_distributed: bool,
    /// 模型层数 (用于 Parallax 层分配)
    pub num_layers: u32,
    /// 上下文窗口大小
    pub context_window: usize,
    /// 是否支持流式输出
    pub supports_streaming: bool,
    /// 是否支持函数调用
    pub supports_function_calling: bool,
    /// 提供方: "llamacpp" | "openai-compatible" | "deepseek-api"
    pub provider: String,
    /// API 基础 URL (对于远端模型)
    pub api_base_url: Option<String>,
    /// API key 环境变量名
    pub api_key_env: Option<String>,
}

/// 模型类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelType {
    /// 通用对话模型
    Chat,
    /// 代码专用模型
    Code,
    /// 视觉多模态模型
    Vision,
    /// 嵌入/向量模型
    Embedding,
}

/// 调度配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulingConfig {
    /// 最大并发任务数
    pub max_concurrent_tasks: usize,
    /// 队列最大长度
    pub max_queue_size: usize,
    /// 心跳超时 (秒)
    pub heartbeat_timeout_secs: u64,
    /// 最小引导节点数
    pub min_bootstrap_nodes: usize,
    /// 分配策略: "greedy" | "dp"
    pub allocation_strategy: String,
    /// 路由策略: "dp" | "random" | "round_robin"
    pub routing_strategy: String,
    /// 启用 GOAP 任务规划
    pub enable_goap: bool,
    /// 启用动态节点调度
    pub enable_dynamic_nodes: bool,
    /// 启用虚拟内存推理
    pub enable_virtual_memory: bool,
    /// 虚拟内存重试阈值 (文件存储超过此大小则降级)
    pub vm_large_file_threshold_mb: u64,
}

/// 权限认证配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// JWT 密钥 (环境变量名)
    pub jwt_secret_env: String,
    /// JWT 过期时间 (小时)
    pub jwt_expiry_hours: u32,
    /// API Key 的哈希算法 ("sha256" | "sha512")
    pub api_key_hash: String,
    /// 允许注册 (公开注册)
    pub allow_signup: bool,
    /// 需要邮箱验证
    pub require_email_verification: bool,
    /// 会话超时 (分钟)
    pub session_timeout_minutes: u32,
}

/// 用量限制配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimitsConfig {
    /// 免费组织最大用户数
    pub free_org_max_users: u32,
    /// 免费组织每日 Token 上限
    pub free_org_daily_token_limit: u64,
    /// 免费组织并发请求上限
    pub free_org_concurrent_limit: u32,
    /// 企业组织最大用户数 (0 = 不限)
    pub enterprise_org_max_users: u32,
    /// 企业组织每日 Token 上限 (0 = 不限)
    pub enterprise_org_daily_token_limit: u64,
    /// 企业组织并发请求上限 (0 = 不限)
    pub enterprise_org_concurrent_limit: u32,
    /// 单请求最大 Token
    pub max_tokens_per_request: u32,
    /// 速率限制 (每分钟请求数)
    pub rate_limit_per_minute: u32,
}

/// 审计日志配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// 启用审计日志
    pub enabled: bool,
    /// 日志保留天数
    pub retention_days: u32,
    /// 记录请求体 (可能较大)
    pub log_request_body: bool,
    /// 记录响应体
    pub log_response_body: bool,
}

/// 虚拟内存配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualMemoryConfig {
    /// 启用虚拟内存推理
    pub enabled: bool,
    /// KV Cache 映射文件目录
    pub mmap_dir: PathBuf,
    /// 最大映射文件大小 (GB)
    pub max_mmap_file_gb: u64,
    /// 预分配大小 (MB)
    pub preallocate_mb: u64,
    /// 虚拟内存交换延迟容忍度 (ms)
    pub swap_latency_tolerance_ms: u64,
}

impl Default for EnterpriseConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                bind: "0.0.0.0".into(),
                api_port: 8000,
                admin_port: 8001,
                node_port: 8002,
                log_level: "info".into(),
                json_log: false,
                max_body_mb: 32,
                request_timeout_secs: 300,
            },
            database: DatabaseConfig {
                url: "sqlite://./carpai_enterprise.db?mode=rwc".into(),
                max_connections: 10,
                connect_timeout_secs: 30,
                auto_migrate: true,
            },
            models: ModelsConfig {
                supported_models: vec![
                    ModelEntry {
                        name: "qwen3-72b-int4".into(),
                        display_name: "通义千问 3.5 72B (4bit)".into(),
                        model_type: ModelType::Chat,
                        quantized: true,
                        quantization: "Q4_K_M".into(),
                        gguf_path: Some(PathBuf::from("./models/qwen3-72b-Q4_K_M.gguf")),
                        min_memory_gb: 36.0,
                        supports_distributed: true,
                        num_layers: 80,
                        context_window: 32768,
                        supports_streaming: true,
                        supports_function_calling: true,
                        provider: "llamacpp".into(),
                        api_base_url: None,
                        api_key_env: None,
                    },
                    ModelEntry {
                        name: "qwq-32b-int4".into(),
                        display_name: "QwQ 32B (4bit)".into(),
                        model_type: ModelType::Chat,
                        quantized: true,
                        quantization: "Q4_K_M".into(),
                        gguf_path: Some(PathBuf::from("./models/qwq-32b-Q4_K_M.gguf")),
                        min_memory_gb: 18.0,
                        supports_distributed: true,
                        num_layers: 40,
                        context_window: 32768,
                        supports_streaming: true,
                        supports_function_calling: true,
                        provider: "llamacpp".into(),
                        api_base_url: None,
                        api_key_env: None,
                    },
                    ModelEntry {
                        name: "deepseek-r1-32b-int4".into(),
                        display_name: "DeepSeek R1 32B (4bit)".into(),
                        model_type: ModelType::Chat,
                        quantized: true,
                        quantization: "Q4_K_M".into(),
                        gguf_path: Some(PathBuf::from("./models/deepseek-r1-32b-Q4_K_M.gguf")),
                        min_memory_gb: 18.0,
                        supports_distributed: true,
                        num_layers: 40,
                        context_window: 16384,
                        supports_streaming: true,
                        supports_function_calling: true,
                        provider: "llamacpp".into(),
                        api_base_url: None,
                        api_key_env: None,
                    },
                    ModelEntry {
                        name: "glm5-9b-int4".into(),
                        display_name: "GLM 5 9B (4bit)".into(),
                        model_type: ModelType::Chat,
                        quantized: true,
                        quantization: "Q4_K_M".into(),
                        gguf_path: Some(PathBuf::from("./models/glm5-9b-Q4_K_M.gguf")),
                        min_memory_gb: 6.0,
                        supports_distributed: false,
                        num_layers: 28,
                        context_window: 8192,
                        supports_streaming: true,
                        supports_function_calling: true,
                        provider: "llamacpp".into(),
                        api_base_url: None,
                        api_key_env: None,
                    },
                ],
                default_model: "qwen3-72b-int4".into(),
                model_cache_dir: PathBuf::from("./models"),
                warm_up_models: vec!["qwen3-72b-int4".into()],
            },
            scheduling: SchedulingConfig {
                max_concurrent_tasks: 16,
                max_queue_size: 1024,
                heartbeat_timeout_secs: 30,
                min_bootstrap_nodes: 1,
                allocation_strategy: "dp".into(),
                routing_strategy: "dp".into(),
                enable_goap: true,
                enable_dynamic_nodes: true,
                enable_virtual_memory: true,
                vm_large_file_threshold_mb: 4096,
            },
            auth: AuthConfig {
                jwt_secret_env: "CARPAI_JWT_SECRET".into(),
                jwt_expiry_hours: 24,
                api_key_hash: "sha256".into(),
                allow_signup: true,
                require_email_verification: false,
                session_timeout_minutes: 120,
            },
            limits: UsageLimitsConfig {
                free_org_max_users: 5,
                free_org_daily_token_limit: 100000,
                free_org_concurrent_limit: 2,
                enterprise_org_max_users: 100,
                enterprise_org_daily_token_limit: 0,
                enterprise_org_concurrent_limit: 0,
                max_tokens_per_request: 16384,
                rate_limit_per_minute: 60,
            },
            audit: AuditConfig {
                enabled: true,
                retention_days: 90,
                log_request_body: true,
                log_response_body: false,
            },
            virtual_memory: VirtualMemoryConfig {
                enabled: true,
                mmap_dir: PathBuf::from("./kv_cache_mmap"),
                max_mmap_file_gb: 64,
                preallocate_mb: 1024,
                swap_latency_tolerance_ms: 100,
            },
        }
    }
}

impl EnterpriseConfig {
    /// 从文件加载配置
    pub fn from_file(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path: PathBuf = path.into();
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("无法读取配置文件 {:?}: {}", path, e))?;

        let config: EnterpriseConfig = match path.extension().and_then(|s| s.to_str()) {
            Some("yaml" | "yml") => serde_yaml::from_str(&content)?,
            Some("toml") => toml::from_str(&content)?,
            Some("json") => serde_json::from_str(&content)?,
            _ => return Err(anyhow::anyhow!("不支持的配置格式，支持: yaml, toml, json")),
        };

        Ok(config)
    }

    /// 加载并根据环境变量覆盖
    pub fn load() -> Self {
        // 先从配置文件加载（可选）
        let mut config = std::env::var("CARPAI_CONFIG")
            .ok()
            .and_then(|p| Self::from_file(p).ok())
            .unwrap_or_default();

        // 环境变量覆盖
        if let Ok(port) = std::env::var("CARPAI_API_PORT") {
            if let Ok(p) = port.parse() { config.server.api_port = p; }
        }
        if let Ok(port) = std::env::var("CARPAI_ADMIN_PORT") {
            if let Ok(p) = port.parse() { config.server.admin_port = p; }
        }
        if let Ok(db) = std::env::var("CARPAI_DATABASE_URL") {
            config.database.url = db;
        }
        if let Ok(level) = std::env::var("CARPAI_LOG_LEVEL") {
            config.server.log_level = level;
        }
        if let Ok(dir) = std::env::var("CARPAI_MODEL_DIR") {
            config.models.model_cache_dir = PathBuf::from(dir);
        }

        config
    }
}
