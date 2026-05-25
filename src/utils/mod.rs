//! # 工具函数库
pub mod lru_cache;
pub mod rope;

pub use lru_cache::{LruCache, StringResultCache, CacheStats};
pub use rope::Rope;

use crate::core::util;

// 从 core::util 重新导出（供 crate::util::xxx 调用路径使用）
pub use crate::core::util::format_error_chain;
pub use crate::core::util::http_error_body;

/// Token 估算（按英文单词4:1比例粗略估计）
pub fn estimate_tokens(s: &str) -> u64 {
    (s.len() as f64 * 0.25).ceil() as u64
}

/// Token 数量的严重程度
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApproxTokenSeverity {
    Normal,
    Warning,
    Danger,
}

/// 根据 token 数返回严重程度
pub fn approx_tool_output_token_severity(tokens: u64) -> ApproxTokenSeverity {
    if tokens > 8000 {
        ApproxTokenSeverity::Danger
    } else if tokens > 2000 {
        ApproxTokenSeverity::Warning
    } else {
        ApproxTokenSeverity::Normal
    }
}

/// 格式化 token 数为人类可读形式（如 "1.2K tokens"）
pub fn format_approx_token_count(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{:.1}K tokens", tokens as f64 / 1000.0)
    } else {
        format!("{tokens} tokens")
    }
}

/// 格式化数字（如 1234 -> "1,234"）
pub fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, ch);
    }
    result
}

pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// 进程 FD 诊断快照（桩实现）
pub struct ProcessFdDiagnosticSnapshot(pub Vec<(String, u64)>);

impl std::fmt::Display for ProcessFdDiagnosticSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProcessFdDiagnosticSnapshot({} entries)", self.0.len())
    }
}

pub fn process_fd_diagnostic_snapshot() -> ProcessFdDiagnosticSnapshot {
    ProcessFdDiagnosticSnapshot(Vec::new())
}
