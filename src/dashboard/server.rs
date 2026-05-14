use actix_web::{App, HttpServer, web};
use std::sync::Arc;

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

        HttpServer::new(|| {
            App::new()
                .route("/", web::get().to(DashboardRoutes::index))
                .route("/api/metrics", web::get().to(DashboardRoutes::api_metrics))
                .route("/api/config", web::get().to(DashboardRoutes::api_config))
                .route("/api/health", web::get().to(DashboardRoutes::api_health))
                .route("/api/stats", web::get().to(DashboardRoutes::api_stats))
        })
        .bind(format!("{}:{}", self.host, self.port))?
        .run()
        .await
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}
