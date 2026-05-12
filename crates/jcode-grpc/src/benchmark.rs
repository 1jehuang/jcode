//! Performance Benchmarking Framework for CarpAI
//!
//! Comprehensive benchmarking system to measure and compare performance
//! against Cursor, CodeBuddy, and other AI coding assistants.
//!
//! ## Metrics Tracked:
//! - Latency (P50, P95, P99)
//! - Throughput (requests/second)
//! - Token generation speed (tokens/second)
//! - Memory usage
//! - CPU utilization
//! - Error rates
//! - Streaming first-byte time

use std::time::{Duration, Instant};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};
use tracing::{info, warn, debug};

/// Benchmark configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    /// Number of concurrent connections/clients
    pub concurrency: usize,
    
    /// Total duration of benchmark run
    pub duration: Duration,
    
    /// Warm-up period before measurements start
    pub warmup_duration: Duration,
    
    /// Requests per second rate limit (0 = unlimited)
    pub rps_limit: u32,
    
    /// Enable detailed profiling
    pub profiling_enabled: bool,
    
    /// Output format for results
    pub output_format: OutputFormat,
    
    /// Custom labels for this benchmark run
    pub labels: HashMap<String, String>,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            concurrency: 10,
            duration: Duration::from_secs(60),
            warmup_duration: Duration::from_secs(5),
            rps_limit: 0,
            profiling_enabled: false,
            output_format: OutputFormat::Json,
            labels: HashMap::new(),
        }
    }
}

/// Output format for benchmark results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputFormat {
    Json,
    Csv,
    Prometheus,
    Custom(String),
}

/// Single request measurement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMeasurement {
    /// Unique request ID
    pub request_id: String,
    
    /// Timestamp when request was sent
    pub timestamp_start: u64,
    
    /// Timestamp when response was fully received
    pub timestamp_end: u64,
    
    /// Total latency in milliseconds
    pub latency_ms: f64,
    
    /// Time to first byte (for streaming) in milliseconds
    pub ttfb_ms: Option<f64>,
    
    /// Number of tokens generated (if applicable)
    pub tokens_generated: Option<u32>,
    
    /// Tokens per second (throughput metric)
    pub tokens_per_second: Option<f64>,
    
    /// HTTP/gRPC status code
    pub status_code: u16,
    
    /// Error message (if failed)
    pub error: Option<String>,
    
    /// Request size in bytes
    pub request_size_bytes: usize,
    
    /// Response size in bytes
    pub response_size_bytes: usize,
    
    /// Custom metadata
    pub metadata: HashMap<String, String>,
}

/// Aggregated benchmark statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkStatistics {
    /// Total number of requests
    pub total_requests: usize,
    
    /// Successful requests
    pub successful_requests: usize,
    
    /// Failed requests
    pub failed_requests: usize,
    
    /// Success rate (0.0 - 1.0)
    pub success_rate: f64,
    
    // Latency statistics (in milliseconds)
    pub latency_min_ms: f64,
    pub latency_max_ms: f64,
    pub latency_mean_ms: f64,
    pub latency_median_ms: f64,
    pub latency_p50_ms: f64,
    pub latency_p90_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_p99_ms: f64,
    pub latency_std_dev_ms: f64,
    
    // Throughput statistics
    pub requests_per_second: f64,
    pub tokens_per_second: f64,
    
    // Time to First Byte (streaming)
    pub ttfb_p50_ms: Option<f64>,
    pub ttfb_p95_ms: Option<f64>,
    pub ttfb_p99_ms: Option<f64>,
    
    // Resource utilization
    pub memory_usage_mb: f64,
    pub cpu_usage_percent: f64,
    
    // Timing information
    pub total_duration_secs: f64,
    pub warmup_duration_secs: f64,
    
    // Configuration used
    pub config: BenchmarkConfig,
    
    // Labels/metadata
    pub labels: HashMap<String, String>,
    
    /// Timestamp of benchmark completion
    pub completed_at: String,
}

/// Histogram bucket for latency distribution
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistogramBucket {
    pub upper_bound: f64,
    pub count: u64,
}

/// Latency histogram
struct LatencyHistogram {
    buckets: Vec<HistogramBucket>,
    total_count: u64,
    min_value: f64,
    max_value: f64,
    sum: f64,
    sum_of_squares: f64,
}

