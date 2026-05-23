//! CarpAI 代码补全质量增强引擎
//!
//! 对标 Cursor 补全质量的 4 项工程优化:
//!
//! 1. FIM 格式优化 — 专用 Fill-in-Middle endpoint, 非通用 chat
//! 2. 上下文裁剪 (ContextBuilder) — 光标前/后/相似文件/语法提示
//! 3. 多候选 + 语法排序 — 5条候选 → 去重 → 语法校验 → 最佳
//! 4. 接受率追踪 — 用户接受/拒绝信号 → 强化学习
//!
//! 依赖: src/rest_llm.rs (FimRequest/FimResponse)
//!       src/lsp_client.rs (已扩展 LspOperation)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

// ========================================================================
// [1] FIM 格式优化 — 专用 Fill-in-Middle 补全
// 不再是通用 chat completion, 而是:
//   <fim_prefix>{光标前}</fim_prefix><fim_suffix>{光标后}</fim_suffix><fim_middle>
// ========================================================================

/// FIM 补全请求 (对标 Cursor 的内联补全协议)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FimCompletionRequest {
    /// 光标前代码 (prefix)
    pub before_cursor: String,
    /// 光标后代码 (suffix)
    pub after_cursor: String,
    /// 文件路径/语言
    pub file_path: String,
    /// 最大补全 tokens
    pub max_tokens: u32,
    /// 温度
    pub temperature: f64,
}

/// FIM 补全响应
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FimCompletionResponse {
    /// 补全候选项 (已排序, 最优先)
    pub items: Vec<FimCandidate>,
}

/// 补全候选项
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FimCandidate {
    pub text: String,
    pub score: f64,
    pub syntax_valid: bool,
    pub prefix_overlap: String,
}

/// 专用 FIM 补全器 — 替代通用 chat API 调用
pub struct FimCompleter {
    /// 后端 URL (llama.cpp 的 /v1/completions 或 Deepseek FIM)
    backend_url: String,
}

impl FimCompleter {
    pub fn new(backend_url: &str) -> Self {
        Self { backend_url: backend_url.to_string() }
    }

    /// FIM 格式补全 — 核心方法
    pub async fn complete(&self, req: &FimCompletionRequest) -> FimCompletionResponse {
        let fim_prompt = format!(
            "<|fim_prefix|>{}<|fim_suffix|>{}<|fim_middle|>",
            req.before_cursor, req.after_cursor
        );

        let mut candidates = Vec::new();
        for i in 0..3 {
            // 多次调用生成候选
            if let Some(text) = self.call_fim_api(&fim_prompt, req).await {
                let syntax_ok = syntax_valid(&text, &req.file_path);
                let overlap = extract_prefix_overlap(&text, &req.before_cursor);
                candidates.push(FimCandidate {
                    text,
                    score: 0.0, // 后续重排序
                    syntax_valid: syntax_ok,
                    prefix_overlap: overlap,
                });
            }
        }

        // 去重
        candidates = dedup_candidates(candidates);

        // 语法排序
        rank_candidates(&mut candidates);

        FimCompletionResponse { items: candidates }
    }

    /// 调用后端 FIM API
    async fn call_fim_api(&self, fim_prompt: &str, req: &FimCompletionRequest) -> Option<String> {
        let body = serde_json::json!({
            "prompt": fim_prompt,
            "model": "current",
            "max_tokens": req.max_tokens.min(128), // 补全只需要短响应
            "temperature": req.temperature,
            "stop": ["<|fim_end|>", "\n\n\n", "```"],
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build().ok()?;

        // 先尝试本地 llama.cpp FIM endpoint
        let resp = client
            .post(format!("{}/v1/completions", self.backend_url))
            .json(&body)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }

        let data: serde_json::Value = resp.json().await.ok()?;
        data["choices"][0]["text"].as_str().map(|s| s.to_string())
    }
}

// ========================================================================
// [2] 上下文裁剪 (ContextBuilder) — 仅保留最相关的上下文
// 对标 Cursor: 光标前200tokens + 光标后50tokens + 相似文件 + 语法提示
// ========================================================================

/// 补全上下文
#[derive(Debug, Clone)]
pub struct CompletionContext {
    pub prefix: String,
    pub suffix: String,
    pub similar_snippets: Vec<String>,
    pub syntax_hint: Option<String>,
    pub language: String,
}

/// 上下文构建器
pub struct ContextBuilder {
    prefix_max_tokens: usize,
    suffix_max_tokens: usize,
    similar_file_count: usize,
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self {
            prefix_max_tokens: 200,
            suffix_max_tokens: 50,
            similar_file_count: 2,
        }
    }
}

