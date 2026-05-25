//! Prometheus metrics collection for CarpAI Server
//!
//! This module provides metrics for monitoring:
//! - Request counts and latencies
//! - Token usage
//! - Error rates
//! - Active sessions and connections

use prometheus_client::metrics::{counter::Counter, gauge::Gauge, histogram::Histogram};
use prometheus_client::registry::Registry;
use std::sync::Arc;

/// Central metrics collector for the server
pub struct MetricsCollector {
    /// Prometheus registry
    pub registry: Registry,

    // === HTTP/gRPC Request Metrics ===
    /// Total HTTP requests by path and method
    pub http_requests_total: Counter,

    /// HTTP request duration in seconds
    pub http_request_duration_seconds: Histogram,

    /// Total gRPC requests by service and method
    pub grpc_requests_total: Counter,

    /// gRPC request duration in seconds
    pub grpc_request_duration_seconds: Histogram,

    // === Agent Metrics ===
    /// Total agent turns executed
    pub agent_turns_total: Counter,

    /// Agent turn duration in seconds
    pub agent_turn_duration_seconds: Histogram,

    /// Current active agent turns
    pub active_agent_turns: Gauge,

    // === Token Metrics ===
    /// Total tokens processed (prompt)
    pub tokens_prompt_total: Counter,

    /// Total tokens generated (completion)
    pub tokens_completion_total: Counter,

    /// Current token processing rate (tokens/sec)
    pub tokens_per_second: Gauge,

    // === Session Metrics ===
    /// Total active sessions
    pub active_sessions: Gauge,

    /// Total sessions created
    pub sessions_created_total: Counter,

    // === Tool Metrics ===
    /// Total tool executions
    pub tool_executions_total: Counter,

    /// Tool execution failures
    pub tool_execution_failures_total: Counter,

    /// Tool execution duration in seconds
    pub tool_execution_duration_seconds: Histogram,

    // === Error Metrics ===
    /// Total errors by type
    pub errors_total: Counter,

    // === Quota Metrics ===
    /// Total quota exceeded events
    pub quota_exceeded_total: Counter,

    /// Rate limit exceeded events
    pub rate_limit_exceeded_total: Counter,
}

impl MetricsCollector {
    /// Create a new metrics collector with all metrics registered
    pub fn new() -> Self {
        let mut registry = Registry::default();

        let http_requests_total = Counter::default();
        registry.register(
            "http_requests_total",
            "Total HTTP requests",
            http_requests_total.clone(),
        );

        let http_request_duration_seconds = Histogram::new([0.001, 0.01, 0.1, 1.0, 10.0, 60.0].into_iter());
        registry.register(
            "http_request_duration_seconds",
            "HTTP request duration",
            http_request_duration_seconds.clone(),
        );

        let grpc_requests_total = Counter::default();
        registry.register(
            "grpc_requests_total",
            "Total gRPC requests",
            grpc_requests_total.clone(),
        );

        let grpc_request_duration_seconds = Histogram::new([0.001, 0.01, 0.1, 1.0, 10.0, 60.0].into_iter());
        registry.register(
            "grpc_request_duration_seconds",
            "gRPC request duration",
            grpc_request_duration_seconds.clone(),
        );

        let agent_turns_total = Counter::default();
        registry.register(
            "agent_turns_total",
            "Total agent turns executed",
            agent_turns_total.clone(),
        );

        let agent_turn_duration_seconds = Histogram::new([0.001, 0.01, 0.1, 1.0, 10.0, 60.0].into_iter());
        registry.register(
            "agent_turn_duration_seconds",
            "Agent turn duration",
            agent_turn_duration_seconds.clone(),
        );

        let active_agent_turns = Gauge::default();
        registry.register(
            "active_agent_turns",
            "Currently active agent turns",
            active_agent_turns.clone(),
        );

        let tokens_prompt_total = Counter::default();
        registry.register(
            "tokens_prompt_total",
            "Total prompt tokens processed",
            tokens_prompt_total.clone(),
        );

        let tokens_completion_total = Counter::default();
        registry.register(
            "tokens_completion_total",
            "Total completion tokens generated",
            tokens_completion_total.clone(),
        );

        let tokens_per_second = Gauge::default();
        registry.register(
            "tokens_per_second",
            "Current token processing rate",
            tokens_per_second.clone(),
        );

        let active_sessions = Gauge::default();
        registry.register(
            "active_sessions",
            "Currently active sessions",
            active_sessions.clone(),
        );

        let sessions_created_total = Counter::default();
        registry.register(
            "sessions_created_total",
            "Total sessions created",
            sessions_created_total.clone(),
        );

        let tool_executions_total = Counter::default();
        registry.register(
            "tool_executions_total",
            "Total tool executions",
            tool_executions_total.clone(),
        );

        let tool_execution_failures_total = Counter::default();
        registry.register(
            "tool_execution_failures_total",
            "Total tool execution failures",
            tool_execution_failures_total.clone(),
        );

        let tool_execution_duration_seconds = Histogram::new([0.001, 0.01, 0.1, 1.0, 10.0, 60.0].into_iter());
        registry.register(
            "tool_execution_duration_seconds",
            "Tool execution duration",
            tool_execution_duration_seconds.clone(),
        );

        let errors_total = Counter::default();
        registry.register(
            "errors_total",
            "Total errors by type",
            errors_total.clone(),
        );

        let quota_exceeded_total = Counter::default();
        registry.register(
            "quota_exceeded_total",
            "Total quota exceeded events",
            quota_exceeded_total.clone(),
        );

        let rate_limit_exceeded_total = Counter::default();
        registry.register(
            "rate_limit_exceeded_total",
            "Rate limit exceeded events",
            rate_limit_exceeded_total.clone(),
        );

        Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            grpc_requests_total,
            grpc_request_duration_seconds,
            agent_turns_total,
            agent_turn_duration_seconds,
            active_agent_turns,
            tokens_prompt_total,
            tokens_completion_total,
            tokens_per_second,
            active_sessions,
            sessions_created_total,
            tool_executions_total,
            tool_execution_failures_total,
            tool_execution_duration_seconds,
            errors_total,
            quota_exceeded_total,
            rate_limit_exceeded_total,
        }
    }

    /// Get the metrics as a shared Arc instance
    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Register all metrics with the global registry
pub fn register_metrics() -> Arc<MetricsCollector> {
    MetricsCollector::new().into_arc()
}

/// Helper to record HTTP request metrics
pub fn record_http_request(path: &str, method: &str, duration_secs: f64, status: u16) {
    // This would be called from middleware
    let _ = (path, method, duration_secs, status);
}
