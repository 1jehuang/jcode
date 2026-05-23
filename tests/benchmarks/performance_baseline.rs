//! Performance Baseline Measurement Suite
//!
//! Measures CarpAI server performance characteristics:
//! - P50/P95/P99 latency for various endpoints
//! - Throughput (requests/second) under different concurrency levels
//! - Resource utilization (CPU, memory)
//! - KV Cache hit rate impact on latency
//! - Scalability (linear vs sub-linear scaling)
//!
//! Usage:
//! ```bash
//! cargo test --test performance_baseline_benchmark -- --nocapture
//! ```

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

// ============================================================================
// Test Configuration
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceTestConfig {
    pub name: String,
    pub endpoint: String,
    pub method: HttpMethod,
    pub payload: Option<serde_json::Value>,
    pub concurrency_levels: Vec<usize>,
    pub requests_per_level: usize,
    pub warmup_requests: usize,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HttpMethod {
    GET,
    POST,
}

impl Default for PerformanceTestConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            method: HttpMethod::POST,
            payload: Some(serde_json::json!({
                "model": "gpt-4",
                "messages": [{"role": "user", "content": "Hello"}],
                "max_tokens": 100
            })),
            concurrency_levels: vec![1, 10, 50, 100, 200],
            requests_per_level: 100,
            warmup_requests: 10,
            timeout_secs: 300,
        }
    }
}

