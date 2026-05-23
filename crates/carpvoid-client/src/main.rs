//! carpvoid-client — 边缘节点推理客户端
//!
//! 功能: 在网吧电脑/笔记本上运行, 接收 CarpAI 协调器分发的推理任务
//! 架构: gRPC + WebSocket 双通道 → 本地 llama.cpp GGUF 推理 → 返回结果
//!
//! 运行: carpvoid-client --coordinator https://carpai.example.com:50051
//!
//! 最低硬件: 核显 + 4GB RAM (Qwen3-1.5B Q4_0)
//! 推荐硬件: GTX 1060 + 8GB RAM (Qwen3-7B Q4_K_M)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::time::sleep;

const VERSION: &str = "0.1.0";

// ========================================================================
// 配置
// ========================================================================

#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// 协调器地址
    pub coordinator_url: String,
    /// 节点名称 (默认: 主机名)
    pub node_name: String,
    /// 工作线程数
    pub worker_threads: usize,
    /// 模型路径 (自动下载到 ~/.carpvoid/models/)
    pub model_path: Option<String>,
    /// 最大并发推理数
    pub max_concurrent: usize,
    /// 心跳间隔 (秒)
    pub heartbeat_interval: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            coordinator_url: "http://127.0.0.1:50051".to_string(),
            node_name: hostname(),
            worker_threads: num_cpus(),
            model_path: None,
            max_concurrent: 2,
            heartbeat_interval: 30,
        }
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-pc".to_string())
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

// ========================================================================
// 硬件检测
// ========================================================================

#[derive(Debug, Clone)]
pub struct HardwareInfo {
    pub gpu_name: String,
    pub vram_mb: u64,
    pub ram_mb: u64,
    pub cpu_cores: usize,
    pub os: String,
    pub has_cuda: bool,
    pub has_vulkan: bool,
    pub is_laptop: bool,
}

impl HardwareInfo {
    /// 自动检测硬件信息
    pub fn detect() -> Self {
        let os = if cfg!(windows) { "windows".to_string() }
                 else if cfg!(macos) { "macos".to_string() }
                 else { "linux".to_string() };

        let ram_mb = sys_info::mem_info()
            .map(|m| m.total / 1024)
            .unwrap_or(8192);

        Self {
            gpu_name: detect_gpu(),
            vram_mb: detect_vram(),
            ram_mb,
            cpu_cores: num_cpus() as u64,
            os,
            has_cuda: detect_cuda(),
            has_vulkan: detect_vulkan(),
            is_laptop: detect_is_laptop(),
        }
    }

    /// 根据硬件选择最适合的模型
    pub fn suggest_model(&self) -> &str {
        if self.vram_mb >= 12000 {
            "Qwen3-14B-Q4_K_M"   // RTX 3060+
        } else if self.vram_mb >= 6000 {
            "Qwen3-7B-Q4_K_M"    // GTX 1060
        } else if self.ram_mb >= 16000 {
            "Qwen3-7B-Q4_0"      // CPU only, 16GB RAM
        } else {
            "Qwen3-1.5B-Q4_0"    // 核显 / 低配
        }
    }
}

