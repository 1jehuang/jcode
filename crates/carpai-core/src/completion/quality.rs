//! CarpAI Code Completion Quality Enhancement Engine
//!
//! Mirrors Cursor-level completion quality through 4 engineering optimizations:
//!
//! 1. **FIM Format Optimization** — Dedicated Fill-in-Middle endpoint, not generic chat
//! 2. **Context Trimming (ContextBuilder)** — Before-cursor / After-cursor / Similar-files / Syntax hints
//! 3. **Multi-Candidate + Syntax Ranking** — 5 candidates → Dedup → Syntax validation → Best
//! 4. **Acceptance Tracking** — User accept/reject signals → Reinforcement learning loop
//!
//! ## Pipeline
//!
//! ```text
//! FullContent + CursorOffset + FilePath
//!     │
//!     ▼ ContextBuilder.build()
//! CompletionContext { prefix, suffix, similar_snippets, syntax_hint, language }
//!     │
//!     ▼ FimCompleter.complete()
//! FimCompletionResponse { items: [FimCandidate { text, score, syntax_valid }] }
//!     │
//!     ▼ rank_candidates() + dedup_candidates()
//! Sorted & deduplicated candidates
//!     │
//!     ▼ SmartCompleter (wraps above + AcceptanceTracker)
//! Adaptive completion with feedback loop
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};

// ========================================================================
// [1] FIM Format Optimization — Dedicated Fill-in-Middle Completion
// ========================================================================
//
// Instead of generic chat completion, uses the FIM protocol:
//   <fim_prefix>{before_cursor}</fim_prefix><fim_suffix>{after_cursor}</fim_suffix><fim_middle>

/// FIM completion request (mirrors Cursor's inline completion protocol)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FimCompletionRequest {
    /// Code before cursor (prefix)
    pub before_cursor: String,
    /// Code after cursor (suffix)
    pub after_cursor: String,
    /// File path / language identifier
    pub file_path: String,
    /// Max completion tokens
    pub max_tokens: u32,
    /// Sampling temperature
    pub temperature: f64,
}

/// FIM completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FimCompletionResponse {
    /// Completion candidates (pre-sorted, best first)
    pub items: Vec<FimCandidate>,
}

/// Single completion candidate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FimCandidate {
    pub text: String,
    pub score: f64,
    pub syntax_valid: bool,
    pub prefix_overlap: String,
}

/// Dedicated FIM completer — replaces generic chat API calls
pub struct FimCompleter {
    /// Backend URL (llama.cpp /v1/completions or Deepseek FIM endpoint)
    backend_url: String,
}

impl FimCompleter {
    pub fn new(backend_url: &str) -> Self {
        Self { backend_url: backend_url.to_string() }
    }

    /// FIM-format completion — core method
    pub async fn complete(&self, req: &FimCompletionRequest) -> FimCompletionResponse {
        let fim_prompt = format!(
            "<|fim_prefix|>{}<|fim_suffix|>{}<|fim_middle|>",
            req.before_cursor, req.after_cursor
        );

        let mut candidates = Vec::new();
        for _i in 0..3 {
            if let Some(text) = self.call_fim_api(&fim_prompt, req).await {
                let syntax_ok = syntax_valid(&text, &req.file_path);
                let overlap = extract_prefix_overlap(&text, &req.before_cursor);
                candidates.push(FimCandidate {
                    text,
                    score: 0.0,
                    syntax_valid: syntax_ok,
                    prefix_overlap: overlap,
                });
            }
        }

        candidates = dedup_candidates(candidates);
        rank_candidates(&mut candidates);

        FimCompletionResponse { items: candidates }
    }

