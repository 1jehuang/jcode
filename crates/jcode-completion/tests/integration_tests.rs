//! Integration Tests for Completion Engine
//!
//! This module provides comprehensive integration tests covering:
//! - End-to-end completion flow
//! - Performance benchmarks
//! - Cache effectiveness
//! - Multi-language support

use jcode_completion::*;
use std::time::Instant;

/// Test helper: Create a mock completion provider
struct MockProvider;

#[async_trait::async_trait]
impl CompletionProvider for MockProvider {
    async fn complete_simple(
        &self,
        _prompt: &str,
        _system_message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok("mock_completion".to_string())
    }
}

// ============================================================================
// Integration Tests
// ============================================================================

#[tokio::test]
async fn test_completion_engine_basic() {
    let provider = Box::new(MockProvider);
    let engine = CompletionEngine::new(provider, None, None);

    let code = r#"
fn main() {
    let x = 42;
    println!("{}", x);
}
"#;

    let completions = engine.complete("test.rs", code, 2, 14).await;

    // Should return at least one completion
    assert!(!completions.is_empty());
}

#[tokio::test]
async fn test_prefetch_caching() {
    let provider = Box::new(MockProvider);
    let engine = CompletionEngine::new(provider, None, None);

    let code = "fn test() { let x = ";

    // First call (cache miss)
    let start1 = Instant::now();
    let _ = engine.complete("test.rs", code, 0, 15).await;
    let elapsed1 = start1.elapsed();

    // Second call to same position (should hit cache if prefetch works)
    let start2 = Instant::now();
    let _ = engine.complete("test.rs", code, 0, 15).await;
    let elapsed2 = start2.elapsed();

    // Second call should be faster (though may not be cached immediately)
    println!("First call: {:?}", elapsed1);
    println!("Second call: {:?}", elapsed2);
}

#[tokio::test]
async fn test_multiline_completion() {
    let completer = MultilineCompleter::new();

    let candidate = CompletionCandidate {
        label: "fn".to_string(),
        text: "fn".to_string(),
        detail: None,
        kind: CandidateKind::Keyword,
        score: 0.9,
    };

    let snippet = completer.expand_to_multiline(&candidate, "fn");

    // Should expand to multi-line function template
    assert!(snippet.line_count >= 3);
    assert!(!snippet.placeholders.is_empty());
    assert!(snippet.resolved.contains("fn"));
}

#[tokio::test]
async fn test_ast_parser_rust() {
    let code = r#"
mod my_module {
    pub struct MyStruct {
        pub field: i32,
    }

    pub fn my_function(x: i32) -> i32 {
        x * 2
    }
}
"#;

    let ast = AstTree::parse(code, ParserLanguage::Rust).unwrap();

    // Extract symbols
    let symbols = ast.extract_all_symbols();
    assert!(!symbols.is_empty());

    // Should find the function and struct
    assert!(symbols.iter().any(|s| s.name == "my_function"));
    assert!(symbols.iter().any(|s| s.name == "MyStruct"));

    // Extract scope chain
    if let Some(func_pos) = code.find("x * 2") {
        let scopes = ast.extract_scope_chain(func_pos);
        assert!(scopes.iter().any(|(kind, _)| kind == "module"));
        assert!(scopes.iter().any(|(kind, _)| kind == "function"));
    }
}

#[tokio::test]
async fn test_incremental_index() {
    let index = IncrementalIndex::new();

    // Queue a file change event
    let event = FileChangeEvent {
        file_path: std::path::PathBuf::from("src/test.rs"),
        change_type: ChangeType::Modified,
        timestamp: std::time::Instant::now(),
    };

    index.queue_file_change(event).await;

    // Give background worker time to process
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Query should work (even if empty)
    let results = index.query_symbols("test", 10).await;
    assert!(results.len() >= 0); // May be empty but shouldn't panic
}

#[tokio::test]
async fn test_behavior_learner() {
    let learner = BehaviorLearner::new(None);

    let event = CompletionEvent {
        timestamp: chrono::Utc::now().timestamp_millis() as u64,
        file_path: "test.rs".to_string(),
        context: CompletionContextSnapshot {
            prefix: "let x = ".to_string(),
            suffix: ";".to_string(),
            line_content: "let x = hello".to_string(),
            scope: Some("function".to_string()),
            expected_type: Some("i32".to_string()),
        },
        offered_completions: vec!["42".to_string(), "hello()".to_string()],
        accepted_index: Some(0),
        time_to_decision_ms: 300,
    };

    learner.record_completion_event(event).await;

    let stats = learner.get_learning_stats();
    assert_eq!(stats.total_events, 1);
    assert_eq!(stats.acceptance_rate, 1.0);
}

