use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    resource::{self, Resource},
    trace::{self, Sampler, TracerProvider},
};
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, Registry};

use crate::observability::config::TracingConfig;

/// Initialize OpenTelemetry tracer with OTLP exporter
pub fn init_tracer(config: &TracingConfig) -> Result<TracerProvider, Box<dyn std::error::Error + Send + Sync>> {
    if !config.enabled {
        tracing::info!("Tracing is disabled");
        return Ok(TracerProvider::builder().build());
    }

    // Set global propagator for distributed tracing context
    global::set_text_map_propagator(TraceContextPropagator::new());

    // Build resource with service attributes
    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new("service.name", config.service_name.clone()),
            KeyValue::new("service.version", config.service_version.clone()),
            KeyValue::new("deployment.environment", config.environment.clone()),
        ])
        .build();

    // Configure sampler based on sample_ratio
    let sampler = if config.sample_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sample_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sample_ratio)
    };

    // Build tracer provider with OTLP exporter
    let mut builder = TracerProvider::builder()
        .with_resource(resource)
        .with_sampler(sampler);

    // Add OTLP span exporter if endpoint is configured
    if !config.otlp_endpoint.is_empty() {
        let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&config.otlp_endpoint)
            .with_timeout(Duration::from_secs(config.export_timeout_secs))
            .build()?;

        let span_processor = trace::BatchSpanProcessor::builder(otlp_exporter, opentelemetry_sdk::runtime::Tokio)
            .with_max_queue_size(config.max_queue_size)
            .with_scheduled_delay(Duration::from_secs(config.export_interval_secs))
            .build();

        builder = builder.with_span_processor(span_processor);
    }

    let provider = builder.build();

    // Install the tracer globally
    global::set_tracer_provider(provider.clone());

    // Set up tracing subscriber with OpenTelemetry layer
    let telemetry_layer = tracing_opentelemetry::OpenTelemetryLayer::new(provider.tracer("carpai"));
    let subscriber = Registry::default().with(telemetry_layer);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| format!("Failed to set tracing subscriber: {}", e))?;

    tracing::info!(
        "OpenTelemetry tracer initialized (endpoint={}, sample_ratio={})",
        config.otlp_endpoint,
        config.sample_ratio
    );

    Ok(provider)
}

/// Shutdown tracer and flush pending spans
pub fn shutdown_tracer() {
    tracing::info!("Shutting down OpenTelemetry tracer...");
    if let Err(e) = global::shutdown_tracer_provider() {
        tracing::error!("Error shutting down tracer: {}", e);
    }
}
