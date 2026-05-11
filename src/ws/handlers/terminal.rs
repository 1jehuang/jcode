//! 终端会话管理处理器
//!
//! 提供终端模拟功能：
//! - 创建/销毁终端会话
//! - 输入输出处理
//! - 终端尺寸调整

use crate::ws::protocol::{WsRequest, WsResponse, TerminalSessionInfo, TerminalSize, MessageType};
use crate::ws::session::SessionManager;
use anyhow::Result;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// 活跃的终端进程
struct ActiveTerminal {
    /// 子进程
    child: Child,
    /// PTY 或管道（用于 I/O）
    stdin: mpsc::Sender<Vec<u8>>,
    /// 终端信息
    info: TerminalSessionInfo,
}

/// 处理创建终端请求
pub async fn handle_create(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let shell_type = request.params.get("shell")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            // 根据操作系统选择默认 shell
            if cfg!(target_os = "windows") { "powershell" } else { "bash" }
        });

    let working_dir = request.params.get("working_dir")
        .and_then(|v| v.as_str());

    let rows: u16 = request.params.get("rows")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .unwrap_or(24);

    let cols: u16 = request.params.get("cols")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .unwrap_or(80);

    let terminal_id = format!("term_{}", uuid::Uuid::new_v4());

    info!(
        session_id = %session_id,
        terminal_id = %terminal_id,
        shell = %shell_type,
        rows = rows,
        cols = cols,
        working_dir = ?working_dir,
        "Creating terminal session"
    );

    // 构建命令
    let mut cmd = match shell_type {
        "bash" | "sh" | "zsh" | "fish" => Command::new(shell_type),
        "cmd" => Command::new("cmd.exe"),
        "powershell" | "pwsh" => Command::new(if shell_type == "pwsh" { "pwsh.exe" } else { "powershell.exe" }),
        _ => return Ok(WsResponse::error(&request.id, &format!("Unsupported shell type: {}", shell_type))),
    };

    // 设置工作目录
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    // 创建进程，使用 pipe 进行 I/O
    cmd.stdin(Stdio::piped())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());

    let mut child = cmd.spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn shell process: {}", e))?;

    let pid = child.id().ok_or_else(|| anyhow::anyhow!("Failed to get PID"))?;

    // 获取 stdin 句柄
    let mut stdin = child.stdin.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get stdin handle"))?;

    // 创建 stdout 读取通道
    let stdout = child.stdout.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get stdout handle"))?;

    let stderr = child.stderr.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get stderr handle"))?;

    // 创建输入通道 (mpsc)
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(100);

    // 启动输入写入任务
    let term_id_input = terminal_id.clone();
    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if let Err(e) = stdin.write_all(&data).await {
                error!(terminal_id = %term_id_input, error = %e, "Failed to write to terminal stdin");
                break;
            }
        }
    });

    // 启动 stdout 读取任务（需要广播到 WebSocket）
    // TODO: 将输出发送回客户端

    let term_info = TerminalSessionInfo {
        session_id: terminal_id.clone(),
        shell_type: shell_type.to_string(),
        size: TerminalSize { rows, cols },
        working_dir: working_dir.unwrap_or_else(|| ".".to_string()),
        pid,
    };

    // 注册到会话管理器
    session_manager.create_terminal(session_id, term_info.clone()).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    info!(
        terminal_id = %terminal_id,
        pid = pid,
        "Terminal created successfully"
    );

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "terminal": term_info,
        "message": format!("Terminal {} created with PID {}", terminal_id, pid)
    })))
}

/// 处理向终端写入数据请求
pub async fn handle_write(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let terminal_id = request.params.get("terminal_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'terminal_id' parameter"))?;

    let data = request.params.get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'data' parameter"))?;

    info!(
        session_id = %session_id,
        terminal_id = %terminal_id,
        data_len = data.len(),
        "Writing to terminal"
    );

    // TODO: 发送数据到对应的终端会话
    // 目前仅返回确认

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "bytes_written": data.len()
    })))
}

/// 处理调整终端尺寸请求
pub async fn handle_resize(
    request: &WsRequest,
    session_id: &str,
    _session_manager: &SessionManager,
) -> Result<WsResponse> {
    let terminal_id = request.params.get("terminal_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'terminal_id' parameter"))?;

    let rows: u16 = request.params.get("rows")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .ok_or_else(|| anyhow::anyhow!("Missing 'rows' parameter"))?;

    let cols: u16 = request.params.get("cols")
        .and_then(|v| v.as_u64())
        .map(|v| v as u16)
        .ok_or_else(|| anyhow::anyhow!("Missing 'cols' parameter"))?;

    info!(
        session_id = %session_id,
        terminal_id = %terminal_id,
        rows = rows,
        cols = cols,
        "Resizing terminal"
    );

    // TODO: 实现真正的终端尺寸调整（需要 PTY 支持）
    
    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "new_size": { "rows": rows, "cols": cols },
        "message": format!("Terminal {} resized to {}x{}", terminal_id, rows, cols)
    })))
}

/// 处理关闭终端请求
pub async fn handle_close(
    request: &WsRequest,
    session_id: &str,
    session_manager: &SessionManager,
) -> Result<WsResponse> {
    let terminal_id = request.params.get("terminal_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'terminal_id' parameter"))?;

    info!(
        session_id = %session_id,
        terminal_id = %terminal_id,
        "Closing terminal"
    );

    // 从会话中移除终端
    session_manager.close_terminal(session_id, terminal_id).await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // TODO: 终止实际的子进程

    Ok(WsResponse::new(&request.id, MessageType::Response, serde_json::json!({
        "success": true,
        "message": format!("Terminal {} closed", terminal_id)
    })))
}
