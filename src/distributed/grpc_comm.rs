//! gRPC-based inter-node communication for distributed cluster
//!
//! Implements real network communication using tonic gRPC framework
//! with TLS encryption and JWT authentication support.

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};

// Include generated protobuf code
pub mod proto {
    tonic::include_proto!("distributed");
}

use proto::{
    cluster_node_service_server::{ClusterNodeService, ClusterNodeServiceServer},
    *,
};

/// gRPC communication service for cluster nodes
pub struct GrpcCommunicationService {
    node_id: String,
    server_addr: String,
    tls_config: Option<TlsConfig>,
    auth_config: Option<AuthConfig>,
    client_pool: Arc<RwLock<std::collections::HashMap<String, NodeClient>>>,
}

/// TLS configuration for secure communication
#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
    pub ca_cert_path: Option<String>,
    pub verify_peer: bool,
}

/// Authentication configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub token_expiry_seconds: u64,
    pub require_auth: bool,
}

/// Client connection to a remote node
#[derive(Clone)]
pub struct NodeClient {
    pub node_id: String,
    pub address: String,
    channel: tonic::transport::Channel,
}

impl GrpcCommunicationService {
    /// Create new gRPC communication service
    pub fn new(
        node_id: String,
        host: &str,
        port: u16,
        tls_config: Option<TlsConfig>,
        auth_config: Option<AuthConfig>,
    ) -> Self {
        let server_addr = format!("{}:{}", host, port);
        info!(
            "Creating gRPC communication service for node {} at {}",
            node_id, server_addr
        );

        Self {
            node_id,
            server_addr,
            tls_config,
            auth_config,
            client_pool: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Start the gRPC server
    pub async fn start_server(&self) -> Result<()> {
        info!("Starting gRPC server on {}", self.server_addr);

        let service = ClusterNodeServiceImpl {
            node_id: self.node_id.clone(),
            auth_config: self.auth_config.clone(),
        };

        let addr = self.server_addr.parse()?;

        // Build server with optional TLS
        if let Some(tls) = &self.tls_config {
            info!("TLS enabled, loading certificates...");
            let tls_config = self.load_tls_config(tls)?;

            tonic::transport::Server::builder()
                .tls_config(tls_config)?
                .add_service(ClusterNodeServiceServer::new(service))
                .serve(addr)
                .await?;
        } else {
            warn!("TLS disabled - running in insecure mode (not recommended for production)");
            tonic::transport::Server::builder()
                .add_service(ClusterNodeServiceServer::new(service))
                .serve(addr)
                .await?;
        }

        Ok(())
    }

    /// Connect to a remote node
    pub async fn connect_to_node(&self, node_id: &str, address: &str) -> Result<NodeClient> {
        info!("Connecting to node {} at {}", node_id, address);

        // Check if already connected
        {
            let pool = self.client_pool.read().await;
            if let Some(client) = pool.get(node_id) {
                debug!("Reusing existing connection to {}", node_id);
                return Ok(client.clone());
            }
        }

        // Build channel with optional TLS
        let channel = if let Some(tls) = &self.tls_config {
            let tls_config = self.load_client_tls_config(tls)?;
            tonic::transport::channel::Endpoint::from_shared(address.to_string())?
                .tls_config(tls_config)?
                .connect()
                .await?
        } else {
            tonic::transport::channel::Endpoint::from_shared(address.to_string())?
                .connect()
                .await?
        };

        let client = NodeClient {
            node_id: node_id.to_string(),
            address: address.to_string(),
            channel,
        };

        // Store in pool
        {
            let mut pool = self.client_pool.write().await;
            pool.insert(node_id.to_string(), client.clone());
        }

        info!("Successfully connected to node {}", node_id);
        Ok(client)
    }

    /// Send heartbeat to follower node
    pub async fn send_heartbeat(
        &self,
        target_node_id: &str,
        term: u64,
        metadata: std::collections::HashMap<String, String>,
    ) -> Result<HeartbeatResponse> {
        let client = self.connect_to_node(target_node_id, &format!("http://{}", target_node_id)).await?;

        let mut client = proto::cluster_node_service_client::ClusterNodeServiceClient::new(
            client.channel.clone(),
        );

        let request = tonic::Request::new(HeartbeatRequest {
            leader_id: self.node_id.clone(),
            term,
            timestamp: chrono::Utc::now().timestamp(),
            metadata,
            committed_log_entries: vec![],
        });

        let response = client.heartbeat(request).await?;
        Ok(response.into_inner())
    }

    /// Request vote from peer node
    pub async fn request_vote(
        &self,
        target_address: &str,
        term: u64,
        last_log_index: u64,
        last_log_term: u64,
    ) -> Result<VoteResponse> {
        let endpoint = tonic::transport::channel::Endpoint::from_shared(target_address.to_string())?;
        let channel = endpoint.connect().await?;

        let mut client = proto::cluster_node_service_client::ClusterNodeServiceClient::new(channel);

        let request = tonic::Request::new(VoteRequest {
            candidate_id: self.node_id.clone(),
            term,
            last_log_index,
            last_log_term,
        });

        let response = client.request_vote(request).await?;
        Ok(response.into_inner())
    }

    /// Distribute task to worker node
    pub async fn distribute_task(
        &self,
        target_address: &str,
        task_id: String,
        task_type: String,
        payload: Vec<u8>,
        priority: u64,
    ) -> Result<TaskDistributionResponse> {
        let endpoint = tonic::transport::channel::Endpoint::from_shared(target_address.to_string())?;
        let channel = endpoint.connect().await?;

        let mut client = proto::cluster_node_service_client::ClusterNodeServiceClient::new(channel);

        let request = tonic::Request::new(TaskDistributionRequest {
            task_id,
            task_type,
            payload,
            metadata: std::collections::HashMap::new(),
            priority,
            deadline: 0,
            required_capabilities: vec![],
        });

        let response = client.distribute_task(request).await?;
        Ok(response.into_inner())
    }

    /// Load TLS configuration for client connections
    fn load_client_tls_config(&self, config: &TlsConfig) -> Result<tonic::transport::ClientTlsConfig> {
        use tonic::transport::ClientTlsConfig;

        let ca_cert = std::fs::read_to_string(&config.cert_path)?;
        let tls_config = ClientTlsConfig::new()
            .ca_certificate(tonic::transport::Certificate::from_pem(ca_cert));

        Ok(tls_config)
    }

    /// Load TLS configuration for server
    fn load_tls_config(&self, config: &TlsConfig) -> Result<tonic::transport::server::ServerTlsConfig> {
        use tonic::transport::{Identity, ServerTlsConfig};

        let cert = std::fs::read_to_string(&config.cert_path)?;
        let key = std::fs::read_to_string(&config.key_path)?;

        let identity = Identity::from_pem(cert, key);
        let tls_config = ServerTlsConfig::new().identity(identity);

        Ok(tls_config)
    }

    /// Generate JWT token for authentication
    pub fn generate_jwt_token(&self) -> Result<String> {
        if let Some(auth) = &self.auth_config {
            use jsonwebtoken::{encode, Header, EncodingKey};

            let claims = JwtClaims {
                sub: self.node_id.clone(),
                exp: (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs()
                    + auth.token_expiry_seconds) as usize,
                iat: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs() as usize,
            };

            let token = encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(auth.jwt_secret.as_bytes()),
            )?;

            Ok(token)
        } else {
            Err(anyhow::anyhow!("Auth not configured"))
        }
    }
}

/// JWT claims structure
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct JwtClaims {
    sub: String,
    exp: usize,
    iat: usize,
}

/// Implementation of the gRPC service
#[derive(Clone)]
struct ClusterNodeServiceImpl {
    node_id: String,
    auth_config: Option<AuthConfig>,
}

#[tonic::async_trait]
impl ClusterNodeService for ClusterNodeServiceImpl {
    async fn heartbeat(
        &self,
        request: tonic::Request<HeartbeatRequest>,
    ) -> Result<tonic::Response<HeartbeatResponse>, tonic::Status> {
        let req = request.into_inner();

        debug!(
            "Received heartbeat from leader {} (term: {})",
            req.leader_id, req.term
        );

        // Verify authentication if enabled
        if let Some(auth) = &self.auth_config {
            if auth.require_auth {
                // TODO: Verify JWT token from metadata
            }
        }

        let response = HeartbeatResponse {
            success: true,
            term: req.term,
            follower_id: self.node_id.clone(),
            last_heartbeat_received: chrono::Utc::now().timestamp(),
        };

        Ok(tonic::Response::new(response))
    }

