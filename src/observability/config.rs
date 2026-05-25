//! OpenTelemetry Configuration

use serde::{Deserialize, Serialize};

/// Unified observability configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    /// Service name for all telemetry data
    pub service_name: String,

    /// Service version
    pub service_version: String,

    /// Tracing configuration
    pub tracing: TracingConfig,

    /// Metrics configuration
    pub metrics: MetricsConfig,

    /// Logging configuration
    pub logging: LoggingConfig,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            service_name: "carpai".to_string(),
            service_version: env!("CARGO_PKG_VERSION").to_string(),
            tracing: TracingConfig::default(),
            metrics: MetricsConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

/// Tracing (distributed traces) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Enable tracing
    pub enabled: bool,

    /// OTLP endpoint for traces (e.g., http://localhost:4317)
    pub otlp_endpoint: String,

    /// Sampler ratio (0.0 - 1.0)
    pub sample_ratio: f64,

    /// Export interval in seconds
    pub export_interval_secs: u64,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            otlp_endpoint: "http://localhost:4317".to_string(),
            sample_ratio: 1.0, // Sample all traces by default
            export_interval_secs: 5,
        }
    }
}

/// Metrics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Enable metrics export
    pub enabled: bool,

    /// Prometheus scrape endpoint port
    pub prometheus_port: u16,

    /// Whether to export metrics via OTLP
    pub export_otlp: bool,

    /// OTLP endpoint for metrics (alternative to Prometheus)
    pub otlp_endpoint: Option<String>,

    /// Export interval in seconds
    pub export_interval_secs: u64,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            prometheus_port: 9090,
            export_otlp: false,
            otlp_endpoint: None,
            export_interval_secs: 10,
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,

    /// Enable JSON structured logging
    pub json_format: bool,

    /// OTLP endpoint for logs (optional)
    pub otlp_endpoint: Option<String>,

    /// Loki endpoint for logs (optional)
    pub loki_endpoint: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            json_format: false,
            otlp_endpoint: None,
            loki_endpoint: None,
        }
    }
}

impl ObservabilityConfig {
    /// Load from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("OTEL_SERVICE_NAME") {
            config.service_name = val;
        }

        if let Ok(val) = std::env::var("OTEL_TRACING_ENABLED") {
            config.tracing.enabled = val.parse().unwrap_or(false);
        }

        if let Ok(val) = std::env::var("OTEL_OTLP_ENDPOINT") {
            config.tracing.otlp_endpoint = val.clone();
            config.metrics.otlp_endpoint = Some(val);
        }

        if let Ok(val) = std::env::var("OTEL_PROMETHEUS_PORT") {
            config.metrics.prometheus_port = val.parse().unwrap_or(9090);
        }

        if let Ok(val) = std::env::var("RUST_LOG") {
            config.logging.level = val;
        }

        if let Ok(val) = std::env::var("OTEL_LOG_JSON") {
            config.logging.json_format = val.parse().unwrap_or(false);
        }

        config
    }
}
