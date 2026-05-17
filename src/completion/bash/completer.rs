//! # 智能补全引擎
//!
//! 提供上下文感知的命令行补全功能：
//! - **上下文分析** - 理解当前输入状态
//! - **智能排序** - 基于使用频率和上下文排序
//! - **模糊匹配** - 支持拼写容错
//! - **多源聚合** - 整合多个补全来源

use crate::completion::bash::{
    CommandRegistry, CompletionSuggestion, CompletionKind,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 补全请求
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    /// 当前输入的文本
    input: String,
    /// 光标位置
    cursor_position: usize,
    /// 工作目录
    working_dir: Option<String>,
    /// 环境变量
    env_vars: HashMap<String, String>,
}

impl CompletionRequest {
    fn new(input: impl Into<String>, cursor_position: usize) -> Self {
        Self {
            input: input.into(),
            cursor_position,
            working_dir: None,
            env_vars: HashMap::new(),
        }

    }

    fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// 获取光标前的单词（用于补全）
    fn word_before_cursor(&self) -> &str {
        let end = self.cursor_position.min(self.input.len());
        let start = self.input[..end]
            .rfind(|c: char| c.is_whitespace())
            .map(|p| p + 1)
            .unwrap_or(0);
        &self.input[start..end]
    }
}

/// 补全结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResult {
    /// 补全建议列表
    suggestions: Vec<CompletionSuggestion>,
    /// 原始请求的前缀
    prefix: String,
    /// 是否有更多结果（分页）
    has_more: bool,
}

impl CompletionResult {
    fn empty(prefix: String) -> Self {
        Self {
            suggestions: Vec::new(),
            prefix,
            has_more: false,
        }
    }

    fn new(suggestions: Vec<CompletionSuggestion>, prefix: String) -> Self {
        let has_more = suggestions.len() >= 20;
        Self {
            suggestions,
            prefix,
            has_more,
        }
    }
}

/// 智能补全器
pub struct SmartCompleter {
    registry: CommandRegistry,
    history: Vec<String>,
    frequent_commands: HashMap<String, u32>,
}

impl SmartCompleter {
    fn new(registry: CommandRegistry) -> Self {
        Self {
            registry,
            history: Vec::new(),
            frequent_commands: HashMap::new(),
        }
    }

    /// 执行智能补全
    fn complete(&self, request: &CompletionRequest) -> CompletionResult {
        let word = request.word_before_cursor().to_string();
        if word.is_empty() {
            return CompletionResult::empty(word);
        }

        let mut suggestions = Vec::new();

        // 1. 命令名补全
        if let Some(cmds) = self.registry.get_command_suggestions(&word) {
            suggestions.extend(cmds);
        }

        // 2. 子命令补全（如果已有命令名）
        let parts: Vec<&str> = request.input[..request.cursor_position].split_whitespace().collect();
        if parts.len() >= 1 {
            if let Some(subcmds) = self.registry.get_subcommand_suggestions(parts[0], &word) {
                suggestions.extend(subcmds);
            }
        }

        // 3. 历史匹配
        for hist in &self.history {
            if hist.starts_with(&word) && !suggestions.iter().any(|s| s.text == *hist) {
                suggestions.push(CompletionSuggestion {
                    text: hist.clone(),
                    display_text: format!("{} (history)", hist),
                    description: "历史命令".to_string(),
                    kind: CompletionKind::Command,
                    priority: 50,
                    metadata: HashMap::new(),
                });
            }
        }

        // 排序：优先级高的在前
        suggestions.sort_by(|a, b| b.priority.cmp(&a.priority));
        suggestions.dedup_by(|a, b| a.text == b.text);

        CompletionResult::new(suggestions, word)
    }

    /// 记录命令到历史
    fn record_command(&mut self, command: &str) {
        self.history.push(command.to_string());
        *self.frequent_commands.entry(command.to_string()).or_insert(0) += 1;

        // 保持历史记录在合理范围
        if self.history.len() > 1000 {
            self.history = self.history.split_off(self.history.len() - 1000);
        }
    }

    /// 获取常用命令建议
    fn get_frequent_suggestions(&self, limit: usize) -> Vec<CompletionSuggestion> {
        let mut freq: Vec<_> = self.frequent_commands.iter().collect();
        freq.sort_by(|a, b| b.1.cmp(a.1));

        freq.into_iter()
            .take(limit)
            .map(|(cmd, count)| CompletionSuggestion {
                text: cmd.clone(),
                display_text: format!("{} ({} times)", cmd, count),
                description: "常用命令".to_string(),
                kind: CompletionKind::Command,
                priority: (*count).min(100) as u8,
                metadata: HashMap::new(),
            })
            .collect()
    }
}

impl Default for SmartCompleter {
    fn default() -> Self {
        Self::new(CommandRegistry::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_request_word_before_cursor() {
        let req = CompletionRequest::new("git che", 7);
        assert_eq!(req.word_before_cursor(), "che");
    }

    #[test]
    fn test_completion_result_empty() {
        let result = CompletionResult::empty("test".into());
        assert!(result.suggestions.is_empty());
        assert_eq!(result.prefix, "test");
    }
}
