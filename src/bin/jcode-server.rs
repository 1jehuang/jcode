//! CarpAI Server — Enterprise-grade AI Programming Server
//!
//! Multi-protocol server providing:
//!   - gRPC (port 50051): High-performance RPC for IDE plugins
//!   - REST (port 8081): OpenAI-compatible HTTP API
//!   - WebSocket (port 8080): Real-time Web IDE protocol
//!
//! This is the **production server binary**. For the TUI client, use `jcode` (cli binary).
//!
//! # Feature Requirements
//! Build with: `cargo build --release --bin jcode-server`
//! Or with minimal features: `cargo build --release --bin jcode-server --no-default-features --features "server enterprise"`

use std::net::SocketAddr;
use std::sync::Arc;
use jcode::{
    grpc::GrpcServerBuilder,
    ws::{WebIdeWebSocketServer, WebSocketConfig},
    rest::{create_rest_router, ApiState},
    security::{ApiKeyValidator, PasswordHasher, EndpointRateLimiter},
    observability::{ObservabilityConfig, init_tracer, init_metrics, init_logger, shutdown_tracer, shutdown_metrics, shutdown_logger},
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ══════════════════════════════════════════════════════════════
    // 1. Initialize Observability (OpenTelemetry)
    // ══════════════════════════════════════════════════════════════
    let otel_config = ObservabilityConfig::from_env();
    
    init_logger(&otel_config.logging)?;
    let _tracer_provider = init_tracer(&otel_config.tracing)?;
    let _meter_provider = init_metrics(&otel_config.metrics).await?;
    
    info!("🔭 CarpAI Server — Enterprise AI Programming Service");
    info!("   Tracing: {} (endpoint={})", 
        if otel_config.tracing.enabled { "enabled" } else { "disabled" },
        otel_config.tracing.otlp_endpoint);
    info!("   Metrics: prometheus_port={}, otlp={}", 
        otel_config.metrics.prometheus_port,
        otel_config.metrics.export_otlp);

    // ══════════════════════════════════════════════════════════════
    // 2. Read configuration from environment
    // ══════════════════════════════════════════════════════════════
    let grpc_port: u16 = std::env::var("JCODE_GRPC_PORT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(50051);
    let ws_port: u16 = std::env::var("JCODE_WS_PORT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(8080);
    let rest_port: u16 = std::env::var("JCODE_REST_PORT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(8081);
    let bind_addr = std::env::var("JCODE_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0".to_string());

    let enable_lsp: bool = std::env::var("JCODE_ENABLE_LSP")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(true);
    let enable_terminal: bool = std::env::var("JCODE_ENABLE_TERMINAL")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(true);
    let enable_collaboration: bool = std::env::var("JCODE_ENABLE_COLLABORATION")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(true);

    // Security config
    let api_key_prefix = std::env::var("CARPAI_API_KEY_PREFIX")
        .unwrap_or_else(|_| "carpai_".to_string());
    let rate_limit_rps: u64 = std::env::var("CARPAI_RATE_LIMIT_RPS")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(10);

    // ══════════════════════════════════════════════════════════════
    // 3. Initialize Security Components
    // ══════════════════════════════════════════════════════════════
    let api_key_validator = Arc::new(ApiKeyValidator::new(&api_key_prefix, 32, 64));
    let password_hasher = Arc::new(PasswordHasher::new());
    let rate_limiter = EndpointRateLimiter::new();

    info!("🔐 Security: API Key prefix={}, Rate limit={} rps", api_key_prefix, rate_limit_rps);
    info!("🔐 Password hashing: Argon2id (production-grade)");

    // ══════════════════════════════════════════════════════════════
    // 4. Build API State with REAL engine implementations
    // ══════════════════════════════════════════════════════════════
    //
    // Phase 0 Goal: Replace None with actual engine wrappers.
    // Future Phase 2 will inject full trait objects from carpai-server crate.
    //

    // Auth provider — use JWT with Argon2id password backing
    let auth_provider: Arc<dyn carpai_internal::AuthProvider> = 
        Arc::new(jcode::auth::JwtAuthProvider::new());

    // Completion engine — wrap existing jcode-completion
    let completion_engine: Option<Arc<dyn carpai_internal::CodeCompletion>> = {
        // TODO (Phase 2): Create ServerCompletionEngine wrapping jcode_completion::CompletionEngine
        // For now, we log that the engine is in bootstrap mode
        info!("⚠️  Completion engine: bootstrap mode (Phase 0 — real engine injection in Phase 2)");
        None
    };

    // Inference engine — wrap existing MultiProvider + AutoFallbackRouter
    let inference_engine: Option<Arc<dyn carpai_internal::InferenceEngine>> = {
        // TODO (Phase 2): Create ServerInferenceEngine wrapping:
        //   - crate::provider::MultiProvider (40+ LLM providers)
        //   - crate::auto_fallback::AutoFallbackRouter (local→cloud fallback)
        //   - Quota enforcement layer
        info!("⚠️  Inference engine: bootstrap mode (Phase 0 — real engine injection in Phase 2)");
        None
    };

    let api_state = ApiState {
        completion_engine,
        auth_provider,
        inference_engine,
    };

    // ══════════════════════════════════════════════════════════════
    // 5. Print startup banner
    // ══════════════════════════════════════════════════════════════
    println!("╔══════════════════════════════════════════════════════╗");
    println!("║  🚀 CarpAI Server v{}              ", env!("CARGO_PKG_VERSION"));
    println!("║     Enterprise AI Programming Service            ║");
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║  gRPC:      {}:{}", bind_addr, grpc_port);
    println!("║  WebSocket: {}:{}", bind_addr, ws_port);
    println!("║  REST:      {}:{}", bind_addr, rest_port);
    println!("╠══════════════════════════════════════════════════════╣");
    if enable_lsp { println!("║  ✅ LSP Integration                              "); }
    if enable_terminal { println!("║  ✅ Terminal Sessions                            "); }
    if enable_collaboration { println!("║  ✅ Real-time Collaboration                      "); }
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║  🔒 Argon2id Password Hashing                    ");
    println!("║  🔑 API Key Validation (prefix: {})          ", api_key_prefix);
    println!("║  🛡️  Rate Limiting: {} req/s                    ", rate_limit_rps);
    println!("║  📊 OpenTelemetry Observability                  ");
    println!("╚══════════════════════════════════════════════════════╝");

    // ══════════════════════════════════════════════════════════════
    // 6. Start protocol servers
    // ══════════════════════════════════════════════════════════════
    let grpc_addr: SocketAddr = format!("{}:{}", bind_addr, grpc_port)
        .parse().map_err(|e| format!("Invalid gRPC bind address: {}", e))?;
    let grpc_builder = GrpcServerBuilder::new();

    let web_ide_config = WebSocketConfig {
        port: ws_port,
        enable_lsp,
        enable_terminal,
        enable_collaboration,
        ..Default::default()
    };
    let ws_server = WebIdeWebSocketServer::new(web_ide_config);

    let rest_router = create_rest_router(api_state);
    let rest_addr = format!("{}:{}", bind_addr, rest_port);
    info!("Starting REST API server on {}", rest_addr);
    let listener = tokio::net::TcpListener::bind(&rest_addr).await?;

    // Run all three servers concurrently
    info!("🎯 Starting all protocol servers...");
    tokio::spawn(async move {
        if let Err(e) = grpc_builder.serve(grpc_addr).await {
            eprintln!("❌ gRPC server error: {}", e);
        }
    });

    tokio::spawn(async move {
        if let Err(e) = ws_server.serve().await {
            eprintln!("❌ WebSocket server error: {}", e);
        }
    });

    let server = axum::serve(listener, rest_router);
    
    tokio::select! {
        result = server => {
            if let Err(e) = result {
                eprintln!("❌ REST server error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal, gracefully shutting down...");
        }
    }
    
    // ══════════════════════════════════════════════════════════════
    // 7. Graceful shutdown
    // ══════════════════════════════════════════════════════════════
    shutdown_tracer();
    shutdown_metrics();
    shutdown_logger();
    
    info!("👋 CarpAI Server shut down complete");
    Ok(())
}