// ============================================================================
// Performance Metrics
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct LatencyMetrics {
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    pub stddev_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThroughputMetrics {
    pub requests_per_second: f64,
    pub successful_requests: usize,
    pub failed_requests: usize,
    pub total_requests: usize,
    pub success_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceMetrics {
    pub avg_cpu_percent: Option<f64>,
    pub peak_memory_mb: Option<f64>,
    pub kv_cache_hit_rate: Option<f64>,
    pub gpu_utilization: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PerformanceResult {
    pub test_name: String,
    pub concurrency: usize,
    pub latency: LatencyMetrics,
    pub throughput: ThroughputMetrics,
    pub resources: ResourceMetrics,
    pub test_duration_secs: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregatePerformanceMetrics {
    pub timestamp: String,
    pub carpai_url: String,
    pub test_config: PerformanceTestConfig,

    // Results by concurrency level
    pub results_by_concurrency: Vec<PerformanceResult>,

    // Scalability analysis
    pub scalability_factor: f64, // How well throughput scales with concurrency
    pub optimal_concurrency: usize, // Concurrency level with best throughput/latency tradeoff

    // Overall metrics
    pub overall_p50_ms: f64,
    pub overall_p95_ms: f64,
    pub overall_p99_ms: f64,
    pub peak_throughput_rps: f64,
    pub avg_success_rate: f64,
}

// ============================================================================
// Benchmark Runner
// ============================================================================

pub struct PerformanceBaselineBenchmark {
    base_url: String,
    api_key: Option<String>,
    configs: Vec<PerformanceTestConfig>,
}

impl PerformanceBaselineBenchmark {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            base_url,
            api_key,
            configs: vec![
                PerformanceTestConfig {
                    name: "chat_completions".to_string(),
                    endpoint: "/v1/chat/completions".to_string(),
                    method: HttpMethod::POST,
                    payload: Some(serde_json::json!({
                        "model": "gpt-4",
                        "messages": [{"role": "user", "content": "Write a Rust function to calculate factorial"}],
                        "max_tokens": 150,
                        "temperature": 0.2
                    })),
                    concurrency_levels: vec![1, 10, 50, 100],
                    requests_per_level: 50,
                    warmup_requests: 5,
                    timeout_secs: 300,
                },
                PerformanceTestConfig {
                    name: "embeddings".to_string(),
                    endpoint: "/v1/embeddings".to_string(),
                    method: HttpMethod::POST,
                    payload: Some(serde_json::json!({
                        "model": "text-embedding-ada-002",
                        "input": "This is a test sentence for embedding."
                    })),
                    concurrency_levels: vec![1, 10, 50, 100, 200],
                    requests_per_level: 100,
                    warmup_requests: 10,
                    timeout_secs: 180,
                },
                PerformanceTestConfig {
                    name: "rag_search".to_string(),
                    endpoint: "/api/v1/rag/search".to_string(),
                    method: HttpMethod::POST,
                    payload: Some(serde_json::json!({
                        "query_text": "How does authentication work?",
                        "top_k": 10,
                        "threshold": 0.7
                    })),
                    concurrency_levels: vec![1, 10, 50],
                    requests_per_level: 50,
                    warmup_requests: 5,
                    timeout_secs: 180,
                },
            ],
        }
    }

    pub fn with_configs(mut self, configs: Vec<PerformanceTestConfig>) -> Self {
        self.configs = configs;
        self
    }

    /// Run all performance benchmarks
    pub async fn run(&self) -> anyhow::Result<Vec<AggregatePerformanceMetrics>> {
        println!("\n⚡ Starting Performance Baseline Benchmark");
        println!("   Target: {}", self.base_url);
        println!("   Test configurations: {}\n", self.configs.len());

        let mut all_results = Vec::new();

        for config in &self.configs {
            println!("\n{}", "=".repeat(80));
            println!("  Testing: {}", config.name);
            println!("{}", "=".repeat(80));

            let result = self.run_test_config(config).await?;
            all_results.push(result);
        }

        Ok(all_results)
    }

    /// Run a single test configuration
    async fn run_test_config(
        &self,
        config: &PerformanceTestConfig,
    ) -> anyhow::Result<AggregatePerformanceMetrics> {
        let mut results_by_concurrency = Vec::new();

        for &concurrency in &config.concurrency_levels {
            println!("\n  Concurrency: {} | Requests: {}", concurrency, config.requests_per_level);

            let result = self.run_at_concurrency(config, concurrency).await?;

            println!("    P50: {:.0}ms | P95: {:.0}ms | P99: {:.0}ms | RPS: {:.1}",
                result.latency.p50_ms,
                result.latency.p95_ms,
                result.latency.p99_ms,
                result.throughput.requests_per_second
            );

            results_by_concurrency.push(result);
        }

        // Calculate aggregate metrics
        let aggregate = self.calculate_aggregate_metrics(config, &results_by_concurrency);

        self.print_aggregate_summary(&aggregate);

        Ok(aggregate)
    }

    /// Run test at specific concurrency level
    async fn run_at_concurrency(
        &self,
        config: &PerformanceTestConfig,
        concurrency: usize,
    ) -> anyhow::Result<PerformanceResult> {
        let semaphore = Semaphore::new(concurrency);
        let mut latencies: Vec<f64> = Vec::with_capacity(config.requests_per_level);
        let mut successful = 0usize;
        let mut failed = 0usize;

        let start_time = Instant::now();
        let mut tasks = JoinSet::new();

        // Warmup phase
        for _ in 0..config.warmup_requests {
            let _ = self.make_request(config).await;
        }

        // Actual test
        for i in 0..config.requests_per_level {
            let permit = semaphore.acquire().await?.clone();
            let config_clone = config.clone();
            let base_url = self.base_url.clone();
            let api_key = self.api_key.clone();

            tasks.spawn(async move {
                let req_start = Instant::now();
                let result = Self::make_request_with_config(&config_clone, &base_url, api_key.as_deref()).await;
                let elapsed = req_start.elapsed().as_millis() as f64;

                drop(permit); // Release semaphore

                (result.is_ok(), elapsed)
            });

            // Print progress every 10 requests
            if (i + 1) % 10 == 0 {
                print!(".");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }

        println!();

        // Collect results
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((success, latency)) => {
                    latencies.push(latency);
                    if success {
                        successful += 1;
                    } else {
                        failed += 1;
                    }
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }

        let test_duration = start_time.elapsed().as_secs_f64();

        // Calculate metrics
        let latency = calculate_latency_metrics(&mut latencies);
        let throughput = ThroughputMetrics {
            requests_per_second: successful as f64 / test_duration,
            successful_requests: successful,
            failed_requests: failed,
            total_requests: config.requests_per_level,
            success_rate: successful as f64 / config.requests_per_level as f64,
        };

        // Resource metrics (placeholder - would need actual monitoring integration)
        let resources = ResourceMetrics {
            avg_cpu_percent: None,
            peak_memory_mb: None,
            kv_cache_hit_rate: None,
            gpu_utilization: None,
        };

        Ok(PerformanceResult {
            test_name: config.name.clone(),
            concurrency,
            latency,
            throughput,
            resources,
            test_duration_secs: test_duration,
        })
    }

    /// Make a single request
    async fn make_request(&self, config: &PerformanceTestConfig) -> anyhow::Result<serde_json::Value> {
        Self::make_request_with_config(config, &self.base_url, self.api_key.as_deref()).await
    }

    async fn make_request_with_config(
        config: &PerformanceTestConfig,
        base_url: &str,
        api_key: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;

        let url = format!("{}{}", base_url, config.endpoint);

        let mut request = match config.method {
            HttpMethod::GET => client.get(&url),
            HttpMethod::POST => client.post(&url),
        };

        if let Some(ref key) = api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        if let Some(ref payload) = config.payload {
            request = request.json(payload);
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Request failed with status: {}", response.status());
        }

        let json: serde_json::Value = response.json().await?;
        Ok(json)
    }

    /// Calculate aggregate metrics across all concurrency levels
    fn calculate_aggregate_metrics(
        &self,
        config: &PerformanceTestConfig,
        results: &[PerformanceResult],
    ) -> AggregatePerformanceMetrics {
        // Find peak throughput
        let peak_throughput = results.iter()
            .map(|r| r.throughput.requests_per_second)
            .fold(0.0_f64, f64::max);

        // Calculate overall latency percentiles (across all concurrency levels)
        let mut all_latencies: Vec<f64> = results.iter()
            .flat_map(|r| {
                // Reconstruct approximate distribution from percentiles
                vec![r.latency.p50_ms, r.latency.p95_ms, r.latency.p99_ms]
            })
            .collect();
        all_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let overall_p50 = percentile(&all_latencies, 50);
        let overall_p95 = percentile(&all_latencies, 95);
        let overall_p99 = percentile(&all_latencies, 99);

        // Average success rate
        let avg_success_rate = results.iter()
            .map(|r| r.throughput.success_rate)
            .sum::<f64>() / results.len().max(1) as f64;

        // Scalability factor: compare throughput at max concurrency vs single concurrency
        let scalability_factor = if results.len() >= 2 {
            let single_rps = results[0].throughput.requests_per_second;
            let max_rps = results.last().unwrap().throughput.requests_per_second;
            let max_concurrency = *config.concurrency_levels.last().unwrap_or(&1) as f64;

            if single_rps > 0.0 {
                (max_rps / single_rps) / max_concurrency // 1.0 = linear scaling
            } else {
                0.0
            }
        } else {
            1.0
        };

        // Find optimal concurrency (best throughput/latency tradeoff)
        let optimal_concurrency = results.iter()
            .map(|r| {
                // Score = throughput / (p99 latency)^1.5
                let score = r.throughput.requests_per_second / (r.latency.p99_ms.powi(1));
                (r.concurrency, score)
            })
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(c, _)| c)
            .unwrap_or(1);

        AggregatePerformanceMetrics {
            timestamp: chrono::Utc::now().to_rfc3339(),
            carpai_url: self.base_url.clone(),
            test_config: config.clone(),
            results_by_concurrency: results.to_vec(),
            scalability_factor,
            optimal_concurrency,
            overall_p50_ms: overall_p50,
            overall_p95_ms: overall_p95,
            overall_p99_ms: overall_p99,
            peak_throughput_rps: peak_throughput,
            avg_success_rate,
        }
    }

    /// Print aggregate summary
    fn print_aggregate_summary(&self, aggregate: &AggregatePerformanceMetrics) {
        println!("\n{}", "=".repeat(80));
        println!("  PERFORMANCE SUMMARY: {}", aggregate.test_config.name);
        println!("{}", "=".repeat(80));

        println!("\n📊 Overall Latency:");
        println!("   P50:  {:.0}ms", aggregate.overall_p50_ms);
        println!("   P95:  {:.0}ms", aggregate.overall_p95_ms);
        println!("   P99:  {:.0}ms", aggregate.overall_p99_ms);

        println!("\n🚀 Throughput:");
        println!("   Peak: {:.1} req/s", aggregate.peak_throughput_rps);
        println!("   Optimal Concurrency: {} concurrent requests", aggregate.optimal_concurrency);
        println!("   Scalability Factor: {:.2} (1.0 = linear)", aggregate.scalability_factor);

        println!("\n✅ Reliability:");
        println!("   Avg Success Rate: {:.1}%", aggregate.avg_success_rate * 100.0);

        println!("\n📈 By Concurrency Level:");
        println!("   {:>12} | {:>8} | {:>8} | {:>8} | {:>10}",
            "Concurrency", "P50(ms)", "P95(ms)", "P99(ms)", "RPS");
        println!("   {}", "-".repeat(58));

        for result in &aggregate.results_by_concurrency {
            println!("   {:>12} | {:>8.0} | {:>8.0} | {:>8.0} | {:>10.1}",
                result.concurrency,
                result.latency.p50_ms,
                result.latency.p95_ms,
                result.latency.p99_ms,
                result.throughput.requests_per_second
            );
        }

        println!("{}", "=".repeat(80));
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn calculate_latency_metrics(latencies: &mut [f64]) -> LatencyMetrics {
    if latencies.is_empty() {
        return LatencyMetrics {
            p50_ms: 0.0,
            p90_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            min_ms: 0.0,
            max_ms: 0.0,
            mean_ms: 0.0,
            stddev_ms: 0.0,
        };
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let n = latencies.len();
    let min = latencies[0];
    let max = latencies[n - 1];
    let mean = latencies.iter().sum::<f64>() / n as f64;

    let variance = latencies.iter()
        .map(|x| (x - mean).powi(2))
        .sum::<f64>() / n as f64;
    let stddev = variance.sqrt();

    LatencyMetrics {
        p50_ms: percentile(latencies, 50),
        p90_ms: percentile(latencies, 90),
        p95_ms: percentile(latencies, 95),
        p99_ms: percentile(latencies, 99),
        min_ms: min,
        max_ms: max,
        mean_ms: mean,
        stddev_ms: stddev,
    }
}

fn percentile(sorted_data: &[f64], p: u32) -> f64 {
    if sorted_data.is_empty() {
        return 0.0;
    }

    let index = (p as f64 / 100.0 * sorted_data.len() as f64) as usize;
    let index = index.min(sorted_data.len() - 1);
    sorted_data[index]
}

// ============================================================================
// Test Entry Point
// ============================================================================

#[tokio::test]
async fn test_performance_baseline_benchmark() {
    let carpai_url = std::env::var("CARPAI_BENCHMARK_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string());

    let api_key = std::env::var("CARPAI_API_KEY").ok();

    let benchmark = PerformanceBaselineBenchmark::new(carpai_url, api_key);

    let results = benchmark.run().await.expect("Performance benchmark failed");

    // Print overall summary
    println!("\n\n{}", "=".repeat(80));
    println!("  OVERALL PERFORMANCE RESULTS");
    println!("{}", "=".repeat(80));

    for result in &results {
        println!("\nEndpoint: {}", result.test_config.name);
        println!("  Peak Throughput: {:.1} req/s", result.peak_throughput_rps);
        println!("  Overall P99: {:.0}ms", result.overall_p99_ms);
        println!("  Success Rate: {:.1}%", result.avg_success_rate * 100.0);
    }

    // Assertions for CI
    assert!(!results.is_empty(), "Should have at least one result");

    // Check that P99 latency is within acceptable range (< 5 seconds for chat)
    for result in &results {
        if result.test_config.name == "chat_completions" {
            assert!(result.overall_p99_ms < 5000.0,
                "P99 latency too high: {:.0}ms (target: <5000ms)", result.overall_p99_ms);
        }
    }
}
