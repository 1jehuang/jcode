//! Prometheus 监控指标模块
//! 
//! 提供标准化的 Prometheus 指标收集和暴露功能

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use prometheus_client::registry::Registry;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::encoding::text::encode;

#[derive(Clone)]
pub struct PrometheusMetrics {
    registry: Arc<RwLock<Registry>>,
    counters: Arc<RwLock<HashMap<String, Counter>>>,
    gauges: Arc<RwLock<HashMap<String, Gauge>>>,
    histograms: Arc<RwLock<HashMap<String, Histogram>>>,
}

impl PrometheusMetrics {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(Registry::default())),
            counters: Arc::new(RwLock::new(HashMap::new())),
            gauges: Arc::new(RwLock::new(HashMap::new())),
            histograms: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_counter(&self, name: &str, description: &str) {
        let mut counters = self.counters.write().await;
        if !counters.contains_key(name) {
            let counter = Counter::default();
            self.registry.write().await.register(
                name,
                description,
                counter.clone(),
            ).unwrap_or(());
            counters.insert(name.to_string(), counter);
        }
    }

    pub async fn increment_counter(&self, name: &str) {
        let counters = self.counters.read().await;
        if let Some(counter) = counters.get(name) {
            counter.inc();
        }
    }

    pub async fn register_gauge(&self, name: &str, description: &str) {
        let mut gauges = self.gauges.write().await;
        if !gauges.contains_key(name) {
            let gauge = Gauge::default();
            self.registry.write().await.register(
                name,
                description,
                gauge.clone(),
            ).unwrap_or(());
            gauges.insert(name.to_string(), gauge);
        }
    }

    pub async fn set_gauge(&self, name: &str, value: f64) {
        let gauges = self.gauges.read().await;
        if let Some(gauge) = gauges.get(name) {
            gauge.set(value);
        }
    }

    pub async fn register_histogram(&self, name: &str, description: &str) {
        let mut histograms = self.histograms.write().await;
        if !histograms.contains_key(name) {
            let histogram = Histogram::new([0.001, 0.01, 0.1, 1.0, 10.0, 60.0].into_iter());
            self.registry.write().await.register(
                name,
                description,
                histogram.clone(),
            ).unwrap_or(());
            histograms.insert(name.to_string(), histogram);
        }
    }

    pub async fn observe_histogram(&self, name: &str, value: f64) {
        let histograms = self.histograms.read().await;
        if let Some(histogram) = histograms.get(name) {
            histogram.observe(value);
        }
    }

    pub async fn encode(&self) -> String {
        let registry = self.registry.read().await;
        let mut output = String::new();
        encode(&mut output, &registry).unwrap_or(());
        output
    }
}

impl Default for PrometheusMetrics {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MetricsService {
    metrics: Arc<PrometheusMetrics>,
}

impl MetricsService {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(PrometheusMetrics::new()),
        }
    }

    pub async fn initialize_default_metrics(&self) {
        self.metrics.register_counter("jcode_requests_total", "Total number of requests").await;
        self.metrics.register_counter("jcode_errors_total", "Total number of errors").await;
        self.metrics.register_counter("jcode_sessions_created", "Total number of sessions created").await;
        self.metrics.register_counter("jcode_sessions_destroyed", "Total number of sessions destroyed").await;
        self.metrics.register_counter("jcode_completions_total", "Total number of completions").await;
        self.metrics.register_counter("jcode_completions_cache_hits", "Number of completion cache hits").await;
        self.metrics.register_counter("jcode_completions_cache_misses", "Number of completion cache misses").await;
        self.metrics.register_counter("jcode_provider_requests", "Number of provider requests").await;
        self.metrics.register_counter("jcode_provider_errors", "Number of provider errors").await;
        self.metrics.register_counter("jcode_sso_logins", "Number of SSO logins").await;
        self.metrics.register_counter("jcode_sso_failures", "Number of SSO failures").await;
        
        self.metrics.register_gauge("jcode_active_sessions", "Number of active sessions").await;
        self.metrics.register_gauge("jcode_memory_usage_bytes", "Memory usage in bytes").await;
        self.metrics.register_gauge("jcode_provider_latency_ms", "Provider latency in milliseconds").await;
        self.metrics.register_gauge("jcode_crdt_conflicts", "Number of CRDT conflicts").await;
        
        self.metrics.register_histogram("jcode_request_duration_seconds", "Request duration in seconds").await;
        self.metrics.register_histogram("jcode_completion_duration_seconds", "Completion duration in seconds").await;
        self.metrics.register_histogram("jcode_provider_request_duration_seconds", "Provider request duration").await;
        self.metrics.register_histogram("jcode_crdt_operation_duration_seconds", "CRDT operation duration").await;
    }

    pub fn get_metrics(&self) -> Arc<PrometheusMetrics> {
        self.metrics.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_counter() {
        let metrics = PrometheusMetrics::new();
        metrics.register_counter("test_counter", "Test counter").await;
        metrics.increment_counter("test_counter").await;
        metrics.increment_counter("test_counter").await;
        
        let output = metrics.encode().await;
        assert!(output.contains("test_counter 2"));
    }

    #[tokio::test]
    async fn test_gauge() {
        let metrics = PrometheusMetrics::new();
        metrics.register_gauge("test_gauge", "Test gauge").await;
        metrics.set_gauge("test_gauge", 42.0).await;
        
        let output = metrics.encode().await;
        assert!(output.contains("test_gauge 42"));
    }

    #[tokio::test]
    async fn test_histogram() {
        let metrics = PrometheusMetrics::new();
        metrics.register_histogram("test_histogram", "Test histogram").await;
        metrics.observe_histogram("test_histogram", 0.5).await;
        metrics.observe_histogram("test_histogram", 1.5).await;
        
        let output = metrics.encode().await;
        assert!(output.contains("test_histogram"));
    }
}