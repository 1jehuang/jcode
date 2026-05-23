//! # Diff-Aware 文件编辑 — 借鉴 Claude Code 的 FileEditTool 算法
//!
//! 与 `edit.rs` 现有 `EditTool` 互补，提供更智能的 diff 感知匹配策略：
//!
//! 1. 精确匹配 → 2. 空白归一化匹配 → 3. 行尾归一化匹配 → 4. 上下文辅助匹配
//!
//! 匹配策略源自 Claude Code 的 FileEditTool 实现。

use similar::{ChangeTag, TextDiff};
use std::path::Path;

/// 匹配策略级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchStrategy {
    /// 精确匹配（默认）
    Exact,
    /// 去除两端空白后匹配
    Trimmed,
    /// 行尾归一化（忽略 CRLF/LF 差异）
    NormalizeLineEndings,
    /// 仅匹配行内容（忽略缩进差异）
    ContentOnly,
}

/// Diff 匹配结果
#[derive(Debug)]
pub struct DiffMatchResult {
    /// 是否匹配成功
    pub matched: bool,
    /// 使用的匹配策略
    pub strategy: MatchStrategy,
    /// 匹配位置（字节偏移）
    pub position: Option<usize>,
    /// 匹配行号
    pub line: Option<usize>,
    /// 差异详情（用于 LLM 展示）
    pub diff_summary: Option<String>,
}

/// 对文件内容执行 diff-aware 字符串匹配
///
/// 依次尝试：
/// 1. `old_str` 精确匹配
/// 2. 两端 trim 后匹配
/// 3. 行尾归一化（CRLF → LF）后匹配
/// 4. 行内容匹配（去缩进 + 去行尾空白）
pub fn find_match(content: &str, old_str: &str) -> DiffMatchResult {
    // 策略 1: 精确匹配
    if let Some(pos) = content.find(old_str) {
        let line = content[..pos].matches('\n').count() + 1;
        return DiffMatchResult {
            matched: true,
            strategy: MatchStrategy::Exact,
            position: Some(pos),
            line: Some(line),
            diff_summary: None,
        };
    }

    // 策略 2: 去除两端空白
    let trimmed = old_str.trim();
    if let Some(pos) = content.find(trimmed) {
        let line = content[..pos].matches('\n').count() + 1;
        return DiffMatchResult {
            matched: true,
            strategy: MatchStrategy::Trimmed,
            position: Some(pos),
            line: Some(line),
            diff_summary: Some("使用了去两端空白匹配".into()),
        };
    }

    // 策略 3: 行尾归一化
    let normalized_content = content.replace("\r\n", "\n");
    let normalized_old = old_str.replace("\r\n", "\n");
    if let Some(pos) = normalized_content.find(&normalized_old) {
        let line = normalized_content[..pos].matches('\n').count() + 1;
        return DiffMatchResult {
            matched: true,
            strategy: MatchStrategy::NormalizeLineEndings,
            position: Some(pos),
            line: Some(line),
            diff_summary: Some("行尾归一化后匹配成功（CRLF → LF）".into()),
        };
    }

    // 策略 4: 尝试跨行模糊匹配
    // 计算行级 diff，找到 old_str 中最有特征的行的近似位置
    let old_lines: Vec<&str> = old_str.lines().collect();
    let content_lines: Vec<&str> = content.lines().collect();
    let diff = TextDiff::from_lines(content, old_str);

    // 检查是否有足够多的行匹配
    let mut matched_changes = 0;
    let mut total_changes = 0;
    for change in diff.iter_all_changes() {
        total_changes += 1;
        match change.tag() {
            ChangeTag::Equal => {
                matched_changes += 1;
            }
            _ => {}
        }
    }

    // 如果超过 60% 的行内容匹配，认定为模糊匹配成功
    if total_changes > 0 && (matched_changes as f64 / total_changes as f64) > 0.6 {
        // 尝试找到最佳的匹配位置 — 取第一行匹配内容的位置
        if let Some(first_line) = old_lines.first() {
            let trimmed_first = first_line.trim();
            for (i, content_line) in content_lines.iter().enumerate() {
                if content_line.trim() == trimmed_first {
                    let pos = content[..content_line.as_ptr() as usize - content.as_ptr() as usize].len();
                    return DiffMatchResult {
                        matched: true,
                        strategy: MatchStrategy::ContentOnly,
                        position: Some(pos),
                        line: Some(i + 1),
                        diff_summary: Some(format!(
                            "模糊匹配（{:.0}% 行内容匹配）",
                            (matched_changes as f64 / total_changes as f64) * 100.0
                        )),
                    };
                }
            }
        }
    }

    // 所有策略失败
    // 生成 diff 摘要帮助 LLM 调试
    let diff_summary = generate_diff_summary(content, old_str);

    DiffMatchResult {
        matched: false,
        strategy: MatchStrategy::Exact,
        position: None,
        line: None,
        diff_summary: Some(diff_summary),
    }
}

