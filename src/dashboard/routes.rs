use axum::extract::Query;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
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
    pub async fn index() -> Html<&'static str> {
        let html = include_str!("templates/index.html");
        Html(html)
    }

    pub async fn api_metrics() -> Response {
        let metrics = SystemMetrics::new();
        let json = metrics.to_json().unwrap_or_else(|_| "{}".to_string());

        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json,
        )
            .into_response()
    }

    pub async fn api_config() -> Response {
        let config = DashboardConfig::default();
        let json = serde_json::to_string(&config).unwrap_or_else(|_| "{}".to_string());

        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json,
        )
            .into_response()
    }

    pub async fn api_health() -> Response {
        let health = serde_json::json!({
            "status": "healthy",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "version": env!("CARGO_PKG_VERSION"),
            "uptime": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });

        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            health.to_string(),
        )
            .into_response()
    }

    pub async fn api_stats(Query(query): Query<StatsQuery>) -> Response {
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

        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            stats.to_string(),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub range: Option<u64>,
    pub interval: Option<u64>,
}
