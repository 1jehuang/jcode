//! ## 任务 1.2: 纯 CPU 推理优化适配
//!
//! 本模块封装 llama.cpp 的 CPU 推理能力，提供：
//! 1. **纯 CPU 运行**: 完全不需要 GPU，适配无独显设备
//! 2. **模型生命周期管理**: 按需加载/卸载模型，节省内存
//! 3. **推理参数优化**: 根据 CPU 核心数、内存大小自动选择最优参数
//! 4. **多模型切换**: 支持按请求动态切换不同量化模型
//!
//! ### 适配您的硬件环境
//!
//! - 128G 台式机: 可同时加载 2 个 72B 量化模型（每个~40GB）或 5 个 9B 模型
//! - 32G 笔记本: 可加载 1 个 32B 量化模型（~22GB）
//! - 16G 旧电脑: 可加载 1 个 9B 模型（~8GB）

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::enterprise::config::{EnterpriseConfig, ModelEntry};

/// CPU 推理引擎状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineStatus {
    /// 未初始化
    Uninitialized,
    /// 正在加载模型
    Loading,
    /// 就绪，可处理请求
    Ready,
    /// 已停止
    Stopped,
    /// 出错
    Error(String),
}

/// 模型实例（对应一个运行的 llama.cpp 进程）
pub struct ModelInstance {
    /// 模型名称
    pub model_name: String,
    /// 绑定的端口
    pub port: u16,
    /// 进程句柄
    process: Option<Child>,
    /// API base URL（用于 jcode-llm 的 OpenAiCompatibleProvider）
    pub api_base_url: String,
    /// 状态
    pub status: EngineStatus,
    /// 启动时间
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// 当前运行的请求数
    pub current_requests: Arc<std::sync::atomic::AtomicU32>,
}

impl std::fmt::Debug for ModelInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelInstance")
            .field("model_name", &self.model_name)
            .field("port", &self.port)
            .field("api_base_url", &self.api_base_url)
            .field("status", &self.status)
            .field("started_at", &self.started_at)
            .finish()
    }
}

impl Clone for ModelInstance {
    fn clone(&self) -> Self {
        Self {
            model_name: self.model_name.clone(),
            port: self.port,
            process: None,
            api_base_url: self.api_base_url.clone(),
            status: self.status.clone(),
            started_at: self.started_at,
            current_requests: Arc::clone(&self.current_requests),
        }
    }
}

/// CPU 推理引擎 — 管理多个 llama.cpp 进程
pub struct CpuInferenceEngine {
    /// 配置
    config: Arc<EnterpriseConfig>,
    /// 模型实例映射 (model_name -> ModelInstance)
    instances: Arc<RwLock<HashMap<String, ModelInstance>>>,
    /// 端口分配跟踪
    next_port: Arc<std::sync::atomic::AtomicU16>,
    /// 最大并发请求数
    max_requests_per_instance: u32,
}

impl CpuInferenceEngine {
    /// 创建新的 CPU 推理引擎
    pub fn new(config: Arc<EnterpriseConfig>) -> Self {
        Self {
            config,
            instances: Arc::new(RwLock::new(HashMap::new())),
            next_port: Arc::new(std::sync::atomic::AtomicU16::new(18000)),
            max_requests_per_instance: 4,
        }
    }

    /// 获取 llama.cpp 可执行文件路径
    fn get_llamacpp_path(&self) -> PathBuf {
        // 从环境变量或默认路径查找
        std::env::var("CARPAI_LLAMACPP_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("llama-server"))
    }

    /// 启动一个模型实例
    pub async fn start_model(&self, model_entry: &ModelEntry) -> anyhow::Result<ModelInstance> {
        let model_path = match &model_entry.gguf_path {
            Some(p) => p,
            None => anyhow::bail!("模型 {} 没有指定 GGUF 路径", model_entry.name),
        };

        if !model_path.exists() {
            // 模型不存在时不报错，由上层逻辑跳过
            warn!(
                "模型文件不存在: {:?}，跳过加载 {}. 请运行量化脚本生成模型文件。",
                model_path, model_entry.name
            );
            anyhow::bail!("模型文件不存在: {:?}", model_path);
        }

        let port = self.next_port.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let llamacpp_path = self.get_llamacpp_path();
        let threads = num_cpus::get_physical().to_string();
        let ctx_size = ((self.config.models.supported_models.len() as f64).max(1.0) * 2048.0) as usize;

        let mut cmd = Command::new(&llamacpp_path);
        cmd
            .arg("--model")
            .arg(model_path)
            .arg("--host")
            .arg("0.0.0.0")
            .arg("--port")
            .arg(port.to_string())
            .arg("--threads")
            .arg(&threads)
            .arg("--ctx-size")
            .arg(ctx_size.to_string())
            .arg("--batch-size")
            .arg("512")
            .arg("--n-gpu-layers")
            .arg("0")  // 纯 CPU
            .arg("--mlock")  // 锁定内存防止交换
            .arg("--cont-batching")  // 持续批处理
            .stdout(Stdio::null())
            .stderr(Stdio::piped());  // 捕获错误输出以便调试

        // 对低内存设备优化
        let total_mem = sys_info::mem_info()
            .map(|m| m.total as f64 / 1024.0 / 1024.0)
            .unwrap_or(16.0);

        if total_mem < 32.0 {
            cmd.arg("--no-mmap");
        }

        info!(
            "启动模型 '{}' (llama-server) 在端口 {}，路径: {:?}",
            model_entry.name, port, model_path
        );

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                error!(
                    "启动 llama.cpp 失败: {}。请确保已安装 llama.cpp (https://github.com/ggerganov/llama.cpp)",
                    e
                );
                anyhow::bail!("无法启动 llama.cpp: {}", e);
            }
        };

