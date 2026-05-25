use opentelemetry::{global, metrics::MeterProvider as _};
use opentelemetry_sdk::metrics::{meter_provider::MeterProvider, PeriodicReader};
use opentelemetry_otlp::WithExportConfig;
use std::time::Duration;
use tokio::net::TcpListener;

use crate::observability::config::MetricsConfig;

/// Initialize OpenTelemetry metrics with Prometheus exporter
pub async fn init_metrics(config: &MetricsConfig) -> Result<MeterProvider, Box<dyn std::error::Error + Send + Sync>> {
    // Build meter provider
    let mut builder = MeterProvider::builder();

    // Add OTLP metrics exporter if endpoint is configured
    if let Some(endpoint) = &config.otlp_endpoint {
        if config.export_otlp && !endpoint.is_empty() {
            let otlp_exporter = opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .with_timeout(Duration::from_secs(30))
                .build()?;

            let reader = PeriodicReader::builder(otlp_exporter, opentelemetry_sdk::runtime::Tokio)
                .with_interval(Duration::from_secs(config.export_interval_secs))
                .build();

            builder = builder.with_reader(reader);
        }
    }

    let provider = builder.build();
    global::set_meter_provider(provider.clone());

    // Start Prometheus HTTP server if port is configured
    if config.prometheus_port > 0 {
        start_prometheus_server(config.prometheus_port).await?;
        tracing::info!("Prometheus metrics server started on port {}", config.prometheus_port);
    }

    tracing::info!("OpenTelemetry metrics initialized (prometheus_port={}, export_otlp={})",
        config.prometheus_port, config.export_otlp);

    Ok(provider)
}

/// Start a simple Prometheus HTTP scraper endpoint
async fn start_prometheus_server(port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::{http::StatusCode, routing::get, Router};
    use std::net::SocketAddr;

    let app = Router::new().route("/metrics", get(|| async {
        // TODO: Integrate with prometheus-client registry for actual metrics scraping
        // For now, return placeholder
        "# HELP carpai_info CarpAI service information\n# TYPE carpai_info gauge\ncarpai_info{version=\"0.12.0\"} 1\n".to_string()
    }));

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();

    // Try to bind, but don't fail if port is already in use
    match TcpListener::bind(addr).await {
        Ok(_) => {
            tokio::spawn(async move {
                if let Err(e) = axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app).await {
                    tracing::error!("Prometheus server error: {}", e);
                }
            });
        }
        Err(e) => {
            tracing::warn!("Could not bind Prometheus server to {}: {}", addr, e);
        }
    }

    Ok(())
}

/// Shutdown metrics and flush pending data
pub fn shutdown_metrics() {
    tracing::info!("Shutting down OpenTelemetry metrics...");
    if let Err(e) = global::shutdown_meter_provider() {
        tracing::error!("Error shutting down meter provider: {}", e);
    }
}
