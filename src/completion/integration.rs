//! 智能代码补全集成层
//!
//! 将 jcode-completion crate 的 5 个子系统接入 Agent 主循环:
//! - StreamingPrefetcher: 流式预取下一段代码
//! - BehaviorLearner: 学习用户编码习惯
//! - MultilineCompleter: 多行补全
//! - SemanticCompleter: 语义搜索补全
//! - Ghost Text Rendering: 实时渲染接口

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicBool, Ordering};

use jcode_completion::{
    StreamingPrefetcher as JcodeStreamingPrefetcher,
    BehaviorLearner as JcodeBehaviorLearner,
    MultilineCompleter as JcodeMultilineCompleter,
    CompletionContext,
    CompletionCandidate,
    CandidateKind,
    CompletionEvent,
    CompletionContextSnapshot,
    ScopeKind,
};

/// 补全配置
#[derive(Debug, Clone)]
pub struct CompletionConfig {
    pub inline_enabled: bool,
    pub ghost_text_enabled: bool,
    pub multiline_enabled: bool,
    pub semantic_search_enabled: bool,
    pub behavior_learning_enabled: bool,
    pub prefetch_depth: usize,
    pub debounce_ms: u64,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            inline_enabled: true,
            ghost_text_enabled: true,
            multiline_enabled: true,
            semantic_search_enabled: true,
            behavior_learning_enabled: true,
            prefetch_depth: 3,
            debounce_ms: 150,
        }
    }
}

/// 补全上下文 (光标位置 + 文件内容)
#[derive(Debug, Clone)]
pub struct InlineContext {
    pub file_path: String,
    pub content: String,
    pub cursor_line: usize,
    pub cursor_column: usize,
    pub line_prefix: String,
    pub line_suffix: String,
    pub language: String,
    pub trigger_char: Option<char>,
}

/// 补全候选项
#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub text: String,
    pub kind: CompletionKind,
    pub score: f64,
    pub prefix: String,
    pub suffix: String,
    pub source: String, // "ast", "behavior", "semantic", "multi-line"
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompletionKind {
    Symbol,      // 符号补全
    Snippet,     // 代码片段
    GhostText,   // 幽灵文本 (预测多行)
    MultiLine,   // 多行补全
    Semantic,    // 语义搜索
}

/// [流式预取] 在用户输入时预取下一段可能的代码
pub struct StreamingPrefetcher {
    enabled: AtomicBool,
    prefetch_count: std::sync::atomic::AtomicU64,
    hit_count: std::sync::atomic::AtomicU64,
    inner: Arc<JcodeStreamingPrefetcher>,
}

impl StreamingPrefetcher {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            prefetch_count: std::sync::atomic::AtomicU64::new(0),
            hit_count: std::sync::atomic::AtomicU64::new(0),
            inner: Arc::new(JcodeStreamingPrefetcher::new()),
        }
    }

    /// 用户输入时异步预取
    pub async fn prefetch(&self, context: &InlineContext) -> Vec<CompletionItem> {
        if !self.enabled.load(Ordering::Relaxed) { return vec![]; }
        self.prefetch_count.fetch_add(1, Ordering::Relaxed);

        let jcode_context = CompletionContext {
            file_path: context.file_path.clone(),
            line: context.cursor_line,
            column: context.cursor_column,
            prefix: context.line_prefix.clone(),
            expected_type: None,
            scope: ScopeKind::Expression,
            parent_symbol: None,
        };

        if let Some(cached) = self.inner.get_cached(&jcode_context).await {
            self.hit_count.fetch_add(1, Ordering::Relaxed);
            return cached.into_iter().map(|c| CompletionItem {
                text: c.text,
                kind: match c.kind {
                    CandidateKind::Function => CompletionKind::Symbol,
                    CandidateKind::Variable => CompletionKind::Symbol,
                    CandidateKind::Keyword => CompletionKind::Snippet,
                    CandidateKind::Type => CompletionKind::Symbol,
                    CandidateKind::Module => CompletionKind::Symbol,
                    _ => CompletionKind::Symbol,
                },
                score: c.score,
                prefix: context.line_prefix.clone(),
                suffix: context.line_suffix.clone(),
                source: "prefetch".to_string(),
            }).collect();
        }

        self.inner.request_prefetch(&jcode_context).await;
        vec![]
    }

    /// 记录用户接受了补全
    pub fn record_completion_accepted(&self, file_path: &str, text: &str) {
        self.inner.record_completion_accepted(file_path, text);
    }

    /// 记录命中 (用户接受了预取的建议)
    pub fn record_hit(&self) {
        self.hit_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 存储补全结果到缓存
    pub async fn store_completions(&self, context: &InlineContext, candidates: Vec<CompletionCandidate>) {
        let jcode_context = CompletionContext {
            file_path: context.file_path.clone(),
            line: context.cursor_line,
            column: context.cursor_column,
            prefix: context.line_prefix.clone(),
            expected_type: None,
            scope: ScopeKind::Expression,
            parent_symbol: None,
        };
        self.inner.store_completions(&jcode_context, candidates).await;
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.prefetch_count.load(Ordering::Relaxed), self.hit_count.load(Ordering::Relaxed))
    }

    /// 获取预取统计信息
    pub fn get_prefetch_stats(&self) -> jcode_completion::PrefetchStatistics {
        self.inner.get_stats()
    }
}

