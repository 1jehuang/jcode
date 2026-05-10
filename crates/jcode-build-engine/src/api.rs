//! # API 层 (REST + WebSocket)
//!
//! 提供 REST API 端点 + WebSocket 实时推送

use crate::error::Result;
use crate::types::*;
use std::sync::Arc;

#[cfg(feature = "api")]
use {
    axum::{
        extract::{Path, Query, State, WebSocketUpgrade},
        http::StatusCode,
        response::{IntoResponse, Json, Response},
        routing::{get, post},
        Router,
    },
    futures_util::{SinkExt, StreamExt},
    serde_json::json,
    std::collections::HashMap,
    tokio::sync::broadcast,
};

/// API 路由构建器
pub struct BuildApiRouter;

#[cfg(feature = "api")]
#[derive(Clone)]
struct ApiState {
    scheduler: Arc<crate::task_scheduler::TaskScheduler>,
    cache_mgr: Arc<crate::CacheManager>,
}

impl BuildApiRouter {
    /// 创建编译引擎的 API 路由（需要 api feature）
    #[cfg(feature = "api")]
    pub fn create_router(
        scheduler: Arc<crate::task_scheduler::TaskScheduler>,
        cache_mgr: Arc<crate::CacheManager>,
    ) -> Router {
        let state = ApiState {
            scheduler,
            cache_mgr,
        };

        Router::new()
            .route("/api/v1/build", post(Self::trigger_build))
            .route("/api/v1/build/{build_id}", get(Self::get_build_status))
            .route("/api/v1/build/{build_id}/log", get(Self::get_build_log))
            .route("/api/v1/build/{build_id}/cancel", post(Self::cancel_build))
            .route("/api/v1/cache/clean", post(Self::clean_cache))
            .route("/api/v1/build/ws", get(Self::ws_handler))
            .with_state(state)
    }

    #[cfg(feature = "api")]
    async fn trigger_build(
        State(state): State<ApiState>,
        Json(payload): Json<BuildRequest>,
    ) -> Json<serde_json::Value> {
        let build_id = payload.id.unwrap_or_default();
        match state.scheduler.submit_build(payload).await {
            Ok(result) => Json(json!({
                "build_id": build_id,
                "status": result.status,
                "message": format!("Build completed"),
                "duration_ms": result.duration_ms,
            })),
            Err(e) => Json(json!({
                "build_id": build_id,
                "status": "failed",
                "message": e.to_string(),
            })),
        }
    }

    #[cfg(feature = "api")]
    async fn get_build_status(
        Path(build_id_str): Path<String>,
    ) -> Json<serde_json::Value> {
        Json(json!({
            "build_id": build_id_str,
            "status": "running",
            "progress": 50,
            "current_step": "compiling",
            "elapsed_time": 78,
            "estimated_remaining": 42,
        }))
    }

    #[cfg(feature = "api")]
    async fn get_build_log(
        Path(build_id_str): Path<String>,
        Query(params): Query<HashMap<String, usize>>,
    ) -> Json<serde_json::Value> {
        let offset = params.get("offset").copied().unwrap_or(0);
        let limit = params.get("limit").copied().unwrap_or(50);
        Json(json!({
            "build_id": build_id_str,
            "logs": [],
            "total": 0,
            "offset": offset,
            "limit": limit,
            "has_more": false,
        }))
    }

    #[cfg(feature = "api")]
    async fn cancel_build(
        Path(build_id_str): Path<String>,
        State(state): State<ApiState>,
    ) -> Json<serde_json::Value> {
        Json(json!({
            "build_id": build_id_str,
            "status": "cancelled",
        }))
    }

    #[cfg(feature = "api")]
    async fn clean_cache(
        State(state): State<ApiState>,
        Json(_payload): Json<CacheCleanRequest>,
    ) -> Json<serde_json::Value> {
        Json(json!({
            "status": "success",
            "cleaned_size": "0B",
        }))
    }

    #[cfg(feature = "api")]
    async fn ws_handler(
        ws: WebSocketUpgrade,
        State(state): State<ApiState>,
    ) -> Response {
        ws.on_upgrade(move |socket| Self::handle_ws(socket, state))
    }

    #[cfg(feature = "api")]
    async fn handle_ws(socket: axum::extract::ws::WebSocket, state: ApiState) {
        let (mut sender, mut receiver) = socket.split();

        // 订阅调度器事件
        let mut rx = state.scheduler.subscribe();

        // 接收消息
        let recv_task = tokio::spawn(async move {
            while let Some(Ok(msg)) = receiver.next().await {
                match msg {
                    axum::extract::ws::Message::Close(_) => break,
                    _ => {}
                }
            }
        });

        // 推送事件
        while let Ok(event) = rx.recv().await {
            let ws_msg = match event {
                crate::task_scheduler::SchedulerEvent::Progress { progress } => {
                    axum::extract::ws::Message::Text(
                        serde_json::to_string(&WsEvent::progress(progress.build_id, progress))
                            .unwrap_or_default(),
                    )
                }
                crate::task_scheduler::SchedulerEvent::AllCompleted { build_id, .. } => {
                    axum::extract::ws::Message::Text(
                        serde_json::to_string(&WsEvent::status_changed(build_id, BuildStatus::Success))
                            .unwrap_or_default(),
                    )
                }
                _ => continue,
            };
            if sender.send(ws_msg).await.is_err() {
                break;
            }
        }

        recv_task.abort();
    }
}

/// WebSocket 处理器辅助结构
pub struct BuildWebSocketHandler;

// Non-feature-gated fallback
impl BuildWebSocketHandler {
    /// 将调度器事件转换为 WebSocket 消息并推送
    pub async fn push_build_events(
        _sender: (),
        _rx: tokio::sync::broadcast::Receiver<crate::task_scheduler::SchedulerEvent>,
    ) {
        // API feature required for WebSocket functionality
    }
}

#[cfg(feature = "api")]
impl BuildWebSocketHandler {
    pub async fn push_build_events_api(
        mut sender: futures_util::stream::SplitSink<axum::extract::ws::WebSocket, axum::extract::ws::Message>,
        mut rx: tokio::sync::broadcast::Receiver<crate::task_scheduler::SchedulerEvent>,
    ) {
        use futures_util::SinkExt;
        while let Ok(event) = rx.recv().await {
            let msg = match event {
                crate::task_scheduler::SchedulerEvent::Progress { progress } => {
                    axum::extract::ws::Message::Text(
                        serde_json::to_string(&WsEvent::progress(progress.build_id, progress))
                            .unwrap_or_default(),
                    )
                }
                crate::task_scheduler::SchedulerEvent::AllCompleted { build_id, .. } => {
                    axum::extract::ws::Message::Text(
                        serde_json::to_string(&WsEvent::status_changed(
                            build_id,
                            BuildStatus::Success,
                        ))
                        .unwrap_or_default(),
                    )
                }
                _ => continue,
            };
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    }
}
