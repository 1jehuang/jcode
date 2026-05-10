//! # jcode-lora-train
//! Qwen 代码补全 LoRA — 数据采集 + 训练管线 + 推理适配器
//!
//! ## 流程
//! 1. 采集: 用户每次接受/拒绝补全时，记录 (before, after, accepted) 三元组
//! 2. 训练: Python 脚本读取数据 → unsloth LoRA → 输出 adapter
//! 3. 推理: jcode 在补全时加载 LoRA adapter → Qwen 补全更精准

mod collector;
mod inference;

pub use collector::{EditCollector, EditSample, EditDataset};
pub use inference::{LoraAdapter, LoraInferenceEngine, LoraConfig};

use std::path::PathBuf;
use std::sync::Arc;

/// LoRA 训练管线入口
pub struct LoraPipeline {
    collector: Arc<EditCollector>,
    data_dir: PathBuf,
    adapter_dir: PathBuf,
}

impl LoraPipeline {
    pub fn new(data_dir: PathBuf, adapter_dir: PathBuf) -> Self {
        Self {
            collector: Arc::new(EditCollector::new(data_dir.join("edits.jsonl"))),
            data_dir,
            adapter_dir,
        }
    }

    pub fn collector(&self) -> &EditCollector { &self.collector }

    /// 导出数据集 → 供 Python 训练脚本使用
    pub async fn export_dataset(&self) -> anyhow::Result<PathBuf> {
        let samples = self.collector.samples();
        let path = self.data_dir.join("completion_dataset.jsonl");
        let file = tokio::fs::File::create(&path).await?;
        let mut writer = tokio::io::BufWriter::new(file);

        for sample in &samples {
            let line = serde_json::to_string(sample)?;
            use tokio::io::AsyncWriteExt;
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
        }
        writer.flush().await?;
        Ok(path)
    }
}