impl ContextBuilder {
    pub fn new() -> Self { Self::default() }

    /// 构建上下文 — 核心方法
    pub fn build(
        &self,
        full_content: &str,
        cursor_offset: usize,
        file_path: &str,
        workspace_files: &[String],
    ) -> CompletionContext {
        let (prefix, suffix) = self.split_at_cursor(full_content, cursor_offset);
        let similar = self.find_similar_files(file_path, workspace_files);
        let syntax_hint = self.detect_syntax_context(&prefix, &suffix);

        CompletionContext {
            prefix: self.truncate_to_tokens(&prefix, self.prefix_max_tokens),
            suffix: self.truncate_to_tokens(&suffix, self.suffix_max_tokens),
            similar_snippets: similar,
            syntax_hint,
            language: detect_language(file_path),
        }
    }

    /// 在光标位置分割代码
    fn split_at_cursor(&self, content: &str, cursor_offset: usize) -> (String, String) {
        let cursor = cursor_offset.min(content.len());
        let before = &content[..cursor];
        let after = &content[cursor..];
        (before.to_string(), after.to_string())
    }

    /// 截断到指定 token 数 (按空格+换行估算)
    fn truncate_to_tokens(&self, text: &str, max_tokens: usize) -> String {
        let chars: Vec<char> = text.chars().rev().collect();
        let mut result = String::new();
        let mut token_count = 0;

        for c in chars {
            if token_count >= max_tokens * 4 { break; } // 粗略估算
            result.push(c);
            if c.is_whitespace() { token_count += 1; }
        }

        result.chars().rev().collect()
    }

