//! OpenTelemetry Unified Observability
//!
//! Provides unified export for:
//! - Traces → Jaeger/Tempo
//! - Metrics → Prometheus/VictoriaMetrics
//! - Logs → Loki/Elasticsearch

pub mod config;
pub mod logger;
pub mod metrics;
pub mod tracer;

pub use config::ObservabilityConfig;
pub use logger::{init_logger, shutdown_logger};
pub use metrics::{init_metrics, shutdown_metrics};
pub use tracer::{init_tracer, shutdown_tracer};
