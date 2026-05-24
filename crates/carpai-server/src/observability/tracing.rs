// OpenTelemetry distributed tracing setup

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize tracing with OpenTelemetry support
pub fn init_tracing(service_name: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        // TODO: Add OpenTelemetry layer in Week 8 when OTLP exporter is configured
        // .with(opentelemetry_layer)
        .init();

    tracing::info!(service_name, "Tracing initialized");
}
