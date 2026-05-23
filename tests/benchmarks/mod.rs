//! CarpAI Benchmark Suite
//!
//! Comprehensive benchmarking for CarpAI server capabilities:
//! - Code generation quality
//! - RAG retrieval effectiveness
//! - Cross-file refactoring capability
//! - Performance and latency baselines
//! - KV Cache cost savings verification

pub mod code_generation;
pub mod rag_retrieval;
pub mod cross_file_refactoring;
pub mod performance_baseline;
pub mod kv_cache_cost;

// Re-export main types for convenience
pub use code_generation::{CodeGenerationBenchmark, TestCase, BenchmarkResult};
pub use rag_retrieval::{RagRetrievalBenchmark, RagTestCase, AggregateRagMetrics};
pub use cross_file_refactoring::{CrossFileRefactorBenchmark, RefactorTestCase, AggregateRefactorMetrics};
pub use performance_baseline::{PerformanceBaselineBenchmark, PerformanceTestConfig, AggregatePerformanceMetrics};
pub use kv_cache_cost::{KVCacheCostBenchmark, KVCacheTestConfig, AggregateCostMetrics};
