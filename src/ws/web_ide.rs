//! Web IDE WebSocket Server
//!
//! 提供完整的 Web IDE 功能支持：
//! - 实时代码编辑与同步 (OT/CRDT)
//! - LSP 语言服务集成（补全、诊断、定义跳转）
//! - 文件系统操作（浏览、读写、监控）
//! - 终端会话管理
//! - Git 工作流集成
//! - AI 助手交互（流式响应）
//! - 多用户协作编辑
//! - 项目管理与构建

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use super::protocol::{WsMessage, WsRequest, WsResponse, MessageType};
use super::session::SessionManager;
use super::handlers;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message};
use tracing::{error, info, warn};

/// WebSocket 服务器配置
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// 监听端口
    pub port: u16,
    /// 最大连接数
    pub max_connections: usize,
    /// 心跳间隔（秒）
    pub heartbeat_interval: u64,
    /// 是否启用 LSP 集成
    pub enable_lsp: bool,
    /// 是否启用终端支持
    pub enable_terminal: bool,
    /// 是否启用协作编辑
    pub enable_collaboration: bool,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            max_connections: 100,
            heartbeat_interval: 30,
            enable_lsp: true,
            enable_terminal: true,
            enable_collaboration: true,
        }
    }
}

/// Web IDE WebSocket 服务器
pub struct WebIdeWebSocketServer {
    /// 服务器配置
    config: WebSocketConfig,
    
    /// 会话管理器
    session_manager: Arc<SessionManager>,
    
    /// 广播通道（用于多用户协作）
    broadcast_tx: broadcast::Sender<WsMessage>,
    
    /// 活跃连接计数
    active_connections: Arc<RwLock<usize>>,
}

impl WebIdeWebSocketServer {
    /// 创建新的 WebSocket 服务器实例
    pub fn new(config: WebSocketConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        
        Self {
            config,
            session_manager: Arc::new(SessionManager::new()),
            broadcast_tx,
            active_connections: Arc::new(RwLock::new(0)),
        }
    }

    /// 使用默认配置创建服务器
    pub fn with_port(port: u16) -> Self {
        Self::new(WebSocketConfig {
            port,
            ..Default::default()
        })
    }

