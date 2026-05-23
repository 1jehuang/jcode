use axum::extract::{Query, ws::{WebSocket, WebSocketUpgrade}};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use serde::{Deserialize, Serialize};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::broadcast;

use super::metrics::SystemMetrics;
use super::audit_log::{AuditLogger, AuditFilters, ActionType, LogSeverity};

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
    
    /// 获取任务列表
    pub async fn api_tasks() -> Response {
        let tasks = serde_json::json!({
            "total": 5,
            "active": 2,
            "completed": 3,
            "tasks": [
                {
                    "id": "task_1",
                    "name": "Code Review",
                    "status": "running",
                    "progress": 65,
                    "started_at": "2026-05-22T10:00:00Z"
                },
                {
                    "id": "task_2",
                    "name": "Test Generation",
                    "status": "completed",
                    "progress": 100,
                    "completed_at": "2026-05-22T09:30:00Z"
                }
            ]
        });

        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            tasks.to_string(),
        )
            .into_response()
    }
    
    /// 获取会话列表
    pub async fn api_sessions() -> Response {
        let sessions = serde_json::json!({
            "total": 3,
            "active": 2,
            "sessions": [
                {
                    "id": "session_1",
                    "user": "developer",
                    "status": "active",
                    "created_at": "2026-05-22T08:00:00Z",
                    "last_activity": "2026-05-22T10:30:00Z"
                }
            ]
        });

        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            sessions.to_string(),
        )
            .into_response()
    }
    
    /// WebSocket处理器 - 实时推送metrics
    pub async fn websocket_handler(ws: WebSocketUpgrade) -> Response {
        ws.on_upgrade(handle_websocket)
    }
    
    /// 获取审计日志
    pub async fn api_audit_logs(Query(query): Query<AuditLogQuery>) -> Response {
        // 创建临时logger（在实际实现中应该从状态中获取）
        let logger = AuditLogger::new("./logs/audit");
        
        let filters = AuditFilters {
            agent_id: query.agent_id,
            action_type: query.action_type,
            severity: query.severity,
            start_time: None,
            end_time: None,
            limit: query.limit,
        };
        
        match logger.query_logs(filters).await {
            Ok(logs) => {
                let json = serde_json::json!({
                    "total": logs.len(),
                    "logs": logs
                });
                
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    json.to_string(),
                ).into_response()
            }
            Err(e) => {
                let error = serde_json::json!({"error": e});
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [(header::CONTENT_TYPE, "application/json")],
                    error.to_string(),
                ).into_response()
            }
        }
    }
    
    /// 获取审计统计
    pub async fn api_audit_stats() -> Response {
        let logger = AuditLogger::new("./logs/audit");
        
        match logger.get_stats().await {
            Ok(stats) => {
                let json = serde_json::to_string(&stats).unwrap_or_default();
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    json,
                ).into_response()
            }
            Err(e) => {
                let error = serde_json::json!({"error": e});
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [(header::CONTENT_TYPE, "application/json")],
                    error.to_string(),
                ).into_response()
            }
        }
    }
}

/// WebSocket连接处理
async fn handle_websocket(socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    
    // 创建一个简单的metrics广播（在实际实现中会从DashboardServer传入）
    let (_tx, mut rx) = broadcast::channel::<Arc<SystemMetrics>>(100);
    
    // 发送初始消息
    let init_msg = serde_json::json!({
        "type": "connected",
        "message": "WebSocket connected successfully"
    });
    
    if sender.send(axum::extract::ws::Message::Text(init_msg.to_string())).await.is_err() {
        return;
    }
    
    // 监听客户端消息
    tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                axum::extract::ws::Message::Text(text) => {
                    println!("Received: {}", text);
                    
                    // 响应ping
                    if text == "ping" {
                        let pong = serde_json::json!({"type": "pong", "timestamp": chrono::Utc::now().to_rfc3339()});
                        let _ = sender.send(axum::extract::ws::Message::Text(pong.to_string())).await;
                    }
                }
                axum::extract::ws::Message::Close(_) => {
                    break;
                }
                _ => {}
            }
        }
    });
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub range: Option<u64>,
    pub interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub agent_id: Option<String>,
    pub action_type: Option<ActionType>,
    pub severity: Option<LogSeverity>,
    pub limit: Option<usize>,
}