impl LatencyHistogram {
    fn new() -> Self {
        Self {
            buckets: vec![
                HistogramBucket { upper_bound: 1.0, count: 0 },
                HistogramBucket { upper_bound: 5.0, count: 0 },
                HistogramBucket { upper_bound: 10.0, count: 0 },
                HistogramBucket { upper_bound: 25.0, count: 0 },
                HistogramBucket { upper_bound: 50.0, count: 0 },
                HistogramBucket { upper_bound: 100.0, count: 0 },
                HistogramBucket { upper_bound: 250.0, count: 0 },
                HistogramBucket { upper_bound: 500.0, count: 0 },
                HistogramBucket { upper_bound: 1000.0, count: 0 },
                HistogramBucket { upper_bound: f64::MAX, count: 0 },
            ],
            total_count: 0,
            min_value: f64::MAX,
            max_value: 0.0,
            sum: 0.0,
            sum_of_squares: 0.0,
        }
    }
    
    fn record(&mut self, value: f64) {
        self.total_count += 1;
        self.min_value = self.min_value.min(value);
        self.max_value = self.max_value.max(value);
        self.sum += value;
        self.sum_of_squares += value * value;
        
        for bucket in &mut self.buckets {
            if value <= bucket.upper_bound {
                bucket.count += 1;
                break;
            }
        }
    }
    
    fn percentile(&self, p: f64) -> f64 {
        if self.total_count == 0 {
            return 0.0;
        }
        
        let target_count = (p / 100.0 * self.total_count as f64).ceil() as u64;
        let mut cumulative = 0u64;
        
        for bucket in &self.buckets {
            cumulative += bucket.count;
            if cumulative >= target_count {
                return bucket.upper_bound;
            }
        }
        
        self.buckets.last().map(|b| b.upper_bound).unwrap_or(0.0)
    }
    
    fn mean(&self) -> f64 {
        if self.total_count == 0 {
            return 0.0;
        }
        self.sum / self.total_count as f64
    }
    
    fn std_dev(&self) -> f64 {
        if self.total_count <= 1 {
            return 0.0;
        }
        let variance = (self.sum_of_squares - self.sum * self.mean()) / (self.total_count as f64 - 1.0);
        variance.sqrt()
    }
    
    fn median(&self) -> f64 {
        self.percentile(50.0)
    }
}

/// Main benchmark runner
pub struct BenchmarkRunner {
    config: BenchmarkConfig,
    measurements: Arc<RwLock<Vec<RequestMeasurement>>>,
    latency_histogram: Arc<RwLock<LatencyHistogram>>,
    ttfb_histogram: Arc<RwLock<LatencyHistogram>>,
    start_time: Instant,
    is_running: bool,
}

