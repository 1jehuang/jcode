//! gRPC client for connecting to carpai-server
//!
//! Provides a typed client for all server gRPC services:
//! - AgentService (chat completions)
//! - SessionService (session CRUD)
//! - Health (health check)

/// Generated proto types
pub mod proto {
    tonic::include_proto!("carpai.agent");
    tonic::include_proto!("carpai.session");
    tonic::include_proto!("carpai.health");
}

use tonic::transport::Channel;
use proto::agent_service_client::AgentServiceClient;
use proto::session_service_client::SessionServiceClient;
use proto::health_client::HealthClient;
use tracing::info;

/// CarpAI server gRPC client
pub struct GrpcClient {
    /// Agent service client
    agent: AgentServiceClient<Channel>,
    /// Session service client
    session: SessionServiceClient<Channel>,
    /// Health check client
    health: HealthClient<Channel>,
    /// Server URL (for display/debug)
    server_url: String,
}

impl GrpcClient {
    /// Connect to a CarpAI server
    pub async fn connect(server_url: &str) -> Result<Self, GrpcError> {
        let endpoint = if !server_url.starts_with("http://") && !server_url.starts_with("https://")
        {
            format!("http://{}", server_url)
        } else {
            server_url.to_string()
        };

        info!(url = %endpoint, "Connecting to CarpAI server via gRPC");

        let channel = Channel::from_shared(endpoint.clone())
            .map_err(|e| GrpcError::Connection(format!("Invalid gRPC endpoint: {}", e)))?
            .connect()
            .await
            .map_err(|e| GrpcError::Connection(format!("Failed to connect: {}", e)))?;

        info!(url = %endpoint, "Connected to CarpAI server");

        Ok(Self {
            agent: AgentServiceClient::new(channel.clone()),
            session: SessionServiceClient::new(channel.clone()),
            health: HealthClient::new(channel),
            server_url: endpoint,
        })
    }

    /// Check server health
    pub async fn health_check(&mut self) -> Result<String, GrpcError> {
        use proto::HealthCheckRequest;

        let response = self
            .health
            .check(tonic::Request::new(HealthCheckRequest {
                service: String::new(),
            }))
            .await
            .map_err(|e| GrpcError::Rpc(format!("Health check failed: {}", e)))?;

        let status_int = response.into_inner().status;
        use proto::health_check_response::ServingStatus;
        let status_str = match ServingStatus::try_from(status_int) {
            Ok(ServingStatus::Serving) => "SERVING",
            Ok(ServingStatus::NotServing) => "NOT_SERVING",
            Ok(ServingStatus::ServiceUnknown) => "SERVICE_UNKNOWN",
            _ => "UNKNOWN",
        };

        Ok(format!("Health: {}", status_str))
    }

    /// Send a chat completion request
    pub async fn chat_completion(
        &mut self,
        model: String,
        messages: Vec<proto::ChatMessage>,
        session_id: Option<String>,
    ) -> Result<proto::ChatCompletionResponse, GrpcError> {
        use proto::ChatCompletionRequest;

        let request = ChatCompletionRequest {
            model,
            messages,
            temperature: None,
            max_tokens: None,
            stream: None,
            session_id,
            metadata: std::collections::HashMap::new(),
        };

        let response = self
            .agent
            .chat_completion(tonic::Request::new(request))
            .await
            .map_err(|e| GrpcError::Rpc(format!("Chat completion failed: {}", e)))?;

        Ok(response.into_inner())
    }

    /// Create a new session
    pub async fn create_session(
        &mut self,
        title: String,
        model: Option<String>,
    ) -> Result<proto::SessionResponse, GrpcError> {
        use proto::CreateSessionRequest;

        let request = CreateSessionRequest {
            title,
            model,
            metadata: std::collections::HashMap::new(),
            tenant_id: None,
        };

        let response = self
            .session
            .create_session(tonic::Request::new(request))
            .await
            .map_err(|e| GrpcError::Rpc(format!("Create session failed: {}", e)))?;

        Ok(response.into_inner())
    }

    /// Get server URL
    pub fn server_url(&self) -> &str {
        &self.server_url
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("Failed to connect to gRPC server: {0}")]
    Connection(String),
    #[error("gRPC request failed: {0}")]
    Rpc(String),
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}
