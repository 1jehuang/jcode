use actix_web::{web, HttpResponse, Result};
use serde::{Deserialize, Serialize};

use super::metrics::SystemMetrics;

#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardConfig {
    pub refresh_interval_secs: u64,
    pub max_history_points: usize,
    pub enable_realtime: bool,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        DashboardConfig {
            refresh_interval_secs: 5,
            max_history_points: 100,
            enable_realtime: true,
        }
    }
}

pub struct DashboardRoutes;

impl DashboardRoutes {
    pub async fn index() -> Result<HttpResponse> {
        let html = include_str!("templates/index.html");
        Ok(HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(html))
    }

    pub async fn api_metrics() -> Result<HttpResponse> {
        let metrics = SystemMetrics::new();
        let json = metrics.to_json().unwrap_or_else(|_| "{}".to_string());

        Ok(HttpResponse::Ok()
            .content_type("application/json")
            .body(json))
    }

    pub async fn api_config() -> Result<HttpResponse> {
        let config = DashboardConfig::default();
        let json = serde_json::to_string(&config).unwrap_or_else(|_| "{}".to_string());

        Ok(HttpResponse::Ok()
            .content_type("application/json")
            .body(json))
    }

    pub async fn api_health() -> Result<HttpResponse> {
        let health = serde_json::json!({
            "status": "healthy",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "version": env!("CARGO_PKG_VERSION"),
            "uptime": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });

        Ok(HttpResponse::Ok()
            .content_type("application/json")
            .body(health.to_string()))
    }

    pub async fn api_stats(
        web::Query(query): web::Query<StatsQuery>,
    ) -> Result<HttpResponse> {
        let range = query.range.unwrap_or(3600);
        let interval = query.interval.unwrap_or(60);

        let stats = serde_json::json!({
            "range_seconds": range,
            "interval_seconds": interval,
            "data_points": range / interval,
            "cpu_history": vec![0.0f64; (range / interval) as usize],
            "memory_history": vec![0.0f64; (range / interval) as usize],
            "requests_history": vec![0u64; (range / interval) as usize],
        });

        Ok(HttpResponse::Ok()
            .content_type("application/json")
            .body(stats.to_string()))
    }
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub range: Option<u64>,
    pub interval: Option<u64>,
}