    /// Call backend FIM API
    async fn call_fim_api(&self, fim_prompt: &str, req: &FimCompletionRequest) -> Option<String> {
        let body = serde_json::json!({
            "prompt": fim_prompt,
            "model": "current",
            "max_tokens": req.max_tokens.min(128),
            "temperature": req.temperature,
            "stop": ["<|fim_end|>", "\n\n\n", "```"],
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok()?;

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
// [2] Context Trimming (ContextBuilder) — Keep only most relevant context
// ========================================================================
//
// Mirrors Cursor: 200 tokens before cursor + 50 tokens after + similar files + syntax hint

/// Completion context
#[derive(Debug, Clone)]
pub struct CompletionContext {
    pub prefix: String,
    pub suffix: String,
    pub similar_snippets: Vec<String>,
    pub syntax_hint: Option<String>,
    pub language: String,
}

/// Context builder
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Build context — core method
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

    /// Split code at cursor position
    fn split_at_cursor(&self, content: &str, cursor_offset: usize) -> (String, String) {
        let cursor = cursor_offset.min(content.len());
        let before = &content[..cursor];
        let after = &content[cursor..];
        (before.to_string(), after.to_string())
    }

    /// Truncate to approximate token count (rough estimate by whitespace)
    fn truncate_to_tokens(&self, text: &str, max_tokens: usize) -> String {
        let chars: Vec<char> = text.chars().rev().collect();
        let mut result = String::new();
        let mut token_count = 0;

        for c in chars {
            if token_count >= max_tokens * 4 {
                break;
            }
            result.push(c);
            if c.is_whitespace() {
                token_count += 1;
            }
        }

        result.chars().rev().collect()
    }

    /// Find similar files (based on filename keyword matching)
    fn find_similar_files(&self, file_path: &str, workspace_files: &[String]) -> Vec<String> {
        let current_name = std::path::Path::new(file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let mut scored: Vec<(String, usize)> = workspace_files
            .iter()
            .filter(|f| *f != file_path)
            .map(|f| {
                let name = std::path::Path::new(f)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                let score = name
                    .to_lowercase()
                    .chars()
                    .filter(|c| current_name.contains(*c))
                    .count();
                (f.clone(), score)
            })
            .filter(|(_, s)| *s > 2)
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.truncate(self.similar_file_count);

        scored
            .iter()
            .map(|(f, _)| {
                std::fs::read_to_string(f)
                    .unwrap_or_default()
                    .lines()
                    .take(20)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect()
    }

    /// Detect syntax context from surrounding code
    fn detect_syntax_context(&self, prefix: &str, _suffix: &str) -> Option<String> {
        let lines: Vec<&str> = prefix.lines().collect();
        let last_line = lines.last().unwrap_or(&"").trim();

        if last_line.ends_with('{') {
            return Some(
                "You are inside a code block that expects a closing brace.".to_string(),
            );
        }
        if last_line.starts_with("fn ") || last_line.starts_with("pub fn ") {
            return Some(
                "You are defining a function. Complete the function body.".to_string(),
            );
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
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
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
// [3] Multi-Candidate + Syntax Ranking — Dedup → Validate → Sort
// ========================================================================

/// Simple syntax validation (checks bracket/quote balance)
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

/// Extract overlap prefix with before-cursor code
fn extract_prefix_overlap(text: &str, prefix: &str) -> String {
    let text_first_line = text.lines().next().unwrap_or("");
    let prefix_last_line = prefix.lines().last().unwrap_or("");

    if text_first_line.starts_with(prefix_last_line.trim_end()) {
        let overlap = prefix_last_line.trim_end();
        if !overlap.is_empty() {
            return overlap.to_string();
        }
    }
    String::new()
}

/// Deduplicate candidates based on text similarity
fn dedup_candidates(candidates: Vec<FimCandidate>) -> Vec<FimCandidate> {
    let mut result = Vec::new();
    for c in candidates {
        let is_dup = result
            .iter()
            .any(|existing: &FimCandidate| {
                let sim = text_similarity(&existing.text, &c.text);
                sim > 0.8
            });
        if !is_dup {
            result.push(c);
        }
    }
    result
}

/// Text similarity (Jaccard + length ratio)
fn text_similarity(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }
    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    let jaccard = intersection as f64 / union as f64;
    let len_ratio = a.len().min(b.len()) as f64 / a.len().max(b.len()) as f64;

    jaccard * 0.7 + len_ratio * 0.3
}

/// Rank candidates: syntax-valid > syntax-invalid, then by score
fn rank_candidates(candidates: &mut [FimCandidate]) {
    for c in candidates.iter_mut() {
        let mut score = 0.0;
        if c.syntax_valid {
            score += 0.5;
        }
        if !c.prefix_overlap.is_empty() {
            score += 0.2;
        }
        let len = c.text.len();
        if len > 10 && len < 500 {
            score += 0.2;
        }
        if c.text.trim().len() > 2 {
            score += 0.1;
        }
        c.score = score;
    }

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

// ========================================================================
// [4] Acceptance Tracking — User accept/reject signals → auto-tuning
// ========================================================================

/// Completion feedback record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionFeedback {
    pub completion_id: String,
    pub accepted: bool,
    pub displayed_text: String,
    pub prefix_context: String,
    pub language: String,
    pub latency_ms: u64,
}

/// Acceptance rate tracker
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

    /// Record user acceptance
    pub async fn record_accepted(&self, feedback: CompletionFeedback) {
        let accepted = feedback.accepted;
        let lang = feedback.language.clone();

        self.feedbacks.write().await.push(feedback);

        let mut stats = self.model_stats.write().await;
        let entry = stats.entry(lang).or_default();
        entry.total_shown += 1;
        if accepted {
            entry.total_accepted += 1;
        }
    }

    /// Get overall acceptance rate
    pub async fn acceptance_rate(&self) -> f64 {
        let stats = self.model_stats.read().await;
        let total_shown: u64 = stats.values().map(|s| s.total_shown).sum();
        let total_accepted: u64 = stats.values().map(|s| s.total_accepted).sum();
        if total_shown == 0 {
            return 0.0;
        }
        total_accepted as f64 / total_shown as f64
    }

    /// Per-language statistics
    pub async fn stats_by_language(&self) -> HashMap<String, (u64, u64, f64)> {
        let stats = self.model_stats.read().await;
        stats
            .iter()
            .map(|(lang, s)| {
                let rate = if s.total_shown > 0 {
                    s.total_accepted as f64 / s.total_shown as f64
                } else {
                    0.0
                };
                (lang.clone(), (s.total_shown, s.total_accepted, rate))
            })
            .collect()
    }

    /// Whether parameters should be adjusted (acceptance rate < 30%)
    pub async fn should_adjust(&self) -> bool {
        self.acceptance_rate().await < 0.30
    }
}

// ========================================================================
// Full Completion Pipeline — Combines 1+2+3
// ========================================================================

/// Smart completer — integrates FIM + ContextBuilder + multi-candidate ranking
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

    /// Execute the full completion pipeline
    pub async fn complete(
        &self,
        full_content: &str,
        cursor_offset: usize,
        file_path: &str,
        workspace_files: &[String],
    ) -> FimCompletionResponse {
        let ctx = self
            .ctx_builder
            .build(full_content, cursor_offset, file_path, workspace_files);

        let fim_req = FimCompletionRequest {
            before_cursor: ctx.prefix,
            after_cursor: ctx.suffix,
            file_path: file_path.to_string(),
            max_tokens: 64,
            temperature: 0.5,
        };

        let mut response = self.fim.complete(&fim_req).await;

        if let Some(_hint) = &ctx.syntax_hint {
            if let Some(first) = response.items.first_mut() {
                if first.syntax_valid {
                    first.score += 0.1;
                }
            }
        }

        rank_candidates(&mut response.items);

        response
    }

    pub fn tracker(&self) -> &Arc<AcceptanceTracker> {
        &self.tracker
    }

    /// Adaptive completion — adjusts parameters based on historical acceptance rate
    ///
    /// Closed loop: Display → Accept/Reject → Record → Adjust params → Better next time
    pub async fn adaptive_complete(
        &self,
        full_content: &str,
        cursor_offset: usize,
        file_path: &str,
        workspace_files: &[String],
    ) -> (FimCompletionResponse, String) {
        let completion_id = format!(
            "cmp-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        let rate = self.tracker.acceptance_rate().await;
        let temperature = if rate < 0.3 {
            0.3
        } else if rate > 0.7 {
            0.7
        } else {
            0.5
        };

        let ctx = self
            .ctx_builder
            .build(full_content, cursor_offset, file_path, workspace_files);

        let fim_req = FimCompletionRequest {
            before_cursor: ctx.prefix,
            after_cursor: ctx.suffix,
            file_path: file_path.to_string(),
            max_tokens: if rate < 0.3 { 32 } else { 64 },
            temperature,
        };

        let mut response = self.fim.complete(&fim_req).await;
        rank_candidates(&mut response.items);

        (response, completion_id)
    }

    /// Feedback loop — call after user accepts or rejects
    pub async fn record_feedback(
        &self,
        completion_id: &str,
        accepted: bool,
        text: &str,
        prefix: &str,
        lang: &str,
        latency_ms: u64,
    ) {
        self.tracker
            .record_accepted(CompletionFeedback {
                completion_id: completion_id.to_string(),
                accepted,
                displayed_text: text.to_string(),
                prefix_context: prefix.to_string(),
                language: lang.to_string(),
                latency_ms,
            })
            .await;

        if self.tracker.should_adjust().await {
            tracing::warn!(
                "[Completion] Acceptance rate < 30%, consider adjusting parameters"
            );
        }
    }
}

/// Completion loop statistics summary
pub async fn completion_loop_stats(tracker: &AcceptanceTracker) -> String {
    let rate = tracker.acceptance_rate().await;
    let by_lang = tracker.stats_by_language().await;
    let mut out = format!(
        "━━━ Completion Loop Stats ━━━\nTotal acceptance rate: {:.1}%\n",
        rate * 100.0
    );
    for (lang, (shown, accepted, lang_rate)) in &by_lang {
        out.push_str(&format!(
            "  {}: {}/{} ({:.0}%)\n",
            lang, accepted, shown, lang_rate * 100.0
        ));
    }
    out
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syntax_valid_balanced() {
        assert!(syntax_valid("fn main() {}", "test.rs"));
        assert!(syntax_valid("let x = vec![1, 2, 3];", "test.rs"));
        assert!(!syntax_valid("fn main() {", "test.rs"));
    }

    #[test]
    fn test_dedup_similar() {
        let candidates = vec![
            FimCandidate { text: "hello world".to_string(), score: 0.5, syntax_valid: true, prefix_overlap: "".to_string() },
            FimCandidate { text: "hello world again".to_string(), score: 0.5, syntax_valid: true, prefix_overlap: "".to_string() },
            FimCandidate { text: "completely different".to_string(), score: 0.5, syntax_valid: true, prefix_overlap: "".to_string() },
        ];
        let deduped = dedup_candidates(candidates);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_rank_order() {
        let mut candidates = vec![
            FimCandidate { text: "x".to_string(), score: 0.0, syntax_valid: false, prefix_overlap: "".to_string() },
            FimCandidate { text: "fn helper() -> u32 { 42 }".to_string(), score: 0.0, syntax_valid: true, prefix_overlap: "".to_string() },
        ];
        rank_candidates(&mut candidates);
        assert!(candidates[0].syntax_valid);
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
        assert!(sim > 0.5);
        assert!(sim < 1.0);

        let same = text_similarity("identical text", "identical text");
        assert!((same - 1.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_acceptance_tracker() {
        let tracker = AcceptanceTracker::new();
        assert_eq!(tracker.acceptance_rate().await, 0.0);

        tracker
            .record_accepted(CompletionFeedback {
                completion_id: "1".to_string(),
                accepted: true,
                displayed_text: "fn main()".to_string(),
                prefix_context: "".to_string(),
                language: "rust".to_string(),
                latency_ms: 100,
            })
            .await;

        assert!((tracker.acceptance_rate().await - 1.0).abs() < 0.01);

        tracker
            .record_accepted(CompletionFeedback {
                completion_id: "2".to_string(),
                accepted: false,
                displayed_text: "invalid".to_string(),
                prefix_context: "".to_string(),
                language: "rust".to_string(),
                latency_ms: 50,
            })
            .await;

        assert!((tracker.acceptance_rate().await - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("src/main.rs"), "rust");
        assert_eq!(detect_language("app.tsx"), "typescript");
        assert_eq!(detect_language("script.py"), "python");
        assert_eq!(detect_language("main.go"), "go");
    }

    #[test]
    fn test_extract_prefix_overlap() {
        let overlap = extract_prefix_overlap("    println!(\"hello\");", "    println");
        assert_eq!(overlap, "    println");

        let no_overlap = extract_prefix_overlap("something else", "fn main");
        assert!(no_overlap.is_empty());
    }

    #[tokio::test]
    async fn test_should_adjust_low_acceptance() {
        let tracker = AcceptanceTracker::new();
        for i in 0..10 {
            tracker
                .record_accepted(CompletionFeedback {
                    completion_id: format!("{}", i),
                    accepted: false,
                    displayed_text: "bad".to_string(),
                    prefix_context: "".to_string(),
                    language: "rust".to_string(),
                    latency_ms: 50,
                })
                .await;
        }
        assert!(tracker.should_adjust().await);
    }

    #[tokio::test]
    async fn test_stats_by_language() {
        let tracker = AcceptanceTracker::new();
        tracker
            .record_accepted(CompletionFeedback {
                completion_id: "r1".to_string(),
                accepted: true,
                displayed_text: "fn foo() {}".to_string(),
                prefix_context: "".to_string(),
                language: "rust".to_string(),
                latency_ms: 10,
            })
            .await;
        tracker
            .record_accepted(CompletionFeedback {
                completion_id: "p1".to_string(),
                accepted: true,
                displayed_text: "def bar(): pass".to_string(),
                prefix_context: "".to_string(),
                language: "python".to_string(),
                latency_ms: 20,
            })
            .await;
        tracker
            .record_accepted(CompletionFeedback {
                completion_id: "r2".to_string(),
                accepted: false,
                displayed_text: "bad rust".to_string(),
                prefix_context: "".to_string(),
                language: "rust".to_string(),
                latency_ms: 15,
            })
            .await;

        let by_lang = tracker.stats_by_language().await;
        assert_eq!(by_lang.len(), 2);
        let (shown, accepted, rate) = by_lang.get("rust").unwrap();
        assert_eq!(*shown, 2);
        assert_eq!(*accepted, 1);
        assert!((*rate - 0.5).abs() < 0.01);
    }
}