impl BenchmarkRunner {
    /// Create new benchmark runner with given configuration
    pub fn new(config: BenchmarkConfig) -> Self {
        Self {
            config,
            measurements: Arc::new(RwLock::new(Vec::new())),
            latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new())),
            ttfb_histogram: Arc::new(RwLock::new(LatencyHistogram::new())),
            start_time: Instant::now(),
            is_running: false,
        }
    }
    
    /// Run the full benchmark suite
    pub async fn run_benchmark<F, Fut>(
        &mut self,
        request_generator: F,
    ) -> Result<BenchmarkStatistics>
    where
        F: Fn() -> Fut + Send + Sync + 'static + Clone,
        Fut: std::future::Future<Output = Result<RequestMeasurement>> + Send + 'static,
    {
        info!(
            concurrency = self.config.concurrency,
            duration_secs = self.config.duration.as_secs(),
            "Starting CarpAI benchmark"
        );
        
        self.is_running = true;
        self.start_time = Instant::now();
        
        // Phase 1: Warm-up
        info!(duration_secs = self.config.warmup_duration.as_secs(), "Running warm-up phase");
        self.run_warmup_phase(&request_generator).await?;
        
        // Phase 2: Measurement
        info!("Starting measurement phase");
        self.run_measurement_phase(&request_generator).await?;
        
        // Phase 3: Generate statistics
        let stats = self.generate_statistics().await;
        
        self.is_running = false;
        
        Ok(stats)
    }
    
    /// Run warm-up phase (measurements discarded)
    async fn run_warmup_phase<F, Fut>(&self, generator: &F) -> Result<()>
    where
        F: Fn() -> Fut + Send + Sync + 'static + Clone,
        Fut: std::future::Future<Output = Result<RequestMeasurement>> + Send + 'static,
    {
        let warmup_end = self.start_time + self.config.warmup_duration;
        let mut tasks = tokio::task::JoinSet::new();
        
        while Instant::now() < warmup_end {
            // Spawn up to `concurrency` tasks
            while tasks.len() < self.config.concurrency && Instant::now() < warmup_end {
                let gen = generator.clone();
                tasks.spawn(async move {
                    let _ = gen().await; // Discard warm-up results
                });
                
                // Rate limiting if configured
                if self.config.rps_limit > 0 {
                    tokio::time::sleep(Duration::from_millis(1000 / self.config.rps_limit)).await;
                }
            }
            
            // Wait for at least one task to complete
            if let Some(result) = tasks.join_next().await {
                result?; // Propagate panics
            }
        }
        
        // Abort remaining warm-up tasks
        tasks.abort_all();
        
        debug!("Warm-up phase complete");
        Ok(())
    }
    
    /// Run main measurement phase
    async fn run_measurement_phase<F, Fut>(&self, generator: &F) -> Result<()>
    where
        F: Fn() -> Fut + Send + Sync + 'static + Clone,
        Fut: std::future::Future<Output = Result<RequestMeasurement>> + Send + 'static,
    {
        let measure_end = self.start_time + self.config.warmup_duration + self.config.duration;
        let measurements = Arc::clone(&self.measurements);
        let latencies = Arc::clone(&self.latency_histogram);
        let ttfbs = Arc::clone(&self.ttfb_histogram);
        
        let mut tasks = tokio::task::JoinSet::new();
        
        while Instant::now() < measure_end {
            // Spawn tasks up to concurrency limit
            while tasks.len() < self.config.concurrency && Instant::now() < measure_end {
                let gen = generator.clone();
                let meas = Arc::clone(&measurements);
                let lat = Arc::clone(&latencies);
                let ttfb = Arc::clone(&ttfbs);
                
                tasks.spawn(async move {
                    match gen().await {
                        Ok(measurement) => {
                            // Record latency
                            lat.write().await.record(measurement.latency_ms);
                            
                            // Record TTFB if available
                            if let Some(ttfb_val) = measurement.ttfb_ms {
                                ttfb.write().await.record(ttfb_val);
                            }
                            
                            // Store measurement
                            meas.write().await.push(measurement);
                        }
                        Err(e) => {
                            warn!(error = %e, "Benchmark request failed");
                        }
                    }
                });
                
                // Rate limiting
                if self.config.rps_limit > 0 {
                    tokio::time::sleep(Duration::from_millis(1000 / self.config.rps_limit)).await;
                }
            }
            
            // Wait for task completion
            if let Some(result) = tasks.join_next().await {
                result?; // Propagate panics
            }
        }
        
        // Wait for all remaining tasks to complete
        while let Some(result) = tasks.join_next().await {
            result?;
        }
        
        info!(
            total_measurements = self.measurements.read().await.len(),
            "Measurement phase complete"
        );
        
        Ok(())
    }
    
    /// Generate final statistics from collected measurements
    async fn generate_statistics(&self) -> BenchmarkStatistics {
        let measurements = self.measurements.read().await;
        let latencies = self.latency_histogram.read().await;
        let ttfbs = self.ttfb_histogram.read().await;
        
        let total = measurements.len();
        let successful = measurements.iter().filter(|m| m.status_code >= 200 && m.status_code < 300).count();
        let failed = total - successful;
        
        let total_tokens: u32 = measurements.iter()
            .filter_map(|m| m.tokens_generated)
            .sum();
        
        let elapsed = self.start_time.elapsed();
        
        let stats = BenchmarkStatistics {
            total_requests: total,
            successful_requests: successful,
            failed_requests: failed,
            success_rate: if total > 0 { successful as f64 / total as f64 } else { 0.0 },
            
            latency_min_ms: latencies.min_value,
            latency_max_ms: latencies.max_value,
            latency_mean_ms: latencies.mean(),
            latency_median_ms: latencies.median(),
            latency_p50_ms: latencies.percentile(50.0),
            latency_p90_ms: latencies.percentile(90.0),
            latency_p95_ms: latencies.percentile(95.0),
            latency_p99_ms: latencies.percentile(99.0),
            latency_std_dev_ms: latencies.std_dev(),
            
            requests_per_second: if elapsed.as_secs_f64() > 0.0 {
                total as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            },
            tokens_per_second: if elapsed.as_secs_f64() > 0.0 {
                total_tokens as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            },
            
            ttfb_p50_ms: Some(ttfbs.percentile(50.0)),
            ttfb_p95_ms: Some(ttfbs.percentile(95.0)),
            ttfb_p99_ms: Some(ttfbs.percentile(99.0)),
            
            memory_usage_mb: self.get_memory_usage().await,
            cpu_usage_percent: self.get_cpu_usage().await,
            
            total_duration_secs: elapsed.as_secs_f64(),
            warmup_duration_secs: self.config.warmup_duration.as_secs_f64(),
            
            config: self.config.clone(),
            labels: self.config.labels.clone(),
            
            completed_at: chrono::Utc::now().to_rfc3339(),
        };
        
        stats
    }
    
    /// Get current memory usage in MB
    async fn get_memory_usage(&self) -> f64 {
        #[cfg(target_os = "linux")]
        {
            use sysinfo::{System, SystemExt};
            let mut sys = System::new_all();
            sys.refresh_processes();
            let process = sys.processes()
                .values()
                .find(|p| p.name() == "jcode" || p.name() == "carpai");
            
            process.map(|p| p.memory() as f64 / 1024.0 / 1024.0)
                .unwrap_or(0.0)
        }
        
        #[cfg(target_os = "windows")]
        {
            // Windows memory usage (simplified)
            0.0 // Would need winapi or similar
        }
        
        #[cfg(target_os = "macos")]
        {
            // macOS memory usage (simplified)
            0.0
        }
        
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            0.0
        }
    }
    
    /// Get current CPU usage percentage
    async fn get_cpu_usage(&self) -> f64 {
        // Simplified implementation
        // In production, would use sysinfo or platform-specific APIs
        0.0
    }
    
    /// Export results to file
    pub async fn export_results(
        &self,
        stats: &BenchmarkStatistics,
        path: impl AsRef<std::path::Path>,
    ) -> Result<()> {
        let path = path.as_ref();
        
        match &self.config.output_format {
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(stats)?;
                std::fs::write(path, json)?;
            }
            OutputFormat::Csv => {
                let csv = self.stats_to_csv(stats);
                std::fs::write(path, csv)?;
            }
            OutputFormat::Prometheus => {
                let prometheus = self.stats_to_prometheus(stats);
                std::fs::write(path, prometheus)?;
            }
            OutputFormat::Custom(format) => {
                // Custom format handler would go here
                warn!(format = %format, "Custom output format not implemented");
            }
        }
        
        info!(path = %path.display(), "Results exported successfully");
        Ok(())
    }
    
    /// Convert statistics to CSV format
    fn stats_to_csv(&self, stats: &BenchmarkStatistics) -> String {
        format!(
            "metric,value\n\
             total_requests,{}\n\
             success_rate,{:.4}\n\
             requests_per_second,{:.2}\n\
             tokens_per_second,{:.2}\n\
             latency_min_ms,{:.2}\n\
             latency_mean_ms,{:.2}\n\
             latency_p50_ms,{:.2}\n\
             latency_p95_ms,{:.2}\n\
             latency_p99_ms,{:.2}\n",
            stats.total_requests,
            stats.success_rate,
            stats.requests_per_second,
            stats.tokens_per_second,
            stats.latency_min_ms,
            stats.latency_mean_ms,
            stats.latency_p50_ms,
            stats.latency_p95_ms,
            stats.latency_p99_ms,
        )
    }
    
    /// Convert statistics to Prometheus exposition format
    fn stats_to_prometheus(&self, stats: &BenchmarkStatistics) -> String {
        let labels_str = if !stats.labels.is_empty() {
            let labels: Vec<String> = stats.labels.iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            format!{{{}}}, labels.join(","))
        } else {
            String::new()
        };
        
        format!(
            "# HELP carpai_requests_total Total number of requests\n\
             # TYPE carpai_requests_total gauge\n\
             carpai_requests_total{} {}\n\n\
             # HELP carpai_success_rate Success rate of requests\n\
             # TYPE carpai_success_rate gauge\n\
             carpai_success_rate{:.4} {}\n\n\
             # HELP carpai_latency_seconds Request latency in seconds\n\
             # TYPE carpai_latency_seconds summary\n\
             carpai_latency_seconds{{quantile=\"0.5\"{}}} {:.6}\n\
             carpai_latency_seconds{{quantile=\"0.95\"{}}} {:.6}\n\
             carpai_latency_seconds{{quantile=\"0.99\"{}}} {:.6}\n\n\
             # HELP carpai_throughput Requests per second\n\
             # TYPE carpai_throughput gauge\n\
             carpai_throughput{} {:.2}\n",
            labels_str,
            stats.total_requests,
            labels_str,
            stats.success_rate,
            labels_str,
            stats.latency_p50_ms / 1000.0,
            labels_str,
            stats.latency_p95_ms / 1000.0,
            labels_str,
            stats.latency_p99_ms / 1000.0,
            labels_str,
            stats.requests_per_second,
        )
    }
}

