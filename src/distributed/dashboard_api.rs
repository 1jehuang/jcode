//! Cluster Dashboard API
//!
//! Provides HTTP endpoints for cluster monitoring and management.
//! This module creates a lightweight HTTP server that serves:
//! - Prometheus metrics endpoint (/metrics)
//! - Cluster status dashboard (/dashboard)
//! - Health check endpoint (/health)
//! - Node information API (/api/nodes)

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use super::service::ClusterService;
use super::metrics::get_metrics;

/// Dashboard API server configuration
pub struct DashboardConfig {
    /// Host to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// Enable Prometheus endpoint
    pub enable_metrics: bool,
    /// Enable HTML dashboard
    pub enable_dashboard: bool,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 9090,
            enable_metrics: true,
            enable_dashboard: true,
        }
    }
}

/// Dashboard API server
pub struct DashboardServer {
    config: DashboardConfig,
    cluster_service: Option<Arc<ClusterService>>,
    shutdown_signal: Arc<RwLock<bool>>,
}

impl DashboardServer {
    /// Create new dashboard server
    pub fn new(config: DashboardConfig, cluster_service: Option<Arc<ClusterService>>) -> Self {
        Self {
            config,
            cluster_service,
            shutdown_signal: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the dashboard server
    pub async fn start(&self) -> Result<(), String> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        info!("Starting cluster dashboard on http://{}", addr);

        let metrics = get_metrics();
        let cluster_service = self.cluster_service.clone();
        let shutdown = Arc::clone(&self.shutdown_signal);

        // Create TCP listener
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;

        info!("Dashboard listening on {}", addr);

        // Accept connections loop
        loop {
            if *shutdown.read().await {
                info!("Dashboard server shutting down");
                break;
            }

            match listener.accept().await {
                Ok((socket, addr)) => {
                    let metrics = Arc::clone(&metrics);
                    let cluster_service = cluster_service.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(socket, metrics, cluster_service).await {
                            warn!("Error handling connection from {}: {}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Stop the dashboard server
    pub async fn stop(&self) {
        let mut shutdown = self.shutdown_signal.write().await;
        *shutdown = true;
        info!("Dashboard server stop signal sent");
    }
}

/// Handle incoming HTTP connection
async fn handle_connection(
    socket: tokio::net::TcpStream,
    metrics: Arc<super::metrics::ClusterMetrics>,
    cluster_service: Option<Arc<ClusterService>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut reader, mut writer) = socket.into_split();

    // Read HTTP request
    let mut buffer = [0u8; 4096];
    let n = reader.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..n]);

    // Parse request path
    let path = if let Some(start) = request.find("GET ") {
        let end = request[start + 4..].find(' ').unwrap_or(0);
        &request[start + 4..start + 4 + end]
    } else {
        "/"
    };

    // Generate response
    let (status, content_type, body) = match path {
        "/metrics" if cluster_service.is_some() => {
            ("200 OK", "text/plain; version=0.0.4", metrics.generate_prometheus_metrics().await)
        }
        "/health" => {
            let health = serde_json::json!({
                "status": "healthy",
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            ("200 OK", "application/json", serde_json::to_string_pretty(&health).unwrap())
        }
        "/dashboard" | "/" => {
            ("200 OK", "text/html", generate_dashboard_html(cluster_service.as_ref()).await)
        }
        "/api/status" => {
            if let Some(service) = &cluster_service {
                let status = get_cluster_status_json(service).await;
                ("200 OK", "application/json", status)
            } else {
                ("503 Service Unavailable", "application/json",
                 r#"{"error":"Cluster service not available"}"#.to_string())
            }
        }
        "/api/nodes" => {
            if let Some(service) = &cluster_service {
                let nodes = get_nodes_json(service).await;
                ("200 OK", "application/json", nodes)
            } else {
                ("503 Service Unavailable", "application/json",
                 r#"{"error":"Cluster service not available"}"#.to_string())
            }
        }
        _ => {
            ("404 Not Found", "text/plain", "Not Found".to_string())
        }
    };

    // Send HTTP response
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status, content_type, body.len(), body
    );

    writer.write_all(response.as_bytes()).await?;
    writer.flush().await?;

    Ok(())
}

/// Generate HTML dashboard
async fn generate_dashboard_html(cluster_service: Option<&Arc<ClusterService>>) -> String {
    let status = if let Some(service) = cluster_service {
        let info = service.get_cluster_info().await;
        let is_leader = service.is_leader().await;
        let healthy = service.healthy_node_count().await;
        let has_quorum = service.has_quorum().await;

        format!(
            r#"
            <div class="status-card">
                <h3>Cluster Status</h3>
                <p><strong>Cluster ID:</strong> {}</p>
                <p><strong>Total Nodes:</strong> {}</p>
                <p><strong>Healthy Nodes:</strong> {}</p>
                <p><strong>Leader:</strong> {}</p>
                <p><strong>Is Leader:</strong> {}</p>
                <p><strong>Has Quorum:</strong> {}</p>
            </div>
            "#,
            info.cluster_id,
            info.total_nodes,
            healthy,
            info.leader.as_deref().unwrap_or("None"),
            if is_leader { "Yes" } else { "No" },
            if has_quorum { "Yes" } else { "No" }
        )
    } else {
        r#"<div class="status-card error"><h3>Cluster Service Not Available</h3></div>"#.to_string()
    };

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Cluster Dashboard</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 20px; background: #f5f5f5; }}
        .container {{ max-width: 1200px; margin: 0 auto; }}
        h1 {{ color: #333; }}
        .status-card {{
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
            margin-bottom: 20px;
        }}
        .status-card.error {{ border-left: 4px solid #f44336; }}
        .status-card.ok {{ border-left: 4px solid #4CAF50; }}
        p {{ margin: 10px 0; }}
        strong {{ color: #666; }}
        .refresh {{ color: #999; font-size: 0.9em; }}
    </style>
    <meta http-equiv="refresh" content="30">
</head>
<body>
    <div class="container">
        <h1>🔗 Distributed Cluster Dashboard</h1>
        <p class="refresh">Auto-refreshes every 30 seconds</p>
        {}
        <div class="status-card">
            <h3>Quick Links</h3>
            <ul>
                <li><a href="/metrics">Prometheus Metrics</a></li>
                <li><a href="/health">Health Check</a></li>
                <li><a href="/api/status">API Status (JSON)</a></li>
                <li><a href="/api/nodes">API Nodes (JSON)</a></li>
            </ul>
        </div>
    </div>
</body>
</html>"#,
        status
    )
}

/// Get cluster status as JSON
async fn get_cluster_status_json(service: &Arc<ClusterService>) -> String {
    let info = service.get_cluster_info().await;
    let state = service.get_state().await;
    let is_leader = service.is_leader().await;
    let healthy = service.healthy_node_count().await;
    let has_quorum = service.has_quorum().await;

    let json = serde_json::json!({
        "cluster_id": info.cluster_id,
        "state": format!("{:?}", state),
        "total_nodes": info.total_nodes,
        "healthy_nodes": healthy,
        "leader": info.leader,
        "self_id": info.self_id,
        "is_leader": is_leader,
        "has_quorum": has_quorum,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });

    serde_json::to_string_pretty(&json).unwrap()
}

/// Get nodes information as JSON
async fn get_nodes_json(service: &Arc<ClusterService>) -> String {
    let info = service.get_cluster_info().await;

    let json = serde_json::json!({
        "cluster_id": info.cluster_id,
        "nodes": [{
            "id": info.self_id,
            "address": format!("{}:{}", 
                service.get_cluster_info().await.self_id,
                9000 // Would need actual address tracking
            ),
            "is_self": true,
            "is_leader": info.leader.as_ref() == Some(&info.self_id)
        }]
    });

    serde_json::to_string_pretty(&json).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_config_default() {
        let config = DashboardConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9090);
        assert!(config.enable_metrics);
        assert!(config.enable_dashboard);
    }

    #[tokio::test]
    async fn test_generate_prometheus_metrics() {
        let metrics = super::super::metrics::ClusterMetrics::new();
        metrics.record_election_initiated().await;
        let output = metrics.generate_prometheus_metrics().await;
        assert!(output.contains("# HELP"));
        assert!(output.contains("# TYPE"));
    }
}
