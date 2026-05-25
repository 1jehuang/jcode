//! Application struct - assembles all routers and services
//!
//! This module is the main entry point for the CarpAI server.
//! It creates the ServerContext (which wraps carpai-core's AgentContext)
//! and wires it to all gRPC/REST/WS handlers.

use crate::config::ServerConfig;
use crate::service::context::ServerContext;
use axum::{Router, routing::get};
use std::sync::Arc;
use tracing::info;

pub struct Application {
    config: ServerConfig,
    pub router: Router,
    pub ctx: Arc<ServerContext>,
}

impl Application {
    /// Create a new Application with full ServerContext
    ///
    /// This builds the carpai-core AgentContext and wraps it in ServerContext
    /// for use by all gRPC/REST/WS handlers.
    pub async fn new(config: ServerConfig) -> Result<Self, anyhow::Error> {
        // Build ServerContext from CoreConfig (this calls carpai_core::build_local_agent_context)
        let ctx = Arc::new(ServerContext::from_config(&config.core).await?);

        let router = Self::build_router(&config, Arc::clone(&ctx));

        Ok(Self { config, router, ctx })
    }

    fn build_router(config: &ServerConfig, ctx: Arc<ServerContext>) -> Router {
        // Health check endpoint
        let health_router = Router::new()
            .route("/health", get(health_check))
            .route("/ready", get(ready_check));

        // REST API routes (OpenAI compatible) - with ServerContext state
        let rest_router = crate::rest::router_with_ctx(Arc::clone(&ctx));

        // gRPC routes - now with ServerContext injected
        let grpc_router = crate::grpc::grpc_server_with_ctx(Arc::clone(&ctx));

        // WebSocket routes
        let ws_router = crate::ws::router();

        // Combine all routers
        Router::new()
            .merge(health_router)
            .nest("/v1", rest_router)
            .merge(grpc_router)
            .merge(ws_router)
            .layer(tower_http::cors::CorsLayer::permissive())
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .with_state(Arc::clone(&ctx))
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let addr = self.config.full_listen_addr();
        info!("Starting CarpAI server on {}", addr);

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, self.router).await?;

        Ok(())
    }
}

async fn health_check() -> &'static str {
    "OK"
}

async fn ready_check() -> &'static str {
    "Ready"
}
