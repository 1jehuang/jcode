//! # CarpAI Node Agent — 节点代理
//!
//! 安装在每台参与推理的设备上（台式机、网吧电脑、员工笔记本），
//! 自动向企业版服务器注册并接收推理任务。
//!
//! ## 启动方式
//!
//! ```bash
//! # 默认
//! cargo run --bin carpai-node-agent
//!
//! # 指定服务器地址
//! CARPAI_SERVER=http://192.168.1.100:8000
//! CARPAI_NODE_NAME="办公室台式机_01"
//! cargo run --bin carpai-node-agent
//! ```
//!
//! ### 适用操作系统
//!
//! | 节点类型 | 操作系统 | 配置建议 |
//! |---------|---------|---------|
//! | 固定台式机 | Linux/Windows | 始终运行 |
//! | 网吧电脑 | Windows | 通过计划任务开机启动 |
//! | 员工笔记本 | Windows/macOS | 下班后手动启动 |
//!
//! ### 建议的部署方式
//!
//! **网吧电脑**: 通过网吧管理系统设置开机自启动，在游戏结束后启动推理服务。
//! **员工笔记本**: 编写一个下班后自动运行的脚本，当屏幕锁定 30 分钟后自动启动。
//! **固定台式机**: 配置 systemd 服务或 Windows 服务，始终运行。

use std::time::Duration;

/// 节点代理配置
struct AgentConfig {
    /// 企业版服务器地址
    server_url: String,
    /// 节点名称
    node_name: String,
    /// 心跳间隔（秒）
    heartbeat_interval: u64,
    /// 本机端口
    port: u16,
}

impl AgentConfig {
    fn load() -> Self {
        Self {
            server_url: std::env::var("CARPAI_SERVER")
                .unwrap_or_else(|_| "http://localhost:8000".into()),
            node_name: std::env::var("CARPAI_NODE_NAME")
                .unwrap_or_else(|_| {
                    hostname().unwrap_or_else(|_| "unknown-node".into())
                }),
            heartbeat_interval: std::env::var("CARPAI_HEARTBEAT_INTERVAL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            port: std::env::var("CARPAI_NODE_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8002),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 CarpAI Node Agent 启动中...");

    let config = AgentConfig::load();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // 收集本机信息
    let node_id = uuid::Uuid::new_v4().to_string();
    let mem_info = sys_info::mem_info().ok();
    let cpu_cores = num_cpus::get_physical() as u32;
    let total_mem = mem_info.as_ref()
        .map(|m| m.total as f64 / 1024.0 / 1024.0)
        .unwrap_or(16.0);

    println!("📌 节点信息:");
    println!("   ID: {}", &node_id[..8]);
    println!("   名称: {}", config.node_name);
    println!("   CPU: {} 核", cpu_cores);
    println!("   内存: {:.0} GB", total_mem);
    println!("   服务器: {}", config.server_url);

    // 注册到服务器
    let register_payload = serde_json::json!({
        "node_id": node_id,
        "node_name": config.node_name,
        "node_type": "desktop",
        "ip_address": local_ip_address(),
        "port": config.port,
        "total_memory_gb": total_mem,
        "available_memory_gb": total_mem,
        "swap_total_gb": mem_info.map(|m| m.free as f64 / 1024.0 / 1024.0).unwrap_or(0.0),
        "cpu_cores": cpu_cores,
        "cpu_usage": 0.0,
        "has_gpu": false,
        "gpu_vram_mb": 0,
        "loaded_models": [],
        "last_heartbeat": chrono::Utc::now().timestamp(),
        "started_at": chrono::Utc::now().timestamp(),
        "tags": {},
    });

    // 尝试注册
    let register_url = format!("{}/admin/nodes", config.server_url);
    match client.post(&register_url).json(&register_payload).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                println!("✅ 已成功注册到服务器");
            }
        }
        Err(e) => {
            eprintln!("⚠️ 注册失败（服务可能未就绪）: {}", e);
        }
    }

    // 心跳循环
    println!("💓 心跳已启动（间隔 {} 秒）", config.heartbeat_interval);
    let mut interval = tokio::time::interval(Duration::from_secs(config.heartbeat_interval));

    loop {
        interval.tick().await;

        // 收集当前负载
        let heartbeat_payload = serde_json::json!({
            "node_id": node_id,
            "node_name": config.node_name,
            "node_type": "desktop",
            "total_memory_gb": total_mem,
            "available_memory_gb": get_available_memory(),
            "cpu_cores": cpu_cores,
            "cpu_usage": get_cpu_usage(),
            "has_gpu": false,
            "gpu_vram_mb": 0,
            "last_heartbeat": chrono::Utc::now().timestamp(),
            "loaded_models": [],
            "tags": {},
        });

        let heartbeat_url = format!("{}/admin/nodes/heartbeat", config.server_url);
        match client.post(&heartbeat_url).json(&heartbeat_payload).send().await {
            Ok(resp) if resp.status().is_success() => {}
            Ok(_) => {
                eprintln!("⚠️ 心跳响应异常");
            }
            Err(e) => {
                eprintln!("⚠️ 心跳发送失败: {}", e);
            }
        }
    }
}

/// 获取主机名
fn hostname() -> Result<String, std::io::Error> {
    Ok(std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into()))
}

/// 获取本机 IP
fn local_ip_address() -> String {
    // 简单实现：尝试连接外部 DNS 来获取本机 IP
    match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(local) = socket.local_addr() {
                    return local.ip().to_string();
                }
            }
            "127.0.0.1".into()
        }
        Err(_) => "127.0.0.1".into(),
    }
}

/// 获取可用内存 (GB)
fn get_available_memory() -> f64 {
    sys_info::mem_info()
        .map(|m| m.avail as f64 / 1024.0 / 1024.0)
        .unwrap_or(16.0)
}

/// 获取 CPU 使用率
fn get_cpu_usage() -> f64 {
    // 简化实现
    0.0
}
