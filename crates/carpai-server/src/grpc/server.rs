//! gRPC server startup and shutdown
//!
//! This module assembles all gRPC services and returns a tonic Server router.

use crate::grpc::{
    agent_service::create_agent_service,
    session_service::create_session_service,
    tool_service::create_tool_service,
    health_service::create_health_service,
};
use crate::service::context::ServerContext;
use std::sync::Arc;
use tonic::transport::Server;
use tracing::info;

/// Create gRPC server without context (legacy, for testing)
pub fn grpc_server() -> tonic::transport::server::Router {
    info!("Initializing gRPC server with default services");
    Server::builder()
        .add_service(create_agent_service(Arc::new(ServerContext::default_for_test())))
        .add_service(create_session_service())
        .add_service(create_tool_service())
        .add_service(create_health_service())
}

/// Create gRPC server with full ServerContext injected
///
/// This is the main entry point for production server mode.
/// All services receive the ServerContext which contains:
/// - AgentContext from carpai-core (with all Local* implementations)
/// - ServerAppConfig (JWT secrets, API keys, etc.)
pub fn grpc_server_with_ctx(ctx: Arc<ServerContext>) -> tonic::transport::server::Router {
    info!("Initializing gRPC server with ServerContext");
    Server::builder()
        .add_service(create_agent_service(Arc::clone(&ctx)))
        .add_service(create_session_service())
        .add_service(create_tool_service())
        .add_service(create_health_service())
}
