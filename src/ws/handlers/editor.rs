//! 编辑器操作处理器
//!
//! 处理文档打开、关闭、保存和编辑操作

use crate::ws::protocol::{WsRequest, WsResponse, DocumentState, TextEditOperation, CursorPosition, MessageType, EditOperationType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use tracing::{info};

/// 处理文档打开请求
pub async fn handle_open(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    let language_id = request.params.get("language_id")
        .and_then(|v| v.as_str())
        .unwrap_or("plaintext")
        .to_string();

    info!(
        session_id = %session_id,
        file = %file_path,
        language = %language_id,
        "Opening document"
    );

    // 读取文件内容
    let content = tokio::fs::read_to_string(file_path)
        .await
        .unwrap_or_default();

    // 创建文档状态
    let doc_state = DocumentState {
        file_path: file_path.to_string(),
        content: content.clone(),
        version: 1,
        cursor: None,
        selection: None,
        language_id: language_id.clone(),
    };

    // 注册到会话
    session_manager.open_document(session_id, doc_state).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "document": {
            "file_path": file_path,
            "content": content,
            "language_id": language_id,
            "version": 1,
            "line_count": content.lines().count(),
            "char_count": content.chars().count(),
        }
    })))
}

/// 处理文档关闭请求
pub async fn handle_close(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    info!(
        session_id = %session_id,
        file = %file_path,
        "Closing document"
    );

    // 从会话中移除文档
    session_manager.close_document(session_id, file_path).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "message": format!("Document {} closed", file_path)
    })))
}

/// 处理文档编辑请求（OT 操作）
pub async fn handle_edit(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    let operations: Vec<TextEditOperation> = serde_json::from_value(
        request.params.get("operations").cloned().unwrap_or(serde_json::Value::Array(vec![]))
    ).map_err(|e| anyhow::anyhow!("Invalid operations: {}", e))?;

    let expected_version: u64 = request.params.get("version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    info!(
        session_id = %session_id,
        file = %file_path,
        ops_count = operations.len(),
        version = expected_version,
        "Processing edit operations"
    );

    // 获取当前文档状态
    let mut doc_state = session_manager.get_open_document(session_id, file_path)
        .ok_or_else(|| anyhow::anyhow!("Document not open: {}", file_path))?;

    // 版本检查
    if doc_state.version != expected_version {
        return Ok(WsResponse::error(&request.id, &format!(
            "Version mismatch: expected {}, got {}",
            expected_version, doc_state.version
        )));
    }

    // 应用编辑操作（按位置排序后依次应用）
    let mut content = doc_state.content;
    
    for op in &operations {
        content = apply_operation(&content, op)?;
    }

    // 更新文档状态
    doc_state.content = content.clone();
    doc_state.version += 1;
    
    // 更新光标位置（如果有）
    if let Some(cursor) = request.params.get("cursor") {
        doc_state.cursor = serde_json::from_value(cursor.clone()).ok();
    }

    // 保存更新后的文档状态
    session_manager.open_document(session_id, doc_state.clone()).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "version": doc_state.version,
        "line_count": content.lines().count(),
        "char_count": content.chars().count(),
        "applied_operations": operations.len()
    })))
}

/// 处理文档保存请求
pub async fn handle_save(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let file_path = request.params.get("file_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' parameter"))?;

    info!(
        session_id = %session_id,
        file = %file_path,
        "Saving document"
    );

    // 获取当前文档内容
    if let Some(doc_state) = session_manager.get_open_document(session_id, file_path) {
        // 写入文件
        tokio::fs::write(file_path, &doc_state.content).await
            .map_err(|e| anyhow::anyhow!("Failed to save file: {}", e))?;

        info!(file = %file_path, bytes = doc_state.content.len(), "File saved successfully");

        Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
            "success": true,
            "message": format!("File saved: {}", file_path),
            "bytes_written": doc_state.content.len(),
            "saved_at": chrono::Utc::now().to_rfc3339()
        })))
    } else {
        Ok(WsResponse::error(&request.id, &format!("Document not open: {}", file_path)))
    }
}

/// 应用单个文本编辑操作到内容上
fn apply_operation(content: &str, operation: &TextEditOperation) -> Result<String> {
    match operation.op_type {
        EditOperationType::Insert => {
            let pos = position_to_offset(content, &operation.start);
            let mut new_content = String::with_capacity(content.len() + operation.text.as_ref().unwrap_or(&String::new()).len());
            
            if let Some(pos) = pos {
                new_content.push_str(&content[..pos]);
                new_content.push_str(operation.text.as_ref().unwrap_or(&String::new()));
                new_content.push_str(&content[pos..]);
            } else {
                return Err(anyhow::anyhow!("Invalid insert position"));
            }
            
            Ok(new_content)
        },
        
        EditOperationType::Delete => {
            let start_pos = position_to_offset(content, &operation.start)
                .ok_or_else(|| anyhow::anyhow!("Invalid delete start position"))?;
            let end_pos = position_to_offset(content, operation.end.as_ref().unwrap_or(&operation.start))
                .ok_or_else(|| anyhow::anyhow!("Invalid delete end position"))?;
            
            if start_pos > end_pos {
                return Err(anyhow::anyhow!("Start position after end position"));
            }
            
            let mut new_content = String::with_capacity(content.len());
            new_content.push_str(&content[..start_pos]);
            new_content.push_str(&content[end_pos..]);
            
            Ok(new_content)
        },
        
        EditOperationType::Replace => {
            // 先删除，再插入
            let without_deleted = apply_operation(content, &TextEditOperation {
                op_type: EditOperationType::Delete,
                start: operation.start.clone(),
                end: operation.end.clone(),
                text: None,
            })?;

            apply_operation(&without_deleted, &TextEditOperation {
                op_type: EditOperationType::Insert,
                start: operation.start.clone(),
                end: None,
                text: operation.text.clone(),
            })
        },
    }
}

/// 将光标位置转换为字符串偏移量
fn position_to_offset(content: &str, pos: &CursorPosition) -> Option<usize> {
    let lines: Vec<&str> = content.lines().collect();
    
    if pos.line as usize >= lines.len() {
        return None;
    }

    let mut offset = 0;
    for i in 0..pos.line as usize {
        offset += lines[i].len() + 1; // +1 for newline
    }
    
    offset += pos.character as usize;
    
    Some(offset.min(content.len()))
}