/// 生成文件内容与 old_str 之间的差异摘要（供 LLM 调试）
fn generate_diff_summary(content: &str, old_str: &str) -> String {
    let diff = TextDiff::from_lines(content, old_str);
    let mut summary = String::from("精确匹配失败。差异摘要:\n");

    let mut context_lines = Vec::new();
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                if context_lines.len() < 3 {
                    context_lines.push(format!("  {}", change.value().trim_end()));
                }
            }
            ChangeTag::Delete => {
                if !summary.ends_with('\n') {
                    summary.push('\n');
                }
                summary.push_str(&format!("- {}", change.value().trim_end()));
                summary.push('\n');
                context_lines.clear();
            }
            ChangeTag::Insert => {
                if !summary.ends_with('\n') {
                    summary.push('\n');
                }
                summary.push_str(&format!("+ {}", change.value().trim_end()));
                summary.push('\n');
                context_lines.clear();
            }
        }
    }

    if summary.lines().count() <= 3 {
        summary.push_str("(未检测到相似内容 — 请检查 file_path 或 old_string 是否正确)");
    }

    summary.truncate(1000); // 限制长度
    summary
}

/// 验证替换后文件格式是否合理
pub fn validate_replacement(content: &str, old_str: &str, new_str: &str) -> Vec<String> {
    let mut warnings = Vec::new();

    let after = content.replacen(old_str, new_str, 1);

    // 检查括号平衡
    let open_braces = after.matches('{').count();
    let close_braces = after.matches('}').count();
    if open_braces != close_braces {
        warnings.push(format!(
            "花括号不平衡: {0} 个 '{{' vs {1} 个 '}}'",
            open_braces, close_braces
        ));
    }

    let open_parens = after.matches('(').count();
    let close_parens = after.matches(')').count();
    if open_parens != close_parens {
        warnings.push(format!(
            "括号不平衡: {} 个 '(' vs {} 个 ')'",
            open_parens, close_parens
        ));
    }

    // 检查是否引入了 tabs（如果原文件没有 tabs）
    if !content.contains('\t') && new_str.contains('\t') {
        warnings.push("替换内容包含制表符，但原文件使用空格缩进".to_string());
    }

    // 检查最后是否有空行
    if !after.ends_with('\n') && content.ends_with('\n') {
        warnings.push("文件末尾缺少换行符".to_string());
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let content = "fn hello() {\n    println!(\"Hello\");\n}";
        let result = find_match(content, "println!(\"Hello\")");
        assert!(result.matched);
        assert_eq!(result.strategy, MatchStrategy::Exact);
    }

    #[test]
    fn test_trimmed_match() {
        let content = "fn hello() {\n    println!(\"Hello\");\n}";
        let result = find_match(content, "  println!(\"Hello\")  ");
        assert!(result.matched);
        assert_eq!(result.strategy, MatchStrategy::Trimmed);
    }

    #[test]
    fn test_no_match() {
        let content = "fn hello() { }";
        let result = find_match(content, "fn goodbye()");
        assert!(!result.matched);
    }

    #[test]
    fn test_brace_validation() {
        let content = "fn f() { let x = 1; }";
        let warnings = validate_replacement(content, "let x = 1;", "let x = 2;");
        assert!(warnings.is_empty());

        let warnings = validate_replacement(content, "let x = 1;", "let x = { 2;");
        assert!(!warnings.is_empty());
    }
}