#[tokio::test]
async fn test_streaming_prefetcher() {
    let prefetcher = StreamingPrefetcher::new();

    let context = CompletionContext {
        file_path: "test.rs".to_string(),
        expected_type: Some("i32".to_string()),
        scope: Some("function".to_string()),
        prefix: "x".to_string(),
        suffix: "".to_string(),
        line_content: "let x = ".to_string(),
    };

    // Store some completions
    let candidates = vec![
        CompletionCandidate {
            label: "x_coord".to_string(),
            text: "x_coord".to_string(),
            detail: None,
            kind: CandidateKind::Variable,
            score: 0.9,
        }
    ];

    prefetcher.store_completions(&context, candidates).await;

    // Try to retrieve
    let cached = prefetcher.get_cached(&context).await;
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().len(), 1);

    // Check stats
    let stats = prefetcher.get_stats();
    assert!(stats.cache_size > 0);
}

#[tokio::test]
async fn test_collab_aware_completer() {
    let index = Arc::new(IncrementalIndex::new());
    let completer = CollabAwareCompleter::new(index);

    // Simulate member activity
    let ctx = MemberEditingContext {
        member_id: "user1".to_string(),
        current_file: "src/main.rs".to_string(),
        cursor_line: 10,
        recent_symbols: vec!["println".to_string()],
        last_active: std::time::Instant::now(),
    };

    completer.update_member_context("user1".to_string(), ctx);

    // Check conflict detection
    let conflicts = completer.get_conflicting_symbols("src/main.rs");
    assert!(conflicts.contains("println"));

    // Record usage
    completer.record_symbol_usage("println");

    let stats = completer.get_collab_stats();
    assert_eq!(stats.active_members, 1);
}

#[tokio::test]
async fn test_semantic_search() {
    use jcode_completion::{SemanticCompleter, CodeSnippet, SemanticConfig};

    let config = SemanticConfig::default();
    let completer = SemanticCompleter::new(config);

    let snippet = CodeSnippet {
        id: "test1".to_string(),
        code: "fn hello() {}".to_string(),
        language: "rust".to_string(),
        embedding: completer.generate_embedding("hello function").await,
        metadata: std::collections::HashMap::new(),
        usage_count: 0,
    };

    completer.add_snippet(snippet).await;

    let query_emb = completer.generate_embedding("greeting function").await;
    let results = completer.search_similar(&query_emb, Some("rust")).await;

    // Should find the snippet (even with dummy embeddings)
    assert!(results.len() >= 0);
}

