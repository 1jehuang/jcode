//! LoRA 推理适配器 — 加载 LoRA 权重，Qwen 补全时使用
//!
//! 由 Python 训练脚本生成 adapter.safetensors + adapter_config.json
//! 推理时: Qwen 加载基座 + LoRA 权重 -> 高精度补全

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// LoRA 配置 (与 peft 格式兼容)
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LoraConfig {
    pub r: u32,
    pub lora_alpha: f32,
    pub target_modules: Vec<String>,
    pub bias: String,
    pub task_type: String,
}

impl Default for LoraConfig {
    fn default() -> Self {
        Self {
            r: 16,
            lora_alpha: 32.0,
            target_modules: vec!["q_proj".into(), "k_proj".into(), "v_proj".into(), "o_proj".into()],
            bias: "none".into(),
            task_type: "CAUSAL_LM".into(),
        }
    }
}

/// LoRA adapter 元信息
#[derive(Debug, Clone)]
pub struct LoraAdapter {
    pub name: String,
    pub config: LoraConfig,
    pub adapter_path: PathBuf,
    pub metrics: HashMap<String, f64>,
}

impl LoraAdapter {
    pub fn new(name: &str, path: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            config: LoraConfig::default(),
            adapter_path: path,
            metrics: HashMap::new(),
        }
    }

    /// 从目录加载 LoRA 配置
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        let config_path = path.join("adapter_config.json");
        let config_content = std::fs::read_to_string(&config_path)?;
        let config: LoraConfig = serde_json::from_str(&config_content)?;

        let name = path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("lora_adapter")
            .to_string();

        Ok(Self { name, config, adapter_path: path.clone(), metrics: HashMap::new() })
    }
}

/// LoRA 推理引擎 — 在补全时加载 adapter
pub struct LoraInferenceEngine {
    adapters: Vec<LoraAdapter>,
    active_adapter: Option<String>,
}

impl LoraInferenceEngine {
    pub fn new() -> Self {
        Self { adapters: Vec::new(), active_adapter: None }
    }

    /// 注册一个 LoRA adapter
    pub fn register(&mut self, adapter: LoraAdapter) {
        let name = adapter.name.clone();
        self.adapters.push(adapter);
        self.active_adapter = Some(name);
    }

    /// 在调用 Qwen 之前，将 LoRA 上下文注入到 prompt
    pub fn enhance_prompt(&self, prompt: &str) -> String {
        if self.active_adapter.is_none() {
            return prompt.to_string();
        }
        // LoRA 本身通过 Python 运行时加载 (vLLM / SGLang)
        // 这里仅返回原始 prompt，推理服务器负责加载 adapter
        format!(
            "{}",
            prompt
        )
    }

    pub fn active(&self) -> Option<&str> {
        self.active_adapter.as_deref()
    }
}

impl Default for LoraInferenceEngine { fn default() -> Self { Self::new() } }
