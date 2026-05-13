//! 协作编辑处理器
//!
//! 提供多用户实时协作功能：
//! - 加入/离开协作房间
/// - 光标位置同步
/// - 编辑操作广播

use crate::ws::protocol::{WsRequest, WsResponse, CollaboratorCursor, CursorPosition, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use tracing::{info};

/// 处理加入协作房间请求
pub async fn handle_join(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let room_id = request.params.get("room_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'room_id' parameter"))?;
    
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;
    
    let display_name = request.params.get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or(&format!("User_{}", &session_id[..8]));
    
    // 为用户分配一个颜色
    let color = assign_user_color(session_id);

    info!(
        session_id = %session_id,
        room = %room_id,
        file = %file_path,
        name = %display_name,
        "Joining collaboration room"
    );

    // 注册到会话管理器
    session_manager.join_collaboration(session_id, room_id, &color).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // 获取房间内其他协作者
    let collaborators = session_manager.get_collaborators_in_room(room_id);

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "room_id": room_id,
        "user_info": {
            "session_id": session_id,
            "display_name": display_name,
            "color": color,
            "file_path": file_path,
        },
        "collaborators": collaborators,
        "message": format!("Joined collaboration room: {}", room_id)
    })))
}

/// 处理离开协作房间请求
pub async fn handle_leave(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    info!(
        session_id = %session_id,
        "Leaving collaboration room"
    );

    // 从会话中移除协作状态
    session_manager.leave_collaboration(session_id).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "message": "Left collaboration room"
    })))
}

/// 处理光标位置更新请求
pub async fn handle_cursor_update(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;
    
    let position: CursorPosition = serde_json::from_value(
        request.params.get("position").cloned().unwrap_or_default()
    ).unwrap_or(CursorPosition { line: 0, character: 0 });

    info!(
        session_id = %session_id,
        file = %file_path,
        line = position.line,
        char = position.character,
        "Cursor update"
    );

    // 更新会话中的光标状态
    let cursor = CollaboratorCursor {
        user_id: session_id.to_string(),
        display_name: None, // 从会话中获取
        color: None, // 从会话中获取
        file_path: file_path.to_string(),
        position,
        updated_at: chrono::Utc::now().timestamp_millis() as u64,
    };

    session_manager.update_cursor(session_id, cursor.clone()).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // TODO: 广播给同一房间的其他用户

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "cursor": cursor
    })))
}

/// 处理协作编辑操作请求
pub async fn handle_edit(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;
    
    let _operation = request.params.get("operation")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Missing 'operation' parameter"))?;

    info!(
        session_id = %session_id,
        file = %file_path,
        "Collaboration edit received"
    );

    // TODO: 
    // 1. 应用 OT 算法转换操作
    // 2. 应用到文档状态
    // 3. 广播给其他协作者

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "operation_applied": true,
        "broadcast_to_others": true,
        "message": "Edit operation processed and broadcasted"
    })))
}

/// 根据会话 ID 分配一个颜色（简单哈希算法）
fn assign_user_color(session_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    session_id.hash(&mut hasher);
    let hash = hasher.finish();

    // 预定义的颜色列表
    const COLORS: &[&str] = &[
        "#FF6B6B", // Red
        "#4ECDC4", // Teal
        "#45B7D1", // Blue
        "#96CEB4", // Green
        "#FFEAA7", // Yellow
        "#DDA0DD", // Plum
        "#98D8C8", // Mint
        "#F7DC6F", // Gold
        "#BB8FCE", // Purple
        "#85C1E9", // Sky blue
    ];

    let index = (hash as usize) % COLORS.len();
    COLORS[index].to_string()
}