/// Predefined benchmark scenarios
pub mod scenarios {
    use super::*;
    
    /// Chat completion benchmark scenario
    pub async fn chat_completion_benchmark(
        server_url: &str,
        model: &str,
        messages: Vec<HashMap<String, String>>,
    ) -> Result<BenchmarkStatistics> {
        use reqwest::Client;
        
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        
        let url = format!("{}/v1/chat/completions", server_url);
        
        let config = BenchmarkConfig {
            concurrency: 5,
            duration: Duration::from_secs(30),
            ..Default::default()
        };
        
        let mut runner = BenchmarkRunner::new(config);
        
        runner.run_benchmark(move || {
            let client = client.clone();
            let url = url.clone();
            let model = model.to_string();
            let messages = messages.clone();
            
            async move {
                let start = Instant::now();
                
                let body = serde_json::json!({
                    "model": model,
                    "messages": messages,
                    "max_tokens": 150,
                    "temperature": 0.7,
                    "stream": false,
                });
                
                let response = client.post(&url)
                    .json(&body)
                    .send()
                    .await?;
                
                let status = response.status().as_u16();
                let response_bytes = response.content_length().unwrap_or(0) as usize;
                
                let json: serde_json::Value = response.json().await?;
                let content = json["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                
                let tokens = json["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;
                
                let latency_ms = start.elapsed().as_millis() as f64;
                
                Ok(RequestMeasurement {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    timestamp_start: 0,
                    timestamp_end: 0,
                    latency_ms,
                    ttfb_ms: None,
                    tokens_generated: Some(tokens),
                    tokens_per_second: if latency_ms > 0.0 { Some(tokens as f64 / (latency_ms / 1000.0)) } else { None },
                    status_code: status,
                    error: None,
                    request_size_bytes: body.to_string().len(),
                    response_size_bytes: response_bytes,
                    metadata: [
                        ("model".to_string(), model.clone()),
                        ("response_length".to_string(), content.len().to_string()),
                    ].into_iter().collect(),
                })
            }
        }).await
    }
    
    /// Streaming chat benchmark scenario
    pub async fn streaming_chat_benchmark(
        server_url: &str,
        model: &str,
        prompt: &str,
    ) -> Result<BenchmarkStatistics> {
        use futures::StreamExt;
        
        let config = BenchmarkConfig {
            concurrency: 10,
            duration: Duration::from_secs(60),
            ..Default::default()
        };
        
        let mut runner = BenchmarkRunner::new(config);
        
        runner.run_benchmark(move || {
            let server_url = server_url.to_string();
            let model = model.to_string();
            let prompt = prompt.to_string();
            
            async move {
                use jcode_llm::LlmProviderFactory;
                use jcode_llm::presets::*;
                
                let provider = LlmProviderFactory::create_provider(deepseek_chat());
                
                let request = jcode_llm::types::ChatCompletionRequest {
                    model: model.clone(),
                    messages: vec![jcode_llm::types::ChatMessage {
                        role: jcode_llm::types::MessageRole::User,
                        content: Some(prompt.clone()),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    }],
                    temperature: Some(0.7),
                    max_tokens: Some(200),
                    top_p: None,
                    tools: None,
                    stream: Some(true),
                    stop: None,
                };
                
                let start = Instant::now();
                let mut first_byte_time: Option<f64> = None;
                let mut total_tokens: u32 = 0;
                
                let mut stream = provider.chat_completion_stream(request).await?;
                
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    
                    // Record TTFB on first chunk
                    if first_byte_time.is_none() {
                        first_byte_time = Some(start.elapsed().as_millis() as f64);
                    }
                    
                    // Count tokens
                    if let Some(choices) = chunk.choices.first() {
                        if let Some(content) = &choices.delta.content {
                            total_tokens += content.split_whitespace().count() as u32;
                        }
                    }
                }
                
                let latency_ms = start.elapsed().as_millis() as f64;
                
                Ok(RequestMeasurement {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    timestamp_start: 0,
                    timestamp_end: 0,
                    latency_ms,
                    ttfb_ms: first_byte_time,
                    tokens_generated: Some(total_tokens),
                    tokens_per_second: if latency_ms > 0.0 { 
                        Some(total_tokens as f64 / (latency_ms / 1000.0)) 
                    } else { 
                        None 
                    },
                    status_code: 200,
                    error: None,
                    request_size_bytes: prompt.len(),
                    response_size_bytes: total_tokens as usize * 4, // Rough estimate
                    metadata: [
                        ("model".to_string(), model),
                        ("streaming".to_string(), "true".to_string()),
                    ].into_iter().collect(),
                })
            }
        }).await
    }
    
    /// Embedding generation benchmark
    pub async fn embedding_benchmark(
        server_url: &str,
        texts: Vec<String>,
    ) -> Result<BenchmarkStatistics> {
        let config = BenchmarkConfig {
            concurrency: 20,
            duration: Duration::from_secs(45),
            ..Default::default()
        };
        
        let mut runner = BenchmarkRunner::new(config);
        
        runner.run_benchmark(move || {
            let server_url = server_url.to_string();
            let texts = texts.clone();
            
            async move {
                let idx = rand::random::<usize>() % texts.len();
                let text = &texts[idx];
                
                let start = Instant::now();
                
                let client = reqwest::Client::new();
                let url = format!("{}/v1/embeddings", server_url);
                
                let body = serde_json::json!({
                    "model": "text-embedding-ada-002",
                    "input": [text],
                });
                
                let response = client.post(&url)
                    .json(&body)
                    .send()
                    .await?;
                
                let status = response.status().as_u64() as u16;
                let latency_ms = start.elapsed().as_millis() as f64;
                
                Ok(RequestMeasurement {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    timestamp_start: 0,
                    timestamp_end: 0,
                    latency_ms,
                    ttfb_ms: None,
                    tokens_generated: None,
                    tokens_per_second: None,
                    status_code: status,
                    error: None,
                    request_size_bytes: text.len(),
                    response_size_bytes: 1536, // Typical embedding size
                    metadata: HashMap::new(),
                })
            }
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_benchmark_runner_basic() {
        let config = BenchmarkConfig {
            concurrency: 2,
            duration: Duration::from_secs(1),
            warmup_duration: Duration::from_millis(100),
            ..Default::default()
        };
        
        let mut runner = BenchmarkRunner::new(config);
        
        let result = runner.run_benchmark(|| async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            
            Ok(RequestMeasurement {
                request_id: uuid::Uuid::new_v4().to_string(),
                timestamp_start: 0,
                timestamp_end: 0,
                latency_ms: 10.0,
                ttfb_ms: Some(5.0),
                tokens_generated: Some(15),
                tokens_per_second: Some(1500.0),
                status_code: 200,
                error: None,
                request_size_bytes: 100,
                response_size_bytes: 500,
                metadata: HashMap::new(),
            })
        }).await.unwrap();
        
        assert!(result.total_requests > 0);
        assert!(result.success_rate > 0.9);
        assert!(result.latency_mean_ms > 0.0);
    }
}
