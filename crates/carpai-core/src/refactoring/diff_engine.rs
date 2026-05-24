//! # Diff 渲染引擎
//! 结构化 patch, unified diff, 词级 diff


/// Diff 操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffOp { Equal, Insert, Delete, Replace }

/// 单个 diff hunk
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: usize, pub old_lines: usize,
    pub new_start: usize, pub new_lines: usize,
    pub lines: Vec<String>,
}

/// 结构化 patch
#[derive(Debug, Clone)]
pub struct StructuredPatch {
    pub hunks: Vec<DiffHunk>,
    pub old_path: String, pub new_path: String,
}

/// 生成结构化 patch
pub fn generate_patch(old_content: &str, new_content: &str, file_path: &str) -> StructuredPatch {
    let _old_lines: Vec<&str> = old_content.lines().collect();
    let _new_lines: Vec<&str> = new_content.lines().collect();

    // 使用 similar crate 计算 diff
    let diff = similar::TextDiff::from_lines(old_content, new_content);
    let mut hunks = Vec::new();
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut current: Option<DiffHunk> = None;
    let mut ctx_before: Vec<String> = Vec::new();

    for change in diff.iter_all_changes() {
        let tag = change.tag();
        let val = change.value().to_string();
        match tag {
            similar::ChangeTag::Equal => {
                if let Some(ref mut h) = current {
                    h.lines.push(format!(" {}", val.trim_end()));
                    h.old_lines += 1; h.new_lines += 1;
                } else {
                    ctx_before.push(format!(" {}", val.trim_end()));
                    if ctx_before.len() > 3 { ctx_before.remove(0); }
                }
                old_line += 1; new_line += 1;
            }
            similar::ChangeTag::Delete => {
                if current.is_none() {
                    let start = old_line.saturating_sub(ctx_before.len().min(3));
                    current = Some(DiffHunk { old_start: start, old_lines: 0, new_start: new_line.saturating_sub(ctx_before.len().min(3)), new_lines: 0, lines: std::mem::take(&mut ctx_before) });
                }
                if let Some(ref mut h) = current { h.lines.push(format!("-{}", val.trim_end())); h.old_lines += 1; }
                old_line += 1;
            }
            similar::ChangeTag::Insert => {
                if current.is_none() {
                    let start = old_line.saturating_sub(ctx_before.len().min(3));
                    current = Some(DiffHunk { old_start: start, old_lines: 0, new_start: new_line.saturating_sub(ctx_before.len().min(3)), new_lines: 0, lines: std::mem::take(&mut ctx_before) });
                }
                if let Some(ref mut h) = current { h.lines.push(format!("+{}", val.trim_end())); h.new_lines += 1; }
                new_line += 1;
            }
        }
        if let Some(h) = current.take()
            && (h.old_lines > 0 || h.new_lines > 0) { hunks.push(h); }
    }
    if let Some(h) = current.take() && (h.old_lines > 0 || h.new_lines > 0) { hunks.push(h); }

    StructuredPatch { hunks, old_path: file_path.to_string(), new_path: file_path.to_string() }
}

/// 渲染 unified diff 格式
pub fn render_unified(patch: &StructuredPatch) -> String {
    let mut out = format!("--- {}\n+++ {}\n", patch.old_path, patch.new_path);
    for hunk in &patch.hunks {
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", hunk.old_start, hunk.old_lines.max(1), hunk.new_start, hunk.new_lines.max(1)));
        for line in &hunk.lines { out.push_str(line); out.push('\n'); }
    }
    out
}

/// 统计变更行数
#[derive(Debug, Clone, Default)]
pub struct DiffStats { pub added: usize, pub removed: usize, pub files_changed: usize }

pub fn count_changes(patch: &StructuredPatch) -> DiffStats {
    let mut stats = DiffStats::default();
    stats.files_changed = 1;
    for hunk in &patch.hunks {
        for line in &hunk.lines {
            if line.starts_with('+') && !line.starts_with("+++") { stats.added += 1; }
            if line.starts_with('-') && !line.starts_with("---") { stats.removed += 1; }
        }
    }
    stats
}

/// 词级 diff 高亮
#[derive(Debug, Clone)]
pub struct WordDiff { pub word: String, pub tag: DiffOp }

pub fn word_diff(old_text: &str, new_text: &str) -> Vec<WordDiff> {
    let mut result = Vec::new();
    let diff = similar::TextDiff::from_words(old_text, new_text);
    for change in diff.iter_all_changes() {
        result.push(WordDiff { word: change.value().to_string(), tag: match change.tag() { similar::ChangeTag::Equal => DiffOp::Equal, similar::ChangeTag::Delete => DiffOp::Delete, similar::ChangeTag::Insert => DiffOp::Insert } });
    }
    result
}
