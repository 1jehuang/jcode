use axum::Router;
use axum::routing::get;
use std::net::SocketAddr;
use super::routes::DashboardRoutes;

pub struct DashboardServer {
    port: u16,
    host: String,
}

impl DashboardServer {
    pub fn new(port: u16) -> Self {
        DashboardServer {
            port,
            host: "127.0.0.1".to_string(),
        }
    }

    pub fn with_host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    pub async fn run(&self) -> std::io::Result<()> {
        println!("🚀 CarpAI Dashboard starting on http://{}:{}", self.host, self.port);

        let app = Router::new()
            .route("/", get(DashboardRoutes::index))
            .route("/api/metrics", get(DashboardRoutes::api_metrics))
            .route("/api/config", get(DashboardRoutes::api_config))
            .route("/api/health", get(DashboardRoutes::api_health))
            .route("/api/stats", get(DashboardRoutes::api_stats));

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
