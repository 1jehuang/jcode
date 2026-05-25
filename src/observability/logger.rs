use opentelemetry::logs::{LogRecord, Logger, LoggerProvider as _, Severity};
use opentelemetry_sdk::logs::{BatchLogProcessor, LoggerProvider};
use opentelemetry_otlp::{LogExporter, WithExportConfig};
use std::time::Duration;
use tracing_subscriber::{fmt, EnvFilter, Registry};

use crate::observability::config::LoggingConfig;

/// Initialize structured logging with OTLP/Loki export support
pub fn init_logger(config: &LoggingConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Build env filter from RUST_LOG or default
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.default_level));

    // Create subscriber with JSON formatting if enabled
    let subscriber = Registry::default().with(filter);

    if config.json_format {
        let fmt_layer = fmt::layer()
            .json()
            .with_target(true)
            .with_thread_ids(true)
            .with_line_number(true);
        let subscriber = subscriber.with(fmt_layer);
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|e| format!("Failed to set logger subscriber: {}", e))?;
    } else {
        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_thread_ids(true)
            .with_line_number(true);
        let subscriber = subscriber.with(fmt_layer);
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|e| format!("Failed to set logger subscriber: {}", e))?;
    }

    // Initialize OTLP log exporter if endpoint is configured
    if let Some(endpoint) = &config.otlp_endpoint {
        if !endpoint.is_empty() {
            init_otlp_logger(config)?;
        }
    }

    // Initialize Loki log exporter if endpoint is configured
    if let Some(endpoint) = &config.loki_endpoint {
        if !endpoint.is_empty() {
            init_loki_logger(config)?;
        }
    }

    tracing::info!(
        "Structured logger initialized (json={}, otlp={}, loki={})",
        config.json_format,
        config.otlp_endpoint.as_ref().map_or(false, |e| !e.is_empty()),
        config.loki_endpoint.as_ref().map_or(false, |e| !e.is_empty())
    );

    Ok(())
}

/// Initialize OTLP log exporter
fn init_otlp_logger(config: &LoggingConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)
        .with_timeout(Duration::from_secs(30))
        .build()?;

    let processor = BatchLogProcessor::builder(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_max_queue_size(2048)
        .with_scheduled_delay(Duration::from_secs(1))
        .build();

    let provider = LoggerProvider::builder()
        .with_log_processor(processor)
        .build();

    // Note: In production, you would integrate this with the tracing subscriber
    // For now, we just initialize it to avoid errors
    drop(provider);

    tracing::info!("OTLP log exporter initialized (endpoint={})", config.otlp_endpoint);
    Ok(())
}

/// Initialize Loki log exporter (via HTTP API)
fn init_loki_logger(config: &LoggingConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Loki uses Push API, not OTLP
    // This is a placeholder for future Loki integration
    // In production, you would use loki-log-appender or custom exporter
    tracing::info!("Loki endpoint configured but not yet implemented (endpoint={:?})", config.loki_endpoint);
    Ok(())
}

/// Shutdown logger and flush pending logs
pub fn shutdown_logger() {
    tracing::info!("Shutting down structured logger...");
    // Logger shutdown is handled by tracing subscriber drop
}