fn detect_gpu() -> String {
    // Windows: 通过 WMI 查询
    if cfg!(windows) {
        if let Ok(output) = std::process::Command::new("wmic")
            .args(["path", "win32_VideoController", "get", "name"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().skip(1) {
                let trimmed = line.trim();
                if !trimmed.is_empty() && trimmed != "Name" {
                    return trimmed.to_string();
                }
            }
        }
    }
    // Linux: nvidia-smi
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
    {
        if let Ok(name) = String::from_utf8(output.stdout) {
            let name = name.trim().to_string();
            if !name.is_empty() { return name; }
        }
    }
    "Unknown GPU".to_string()
}

fn detect_vram() -> u64 {
    // Windows: WMI 查询
    if cfg!(windows) {
        if let Ok(output) = std::process::Command::new("wmic")
            .args(["path", "win32_VideoController", "get", "adapterram"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().skip(1) {
                if let Ok(bytes) = line.trim().parse::<u64>() {
                    return bytes / (1024 * 1024);
                }
            }
        }
    }
    // nvidia-smi
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
    {
        if let Ok(mib) = String::from_utf8(output.stdout) {
            if let Ok(mib) = mib.trim().parse::<u64>() {
                return mib;
            }
        }
    }
    0
}

fn detect_cuda() -> bool {
    std::process::Command::new("nvidia-smi")
        .output().map(|o| o.status.success()).unwrap_or(false)
}

fn detect_vulkan() -> bool {
    std::process::Command::new("vulkaninfo")
        .output().map(|o| o.status.success()).unwrap_or(false)
}

fn detect_is_laptop() -> bool {
    let name = hostname().to_lowercase();
    name.contains("laptop") || name.contains("notebook") || name.contains("book")
}

// ========================================================================
// Worker 节点
// ========================================================================

/// 推理任务
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InferenceTask {
    pub task_id: String,
    pub model: String,
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f64,
}

/// 推理结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InferenceResult {
    pub task_id: String,
    pub node_id: String,
    pub text: String,
    pub tokens_generated: u32,
    pub duration_ms: u64,
    pub model: String,
    pub success: bool,
    pub error: Option<String>,
}

/// 节点注册信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeRegistration {
    pub node_id: String,
    pub node_name: String,
    pub hardware: HardwareInfo,
    pub suggested_model: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

/// carpvoid 边缘 Worker 节点
pub struct CarpvoidWorker {
    config: ClientConfig,
    hardware: HardwareInfo,
    node_id: String,
    running: Arc<RwLock<bool>>,
    tasks_done: Arc<RwLock<u64>>,
}

impl CarpvoidWorker {
    pub fn new(config: ClientConfig) -> Self {
        let hardware = HardwareInfo::detect();
        let node_id = format!("carpvoid-{}", SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default().as_millis());

        Self {
            config,
            hardware,
            node_id,
            running: Arc::new(RwLock::new(true)),
            tasks_done: Arc::new(RwLock::new(0)),
        }
    }

    /// 启动 Worker 节点
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("━━━ Carpvoid Client v{} ━━━", VERSION);
        println!("Node ID:     {}", self.node_id);
        println!("Node Name:   {}", self.config.node_name);
        println!("Hardware:    {} ({}MB VRAM, {}MB RAM, {} cores)",
            self.hardware.gpu_name, self.hardware.vram_mb,
            self.hardware.ram_mb, self.hardware.cpu_cores);
        println!("OS:          {}", self.hardware.os);
        println!("CUDA:        {}", if self.hardware.has_cuda { "✅" } else { "❌" });
        println!("Model:       {}", self.hardware.suggest_model());
        println!("Coordinator: {}", self.config.coordinator_url);
        println!();

        // 注册到协调器
        let registration = NodeRegistration {
            node_id: self.node_id.clone(),
            node_name: self.config.node_name.clone(),
            hardware: self.hardware.clone(),
            suggested_model: self.hardware.suggest_model().to_string(),
            version: VERSION.to_string(),
            capabilities: vec![
                "inference".to_string(),
                if self.hardware.has_cuda { "cuda".to_string() } else { "cpu".to_string() },
            ],
        };

        println!("[Carpvoid] Registering with coordinator...");
        self.register(&registration).await?;
        println!("[Carpvoid] ✅ Registered as '{}'", self.node_id);

        // 主循环: 心跳 + 拉取任务
        println!("[Carpvoid] Starting heartbeat loop ({}s interval)...", self.config.heartbeat_interval);
        let running = self.running.clone();
        let tasks_done = self.tasks_done.clone();
        let node_id = self.node_id.clone();
        let coord_url = self.config.coordinator_url.clone();
        let model = registration.suggested_model.clone();
        let hw = self.hardware.clone();

        while *running.read().await {
            // 发送心跳 + 拉取任务
            match self.heartbeat_and_poll(&node_id, &coord_url).await {
                Ok(Some(task)) => {
                    println!("[Carpvoid] 📥 Task '{}': {} inference", task.task_id, task.model);
                    let result = self.run_inference(&task, &model, &hw).await;
                    match self.submit_result(&coord_url, &result).await {
                        Ok(_) => {
                            let mut done = tasks_done.write().await;
                            *done += 1;
                            println!("[Carpvoid] ✅ Task '{}' complete ({} done, {}ms)", 
                                task.task_id, *done, result.duration_ms);
                        }
                        Err(e) => eprintln!("[Carpvoid] ⚠️  Result submit failed: {}", e),
                    }
                }
                Ok(None) => {
                    // 无任务, 等待
                    sleep(Duration::from_secs(self.config.heartbeat_interval)).await;
                }
                Err(e) => {
                    eprintln!("[Carpvoid] ⚠️  Heartbeat failed: {} (reconnect in {}s)", 
                        e, self.config.heartbeat_interval);
                    sleep(Duration::from_secs(self.config.heartbeat_interval)).await;
                }
            }
        }

        Ok(())
    }

    /// 注册到协调器
    async fn register(&self, registration: &NodeRegistration) -> Result<(), String> {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/api/v1/distributed/register", self.config.coordinator_url))
            .json(registration)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Register failed: {}", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("Register returned {}", resp.status()))
        }
    }

    /// 心跳 + 拉取任务
    async fn heartbeat_and_poll(&self, node_id: &str, coord_url: &str) -> Result<Option<InferenceTask>, String> {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/api/v1/distributed/poll", coord_url))
            .json(&serde_json::json!({
                "node_id": node_id,
                "resources": {
                    "gpu": self.hardware.gpu_name,
                    "vram_mb": self.hardware.vram_mb,
                    "ram_mb": self.hardware.ram_mb,
                    "cpu_cores": self.hardware.cpu_cores,
                    "load": 0.5,
                }
            }))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("Poll failed: {}", e))?;

        if resp.status() == serde_status(204) {
            return Ok(None); // 无任务
        }

        let task: InferenceTask = resp.json().await
            .map_err(|e| format!("Parse task failed: {}", e))?;
        Ok(Some(task))
    }

    /// 执行本地推理 (调用 llama.cpp)
    async fn run_inference(&self, task: &InferenceTask, model: &str, _hw: &HardwareInfo) -> InferenceResult {
        let start = Instant::now();
        let model_path = self.config.model_path.as_ref()
            .cloned()
            .unwrap_or_else(|| format!("~/.carpvoid/models/{}.gguf", model));

        // 调用 llama.cpp 命令行
        let output = std::process::Command::new("llama-cli")
            .args([
                "-m", &model_path,
                "-p", &task.prompt,
                "-n", &task.max_tokens.to_string(),
                "-t", &self.config.worker_threads.to_string(),
                "--temp", &task.temperature.to_string(),
                "--no-display-prompt",
            ])
            .output();

        let duration_ms = start.elapsed().as_millis() as u64;

        match output {
            Ok(output) => {
                let text = String::from_utf8_lossy(&output.stdout).to_string();
                let tokens = text.split_whitespace().count() as u32;

                InferenceResult {
                    task_id: task.task_id.clone(),
                    node_id: self.node_id.clone(),
                    text,
                    tokens_generated: tokens,
                    duration_ms,
                    model: model.to_string(),
                    success: true,
                    error: None,
                }
            }
            Err(e) => InferenceResult {
                task_id: task.task_id.clone(),
                node_id: self.node_id.clone(),
                text: String::new(),
                tokens_generated: 0,
                duration_ms,
                model: model.to_string(),
                success: false,
                error: Some(format!("llama-cli error: {}", e)),
            },
        }
    }

    /// 提交推理结果
    async fn submit_result(&self, coord_url: &str, result: &InferenceResult) -> Result<(), String> {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/api/v1/distributed/result", coord_url))
            .json(result)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Submit result failed: {}", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("Submit returned {}", resp.status()))
        }
    }

    pub async fn stop(&self) {
        *self.running.write().await = false;
    }
}

fn serde_status(code: u16) -> reqwest::StatusCode {
    reqwest::StatusCode::from_u16(code).unwrap()
}

// ========================================================================
// 入口
// ========================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let coord_url = args.iter()
        .position(|a| a == "--coordinator" || a == "-c")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:50051".to_string());

    let config = ClientConfig {
        coordinator_url: coord_url,
        ..Default::default()
    };

    let worker = CarpvoidWorker::new(config);
    worker.run().await
}