    async fn request_vote(
        &self,
        request: tonic::Request<VoteRequest>,
    ) -> Result<tonic::Response<VoteResponse>, tonic::Status> {
        let req = request.into_inner();

        info!(
            "Received vote request from candidate {} (term: {})",
            req.candidate_id, req.term
        );

        // Simple voting logic - always grant vote for now
        let response = VoteResponse {
            vote_granted: true,
            term: req.term,
            voter_id: self.node_id.clone(),
            reason: "Vote granted".to_string(),
        };

        Ok(tonic::Response::new(response))
    }

    async fn sync_state(
        &self,
        request: tonic::Request<StateSyncRequest>,
    ) -> Result<tonic::Response<StateSyncResponse>, tonic::Status> {
        let req = request.into_inner();

        debug!(
            "Received state sync from {} (version: {})",
            req.source_node_id, req.version
        );

        let response = StateSyncResponse {
            success: true,
            current_version: req.version,
            conflict_data: vec![],
            resolution_strategy: "last_write_wins".to_string(),
        };

        Ok(tonic::Response::new(response))
    }

    async fn distribute_task(
        &self,
        request: tonic::Request<TaskDistributionRequest>,
    ) -> Result<tonic::Response<TaskDistributionResponse>, tonic::Status> {
        let req = request.into_inner();

        info!(
            "Received task distribution: {} (type: {}, priority: {})",
            req.task_id, req.task_type, req.priority
        );

        let response = TaskDistributionResponse {
            accepted: true,
            worker_node_id: self.node_id.clone(),
            execution_id: format!("exec-{}", uuid::Uuid::new_v4()),
            estimated_completion_time: chrono::Utc::now().timestamp() + 60,
        };

        Ok(tonic::Response::new(response))
    }

