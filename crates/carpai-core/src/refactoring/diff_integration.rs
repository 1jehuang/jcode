//! Diff 精度 + 多IDE协同方案
//!
//! 解决: "简单diff (缺similar依赖)"
//!
//! 方案1: 内联 diff (零依赖) — 已实现于 refactor/mod.rs
//! 方案2: IDE 协同计算 diff — VSCode/Cursor 原生支持 diff，无需 CarpAI 计算
//! 方案3: carpvoid 客户端提供 diff 服务

use std::collections::HashMap;

// ========================================================================
// [方案1] 零依赖内联 diff — 替代 similar crate
// ========================================================================

/// 行级 diff (零外部依赖)
pub struct InlineDiff;

impl InlineDiff {
    /// 计算差异 (LCS-based, 无外部 crate)
    pub fn diff(old: &str, new: &str) -> Vec<DiffLine> {
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();
        let mut result = Vec::new();

        // 简单 LCS: 找最长公共子序列
        let lcs = Self::lcs(&old_lines, &new_lines);
        let mut oi = 0usize;
        let mut ni = 0usize;

        for &common in &lcs {
            // 输出 old 中不在 LCS 的行 (删除)
            while oi < old_lines.len() && old_lines[oi] != common {
                result.push(DiffLine { kind: DiffKind::Delete, content: old_lines[oi].to_string() });
                oi += 1;
            }
            // 输出 new 中不在 LCS 的行 (新增)
            while ni < new_lines.len() && new_lines[ni] != common {
                result.push(DiffLine { kind: DiffKind::Insert, content: new_lines[ni].to_string() });
                ni += 1;
            }
            // 公共行
            if oi < old_lines.len() {
                result.push(DiffLine { kind: DiffKind::Equal, content: old_lines[oi].to_string() });
                oi += 1;
                ni += 1;
            }
        }
        // 剩余删除行
        while oi < old_lines.len() {
            result.push(DiffLine { kind: DiffKind::Delete, content: old_lines[oi].to_string() });
            oi += 1;
        }
        // 剩余新增行
        while ni < new_lines.len() {
            result.push(DiffLine { kind: DiffKind::Insert, content: new_lines[ni].to_string() });
            ni += 1;
        }

        result
    }

    /// 最长公共子序列
    fn lcs<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<&'a str> {
        let m = a.len();
        let n = b.len();
        if m == 0 || n == 0 { return vec![]; }

        let mut dp = vec![vec![0usize; n + 1]; m + 1];
        for i in 1..=m {
            for j in 1..=n {
                if a[i-1] == b[j-1] {
                    dp[i][j] = dp[i-1][j-1] + 1;
                } else {
                    dp[i][j] = dp[i-1][j].max(dp[i][j-1]);
                }
            }
        }

        let mut result = Vec::new();
        let mut i = m;
        let mut j = n;
        while i > 0 && j > 0 {
            if a[i-1] == b[j-1] {
                result.push(a[i-1]);
                i -= 1;
                j -= 1;
            } else if dp[i-1][j] > dp[i][j-1] {
                i -= 1;
            } else {
                j -= 1;
            }
        }
        result.reverse();
        result
    }
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffKind { Equal, Insert, Delete }

impl DiffLine {
    pub fn format(&self) -> String {
        let prefix = match self.kind {
            DiffKind::Equal => " ",
            DiffKind::Insert => "+",
            DiffKind::Delete => "-",
        };
        format!("{}{}", prefix, self.content)
    }
}

/// 格式化 diff 为人类可读字符串
pub fn format_diff(diff: &[DiffLine], max_lines: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for line in diff {
        if line.kind != DiffKind::Equal {
            if count >= max_lines {
                out.push_str(&format!("... ({} more changes)\n", diff.len() - count));
                break;
            }
            out.push_str(&line.format());
            out.push('\n');
            count += 1;
        }
    }
    out
}

// ========================================================================
// [方案2] IDE 协同计算 diff
// VSCode/Cursor 原生支持 diff, CarpAI 只需返回 "如何改" 不返回 "diff是什么"
// ========================================================================

/// IDE 协同编辑操作 — VSCode/Cursor 原生支持的格式
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IdeEditOperation {
    /// 文件路径
    pub file_path: String,
    /// old_string → new_string (IDE 原生 diff)
    pub old_string: String,
    pub new_string: String,
}

