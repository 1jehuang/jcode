//! Coordinator 客户端 — 用于向远程 Worker 节点分发推理任务

use crate::proto::{
    distributed_inference_service_client::DistributedInferenceServiceClient,
    LayerExecutionRequest, LayerExecutionResponse,
    KVCacheChunk, NodeStatus, HeartbeatAck,
};
use anyhow::{Result, Context};
use tonic::transport::Channel;
use tracing::{info, debug, warn};

/// 分布式推理 Coordinator 客户端
pub struct DistributedCoordinatorClient {
    client: DistributedInferenceServiceClient<Channel>,
    node_id: String,
}

impl DistributedCoordinatorClient {
    /// 连接到远程 Worker 节点
    pub async fn connect(addr: &str) -> Result<Self> {
        info!("🔗 连接 Worker 节点: {}", addr);

        let channel = Channel::from_shared(addr.to_string())?
            .connect()
            .await
            .context("Failed to connect to worker node")?;

        let client = DistributedInferenceServiceClient::new(channel);

        Ok(Self {
            client,
            node_id: addr.to_string(),
        })
    }

    /// 执行远程层计算
    pub async fn execute_remote_layer(
        &mut self,
        request_id: &str,
        model_name: &str,
        start_layer: u32,
        end_layer: u32,
        activations: Vec<u8>,
    ) -> Result<LayerExecutionResponse> {
        debug!(
            "[Coordinator] 发送远程层计算: request_id={}, layers=[{}-{}]",
            request_id, start_layer, end_layer
        );

        let request = tonic::Request::new(LayerExecutionRequest {
            request_id: request_id.to_string(),
            model_name: model_name.to_string(),
            start_layer: start_layer as i32,
            end_layer: end_layer as i32,
            activations,
            metadata: std::collections::HashMap::new(),
        });

        let response = self.client.execute_layer(request).await?;
        let result = response.into_inner();

        debug!(
            "[Coordinator] 远程层计算完成: time={:.2}ms",
            result.execution_time_ms
        );

        Ok(result)
    }

    /// 流式传输 KV Cache
    pub async fn transfer_kv_cache(
        &mut self,
        request_id: &str,
        chunks: Vec<KVCacheChunk>,
    ) -> Result<bool> {
        info!("[Coordinator] 开始传输 KV Cache: {} 个分片", chunks.len());

        let stream = tokio_stream::iter(chunks.into_iter().map(Ok));
        let request = tonic::Request::new(stream);

        let response = self.client.transfer_kv_cache(request).await?;
        let ack = response.into_inner();

        if ack.success {
            info!("[Coordinator] KV Cache 传输成功");
        } else {
            warn!("[Coordinator] KV Cache 传输失败: {}", ack.error_message);
        }

        Ok(ack.success)
    }

    /// 发送心跳与状态上报
    pub async fn send_heartbeat(
        &mut self,
        cpu_usage: f64,
        memory_gb: f64,
        vram_gb: f64,
        active_requests: i32,
        loaded_models: Vec<String>,
    ) -> Result<HeartbeatAck> {
        let request = tonic::Request::new(NodeStatus {
            node_id: self.node_id.clone(),
            cpu_usage,
            memory_usage_gb: memory_gb,
            vram_usage_gb: vram_gb,
            active_requests,
            loaded_models,
        });

        let response = self.client.node_heartbeat(request).await?;
        Ok(response.into_inner())
    }
}