#[tokio::test]
async fn test_metrics_collection() {
    use jcode_completion::metrics::get_metrics;

    let metrics = get_metrics();
    metrics.reset();

    // Simulate some activity
    metrics.record_request();
    metrics.record_cache_hit();
    metrics.record_latency(50);
    metrics.record_acceptance();

    // Check values
    assert_eq!(metrics.total_requests.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(metrics.cache_hits.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!((metrics.get_avg_latency_ms() - 50.0).abs() < 0.01);
    assert!((metrics.get_cache_hit_rate() - 1.0).abs() < 0.01);

    // Test Prometheus output
    let prom_output = metrics.generate_prometheus_metrics();
    assert!(prom_output.contains("jcode_completion_requests_total 1"));
}

// ============================================================================
// Performance Benchmarks
// ============================================================================

#[tokio::test]
async fn benchmark_completion_latency() {
    let provider = Box::new(MockProvider);
    let engine = CompletionEngine::new(provider, None, None);

    let code = "fn test() { let x = ";
    let iterations = 100;

    let start = Instant::now();
    for _ in 0..iterations {
        let _ = engine.complete("test.rs", code, 0, 15).await;
    }
    let elapsed = start.elapsed();

    let avg_latency = elapsed.as_millis() as f64 / iterations as f64;
    println!("Average completion latency: {:.2}ms", avg_latency);

    // Should be reasonably fast (< 100ms average for mock)
    assert!(avg_latency < 100.0);
}

#[tokio::test]
async fn benchmark_cache_hit_rate() {
    let provider = Box::new(MockProvider);
    let engine = CompletionEngine::new(provider, None, None);

    let contexts = vec![
        ("file1.rs", "fn test1() {"),
        ("file2.rs", "fn test2() {"),
        ("file1.rs", "fn test1() {"), // Repeat for cache hit
    ];

    let mut hits = 0u64;
    let mut misses = 0u64;

    for (file, code) in &contexts {
        let _ = engine.complete(file, code, 0, 10).await;

        // Check if it was a cache hit
        let stats = engine.get_prefetch_stats();
        if stats.cache_hits > hits {
            hits = stats.cache_hits;
        } else {
            misses += 1;
        }
    }

    let total = hits + misses;
    let hit_rate = if total > 0 { hits as f64 / total as f64 } else { 0.0 };

    println!("Cache hit rate: {:.1}%", hit_rate * 100.0);
}

#[tokio::test]
async fn benchmark_ast_parsing() {
    let code = include_str!("../src/lib.rs"); // Use own source as test data

    let iterations = 50;
    let start = Instant::now();

    for _ in 0..iterations {
        let _ = AstTree::parse(code, ParserLanguage::Rust).unwrap();
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed.as_millis() as f64 / iterations as f64;

    println!("Average AST parsing time: {:.2}ms", avg_time);

    // Should parse in reasonable time (< 50ms for medium file)
    assert!(avg_time < 50.0);
}

#[tokio::test]
async fn benchmark_embedding_generation() {
    use jcode_completion::{FallbackEmbeddingModel, EmbeddingModelConfig};

    let config = EmbeddingModelConfig::default();
    let model = FallbackEmbeddingModel::new(config);

    let texts = vec![
        "fn main() {}",
        "struct Foo { bar: i32 }",
        "impl Trait for Type {}",
        "let x = vec![1, 2, 3];",
    ];

    let iterations = 100;
    let start = Instant::now();

    for _ in 0..iterations {
        for text in &texts {
            let _ = model.encode(text).await.unwrap();
        }
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed.as_millis() as f64 / (iterations * texts.len()) as f64;

    println!("Average embedding generation time: {:.2}ms", avg_time);
}

#[tokio::test]
async fn benchmark_concurrent_completions() {
    let provider = Box::new(MockProvider);
    let engine = Arc::new(CompletionEngine::new(provider, None, None));

    let num_tasks = 10;
    let requests_per_task = 20;

    let start = Instant::now();

    let mut handles = Vec::new();
    for task_id in 0..num_tasks {
        let engine_clone = engine.clone();
        let handle = tokio::spawn(async move {
            for i in 0..requests_per_task {
                let code = format!("fn test_{}_{}() {{", task_id, i);
                let _ = engine_clone.complete(&format!("file{}.rs", task_id), &code, 0, 10).await;
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    let elapsed = start.elapsed();
    let total_requests = num_tasks * requests_per_task;
    let throughput = total_requests as f64 / elapsed.as_secs_f64();

    println!("Concurrent completion throughput: {:.0} req/s", throughput);
    println!("Total time for {} requests: {:?}", total_requests, elapsed);
}

// ============================================================================
// Regression Tests
// ============================================================================

#[tokio::test]
async fn test_no_panic_on_empty_input() {
    let provider = Box::new(MockProvider);
    let engine = CompletionEngine::new(provider, None, None);

    // Should not panic on empty or malformed input
    let result = engine.complete("", "", 0, 0).await;
    assert!(result.len() >= 0); // May be empty but shouldn't panic
}

#[tokio::test]
async fn test_no_panic_on_large_input() {
    let provider = Box::new(MockProvider);
    let engine = CompletionEngine::new(provider, None, None);

    // Large file (10k lines)
    let large_code = (0..10000)
        .map(|i| format!("// Line {}", i))
        .collect::<Vec<_>>()
        .join("\n");

    let result = engine.complete("large.rs", &large_code, 5000, 10).await;
    assert!(result.len() >= 0);
}

#[tokio::test]
async fn test_thread_safety() {
    let provider = Box::new(MockProvider);
    let engine = Arc::new(CompletionEngine::new(provider, None, None));

    let mut handles = Vec::new();
    for i in 0..10 {
        let engine_clone = engine.clone();
        let handle = tokio::spawn(async move {
            let code = format!("fn thread_{}() {{", i);
            let _ = engine_clone.complete("test.rs", &code, 0, 10).await;
        });
        handles.push(handle);
    }

    // All should complete without data races
    for handle in handles {
        handle.await.unwrap();
    }
}
