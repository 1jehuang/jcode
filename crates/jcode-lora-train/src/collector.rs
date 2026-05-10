//! 补全数据采集器 — 记录用户每次接受/拒绝的编辑操作
//!
//! 每条样本 = (before, after, cursor_pos, accepted_text, context)
//! 训练时: input = before + cursor_pos, output = accepted_text

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

/// 一条编辑样本
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EditSample {
    /// 补全前的代码 (前 50 tokens)
    pub before: String,
    /// 补全后的代码 (接受后完整行)
    pub after: String,
    /// 用户最终接受的文本 (被补全的部分)
    pub accepted: String,
    /// 光标所在行
    pub line: usize,
    /// 光标所在列
    pub column: usize,
    /// 文件后缀
    pub extension: String,
    /// 被拒绝的候选 (如果有)
    pub rejected: Option<String>,
    /// 时间戳
    pub timestamp: String,
}

/// 编辑样本数据集
#[derive(Debug, Clone)]
pub struct EditDataset {
    pub samples: Vec<EditSample>,
    pub total_accepted: usize,
    pub total_rejected: usize,
}

/// 数据采集器
pub struct EditCollector {
    storage: PathBuf,
    buffer: Arc<Mutex<VecDeque<EditSample>>>,
    pending_flush: Arc<Mutex<Vec<EditSample>>>,
}

impl EditCollector {
    pub fn new(storage: PathBuf) -> Self {
        Self {
            storage,
            buffer: Arc::new(Mutex::new(VecDeque::new())),
            pending_flush: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 用户接受了补全 — 记录样本
    pub fn record_accepted(
        &self,
        before: &str,
        after: &str,
        accepted: &str,
        line: usize,
        column: usize,
        extension: &str,
    ) {
        let sample = EditSample {
            before: before.to_string(),
            after: after.to_string(),
            accepted: accepted.to_string(),
            line, column,
            extension: extension.to_string(),
            rejected: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.buffer.lock().push_back(sample);
    }

    /// 用户拒绝了补全 — 记录被拒的候选
    pub fn record_rejected(
        &self,
        before: &str,
        rejected: &str,
        line: usize,
        column: usize,
        extension: &str,
    ) {
        let sample = EditSample {
            before: before.to_string(),
            after: String::new(),
            accepted: String::new(),
            line, column,
            extension: extension.to_string(),
            rejected: Some(rejected.to_string()),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.buffer.lock().push_back(sample);
    }

    /// 刷新到磁盘
    pub async fn flush(&self) -> anyhow::Result<()> {
        let samples: Vec<EditSample> = self.buffer.lock().drain(..).collect();
        if samples.is_empty() { return Ok(()); }

        let file = tokio::fs::OpenOptions::new()
            .create(true).append(true).open(&self.storage).await?;
        let mut writer = tokio::io::BufWriter::new(file);

        for sample in &samples {
            let line = serde_json::to_string(sample)?;
            use tokio::io::AsyncWriteExt;
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
        }
        writer.flush().await?;
        tracing::info!("Flushed {} edit samples to {:?}", samples.len(), self.storage);
        Ok(())
    }

    /// 获取所有已采集的样本
    pub fn samples(&self) -> Vec<EditSample> {
        self.buffer.lock().iter().cloned().collect()
    }

    /// 构建训练数据集
    pub fn build_dataset(&self) -> EditDataset {
        let samples = self.samples();
        let total_accepted = samples.iter().filter(|s| !s.accepted.is_empty()).count();
        let total_rejected = samples.iter().filter(|s| s.rejected.is_some()).count();
        EditDataset { samples, total_accepted, total_rejected }
    }
}
