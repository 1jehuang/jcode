//! WebSocket handler for real-time communication

use axum::{
    Router,
    routing::get,
    extract::ws::{WebSocketUpgrade, WebSocket},
    response::IntoResponse,
};
use tracing::info;

pub fn router() -> Router {
    Router::new()
        .route("/ws/session/:id", get(ws_handler))
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    info!("WebSocket upgrade request");
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(socket: WebSocket) {
    // TODO: Implement WebSocket handling in Week 3-4
    info!("WebSocket connection established (stub)");
}
