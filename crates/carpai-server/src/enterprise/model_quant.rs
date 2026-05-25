//! ## 任务 1.1: 三大国产大模型低比特量化适配
//!
//! 本模块提供 Qwen3、GLM、DeepSeek 模型的 GGUF Q4_K_M/INT4 量化配置和加载逻辑。
//! 量化后的模型可以在 128G 内存台式机上运行 72B 级别的大模型。
//!
//! ### 支持的模型和量化配置
//!
//! | 模型 | 原始大小 | Q4_K_M 大小 | 推理所需内存 |
//! |------|---------|-------------|-------------|
//! | Qwen3.5-72B | ~144 GB (FP16) | ~36 GB | 40-44 GB |
//! | QwQ-32B | ~64 GB (FP16) | ~18 GB | 22-26 GB |
//! | DeepSeek-R1-32B | ~64 GB (FP16) | ~18 GB | 22-26 GB |
//! | GLM-5-9B | ~18 GB (FP16) | ~6 GB | 8-10 GB |

use crate::enterprise::config::{EnterpriseConfig, ModelEntry, ModelType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 模型量化信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedModelInfo {
    /// 模型名称
    pub name: String,
    /// HuggingFace 模型 ID
    pub hf_model_id: String,
    /// 源模型类型
    pub model_type: ModelType,
    /// 推荐的 GGUF 量化文件名
    pub recommended_gguf: String,
    /// 获取命令（如何下载和转换）
    pub acquire_command: String,
    /// 模型参数量
    pub param_count: &'static str,
    /// FP16 版本大小 (GB)
    pub fp16_size_gb: f64,
    /// Q4_K_M 量化后大小 (GB)
    pub q4km_size_gb: f64,
    /// 最小推理内存 (GB)
    pub min_inference_memory_gb: f64,
    /// 推荐 CPU 核数
    pub recommended_cpu_cores: u32,
    /// 推荐系统内存 (GB)
    pub recommended_system_memory_gb: f64,
    /// Transformer 层数
    pub num_layers: u32,
}

impl QuantizedModelInfo {
    /// 获取支持的模型列表（包含下载/转换脚本信息）
    pub fn supported_models() -> Vec<Self> {
        vec![
            Self {
                name: "qwen3-72b-int4".into(),
                hf_model_id: "Qwen/Qwen3-72B".into(),
                model_type: ModelType::Chat,
                recommended_gguf: "qwen3-72b-Q4_K_M.gguf".into(),
                acquire_command: "python3 scripts/download_quantize.py --model Qwen/Qwen3-72B --quant Q4_K_M".into(),
                param_count: "72B",
                fp16_size_gb: 144.0,
                q4km_size_gb: 36.0,
                min_inference_memory_gb: 40.0,
                recommended_cpu_cores: 8,
                recommended_system_memory_gb: 64.0,
                num_layers: 80,
            },
            Self {
                name: "qwq-32b-int4".into(),
                hf_model_id: "Qwen/QwQ-32B-Preview".into(),
                model_type: ModelType::Chat,
                recommended_gguf: "qwq-32b-Q4_K_M.gguf".into(),
                acquire_command: "python3 scripts/download_quantize.py --model Qwen/QwQ-32B-Preview --quant Q4_K_M".into(),
                param_count: "32B",
                fp16_size_gb: 64.0,
                q4km_size_gb: 18.0,
                min_inference_memory_gb: 22.0,
                recommended_cpu_cores: 4,
                recommended_system_memory_gb: 32.0,
                num_layers: 40,
            },
            Self {
                name: "deepseek-r1-32b-int4".into(),
                hf_model_id: "deepseek-ai/DeepSeek-R1-Distill-Qwen-32B".into(),
                model_type: ModelType::Code,
                recommended_gguf: "deepseek-r1-32b-Q4_K_M.gguf".into(),
                acquire_command: "python3 scripts/download_quantize.py --model deepseek-ai/DeepSeek-R1-Distill-Qwen-32B --quant Q4_K_M".into(),
                param_count: "32B",
                fp16_size_gb: 64.0,
                q4km_size_gb: 18.0,
                min_inference_memory_gb: 22.0,
                recommended_cpu_cores: 4,
                recommended_system_memory_gb: 32.0,
                num_layers: 40,
            },
            Self {
                name: "glm5-9b-int4".into(),
                hf_model_id: "THUDM/GLM-5-9B".into(),
                model_type: ModelType::Chat,
                recommended_gguf: "glm5-9b-Q4_K_M.gguf".into(),
                acquire_command: "python3 scripts/download_quantize.py --model THUDM/GLM-5-9B --quant Q4_K_M".into(),
                param_count: "9B",
                fp16_size_gb: 18.0,
                q4km_size_gb: 6.0,
                min_inference_memory_gb: 8.0,
                recommended_cpu_cores: 2,
                recommended_system_memory_gb: 16.0,
                num_layers: 28,
            },
        ]
    }

    /// 根据配置生成 llama.cpp 启动参数
    pub fn to_llamacpp_args(&self, model_path: &PathBuf) -> Vec<String> {
        vec![
            "--model".to_string(),
            model_path.to_string_lossy().to_string(),
            "--threads".to_string(),
            self.recommended_cpu_cores.to_string(),
            "--n-gpu-layers".to_string(),
            "0".to_string(), // CPU only
            "--ctx-size".to_string(),
            "4096".to_string(), // 根据可用内存动态调整
            "--batch-size".to_string(),
            "512".to_string(),
            "--no-mmap".to_string(),
            "--mlock".to_string(), // 锁内存防止交换
        ]
    }

    /// 将量化信息适配为调度器可用的 NodeHardwareInfo 配置
    pub fn scheduler_node_configs(&self) -> Vec<HashMap<String, f64>> {
        // 为每层分配提供参考配置
        vec![
            vec![(String::from("available_memory_gb"), self.min_inference_memory_gb)].into_iter().collect(),
        ]
    }
}

impl ModelEntry {
    /// 从量化信息创建 ModelEntry
    pub fn from_quantized(info: &QuantizedModelInfo) -> Self {
        Self {
            name: info.name.clone(),
            display_name: format!("{} (4bit)", info.name),
            model_type: info.model_type,
            quantized: true,
            quantization: "Q4_K_M".into(),
            gguf_path: Some(PathBuf::from(format!("./models/{}", info.recommended_gguf))),
            min_memory_gb: info.min_inference_memory_gb,
            supports_distributed: info.param_count.starts_with("72") || info.param_count.starts_with("32"),
            num_layers: info.num_layers,
            context_window: 4096,
            supports_streaming: true,
            supports_function_calling: true,
            provider: "llamacpp".into(),
            api_base_url: None,
            api_key_env: None,
        }
    }
}