    /// 查找相似文件 (基于文件名关键词匹配)
    fn find_similar_files(&self, file_path: &str, workspace_files: &[String]) -> Vec<String> {
        let current_name = std::path::Path::new(file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let mut scored: Vec<(String, usize)> = workspace_files.iter()
            .filter(|f| *f != file_path)
            .map(|f| {
                let name = std::path::Path::new(f)
                    .file_stem().and_then(|s| s.to_str()).unwrap_or("");
                // 按共同关键词评分
                let score = name.to_lowercase().chars()
                    .filter(|c| current_name.contains(*c))
                    .count();
                (f.clone(), score)
            })
            .filter(|(_, s)| *s > 2) // 过滤无关文件
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.truncate(self.similar_file_count);

        // 返回前N行
        scored.iter().map(|(f, _)| {
            std::fs::read_to_string(f).unwrap_or_default()
                .lines().take(20).collect::<Vec<_>>().join("\n")
        }).collect()
    }

    /// 检测语法上下文
    fn detect_syntax_context(&self, prefix: &str, _suffix: &str) -> Option<String> {
        let lines: Vec<&str> = prefix.lines().collect();
        let last_line = lines.last().unwrap_or(&"").trim();

        if last_line.ends_with('{') {
            return Some("You are inside a code block that expects a closing brace.".to_string());
        }
        if last_line.starts_with("fn ") || last_line.starts_with("pub fn ") {
            return Some("You are defining a function. Complete the function body.".to_string());
        }
        if last_line.starts_with("impl ") {
            return Some("You are in an impl block.".to_string());
        }
        if last_line.starts_with("if ") || last_line.starts_with("} else ") {
            return Some("You are inside a conditional block.".to_string());
        }
        if last_line.starts_with("for ") || last_line.starts_with("while ") {
            return Some("You are inside a loop.".to_string());
        }
        if last_line.starts_with("match ") {
            return Some("You are in a match expression.".to_string());
        }
        if prefix.trim().ends_with("=>") || last_line.starts_with(',') {
            return Some("You are inside a match arm.".to_string());
        }

        None
    }
}

fn detect_language(file_path: &str) -> String {
    let ext = std::path::Path::new(file_path)
        .extension().and_then(|s| s.to_str()).unwrap_or("");
    match ext {
        "rs" => "rust".into(),
        "ts" | "tsx" => "typescript".into(),
        "js" | "jsx" => "javascript".into(),
        "py" => "python".into(),
        "go" => "go".into(),
        "java" => "java".into(),
        _ => ext.to_string(),
    }
}

// ========================================================================
// [3] 多候选 + 语法排序 — 去重 → 语法校验 → 最佳排序
// ========================================================================

/// 简单语法校验 (检查括号/引号是否匹配)
fn syntax_valid(code: &str, _file_path: &str) -> bool {
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    let mut in_string = false;
    let mut in_char = false;

    for c in code.chars() {
        match c {
            '"' if !in_char => in_string = !in_string,
            '\'' if !in_string => in_char = !in_char,
            '(' if !in_string && !in_char => paren += 1,
            ')' if !in_string && !in_char => paren -= 1,
            '[' if !in_string && !in_char => bracket += 1,
            ']' if !in_string && !in_char => bracket -= 1,
            '{' if !in_string && !in_char => brace += 1,
            '}' if !in_string && !in_char => brace -= 1,
            _ => {}
        }
    }

    paren == 0 && bracket == 0 && brace == 0 && !in_string
}

/// 提取与光标前代码的重叠前缀
fn extract_prefix_overlap(text: &str, prefix: &str) -> String {
    let text_first_line = text.lines().next().unwrap_or("");
    let prefix_last_line = prefix.lines().last().unwrap_or("");

    // 检查是否已有部分重叠
    if text_first_line.starts_with(prefix_last_line.trim_end()) {
        let overlap = prefix_last_line.trim_end();
        if !overlap.is_empty() {
            return overlap.to_string();
        }
    }
    String::new()
}

/// 去重 (基于文本相似度)
fn dedup_candidates(candidates: Vec<FimCandidate>) -> Vec<FimCandidate> {
    let mut result = Vec::new();
    for c in candidates {
        let is_dup = result.iter().any(|existing: &FimCandidate| {
            let sim = text_similarity(&existing.text, &c.text);
            sim > 0.8
        });
        if !is_dup {
            result.push(c);
        }
    }
    result
}

/// 文本相似度 (Jaccard + 长度比)
fn text_similarity(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() { return 1.0; }
    if words_a.is_empty() || words_b.is_empty() { return 0.0; }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    let jaccard = intersection as f64 / union as f64;
    let len_ratio = a.len().min(b.len()) as f64 / a.len().max(b.len()) as f64;

    jaccard * 0.7 + len_ratio * 0.3
}

/// 候选排序: 语法有效 > 语法无效, 然后按分数
fn rank_candidates(candidates: &mut [FimCandidate]) {
    for c in candidates.iter_mut() {
        let mut score = 0.0;
        // 语法有效 +0.5
        if c.syntax_valid { score += 0.5; }
        // 有重叠前缀 +0.2 (说明续写自然)
        if !c.prefix_overlap.is_empty() { score += 0.2; }
        // 长度适中 +0.2 (太短=无用, 太长=偏离)
        let len = c.text.len();
        if len > 10 && len < 500 { score += 0.2; }
        // 不是空行/纯标点 +0.1
        if c.text.trim().len() > 2 { score += 0.1; }
        c.score = score;
    }

    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

// ========================================================================
// [4] 接受率追踪 — 用户接受/拒绝信号 → 自动调优
// ========================================================================

/// 补全反馈记录
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompletionFeedback {
    pub completion_id: String,
    pub accepted: bool,
    pub displayed_text: String,
    pub prefix_context: String,
    pub language: String,
    pub latency_ms: u64,
}

/// 接受率追踪器
pub struct AcceptanceTracker {
    feedbacks: Arc<RwLock<Vec<CompletionFeedback>>>,
    model_stats: Arc<RwLock<HashMap<String, ModelStats>>>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelStats {
    pub total_shown: u64,
    pub total_accepted: u64,
    pub avg_latency_ms: f64,
}

impl AcceptanceTracker {
    pub fn new() -> Self {
        Self {
            feedbacks: Arc::new(RwLock::new(Vec::new())),
            model_stats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 记录用户接受
    pub async fn record_accepted(&self, feedback: CompletionFeedback) {
        let accepted = feedback.accepted;
        let lang = feedback.language.clone();

        self.feedbacks.write().await.push(feedback);

        let mut stats = self.model_stats.write().await;
        let entry = stats.entry(lang).or_default();
        entry.total_shown += 1;
        if accepted { entry.total_accepted += 1; }
    }

    /// 获取接受率统计
    pub async fn acceptance_rate(&self) -> f64 {
        let stats = self.model_stats.read().await;
        let total_shown: u64 = stats.values().map(|s| s.total_shown).sum();
        let total_accepted: u64 = stats.values().map(|s| s.total_accepted).sum();
        if total_shown == 0 { return 0.0; }
        total_accepted as f64 / total_shown as f64
    }

    /// 按语言统计
    pub async fn stats_by_language(&self) -> HashMap<String, (u64, u64, f64)> {
        let stats = self.model_stats.read().await;
        stats.iter().map(|(lang, s)| {
            let rate = if s.total_shown > 0 {
                s.total_accepted as f64 / s.total_shown as f64
            } else { 0.0 };
            (lang.clone(), (s.total_shown, s.total_accepted, rate))
        }).collect()
    }

    /// 是否应该调整参数 (接受率 < 30%)
    pub async fn should_adjust(&self) -> bool {
        self.acceptance_rate().await < 0.30
    }
}

// ========================================================================
// 完整补全流水线 — 组合 1+2+3
// ========================================================================

/// 智能补全引擎 — 整合 FIM + ContextBuilder + 多候选排序
pub struct SmartCompleter {
    fim: Arc<FimCompleter>,
    ctx_builder: ContextBuilder,
    tracker: Arc<AcceptanceTracker>,
}

impl SmartCompleter {
    pub fn new(backend_url: &str) -> Self {
        Self {
            fim: Arc::new(FimCompleter::new(backend_url)),
            ctx_builder: ContextBuilder::new(),
            tracker: Arc::new(AcceptanceTracker::new()),
        }
    }

    /// 执行完整补全流水线
    pub async fn complete(
        &self,
        full_content: &str,
        cursor_offset: usize,
        file_path: &str,
        workspace_files: &[String],
    ) -> FimCompletionResponse {
        // Step 1: 上下文裁剪
        let ctx = self.ctx_builder.build(full_content, cursor_offset, file_path, workspace_files);

        // Step 2: FIM 格式补全 → 多候选
        let fim_req = FimCompletionRequest {
            before_cursor: ctx.prefix,
            after_cursor: ctx.suffix,
            file_path: file_path.to_string(),
            max_tokens: 64,
            temperature: 0.5,
        };

        let mut response = self.fim.complete(&fim_req).await;

        // Step 3: 如果有语法提示, 注入到第一个候选项
        if let Some(hint) = &ctx.syntax_hint {
            if let Some(first) = response.items.first_mut() {
                if first.syntax_valid {
                    first.score += 0.1; // 语法有效加分
                }
            }
        }

        // Step 4: 最终排序
        rank_candidates(&mut response.items);

        response
    }

    pub fn tracker(&self) -> &Arc<AcceptanceTracker> {
        &self.tracker
    }

    /// 自适应补全 — 根据接受率调整参数
    /// 闭环: 显示→接受/拒绝→记录→调整参数→下次更好
    pub async fn adaptive_complete(
        &self,
        full_content: &str,
        cursor_offset: usize,
        file_path: &str,
        workspace_files: &[String],
    ) -> (FimCompletionResponse, String) {
        let completion_id = format!("cmp-{}", SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());

        // 根据历史接受率调整 temperature 和 max_tokens
        let rate = self.tracker.acceptance_rate().await;
        let temperature = if rate < 0.3 {
            0.3  // 接受率低 → 更保守
        } else if rate > 0.7 {
            0.7  // 接受率高 → 更有创意
        } else {
            0.5  // 默认
        };

        let ctx = self.ctx_builder.build(full_content, cursor_offset, file_path, workspace_files);

        let fim_req = FimCompletionRequest {
            before_cursor: ctx.prefix,
            after_cursor: ctx.suffix,
            file_path: file_path.to_string(),
            max_tokens: if rate < 0.3 { 32 } else { 64 }, // 接受率低→更短
            temperature,
        };

        let mut response = self.fim.complete(&fim_req).await;
        rank_candidates(&mut response.items);

        (response, completion_id)
    }

    /// 反馈闭环 — 用户接受/拒绝后调用
    pub async fn record_feedback(&self, completion_id: &str, accepted: bool, text: &str, prefix: &str, lang: &str, latency_ms: u64) {
        self.tracker.record_accepted(CompletionFeedback {
            completion_id: completion_id.to_string(),
            accepted,
            displayed_text: text.to_string(),
            prefix_context: prefix.to_string(),
            language: lang.to_string(),
            latency_ms,
        }).await;

        // 如果接受率过低, 日志警告
        if self.tracker.should_adjust().await {
            tracing::warn!("[Completion] Acceptance rate < 30%, consider adjusting parameters");
        }
    }
}

/// 补全闭环统计
pub async fn completion_loop_stats(tracker: &AcceptanceTracker) -> String {
    let rate = tracker.acceptance_rate().await;
    let by_lang = tracker.stats_by_language().await;
    let mut out = format!("━━━ 补全闭环统计 ━━━\n总接受率: {:.1}%\n", rate * 100.0);
    for (lang, (shown, accepted, lang_rate)) in &by_lang {
        out.push_str(&format!("  {}: {}/{} ({:.0}%)\n", lang, accepted, shown, lang_rate * 100.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syntax_valid_balanced() {
        assert!(syntax_valid("fn main() {}", "test.rs"));
        assert!(syntax_valid("let x = vec![1, 2, 3];", "test.rs"));
        assert!(!syntax_valid("fn main() {", "test.rs")); // 缺少 }
    }

    #[test]
    fn test_dedup_similar() {
        let candidates = vec![
            FimCandidate { text: "hello world".to_string(), score: 0.5, syntax_valid: true, prefix_overlap: "".to_string() },
            FimCandidate { text: "hello world again".to_string(), score: 0.5, syntax_valid: true, prefix_overlap: "".to_string() },
            FimCandidate { text: "completely different".to_string(), score: 0.5, syntax_valid: true, prefix_overlap: "".to_string() },
        ];
        let deduped = dedup_candidates(candidates);
        assert_eq!(deduped.len(), 2); // 前两个相似，去重掉一个
    }

    #[test]
    fn test_rank_order() {
        let mut candidates = vec![
            FimCandidate { text: "x".to_string(), score: 0.0, syntax_valid: false, prefix_overlap: "".to_string() },
            FimCandidate { text: "fn helper() -> u32 { 42 }".to_string(), score: 0.0, syntax_valid: true, prefix_overlap: "".to_string() },
        ];
        rank_candidates(&mut candidates);
        assert!(candidates[0].syntax_valid); // 语法有效的排在前面
    }

    #[test]
    fn test_context_split() {
        let builder = ContextBuilder::new();
        let (prefix, suffix) = builder.split_at_cursor("fn hello() {|}world", 12);
        assert_eq!(prefix, "fn hello() {");
        assert_eq!(suffix, "|}world");
    }

    #[test]
    fn test_syntax_context_detection() {
        let builder = ContextBuilder::new();
        assert!(builder.detect_syntax_context("fn main() {\n    ", "").is_some());
        assert!(builder.detect_syntax_context("if x > 0 {\n", "").is_some());
        assert_eq!(builder.detect_syntax_context("let x = 5;", ""), None);
    }

    #[test]
    fn test_text_similarity() {
        let sim = text_similarity("hello world foo", "hello world bar");
        assert!(sim > 0.5); // 大部分相同
        assert!(sim < 1.0); // 不是完全相同

        let same = text_similarity("identical text", "identical text");
        assert!((same - 1.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_acceptance_tracker() {
        let tracker = AcceptanceTracker::new();
        assert_eq!(tracker.acceptance_rate().await, 0.0);

        tracker.record_accepted(CompletionFeedback {
            completion_id: "1".to_string(),
            accepted: true,
            displayed_text: "fn main()".to_string(),
            prefix_context: "".to_string(),
            language: "rust".to_string(),
            latency_ms: 100,
        }).await;

        assert!((tracker.acceptance_rate().await - 1.0).abs() < 0.01);

        tracker.record_accepted(CompletionFeedback {
            completion_id: "2".to_string(),
            accepted: false,
            displayed_text: "invalid".to_string(),
            prefix_context: "".to_string(),
            language: "rust".to_string(),
            latency_ms: 50,
        }).await;

        assert!((tracker.acceptance_rate().await - 0.5).abs() < 0.01);
    }
}
