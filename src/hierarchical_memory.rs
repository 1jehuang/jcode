//! # Hierarchical Memory — 分层记忆系统（借鉴 TencentDB-Agent-Memory）
//!
//! 四层渐进式记忆管道，解决长链编程任务的上下文膨胀问题：
//!
//! - L0 (Raw Output): 原始工具输出 / 对话日志（完整保真）
//! - L1 (Step Summary): 步骤级摘要（结构化 JSONL）
//! - L2 (Mermaid Canvas): 整个任务状态的 Mermaid 符号图（轻量级，保留在上下文中）
//! - L3 (Persona): 跨会话的用户画像 / 场景记忆
//!
//! 核心创新：上下文卸载 (Context Offload) + Node_ID 追溯
//! Agent 上下文中仅保留 L2 Mermaid 图，需要细节时通过 node_id 从 L1/L0 精确检索

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 记忆层级
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MemoryLayer {
    L0Raw,
    L1StepSummary,
    L2MermaidCanvas,
    L3Persona,
}

/// 记忆条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub layer: MemoryLayer,
    pub session_id: String,
    pub node_id: Option<String>,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub parent_id: Option<String>,
    pub tags: Vec<String>,
    pub token_count: usize,
}

/// 步骤摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepSummary {
    pub step_id: String,
    pub title: String,
    pub action: String,
    pub file_changed: Option<String>,
    pub result: String,
    pub duration_ms: u64,
    pub has_error: bool,
}

/// Mermaid 画布节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MermaidNode {
    pub node_id: String,
    pub label: String,
    pub node_type: MermaidNodeType,
}

/// Mermaid 画布节点类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MermaidNodeType {
    Task,
    File,
    Function,
    Decision,
    Error,
    Checkpoint,
}

/// 分层记忆管理器
pub struct HierarchicalMemory {
    base_dir: PathBuf,
    short_term: Vec<MemoryEntry>,
    long_term: std::sync::RwLock<HashMap<String, Vec<MemoryEntry>>>,
}