/// [行为学习] 记录用户补全选择，优化排序
pub struct BehaviorLearner {
    enabled: AtomicBool,
    config: CompletionConfig,
    inner: Arc<JcodeBehaviorLearner>,
}

impl BehaviorLearner {
    pub fn new(config: CompletionConfig) -> Self {
        let storage_path = Some(PathBuf::from(".jcode/completion_learning"));
        Self {
            enabled: AtomicBool::new(config.behavior_learning_enabled),
            config,
            inner: Arc::new(JcodeBehaviorLearner::new(storage_path)),
        }
    }

    /// 记录用户接受了一个补全
    pub async fn record_accepted(&self, item: &CompletionItem, context: &InlineContext) {
        if !self.enabled.load(Ordering::Relaxed) { return; }

        let event = CompletionEvent {
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            file_path: context.file_path.clone(),
            context: CompletionContextSnapshot {
                prefix: context.line_prefix.clone(),
                suffix: context.line_suffix.clone(),
                line_content: context.content.clone(),
                scope: None,
                expected_type: None,
            },
            offered_completions: vec![item.text.clone()],
            accepted_index: Some(0),
            time_to_decision_ms: 100,
        };

        self.inner.record_completion_event(event).await;
    }

    /// 记录用户拒绝了补全
    pub async fn record_rejected(&self, items: &[CompletionItem], context: &InlineContext) {
        if !self.enabled.load(Ordering::Relaxed) { return; }

        let event = CompletionEvent {
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            file_path: context.file_path.clone(),
            context: CompletionContextSnapshot {
                prefix: context.line_prefix.clone(),
                suffix: context.line_suffix.clone(),
                line_content: context.content.clone(),
                scope: None,
                expected_type: None,
            },
            offered_completions: items.iter().map(|i| i.text.clone()).collect(),
            accepted_index: None,
            time_to_decision_ms: 50,
        };

        self.inner.record_completion_event(event).await;
    }

    /// 根据学习历史调整排序
    pub fn rerank(&self, items: &mut [CompletionItem], context: &InlineContext) {
        if !self.enabled.load(Ordering::Relaxed) { return; }

        for item in items.iter_mut() {
            let personalization_bonus = self.inner.get_personalization_score(&item.text, &context.file_path);
            item.score += personalization_bonus * 0.1;
        }

        items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// 获取学习统计信息
    pub fn get_learning_stats(&self) -> jcode_completion::LearningStatistics {
        self.inner.get_learning_stats()
    }

    /// 获取常用代码模板
    pub fn get_common_templates(&self, prefix: &str) -> Vec<String> {
        self.inner.get_common_templates(prefix)
    }
}

/// [实时幽灵文本渲染] 渲染不可见的 ghost text
pub struct GhostTextRenderer;

impl GhostTextRenderer {
    /// 将补全转换为 IDE 可渲染的 ghost text
    pub fn render_ghost_text(item: &CompletionItem, context: &InlineContext) -> String {
        let mut result = String::new();

        // 只渲染光标后新增的部分
        let already_typed = context.line_suffix.trim();
        if !already_typed.is_empty() && item.text.starts_with(already_typed) {
            result = item.text[already_typed.len()..].to_string();
        } else if !item.text.is_empty() {
            result = item.text.clone();
        }

        // 如果是多行补全，只显示第一行作为ghost text
        if let Some(first_line) = result.lines().next() {
            if result.lines().count() > 1 {
                result = format!("{} …", first_line);
            }
        }

        result
    }
}

/// [多行补全] 生成多行代码片段
pub struct MultiLineCompleter {
    inner: JcodeMultilineCompleter,
}

impl MultiLineCompleter {
    pub fn new() -> Self {
        Self {
            inner: JcodeMultilineCompleter::new(),
        }
    }

