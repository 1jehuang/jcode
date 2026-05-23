//! # jcode-cpu-inference: CPU推理引擎
//!
//! 封装 llama.cpp 服务器的生命周期管理和本地推理适配，
//! 作为企业版服务器的底层推理引擎。

pub mod graceful_manager;
pub mod model_lifecycle_manager;  // P1-5: Hot-swapping and graceful shutdown

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::info;

/// CPU推理引擎 — 管理多个 llama.cpp 服务器进程
pub struct CpuEngine {
    /// 模型实例映射 (model_name -> 进程+端口信息)
    instances: Arc<RwLock<HashMap<String, LlamaInstance>>>,
    /// 端口分配器
    next_port: std::sync::atomic::AtomicU16,
}

/// 单模型 LLM 实例
#[derive(Debug, Clone)]
pub struct LlamaInstance {
    pub model_name: String,
    pub port: u16,
    pub api_url: String,
    pub status: InstanceStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceStatus {
    Loading,
    Ready,
    Error(String),
    Stopped,
}

impl CpuEngine {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            next_port: std::sync::atomic::AtomicU16::new(18000),
        }
    }

    /// 启动一个模型实例
    pub async fn start(
        &self,
        model_name: &str,
        model_path: &PathBuf,
        ctx_size: u32,
        threads: u32,
    ) -> anyhow::Result<LlamaInstance> {
        if !model_path.exists() {
            anyhow::bail!("模型文件不存在: {:?}", model_path);
        }

        let port = self.next_port.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let llamacpp = Self::find_llamacpp();

        let mut cmd = Command::new(&llamacpp);
        cmd
            .arg("--model").arg(model_path)
            .arg("--host").arg("127.0.0.1")
            .arg("--port").arg(port.to_string())
            .arg("--threads").arg(threads.to_string())
            .arg("--ctx-size").arg(ctx_size.to_string())
            .arg("--batch-size").arg("512")
            .arg("--n-gpu-layers").arg("0")  // CPU only
            .arg("--mlock")
            .arg("--cont-batching")
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        info!("启动推理引擎: {} (端口 {})", model_name, port);
        let child = cmd.spawn()
            .map_err(|e| anyhow::anyhow!("无法启动 llama.cpp: {}。请先安装: https://github.com/ggerganov/llama.cpp", e))?;

        // 后台跟踪进程
        let instance = LlamaInstance {
            model_name: model_name.to_string(),
            port,
            api_url: format!("http://127.0.0.1:{}/v1", port),
            status: InstanceStatus::Loading,
            started_at: chrono::Utc::now(),
        };

        let model = model_name.to_string();
        {
            let mut instances = self.instances.write().await;
            instances.insert(model.clone(), instance.clone());
        }

        // 异步等待就绪
        let api_url = instance.api_url.clone();
        let instances = Arc::clone(&self.instances);
        let model_name = model_name.to_string();
        tokio::spawn(async move {
            if wait_for_ready(&api_url, 60).await.is_ok() {
                info!("推理引擎就绪: {}", model_name);
                let mut guard = instances.write().await;
                if let Some(inst) = guard.get_mut(&model_name) {
                    inst.status = InstanceStatus::Ready;
                }
            }
        });

        Ok(instance)
    }

    /// 停止指定模型
    pub async fn stop(&self, model_name: &str) -> anyhow::Result<()> {
        let mut instances = self.instances.write().await;
        instances.remove(model_name);
        info!("已停止: {}", model_name);
        Ok(())
    }

    /// 停止所有模型
    pub async fn stop_all(&self) {
        let mut instances = self.instances.write().await;
        instances.clear();
        info!("所有推理引擎已停止");
    }

    /// 获取就绪的模型实例
    pub async fn get_ready_instance(&self, model_name: &str) -> Option<LlamaInstance> {
        let instances = self.instances.read().await;
        instances.get(model_name)
            .filter(|i| i.status == InstanceStatus::Ready)
            .cloned()
    }

    /// 所有就绪实例列表
    pub async fn list_ready(&self) -> Vec<LlamaInstance> {
        let instances = self.instances.read().await;
        instances.values()
            .filter(|i| i.status == InstanceStatus::Ready)
            .cloned()
            .collect()
    }

    fn find_llamacpp() -> String {
        std::env::var("CARPAI_LLAMACPP_PATH")
            .unwrap_or_else(|_| "llama-server".into())
    }
}

async fn wait_for_ready(api_url: &str, timeout_secs: u64) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let url = format!("{}/models", api_url);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed().as_secs() > timeout_secs {
            anyhow::bail!("超时");
        }
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => return Ok(()),
            _ => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
        }
    }
}

/// 获取系统内存状态
pub fn get_memory_gb() -> f64 {
    sys_info::mem_info()
        .map(|m| m.total as f64 / 1024.0 / 1024.0)
        .unwrap_or(16.0)
}