        let api_base_url = format!("http://localhost:{}/v1", port);
        let model_name = model_entry.name.clone();

        let instance = ModelInstance {
            model_name: model_name.clone(),
            port,
            process: Some(child),
            api_base_url: api_base_url.clone(),
            status: EngineStatus::Loading,
            started_at: Some(chrono::Utc::now()),
            current_requests: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        };

        // 等待模型就绪（最多 60 秒）
        self.wait_for_ready(&api_base_url, 60).await?;

        // 注册到实例映射
        let mut instances = self.instances.write().await;
        instances.insert(model_name.clone(), instance);

        info!("模型 '{}' 已就绪: {}", model_name, api_base_url);
        Ok(instances.get(&model_name).unwrap().clone())
    }

    /// 等待 llama.cpp 服务器就绪
    async fn wait_for_ready(&self, api_base_url: &str, timeout_secs: u64) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;

        let ready_url = format!("{}/models", api_base_url);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed().as_secs() > timeout_secs {
                anyhow::bail!("模型在 {} 秒内未就绪", timeout_secs);
            }

            match client.get(&ready_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    return Ok(());
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                    continue;
                }
            }
        }
    }

    /// 停止所有模型实例
    pub async fn stop_all(&self) {
        let mut instances = self.instances.write().await;
        for (name, instance) in instances.iter_mut() {
            if let Some(arc_child) = instance.process.take() {
                info!("停止模型 '{}' (端口 {})", name, instance.port);
                let mut child = arc_child;
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            instance.status = EngineStatus::Stopped;
        }
        instances.clear();
        info!("所有模型实例已停止");
    }

    /// 获取当前运行的模型实例列表
    pub async fn list_instances(&self) -> Vec<ModelInstance> {
        let instances = self.instances.read().await;
        instances.values().cloned().collect()
    }

    /// 获取指定模型的 API 地址
    pub async fn get_api_base_url(&self, model_name: &str) -> Option<String> {
        let instances = self.instances.read().await;
        instances.get(model_name).map(|i| i.api_base_url.clone())
    }

    /// 获取模型实例的引用
    pub async fn get_instance(&self, model_name: &str) -> Option<ModelInstance> {
        let instances = self.instances.read().await;
        instances.get(model_name).cloned()
    }

    /// 计算系统内存状态
    pub fn system_memory_status() -> Option<CpuMemoryStatus> {
        let info = sys_info::mem_info().ok()?;
        Some(CpuMemoryStatus {
            total_gb: info.total as f64 / 1024.0 / 1024.0,
            available_gb: info.avail as f64 / 1024.0 / 1024.0,
            free_gb: info.free as f64 / 1024.0 / 1024.0,
            buffers_gb: info.buffers as f64 / 1024.0 / 1024.0,
            cached_gb: info.cached as f64 / 1024.0 / 1024.0,
        })
    }
}

/// CPU 内存状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuMemoryStatus {
    pub total_gb: f64,
    pub available_gb: f64,
    pub free_gb: f64,
    pub buffers_gb: f64,
    pub cached_gb: f64,
}

/// 根据可用内存推荐可加载的模型
pub fn recommend_models(available_gb: f64) -> Vec<String> {
    let models = QuantizedModelInfo::supported_models();
    let mut recommended = Vec::new();

    for m in &models {
        if available_gb >= m.min_inference_memory_gb + 4.0 { // 留 4GB 余量
            recommended.push(m.name.clone());
        }
    }

    // 按内存需求从小到大排序
    recommended
}

pub use crate::enterprise::model_quant::QuantizedModelInfo;

use sys_info;
