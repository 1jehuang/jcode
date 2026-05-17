//! Distributed Inference gRPC Service 实现

use crate::proto::{
    distributed_inference_service_server::DistributedInferenceService,
    LayerExecutionRequest, LayerExecutionResponse,
    KVCacheChunk, KVCacheAck, KVCacheMeta,
    NodeStatus, HeartbeatAck,
};
use crate::layer_executor::LayerExecutor;
use crate::kv_cache_manager::KVCacheManager;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn, error};

/// 分布式推理服务实现
pub struct DistributedInferenceServiceImpl {
    executor: Arc<Mutex<LayerExecutor>>,
    kv_manager: Arc<Mutex<KVCacheManager>>,
}

impl DistributedInferenceServiceImpl {
    pub fn new(executor: LayerExecutor, kv_manager: KVCacheManager) -> Self {
        Self {
            executor: Arc::new(Mutex::new(executor)),
            kv_manager: Arc::new(Mutex::new(kv_manager)),
        }
    }
}

#[tonic::async_trait]
impl DistributedInferenceService for DistributedInferenceServiceImpl {
    /// 执行模型层的前向传播
    async fn execute_layer(
        &self,
        request: Request<LayerExecutionRequest>,
    ) -> Result<Response<LayerExecutionResponse>, Status> {
        let req = request.into_inner();
        let request_id = req.request_id.clone();

        info!(
            "[Worker] 收到层执行请求: request_id={}, model={}, layers=[{}-{}]",
            request_id, req.model_name, req.start_layer, req.end_layer
        );

        let start_time = std::time::Instant::now();

        // 1. 反序列化输入激活值
        let activations = match deserialize_activations(&req.activations) {
            Ok(act) => act,
            Err(e) => {
                error!("[Worker] 激活值反序列化失败: {:?}", e);
                return Err(Status::invalid_argument(format!("Invalid activations: {}", e)));
            }
        };

        // 2. 执行层计算
        let mut executor = self.executor.lock().await;
        let output_activations = match executor.forward(
            &req.model_name,
            req.start_layer as usize,
            req.end_layer as usize,
            activations,
        ) {
            Ok(output) => output,
            Err(e) => {
                error!("[Worker] 层执行失败: {:?}", e);
                return Err(Status::internal(format!("Execution failed: {}", e)));
            }
        };

        let execution_time_ms = start_time.elapsed().as_secs_f64() * 1000.0;

        // 3. 序列化输出
        let output_bytes = serialize_activations(&output_activations);

        info!(
            "[Worker] 层执行完成: request_id={}, time={:.2}ms",
            request_id, execution_time_ms
        );

        Ok(Response::new(LayerExecutionResponse {
            request_id,
            activations: output_bytes,
            kv_cache_meta: Some(KVCacheMeta {
                seq_len: output_activations.shape()[0] as i32,
                num_heads: 32, // TODO: 从模型配置获取
                head_dim: 128,
                num_layers: (req.end_layer - req.start_layer) as i32,
            }),
            execution_time_ms,
        }))
    }

    /// 传输 KV Cache 状态（流式接收）
    async fn transfer_kv_cache(
        &self,
        request: Request<Streaming<KVCacheChunk>>,
    ) -> Result<Response<KVCacheAck>, Status> {
        let mut stream = request.into_inner();
        let mut request_id = String::new();
        let mut chunks = Vec::new();

        while let Some(chunk_result) = stream.message().await? {
            if request_id.is_empty() {
                request_id = chunk_result.request_id.clone();
                info!("[Worker] 开始接收 KV Cache: request_id={}", request_id);
            }

            chunks.push(chunk_result.clone());

            if chunk_result.is_last {
                break;
            }
        }

        // 重组 KV Cache
        let mut kv_manager = self.kv_manager.lock().await;
        match kv_manager.assemble_and_store(&request_id, chunks) {
            Ok(_) => {
                info!("[Worker] KV Cache 存储成功: request_id={}", request_id);
                Ok(Response::new(KVCacheAck {
                    request_id,
                    success: true,
                    error_message: String::new(),
                }))
            }
            Err(e) => {
                error!("[Worker] KV Cache 存储失败: {:?}", e);
                Ok(Response::new(KVCacheAck {
                    request_id,
                    success: false,
                    error_message: e.to_string(),
                }))
            }
        }
    }

    /// 节点心跳与状态上报
    async fn node_heartbeat(
        &self,
        request: Request<NodeStatus>,
    ) -> Result<Response<HeartbeatAck>, Status> {
        let status = request.into_inner();

        info!(
            "[Worker] 心跳上报: node_id={}, cpu={:.1}%, mem={:.1}GB, vram={:.1}GB, active_reqs={}",
            status.node_id,
            status.cpu_usage,
            status.memory_usage_gb,
            status.vram_usage_gb,
            status.active_requests
        );

        // TODO: 将状态上报给 Coordinator

        Ok(Response::new(HeartbeatAck {
            acknowledged: true,
            server_timestamp: chrono::Utc::now().timestamp_millis(),
        }))
    }
}

// ============================================================================
// 辅助函数：激活值序列化/反序列化
// ============================================================================

fn deserialize_activations(data: &[u8]) -> Result<ndarray::Array2<f16>, Box<dyn std::error::Error>> {
    // 简单实现：假设数据是行优先的 FP16 数组
    // 生产环境需要更复杂的格式（包含 shape 信息）
    let total_elements = data.len() / 2; // f16 = 2 bytes
    let values: Vec<f16> = (0..total_elements)
        .map(|i| f16::from_bits(u16::from_le_bytes([data[i*2], data[i*2+1]])))
        .collect();

    // 假设形状为 [seq_len, hidden_dim]
    let seq_len = 1; // TODO: 从元数据获取
    let hidden_dim = total_elements / seq_len;

    let arr = ndarray::Array2::from_shape_vec((seq_len, hidden_dim), values)?;
    Ok(arr)
}

fn serialize_activations(arr: &ndarray::Array2<f16>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(arr.len() * 2);
    for &val in arr.iter() {
        let bits = val.to_bits();
        bytes.extend_from_slice(&bits.to_le_bytes());
    }
    bytes
}