    async fn register_node(
        &self,
        request: tonic::Request<NodeRegistrationRequest>,
    ) -> Result<tonic::Response<NodeRegistrationResponse>, tonic::Status> {
        let req = request.into_inner();

        info!(
            "Node registration request: {} at {}:{}",
            req.node_id, req.host, req.port
        );

        let response = NodeRegistrationResponse {
            success: true,
            assigned_role: "Follower".to_string(),
            current_term: 1,
            leader_id: self.node_id.clone(),
            known_peers: vec![],
        };

        Ok(tonic::Response::new(response))
    }

    async fn health_check(
        &self,
        _request: tonic::Request<HealthCheckRequest>,
    ) -> Result<tonic::Response<HealthCheckResponse>, tonic::Status> {
        let response = HealthCheckResponse {
            healthy: true,
            node_id: self.node_id.clone(),
            status: NodeStatus::Healthy as i32,
            metrics: Some(SystemMetrics {
                cpu_usage_percent: 0.0,
                memory_used_mb: 0,
                memory_total_mb: 0,
                disk_usage_percent: 0.0,
                active_connections: 0,
                tasks_processed: 0,
            }),
            uptime_seconds: 0,
        };

        Ok(tonic::Response::new(response))
    }

    type StreamEventsStream =
        tonic::codec::Streaming<ClusterEvent>;

    async fn stream_events(
        &self,
        _request: tonic::Request<EventSubscriptionRequest>,
    ) -> Result<tonic::Response<Self::StreamEventsStream>, tonic::Status> {
        // TODO: Implement event streaming
        Err(tonic::Status::unimplemented("Event streaming not yet implemented"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_service_creation() {
        let service = GrpcCommunicationService::new(
            "test-node".to_string(),
            "127.0.0.1",
            9000,
            None,
            None,
        );

        assert_eq!(service.node_id, "test-node");
        assert_eq!(service.server_addr, "127.0.0.1:9000");
    }
}