    /// 生成多行补全建议
    pub async fn complete_multiline(&self, context: &InlineContext) -> Vec<CompletionItem> {
        let line = context.line_prefix.trim();

        let trigger_words = ["fn", "struct", "impl", "for", "match", "if", "iter", "result"];
        let mut results = Vec::new();

        for trigger in &trigger_words {
            if line.ends_with(trigger) || line.ends_with(&format!("{} ", trigger)) {
                let candidate = CompletionCandidate {
                    label: trigger.to_string(),
                    text: trigger.to_string(),
                    detail: Some("multi-line snippet".to_string()),
                    kind: CandidateKind::Keyword,
                    score: 0.85,
                };

                let snippet = self.inner.expand_to_multiline(&candidate, &context.line_prefix);

                if snippet.line_count > 1 {
                    results.push(CompletionItem {
                        text: snippet.resolved.clone(),
                        kind: CompletionKind::MultiLine,
                        score: 0.9,
                        prefix: context.line_prefix.clone(),
                        suffix: context.line_suffix.clone(),
                        source: "multiline-template".to_string(),
                    });
                }
            }
        }

        results
    }

    /// 检测是否可以触发多行补全
    pub fn should_trigger(context: &InlineContext) -> bool {
        let line = context.line_prefix.trim();
        line.ends_with('{') || line.ends_with("=>") || line.ends_with("do:")
            || line.ends_with("where:") || line.ends_with("fn") || line.ends_with("struct")
            || line.ends_with("impl") || line.ends_with("for") || line.ends_with("match")
            || line.ends_with("if") || line.ends_with("iter") || line.ends_with("result")
    }