    /// 启动 WebSocket 服务器并开始监听连接
    pub async fn serve(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        
        info!(
            port = self.config.port,
            max_connections = self.config.max_connections,
            "Web IDE WebSocket server listening on ws://{}",
            addr
        );

        println!("🌐 Web IDE WebSocket Server started on ws://{}", addr);
        println!("   Features:");
        if self.config.enable_lsp {
            println!("   ✅ LSP Integration (code completion, diagnostics, navigation)");
        }
        if self.config.enable_terminal {
            println!("   ✅ Terminal Sessions");
        }
        if self.config.enable_collaboration {
            println!("   ✅ Real-time Collaboration Editing");
        }
        println!();

        // 启动心跳任务
        let heartbeat_tx = self.broadcast_tx.clone();
        let heartbeat_interval = self.config.heartbeat_interval;
        tokio::spawn(async move {
            Self::heartbeat_task(heartbeat_tx, heartbeat_interval).await;
        });

        // 接受新连接
        while let Ok((stream, peer_addr)) = listener.accept().await {
            let current_connections = *self.active_connections.read();
            
            if current_connections >= self.config.max_connections {
                warn!(
                    addr = %peer_addr,
                    max = self.config.max_connections,
                    "Connection rejected: maximum connections reached"
                );
                drop(stream);
                continue;
            }

            // 增加连接计数
            *self.active_connections.write() += 1;

            let session_manager = self.session_manager.clone();
            let broadcast_tx = self.broadcast_tx.clone();
            let active_connections = self.active_connections.clone();
            let config = self.config.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(
                    stream,
                    peer_addr,
                    session_manager,
                    broadcast_tx,
                    active_connections,
                    config,
                ).await {
                    error!(addr = %peer_addr, error = %e, "Connection error");
                }
                
                // 减少连接计数
                *active_connections.write() -= 1;
            });
        }

        Ok(())
    }

    /// 处理单个 WebSocket 连接
    async fn handle_connection(
        stream: TcpStream,
        peer_addr: std::net::SocketAddr,
        session_manager: Arc<SessionManager>,
        broadcast_tx: broadcast::Sender<WsMessage>,
        _active_connections: Arc<RwLock<usize>>,
        config: WebSocketConfig,
    ) -> Result<()> {
        info!(addr = %peer_addr, "New WebSocket connection");

        let ws_stream = accept_async(stream).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // 创建新的客户端会话
        let session_id = session_manager.create_session(peer_addr).await;
        
        // 发送欢迎消息
        let welcome_msg = WsResponse::new(&session_id, MessageType::Welcome, serde_json::json!({
            "session_id": session_id,
            "server_version": env!("CARGO_PKG_VERSION"),
            "features": {
                "lsp": config.enable_lsp,
                "terminal": config.enable_terminal,
                "collaboration": config.enable_collaboration,
            },
            "supported_methods": [
                "editor.open",
                "editor.close",
                "editor.edit",
                "editor.save",
                "editor.completion",
                "editor.definition",
                "editor.references",
                "editor.diagnostics",
                "file.list",
                "file.read",
                "file.write",
                "file.watch",
                "terminal.create",
                "terminal.write",
                "terminal.resize",
                "terminal.close",
                "git.status",
                "git.diff",
                "git.commit",
                "git.branch",
                "git.log",
                "ai.chat",
                "ai.complete",
                "ai.explain",
                "collaboration.join",
                "collaboration.leave",
                "collaboration.cursor",
                "collaboration.edit",
                "project.build",
                "project.test",
                "project.run",
                "system.ping",
                "system.info",
            ]
        }));
        
        ws_write.send(Message::Text(serde_json::to_string(&welcome_msg)?)).await?;

        // 订阅广播频道
        let mut rx = broadcast_tx.subscribe();

        // 消息处理循环
        loop {
            tokio::select! {
                // 处理来自客户端的消息
                Some(msg_result) = ws_read.next() => {
                    match msg_result {
                        Ok(msg) => {
                            match msg {
                                Message::Text(text) => {
                                    // 解析并处理请求
                                    match Self::process_client_message(&text, &session_id, &session_manager, &config).await {
                                        Ok(response) => {
                                            if let Some(resp) = response {
                                                ws_write.send(Message::Text(serde_json::to_string(&resp)?)).await?;
                                            }
                                        }
                                        Err(e) => {
                                            error!(error = %e, "Failed to process message");
                                            let error_resp = WsResponse::error(&session_id, &e.to_string());
                                            ws_write.send(Message::Text(serde_json::to_string(&error_resp)?)).await?;
                                        }
                                    }
                                }
                                Message::Binary(data) => {
                                    // 处理二进制消息（用于大文件传输等）
                                    warn!(len = data.len(), "Binary message received");
                                }
                                Message::Ping(payload) => {
                                    ws_write.send(Message::Pong(payload)).await?;
                                }
                                Message::Pong(_) => {
                                    // 收到 pong，更新心跳状态
                                    session_manager.update_heartbeat(&session_id).await;
                                }
                                Message::Close(close_frame) => {
                                    info!(addr = %peer_addr, reason = ?close_frame, "Client disconnected");
                                    break;
                                }
                                Message::Frame(_) => {}
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "WebSocket read error");
                            break;
                        }
                    }
                }
                // 处理广播消息（来自其他客户端的协作数据）
                Ok(broadcast_msg) = rx.recv() => {
                    // 转发广播消息给当前客户端（除了发送者自己）
                    if broadcast_msg.session_id != session_id {
                        ws_write.send(Message::Text(serde_json::to_string(&broadcast_msg)?)).await?;
                    }
                }
            }
        }

        // 清理会话
        session_manager.remove_session(&session_id).await;
        info!(addr = %peer_addr, session_id = %session_id, "Connection closed and session cleaned up");

        Ok(())
    }

    /// 处理客户端消息并返回响应（如果有）
    async fn process_client_message(
        message_text: &str,
        session_id: &str,
        session_manager: &Arc<SessionManager>,
        config: &WebSocketConfig,
    ) -> Result<Option<WsResponse>> {
        // 解析 JSON-RPC 风格的请求
        let request: WsRequest = serde_json::from_str(message_text)
            .map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;

        info!(
            session_id = %session_id,
            method = %request.method,
            id = %request.id,
            "Processing request"
        );

        // 根据方法类型路由到不同的处理器
        let response = match request.method.as_str() {
            // === 编辑器操作 ===
            "editor.open" => handlers::editor::handle_open(&request, session_id, session_manager).await,
            "editor.close" => handlers::editor::handle_close(&request, session_id, session_manager).await,
            "editor.edit" => handlers::editor::handle_edit(&request, session_id, session_manager).await,
            "editor.save" => handlers::editor::handle_save(&request, session_id, session_manager).await,
            
            // === LSP 功能 ===
            "editor.completion" if config.enable_lsp => {
                handlers::lsp::handle_completion(&request, session_id, session_manager).await
            }
            "editor.definition" if config.enable_lsp => {
                handlers::lsp::handle_definition(&request, session_id, session_manager).await
            }
            "editor.references" if config.enable_lsp => {
                handlers::lsp::handle_references(&request, session_id, session_manager).await
            }
            "editor.diagnostics" if config.enable_lsp => {
                handlers::lsp::handle_diagnostics(&request, session_id, session_manager).await
            }
            
            // === 文件系统操作 ===
            "file.list" => handlers::fs::handle_list(&request, session_id, session_manager).await,
            "file.read" => handlers::fs::handle_read(&request, session_id, session_manager).await,
            "file.write" => handlers::fs::handle_write(&request, session_id, session_manager).await,
            "file.watch" => handlers::fs::handle_watch(&request, session_id, session_manager).await,
            
            // === 终端操作 ===
            "terminal.create" if config.enable_terminal => {
                handlers::terminal::handle_create(&request, session_id, session_manager).await
            }
            "terminal.write" if config.enable_terminal => {
                handlers::terminal::handle_write(&request, session_id, session_manager).await
            }
            "terminal.resize" if config.enable_terminal => {
                handlers::terminal::handle_resize(&request, session_id, session_manager).await
            }
            "terminal.close" if config.enable_terminal => {
                handlers::terminal::handle_close(&request, session_id, session_manager).await
            }
            
            // === Git 操作 ===
            "git.status" => handlers::git::handle_status(&request, session_id, session_manager).await,
            "git.diff" => handlers::git::handle_diff(&request, session_id, session_manager).await,
            "git.commit" => handlers::git::handle_commit(&request, session_id, session_manager).await,
            "git.branch" => handlers::git::handle_branch(&request, session_id, session_manager).await,
            "git.log" => handlers::git::handle_log(&request, session_id, session_manager).await,
            
            // === AI 助手 ===
            "ai.chat" => handlers::ai::handle_chat(&request, session_id, session_manager).await,
            "ai.complete" => handlers::ai::handle_complete(&request, session_id, session_manager).await,
            "ai.explain" => handlers::ai::handle_explain(&request, session_id, session_manager).await,
            
            // === 协作编辑 ===
            "collaboration.join" if config.enable_collaboration => {
                handlers::collab::handle_join(&request, session_id, session_manager).await
            }
            "collaboration.leave" if config.enable_collaboration => {
                handlers::collab::handle_leave(&request, session_id, session_manager).await
            }
            "collaboration.cursor" if config.enable_collaboration => {
                handlers::collab::handle_cursor_update(&request, session_id, session_manager).await
            }
            "collaboration.edit" if config.enable_collaboration => {
                handlers::collab::handle_edit(&request, session_id, session_manager).await
            }
            
            // === 项目管理 ===
            "project.build" => handlers::project::handle_build(&request, session_id, session_manager).await,
            "project.test" => handlers::project::handle_test(&request, session_id, session_manager).await,
            "project.run" => handlers::project::handle_run(&request, session_id, session_manager).await,
            
            // === 系统操作 ===
            "system.ping" => handlers::system::handle_ping(&request, session_id, session_manager).await,
            "system.info" => handlers::system::handle_info(&request, session_id, session_manager).await,
            
            _ => {
                warn!(method = %request.method, "Unknown method");
                Ok(WsResponse::error(&request.id, &format!("Unknown method: {}", request.method)))
            }
        };

        response.map(Some)
    }

    /// 心跳任务：定期向所有客户端发送心跳
    async fn heartbeat_task(tx: broadcast::Sender<WsMessage>, interval_secs: u64) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        
        loop {
            interval.tick().await;
            
            let heartbeat = WsMessage::heartbeat();
            if let Err(e) = tx.send(heartbeat) {
                error!(error = %e, "Failed to send heartbeat");
                break;
            }
        }
    }
}
