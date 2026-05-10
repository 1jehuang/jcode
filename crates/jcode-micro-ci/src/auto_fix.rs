//! # AutoFixer — 自动修复 Micro-CI 发现的可修复问题
//!
//! 当前支持的修复:
//! - 尾随空格
//! - 缺少末尾换行符
//! - Tab 字符 → 空格
//! - println! → tracing::info! (Rust 文件)
//! - println! in prod → tracing (TypeScript/JavaScript)

use crate::Issue;
use std::path::Path;

/// 修复动作
#[derive(Debug, Clone)]
pub enum FixAction {
    /// 移除尾随空格
    RemoveTrailingWhitespace,
    /// 添加末尾换行符
    AddTrailingNewline,
    /// 将 Tab 替换为空格
    TabToSpaces { spaces_per_tab: usize },
    /// 替换内容 (模式 → 替换)
    ReplaceContent { pattern: String, replacement: String },
}

/// 自动修复器
pub struct AutoFixer {
    enabled: bool,
}

impl AutoFixer {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// 对报告中的可修复问题执行自动修复
    /// 返回实际修复的数量
    pub async fn apply_fixes(&self, issues: &mut [Issue], workspace_root: &str) -> usize {
        if !self.enabled {
            return 0;
        }

        let mut fix_count = 0;

        for issue in issues.iter_mut() {
            if issue.fix_suggestion.is_none() {
                continue;
            }

            let fix_applied = match issue.phase.as_str() {
                "ast" => self.fix_ast_issue(issue, workspace_root).await,
                _ => false,
            };

            if fix_applied {
                fix_count += 1;
                issue.fix_suggestion = Some(format!("{} (已自动修复)", issue.fix_suggestion.as_deref().unwrap_or("")));
            }
        }

        fix_count
    }

    async fn fix_ast_issue(&self, issue: &Issue, workspace_root: &str) -> bool {
        let file = match &issue.file {
            Some(f) => f.clone(),
            None => return false,
        };
        let full_path = Path::new(workspace_root).join(&file);

        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => return false,
        };

        let message = issue.message.to_lowercase();
        let new_content = if message.contains("trailing whitespace") {
            Some(self.fix_trailing_whitespace(&content))
        } else if message.contains("missing trailing newline") {
            Some(self.fix_trailing_newline(&content))
        } else if message.contains("tab characters") {
            Some(self.fix_tabs_to_spaces(&content, 4))
        } else if message.contains("println!") && file.ends_with(".rs") {
            Some(self.fix_println_to_tracing(&content))
        } else {
            None
        };

        match new_content {
            Some(c) if c != content => {
                std::fs::write(&full_path, &c).is_ok()
            }
            _ => false,
        }
    }

    fn fix_trailing_whitespace(&self, content: &str) -> String {
        content
            .lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            + if content.ends_with('\n') { "\n" } else { "" }
    }

    fn fix_trailing_newline(&self, content: &str) -> String {
        if content.ends_with('\n') {
            content.to_string()
        } else {
            format!("{}\n", content)
        }
    }

    fn fix_tabs_to_spaces(&self, content: &str, spaces_per_tab: usize) -> String {
        let spaces = " ".repeat(spaces_per_tab);
        content.replace('\t', &spaces)
    }

    /// 将 println! 替换为 tracing::info! (仅 Rust 文件)
    fn fix_println_to_tracing(&self, content: &str) -> String {
        content
            .replace("println!(\"", "tracing::info!(\"")
            .replace("println!(\"{}\"", "tracing::info!(\"{}\"")
    }
}
