//! # jcode-distributed-inference
//!
//! CarpAI 分布式推理引擎 — 实现 Parallax 流水线并行的 Worker 节点服务。
//!
//! ## 核心功能
//! - **Layer Execution**: 接收 Coordinator 分发的模型层计算任务
//! - **KV Cache Transfer**: 通过 gRPC Stream 高效传输中间激活值
//! - **Node Health**: 心跳上报与负载监控

pub mod worker;
pub mod layer_executor;
pub mod kv_cache_manager;
pub mod kv_cache_optimizer;
pub mod coordinator_client;
pub mod serialization;
pub mod speculative;

// 重导出 tonic 生成的 proto 代码
pub mod proto {
    tonic::include_proto!("jcode");
}

use anyhow::Result;
use tracing::{info, error};

/// 启动分布式推理 Worker 节点
pub async fn start_worker_node(
    listen_addr: String,
    coordinator_addr: String,
) -> Result<()> {
    info!("🚀 启动 Distributed Inference Worker Node");
    info!("   监听地址: {}", listen_addr);
    info!("   Coordinator: {}", coordinator_addr);

    let addr = listen_addr.parse::<std::net::SocketAddr>()?;

    // 初始化层执行器
    let executor = layer_executor::LayerExecutor::new()?;

    // 初始化 KV Cache 管理器
    let kv_manager = kv_cache_manager::KVCacheManager::new();

    // 构建 gRPC 服务
    let service = worker::DistributedInferenceServiceImpl::new(executor, kv_manager);

    // 启动 gRPC 服务器
    tonic::transport::Server::builder()
        .add_service(proto::distributed_inference_service_server::DistributedInferenceServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