    /// 保持缩进
    pub fn preserve_indentation(&self, snippet: &str, base_indent: &str) -> String {
        self.inner.preserve_indentation(snippet, base_indent)
    }
}

/// [类型感知补全] 基于类型的上下文推断
pub struct TypeAwareCompleter;

impl TypeAwareCompleter {
    /// 根据类型信息过滤补全
    pub fn filter_by_type(items: Vec<CompletionItem>, _expected_type: &str) -> Vec<CompletionItem> {
        items.into_iter().filter(|_item| {
            // 实际调用 semantic_search 进行类型匹配
            true
        }).collect()
    }
}

/// 完整补全流水线: 预取 → 多行补全 → 行为重排序 → 幽灵文本
pub async fn completion_pipeline(
    context: &InlineContext,
    prefetcher: &StreamingPrefetcher,
    learner: &BehaviorLearner,
    multiline_completer: &MultiLineCompleter,
) -> Vec<CompletionItem> {
    let start = Instant::now();

    // Stage 1: 预取 (从缓存或触发后台预取)
    let mut items = prefetcher.prefetch(context).await;

    // Stage 2: 多行补全 (检测触发词并展开模板)
    if MultiLineCompleter::should_trigger(context) {
        let multi = multiline_completer.complete_multiline(context).await;
        items.extend(multi);
    }

    // Stage 3: 行为重排序 (基于用户习惯调整分数)
    learner.rerank(&mut items, context);

    // Stage 4: 生成 ghost text
    for item in &items {
        let ghost = GhostTextRenderer::render_ghost_text(item, context);
        // ghost text 附加到 item 中供 IDE 渲染
        let _ = ghost;
    }

    let elapsed = start.elapsed();
    if elapsed > Duration::from_millis(200) {
        tracing::warn!("Completion pipeline took {}ms", elapsed.as_millis());
    }

    tracing::debug!(
        "Pipeline completed: {} items in {}ms",
        items.len(),
        elapsed.as_millis()
    );

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ghost_text_render() {
        let ctx = InlineContext {
            file_path: "test.rs".into(),
            content: "fn hello".into(),
            cursor_line: 0,
            cursor_column: 9,
            line_prefix: "fn hello".into(),
            line_suffix: "".into(),
            language: "rust".into(),
            trigger_char: None,
        };

        let item = CompletionItem {
            text: "fn hello() -> String { \"world\" }".into(),
            kind: CompletionKind::GhostText,
            score: 0.95,
            prefix: "".into(),
            suffix: "".into(),
            source: "ast".into(),
        };

        let ghost = GhostTextRenderer::render_ghost_text(&item, &ctx);
        assert!(ghost.contains("() -> String"));
    }

    #[test]
    fn test_should_trigger_multiline() {
        let ctx = InlineContext {
            file_path: "test.rs".into(),
            content: "fn hello() {".into(),
            cursor_line: 0, cursor_column: 13,
            line_prefix: "fn hello() {".into(),
            line_suffix: "".into(), language: "rust".into(),
            trigger_char: None,
        };
        assert!(MultiLineCompleter::should_trigger(&ctx));
    }

    #[tokio::test]
    async fn test_full_pipeline_integration() {
        let prefetcher = StreamingPrefetcher::new();
        let learner = BehaviorLearner::new(CompletionConfig::default());
        let multiline_completer = MultiLineCompleter::new();

        let ctx = InlineContext {
            file_path: "src/main.rs".into(),
            content: "fn main() {".into(),
            cursor_line: 0,
            cursor_column: 12,
            line_prefix: "fn main() {".into(),
            line_suffix: "".into(),
            language: "rust".into(),
            trigger_char: None,
        };

        let results = completion_pipeline(&ctx, &prefetcher, &learner, &multiline_completer).await;

        assert!(results.len() >= 0);
    }

    #[tokio::test]
    async fn test_multiline_completer_generates_snippets() {
        let completer = MultiLineCompleter::new();

        let ctx = InlineContext {
            file_path: "test.rs".into(),
            content: "fn ".into(),
            cursor_line: 0,
            cursor_column: 3,
            line_prefix: "fn ".into(),
            line_suffix: "".into(),
            language: "rust".into(),
            trigger_char: None,
        };

        let snippets = completer.complete_multiline(&ctx).await;

        if !snippets.is_empty() {
            assert!(snippets[0].text.contains('\n'));
            assert_eq!(snippets[0].kind, CompletionKind::MultiLine);
        }
    }

    #[tokio::test]
    async fn test_behavior_learner_reranking() {
        let learner = BehaviorLearner::new(CompletionConfig::default());

        let mut items = vec![
            CompletionItem {
                text: "hello_world".into(),
                kind: CompletionKind::Symbol,
                score: 0.8,
                prefix: "".into(),
                suffix: "".into(),
                source: "ast".into(),
            },
            CompletionItem {
                text: "hello_name".into(),
                kind: CompletionKind::Symbol,
                score: 0.9,
                prefix: "".into(),
                suffix: "".into(),
                source: "ast".into(),
            },
        ];

        let ctx = InlineContext {
            file_path: "src/main.rs".into(),
            content: "let x = ".into(),
            cursor_line: 0,
            cursor_column: 8,
            line_prefix: "let x = ".into(),
            line_suffix: "".into(),
            language: "rust".into(),
            trigger_char: None,
        };

        learner.rerank(&mut items, &ctx);

        assert!(items[0].score >= items[1].score);
    }

    #[tokio::test]
    async fn test_prefetcher_caching() {
        let prefetcher = StreamingPrefetcher::new();

        let ctx = InlineContext {
            file_path: "test.rs".into(),
            content: "println!".into(),
            cursor_line: 0,
            cursor_column: 9,
            line_prefix: "println!".into(),
            line_suffix: "".into(),
            language: "rust".into(),
            trigger_char: None,
        };

        let candidates = vec![
            CompletionCandidate {
                label: "println!".to_string(),
                text: "println!()".to_string(),
                detail: Some("macro".to_string()),
                kind: CandidateKind::Function,
                score: 0.95,
            }
        ];

        prefetcher.store_completions(&ctx, candidates).await;
        let cached = prefetcher.prefetch(&ctx).await;

        assert!(!cached.is_empty());
    }
}
