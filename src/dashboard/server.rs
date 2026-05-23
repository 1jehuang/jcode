use axum::Router;
use axum::routing::get;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use super::routes::DashboardRoutes;
use super::metrics::SystemMetrics;

pub struct DashboardServer {
    port: u16,
    host: String,
    metrics_tx: broadcast::Sender<Arc<SystemMetrics>>,
}

impl DashboardServer {
    pub fn new(port: u16) -> Self {
        let (metrics_tx, _) = broadcast::channel(100);
        
        DashboardServer {
            port,
            host: "127.0.0.1".to_string(),
            metrics_tx,
        }
    }

    pub fn with_host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }
    
    /// 获取metrics广播发送器
    pub fn metrics_sender(&self) -> broadcast::Sender<Arc<SystemMetrics>> {
        self.metrics_tx.clone()
    }

    pub async fn run(&self) -> std::io::Result<()> {
        println!("🚀 CarpAI Dashboard starting on http://{}:{}", self.host, self.port);
        println!("📊 WebSocket endpoint: ws://{}:{}/ws", self.host, self.port);

        let app = Router::new()
            .route("/", get(DashboardRoutes::index))
            .route("/api/metrics", get(DashboardRoutes::api_metrics))
            .route("/api/config", get(DashboardRoutes::api_config))
            .route("/api/health", get(DashboardRoutes::api_health))
            .route("/api/stats", get(DashboardRoutes::api_stats))
            .route("/api/tasks", get(DashboardRoutes::api_tasks))
            .route("/api/sessions", get(DashboardRoutes::api_sessions))
            .route("/api/audit/logs", get(DashboardRoutes::api_audit_logs))
            .route("/api/audit/stats", get(DashboardRoutes::api_audit_stats))
            .route("/ws", get(DashboardRoutes::websocket_handler));

        let addr: SocketAddr = format!("{}:{}", self.host, self.port).parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}