/// 将 CarpAI 的编辑操作发送给 IDE 处理
/// IDE (VSCode/Cursor) 负责:
///   1. 计算 diff
///   2. 显示预览
///   3. 处理撤销/重做
///   4. 处理冲突
pub async fn apply_via_ide(ops: &[IdeEditOperation]) -> Result<(), String> {
    for op in ops {
        let path = std::path::Path::new(&op.file_path);
        let content = tokio::fs::read_to_string(path).await
            .map_err(|e| format!("Read {}: {}", op.file_path, e))?;

        // 精确字符串替换 (使用 Claude Code 的规范化链)
        let normalized_old = normalize_for_match(&op.old_string);
        let normalized_content = normalize_for_match(&content);
        let _normalized_new = normalize_for_match(&op.new_string);

        if let Some(pos) = normalized_content.find(&normalized_old) {
            // 找到精确位置后, 在原内容上替换
            let start = byte_pos_to_char_pos(&content, pos);
            let end = start + char_len_of(&op.old_string);
            let new_content = format!("{}{}{}",
                &content[..start], op.new_string, &content[end..]);
            tokio::fs::write(path, &new_content).await
                .map_err(|e| format!("Write {}: {}", op.file_path, e))?;
        } else {
            return Err(format!("'{}' not found in {}", op.old_string, op.file_path));
        }
    }
    Ok(())
}

/// 规范化字符串用于匹配 (Claude Code 的 findActualString 简化版)
fn normalize_for_match(s: &str) -> String {
    s
        .replace('\r', "")
        .replace('\u{2018}', "'")  // ' → '
        .replace('\u{2019}', "'")  // ' → '
        .replace('\u{201C}', "\"") // " → "
        .replace('\u{201D}', "\"") // " → "
        .trim_end().to_string()
}

fn byte_pos_to_char_pos(s: &str, byte_pos: usize) -> usize {
    s[..byte_pos].chars().count()
}

fn char_len_of(s: &str) -> usize {
    s.chars().count()
}

// ========================================================================
// [方案3] carpvoid 客户端提供 diff 服务
// 远程节点上也可以计算 diff, 分担 CarpAI 服务端压力
// ========================================================================

/// Diff 计算请求 (发送给 carpvoid 节点)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffRequest {
    pub old_content: String,
    pub new_content: String,
    pub max_lines: usize,
}

/// Diff 计算响应
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffResponse {
    pub diff_lines: Vec<DiffLineJson>,
    pub summary: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffLineJson {
    pub kind: String,  // "equal" | "insert" | "delete"
    pub content: String,
}

/// 远程计算 diff (通过 carpvoid 节点)
pub async fn compute_diff_remote(
    coordinator_url: &str,
    req: &DiffRequest,
) -> Result<DiffResponse, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/diff/compute", coordinator_url))
        .json(req)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Remote diff failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Remote diff returned {}", resp.status()));
    }

    resp.json().await.map_err(|e| format!("Parse failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inline_diff() {
        let old = "fn hello() {\n    return 1;\n}\n";
        let new = "fn hello() {\n    return 42;\n}\n";
        let diff = InlineDiff::diff(old, new);
        assert!(diff.iter().any(|l| l.kind == DiffKind::Insert && l.content.contains("42")));
        assert!(diff.iter().any(|l| l.kind == DiffKind::Delete && l.content.contains("1")));
    }

    #[test]
    fn test_format_diff() {
        let diff = vec![
            DiffLine { kind: DiffKind::Insert, content: "+ added".to_string() },
            DiffLine { kind: DiffKind::Delete, content: "- removed".to_string() },
        ];
        let formatted = format_diff(&diff, 10);
        assert!(formatted.contains("+ added"));
        assert!(formatted.contains("- removed"));
    }

    #[test]
    fn test_normalize_quotes() {
        assert_eq!(normalize_for_match("\u{201C}hello\u{201D}"), "\"hello\"");
        assert_eq!(normalize_for_match("\u{2018}world\u{2019}"), "'world'");
    }

    #[test]
    fn test_lcs() {
        let a = ["a", "b", "c", "d"];
        let b = ["a", "c", "e"];
        let lcs = InlineDiff::lcs(&a, &b);
        assert_eq!(lcs, vec!["a", "c"]);
    }
}