impl HierarchicalMemory {
    /// 创建分层记忆管理器
    pub fn new(base_path: &Path) -> Self {
        let base_dir = base_path.join("hierarchical_memory");
        std::fs::create_dir_all(base_dir.join("L0_raw")).ok();
        std::fs::create_dir_all(base_dir.join("L1_steps")).ok();
        std::fs::create_dir_all(base_dir.join("L2_mermaid")).ok();
        std::fs::create_dir_all(base_dir.join("L3_persona")).ok();

        Self {
            base_dir,
            short_term: Vec::with_capacity(100),
            long_term: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// 记录 L0: 原始输出
    pub fn record_raw(&mut self, session_id: &str, content: &str, tags: Vec<String>) -> String {
        let id = format!("raw_{}", uuid::Uuid::new_v4());
        let entry = MemoryEntry {
            id: id.clone(),
            layer: MemoryLayer::L0Raw,
            session_id: session_id.to_string(),
            node_id: Some(id.clone()),
            content: content.to_string(),
            created_at: chrono::Utc::now(),
            parent_id: None,
            tags,
            token_count: content.len() / 4,
        };
        // 写入文件系统
        let path = self.base_dir.join("L0_raw").join(format!("{}.md", &id));
        std::fs::write(&path, &entry.content).ok();
        self.short_term.push(entry);
        id
    }

    /// 记录 L1: 步骤摘要
    pub fn record_step(&mut self, session_id: &str, summary: &StepSummary) -> String {
        let id = format!("step_{}", uuid::Uuid::new_v4());
        let json = serde_json::to_string(summary).unwrap_or_default();
        let entry = MemoryEntry {
            id: id.clone(),
            layer: MemoryLayer::L1StepSummary,
            session_id: session_id.to_string(),
            node_id: Some(summary.step_id.clone()),
            content: json.clone(),
            created_at: chrono::Utc::now(),
            parent_id: None,
            tags: vec!["step".into()],
            token_count: json.len() / 4,
        };
        let path = self.base_dir.join("L1_steps").join(format!("{}.jsonl", &id));
        std::fs::write(&path, &entry.content).ok();
        self.short_term.push(entry);
        id
    }

    /// 生成 L2: Mermaid 画布（将当前任务状态压缩为符号图）
    pub fn build_mermaid_canvas(&self, session_id: &str, nodes: &[MermaidNode]) -> String {
        let mut canvas = String::from("graph TD\n");
        canvas.push_str(&format!("    subgraph Session_{}\n", session_id));
        for node in nodes {
            let shape = match node.node_type {
                MermaidNodeType::Task => "[",
                MermaidNodeType::File => "([",
                MermaidNodeType::Function => ">",
                MermaidNodeType::Decision => "{",
                MermaidNodeType::Error => "{{",
                MermaidNodeType::Checkpoint => "[\"✓",
            };
            canvas.push_str(&format!("    {}{}\"{}\"]\n", node.node_id, shape, node.label));
        }
        canvas.push_str("    end\n");
        canvas
    }

    /// 记录 L2 画布
    pub fn record_mermaid(&mut self, session_id: &str, canvas: &str) -> String {
        let id = format!("mermaid_{}", uuid::Uuid::new_v4());
        let entry = MemoryEntry {
            id: id.clone(),
            layer: MemoryLayer::L2MermaidCanvas,
            session_id: session_id.to_string(),
            node_id: None,
            content: canvas.to_string(),
            created_at: chrono::Utc::now(),
            parent_id: None,
            tags: vec!["mermaid".into()],
            token_count: canvas.len() / 4,
        };
        let path = self.base_dir.join("L2_mermaid").join(format!("{}.mmd", &id));
        std::fs::write(&path, &entry.content).ok();
        id
    }

    /// 通过 node_id 从 L0/L1 追溯原始内容
    pub fn trace_node(&self, node_id: &str) -> Option<String> {
        // 先查短期记忆
        for entry in &self.short_term {
            if entry.node_id.as_deref() == Some(node_id) {
                return Some(entry.content.clone());
            }
        }
        // 查文件系统
        for layer in &["L0_raw", "L1_steps"] {
            let path = self.base_dir.join(layer);
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.contains(node_id) {
                        return std::fs::read_to_string(entry.path()).ok();
                    }
                }
            }
        }
        None
    }

    /// 获取当前会话的上下文摘要（仅 L2 画布，用于保持上下文轻量）
    pub fn get_context_summary(&self, session_id: &str) -> String {
        let mut summary = String::new();
        for entry in self.short_term.iter().rev().take(20) {
            if entry.session_id == session_id && matches!(entry.layer, MemoryLayer::L2MermaidCanvas) {
                summary.push_str(&entry.content);
                summary.push('\n');
            }
        }
        summary
    }

    /// 估算 Token 节省量（对比原始日志 vs Mermaid 压缩）
    pub fn token_savings(&self) -> (usize, usize) {
        let raw_tokens: usize = self.short_term.iter()
            .filter(|e| matches!(e.layer, MemoryLayer::L0Raw))
            .map(|e| e.token_count).sum();
        let canvas_tokens: usize = self.short_term.iter()
            .filter(|e| matches!(e.layer, MemoryLayer::L2MermaidCanvas))
            .map(|e| e.token_count).sum();
        (raw_tokens, canvas_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut mem = HierarchicalMemory::new(dir.path());

        // L0
        mem.record_raw("sess-1", "ls -la\n总用量 42", vec!["bash".into()]);

        // L1
        mem.record_step("sess-1", &StepSummary {
            step_id: "step-1".into(), title: "列出目录".into(),
            action: "bash".into(), file_changed: None,
            result: "成功".into(), duration_ms: 100, has_error: false,
        });

        // L2
        let nodes = vec![MermaidNode {
            node_id: "N1".into(), label: "Read main.rs".into(),
            node_type: MermaidNodeType::Task,
        }];
        let canvas = mem.build_mermaid_canvas("sess-1", &nodes);
        mem.record_mermaid("sess-1", &canvas);

        let (raw, canvas_tokens) = mem.token_savings();
        assert!(raw > 0);
        assert!(canvas_tokens > 0);
    }
}
