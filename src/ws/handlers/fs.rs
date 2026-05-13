//! 文件系统操作处理器
//!
//! 提供文件浏览、读写和监控功能

use crate::ws::protocol::{WsRequest, WsResponse, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use std::path::Path;
use tracing::{info};

/// 处理文件列表请求
pub async fn handle_list(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let dir_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let show_hidden: bool = request.params.get("show_hidden")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    
    let recursive: bool = request.params.get("recursive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    info!(
        session_id = %session_id,
        path = %dir_path,
        show_hidden = show_hidden,
        recursive = recursive,
        "Listing files"
    );

    let path = Path::new(dir_path);
    
    if !path.exists() {
        return Ok(WsResponse::error(&request.id, &format!("Path does not exist: {}", dir_path)));
    }

    let entries = list_directory(path, show_hidden, recursive)?;

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "entries": entries,
        "path": dir_path,
        "count": entries.len()
    })))
}

/// 处理文件读取请求
pub async fn handle_read(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

    let encoding: Option<String> = request.params.get("encoding")
        .and_then(|v| v.as_str())
        .map(String::from);

    info!(
        session_id = %session_id,
        file = %file_path,
        encoding = ?encoding,
        "Reading file"
    );

    let path = Path::new(file_path);
    
    if !path.exists() {
        return Ok(WsResponse::error(&request.id, &format!("File does not exist: {}", file_path)));
    }

    if !path.is_file() {
        return Ok(WsResponse::error(&request.id, &format!("Not a file: {}", file_path)));
    }

    let content = tokio::fs::read_to_string(file_path).await
        .map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))?;

    let metadata = tokio::fs::metadata(file_path).await.ok();

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "content": content,
        "path": file_path,
        "size": metadata.map(|m| m.len()).unwrap_or(0),
        "encoding": encoding.unwrap_or_else(|| "utf-8".to_string()),
        "line_count": content.lines().count()
    })))
}

/// 处理文件写入请求
pub async fn handle_write(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

    let content = request.params.get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter"))?;

    let create_dirs: bool = request.params.get("create_dirs")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    info!(
        session_id = %session_id,
        file = %file_path,
        bytes = content.len(),
        create_dirs = create_dirs,
        "Writing file"
    );

    let path = Path::new(file_path);

    // 如果需要，创建父目录
    if create_dirs
        && let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| anyhow::anyhow!("Failed to create directories: {}", e))?;
        }

    // 写入文件
    tokio::fs::write(file_path, content).await
        .map_err(|e| anyhow::anyhow!("Failed to write file: {}", e))?;

    let metadata = tokio::fs::metadata(file_path).await.ok();

    info!(file = %file_path, bytes = content.len(), "File written successfully");

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "message": format!("File saved successfully: {}", file_path),
        "bytes_written": content.len(),
        "size": metadata.map(|m| m.len()).unwrap_or(0),
        "written_at": chrono::Utc::now().to_rfc3339()
    })))
}

/// 处理文件监控请求
pub async fn handle_watch(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let path = request.params.get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;
    
    let watch_type: Option<String> = request.params.get("watch_type")
        .and_then(|v| v.as_str())
        .map(String::from);

    info!(
        session_id = %session_id,
        path = %path,
        watch_type = ?watch_type,
        "Setting up file watcher"
    );

    // TODO: 实现文件监控（使用 notify crate 或 tokio 的文件系统监控）
    // 返回 watcher ID 以便后续取消监控
    
    let watcher_id = format!("watcher_{}", uuid::Uuid::new_v4());

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "watcher_id": watcher_id,
        "path": path,
        "status": "watching",
        "message": format!("Now watching: {}", path),
        "supported_events": ["create", "modify", "delete", "rename"]
    })))
}

/// 列出目录内容
fn list_directory(path: &Path, show_hidden: bool, recursive: bool) -> Result<Vec<serde_json::Value>> {
    use std::fs;

    let mut entries = Vec::new();
    
    let read_dir = fs::read_dir(path)
        .map_err(|e| anyhow::anyhow!("Failed to read directory: {}", e))?;

    for entry in read_dir {
        if let Ok(entry) = entry {
            let entry_path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            
            // 过滤隐藏文件
            if !show_hidden && file_name.starts_with('.') {
                continue;
            }

            let metadata = fs::metadata(&entry_path).ok();
            
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let is_file = metadata.as_ref().map(|m| m.is_file()).unwrap_or(false);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = metadata.and_then(|m| m.modified().ok())
                .map(|t| {
                    let datetime: chrono::DateTime<chrono::Utc> = t.into();
                    datetime.to_rfc3339()
                });

            let entry_info = serde_json::json!({
                "name": file_name,
                "path": entry_path.to_string_lossy().to_string(),
                "is_directory": is_dir,
                "is_file": is_file,
                "size": size,
                "modified": modified,
                "extension": entry_path.extension().map(|e| e.to_string_lossy().to_string())
            });

            entries.push(entry_info);

            // 递归处理子目录
            if recursive && is_dir
                && let Ok(sub_entries) = list_directory(&entry_path, show_hidden, true) {
                    entries.extend(sub_entries);
                }
        }
    }

    // 按名称排序（目录优先）
    entries.sort_by(|a, b| {
        let a_is_dir = a.get("is_directory").and_then(|v| v.as_bool()).unwrap_or(false);
        let b_is_dir = b.get("is_directory").and_then(|v| v.as_bool()).unwrap_or(false);
        
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                a_name.cmp(b_name)
            }
        }
    });

    Ok(entries)
}
