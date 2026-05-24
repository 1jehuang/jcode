//! Observability (Metrics + Tracing + Health)
//!
//! This module provides observability infrastructure for CarpAI Server:
//! - Prometheus metrics collection
//! - OpenTelemetry distributed tracing
//! - gRPC/HTTP health checks

pub mod metrics;
pub mod tracing;
pub mod health;

pub use metrics::{MetricsCollector, register_metrics};
pub use tracing::init_tracing;
pub use health::{HealthChecker, HealthStatus, ServiceHealth};
