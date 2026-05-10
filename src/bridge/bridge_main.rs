//! # Bridge Main — WebSocket 服务器主控
//!
//! 对应 Claude Code 的 `bridgeMain.ts` (~3,000行)
//!
//! 核心职责：
//! - WebSocket 连接管理 (accept/upgrade/handshake)
//! - IDE 客户端注册 & 认证
//! - 多连接协调 (同一 IDE 多窗口/多 Tab)
//! - 心跳检测 & 超时断开
//! - 统计信息收集

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, broadcast, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn, error};
use uuid::Uuid;
use chrono::{DateTime, Utc};

// ════════════════════════════════════════════════════════════════
// 配置类型
// ════════════════════════════════════════════════════════════════

/// Bridge 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// 监听地址
    pub bind_addr: SocketAddr,
    
    /// 最大并发连接数
    pub max_connections: usize,
    
    /// 心跳间隔 (秒)
    pub heartbeat_interval_secs: u64,
    
    /// 连接超时 (秒) - 无心跳则断开
    pub connection_timeout_secs: u64,
    
    /// 消息最大大小 (bytes)
    pub max_message_size: usize,
    
    /// 是否启用压缩
    pub enable_compression: bool,
    
    /// IDE 认证密钥 (可选)
    pub auth_token: Option<String>,
    
    /// 是否允许跨域
    pub allow_cors: bool,
    
    /// 允许的 Origin 列表
    pub allowed_origins: Vec<String>,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse()
                .expect("Hardcoded bind address literal should be valid"),
            max_connections: 100,
            heartbeat_interval_secs: 30,
            connection_timeout_secs: 90,
            max_message_size: 4 * 1024 * 1024,
            enable_compression: true,
            auth_token: None,
            allow_cors: true,
            allowed_origins: vec![
                "vscode-webview://*".to_string(),
                "jetbrains://*".to_string(),
                "http://localhost:*".to_string(),
                "http://127.0.0.1:*".to_string(),
            ],
        }
    }
}

/// IDE 客户端信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeClientInfo {
    /// 唯一连接 ID
    pub connection_id: String,
    
    /// IDE 类型
    pub ide_type: String,
    
    /// IDE 版本
    pub ide_version: Option<String>,
    
    /// 工作区路径
    pub workspace_path: Option<String>,
    
    /// 扩展版本
    pub extension_version: Option<String>,
    
    /// 用户代理
    pub user_agent: Option<String>,
    
    /// 连接时间
    pub connected_at: DateTime<Utc>,
    
    /// 最后活动时间
    pub last_activity_at: DateTime<Utc>,
    
    /// 能力列表 (支持的特性)
    pub capabilities: IdeCapabilities,
}

/// IDE 能力声明
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdeCapabilities {
    /// 支持 diff 显示
    pub can_show_diff: bool,
    
    /// 支持终端执行
    pub can_execute_terminal: bool,
    
    /// 支持文件编辑
    pub can_edit_files: bool,
    
    /// 支持状态栏更新
    pub can_update_status_bar: bool,
    
    /// 支持通知显示
    pub can_show_notifications: bool,
    
    /// 支持进度条
    pub can_show_progress: bool,
    
    /// 支持 inline chat
    pub has_inline_chat: bool,
}

/// 连接状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Authenticated,
    Ready,
    Disconnecting,
    Disconnected,
    Error,
}

// ════════════════════════════════════════════════════════════════
// Bridge 事件类型
// ════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeEvent {
    #[serde(rename = "type")]
    pub event_type: BridgeEventType,
    pub timestamp: DateTime<Utc>,
    pub connection_id: Option<String>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BridgeEventType {
    ClientConnected,
    ClientDisconnected,
    ClientAuthenticated,
    MessageReceived,
    MessageSent,
    ErrorOccurred,
    HeartbeatReceived,
    SessionCreated,
    SessionResumed,
    SessionClosed,
    ServerStarted,
    ServerStopped,
}

// ════════════════════════════════════════════════════════════════
// 内部连接状态
// ════════════════════════════════════════════════════════════════

#[derive(Debug)]
struct ActiveConnection {
    info: IdeClientInfo,
    state: ConnectionState,
    tx: mpsc::Sender<Message>,
    last_heartbeat: DateTime<Utc>,
    last_activity_at: DateTime<Utc>,
    message_count: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

// ════════════════════════════════════════════════════════════════
// Bridge Server 核心实现
// ════════════════════════════════════════════════════════════════

/// WebSocket Bridge 服务器
#[derive(Debug)]
pub struct BridgeServer {
    config: BridgeConfig,
    connections: Arc<RwLock<HashMap<String, ActiveConnection>>>,
    event_tx: broadcast::Sender<BridgeEvent>,
    shutdown_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    stats: Arc<RwLock<BridgeStats>>,
}

/// Bridge 运行统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeStats {
    pub total_connections: u64,
    pub active_connections: u64,
    pub total_messages_sent: u64,
    pub total_messages_received: u64,
    pub total_bytes_transferred: u64,
    pub uptime_seconds: u64,
    pub peak_connections: u64,
}

impl BridgeServer {
    /// 创建新的 Bridge 服务器实例
    pub fn new(config: BridgeConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        
        Self {
            config,
            connections: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            shutdown_tx: Arc::new(Mutex::new(None)),
            stats: Arc::new(RwLock::new(BridgeStats::default())),
        }
    }

    /// 启动 WebSocket 服务器
    pub async fn start(&self) -> Result<(), anyhow::Error> {
        let listener = tokio::net::TcpListener::bind(&self.config.bind_addr).await?;
        let actual_addr = listener.local_addr()?;
        
        info!("[BridgeServer] Listening on {}", actual_addr);
        
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        {
            let mut tx = self.shutdown_tx.lock().await;
            *tx = Some(shutdown_tx);
        }
        
        let connections = self.connections.clone();
        let event_tx = self.event_tx.clone();
        let config = self.config.clone();
        let stats = self.stats.clone();
        
        tokio::spawn(async move {
            info!("[BridgeServer] Accept loop started");
            
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, addr)) => {
                                let conn_id = Uuid::new_v4().to_string();
                                debug!("[BridgeServer] New connection from {} -> {}", addr, conn_id);
                                
                                if let Err(e) = handle_new_connection(
                                    stream,
                                    addr,
                                    conn_id.clone(),
                                    connections.clone(),
                                    event_tx.clone(),
                                    config.clone(),
                                    stats.clone(),
                                ).await {
                                    warn!("[BridgeServer] Connection handler error: {:?}", e);
                                }
                            }
                            Err(e) => {
                                error!("[BridgeServer] Accept error: {:?}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("[BridgeServer] Shutdown signal received");
                        break;
                    }
                }
            }
            
            info!("[BridgeServer] Accept loop stopped");
        });
        
        self.emit_event(BridgeEventType::ServerStarted, None, serde_json::json!({"addr": actual_addr.to_string()}));
        
        Ok(())
    }

    /// 停止服务器
    pub async fn stop(&self) -> Result<(), anyhow::Error> {
        let tx_opt = self.shutdown_tx.lock().await;
        if let Some(tx) = tx_opt.as_ref() {
            let _ = tx.send(()).await;
        }
        drop(tx_opt);
        
        let conns = self.connections.read().await;
        for (id, conn) in conns.iter() {
            let _ = conn.tx.send(Message::Close(None)).await;
            debug!("[BridgeServer] Sent close to {}", id);
        }
        
        self.emit_event(BridgeEventType::ServerStopped, None, serde_json::json!(null));
        
        Ok(())
    }

    /// 发送消息到指定客户端
    pub async fn send_to(&self, connection_id: &str, message: Message) -> Result<(), anyhow::Error> {
        let conns = self.connections.read().await;
        
        if let Some(conn) = conns.get(connection_id) {
            conn.tx.send(message).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Connection {} not found", connection_id))
        }
    }

    /// 广播消息到所有客户端
    pub async fn broadcast(&self, message: Message) -> Result<usize, anyhow::Error> {
        let conns = self.connections.read().await;
        let mut sent_count = 0;
        
        for (id, conn) in conns.iter() {
            if let Err(e) = conn.tx.send(message.clone()).await {
                warn!("[BridgeServer] Broadcast failed to {}: {:?}", id, e);
            } else {
                sent_count += 1;
            }
        }
        
        Ok(sent_count)
    }

    /// 获取所有活跃连接信息
    pub async fn get_active_clients(&self) -> Vec<IdeClientInfo> {
        let conns = self.connections.read().await;
        conns.values()
            .filter(|c| c.state == ConnectionState::Ready || c.state == ConnectionState::Authenticated)
            .map(|c| c.info.clone())
            .collect()
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> BridgeStats {
        self.stats.read().await.clone()
    }

    /// 按 IDE 类型查找连接
    pub async fn find_by_ide_type(&self, ide_type: &str) -> Option<IdeClientInfo> {
        let conns = self.connections.read().await;
        conns.values()
            .find(|c| c.info.ide_type == ide_type && (c.state == ConnectionState::Ready))
            .map(|c| c.info.clone())
    }

    // ─── 内部方法 ──────────────────────────────

    fn emit_event(&self, event_type: BridgeEventType, conn_id: Option<String>, data: serde_json::Value) {
        let event = BridgeEvent {
            event_type,
            timestamp: Utc::now(),
            connection_id: conn_id,
            data,
        };
        let _ = self.event_tx.send(event);
    }
}

// ════════════════════════════════════════════════════════════════
// 连接处理器
// ════════════════════════════════════════════════════════════════

async fn handle_new_connection(
    stream: tokio::net::TcpStream,
    addr: SocketAddr,
    conn_id: String,
    connections: Arc<RwLock<HashMap<String, ActiveConnection>>>,
    event_tx: broadcast::Sender<BridgeEvent>,
    _config: BridgeConfig,
    stats: Arc<RwLock<BridgeStats>>,
) -> Result<(), anyhow::Error> {
    use tokio_tungstenite::accept_async;
    use futures::{SinkExt, StreamExt};
    
    let ws_stream = accept_async(stream).await?;
    
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<Message>(100);
    
    let initial_info = IdeClientInfo {
        connection_id: conn_id.clone(),
        ide_type: "unknown".to_string(),
        ide_version: None,
        workspace_path: None,
        extension_version: None,
        user_agent: None,
        connected_at: Utc::now(),
        last_activity_at: Utc::now(),
        capabilities: IdeCapabilities::default(),
    };
    
    {
        let mut conns = connections.write().await;
        conns.insert(conn_id.clone(), ActiveConnection {
            info: initial_info,
            state: ConnectionState::Connected,
            tx,
            last_heartbeat: Utc::now(),
            last_activity_at: Utc::now(),
            message_count: 0,
            bytes_sent: 0,
            bytes_received: 0,
        });
        
        let mut s = stats.write().await;
        s.total_connections += 1;
        s.active_connections = conns.len() as u64;
        s.peak_connections = s.peak_connections.max(s.active_connections);
    }
    
    let _ = event_tx.send(BridgeEvent {
        event_type: BridgeEventType::ClientConnected,
        timestamp: Utc::now(),
        connection_id: Some(conn_id.clone()),
        data: serde_json::json!({"addr": addr.to_string()}),
    });
    
    info!("[BridgeConn] {} connected from {}", conn_id, addr);
    
    let conn_clone = connections.clone();
    let event_tx_clone = event_tx.clone();
    let stats_clone = stats.clone();
    let conn_id_clone = conn_id.clone();
    
    let receive_task = tokio::spawn(async move {
        while let Some(msg_result) = ws_receiver.next().await {
            match msg_result {
                Ok(msg) => {
                    if let Err(e) = handle_incoming_message(
                        msg,
                        &conn_id,
                        &conn_clone,
                        &event_tx_clone,
                        &stats_clone,
                    ).await {
                        warn!("[BridgeConn] Message handling error for {}: {:?}", conn_id, e);
                    }
                }
                Err(e) => {
                    warn!("[BridgeConn] Receive error for {}: {:?}", conn_id, e);
                    break;
                }
            }
        }
        
        cleanup_connection(&conn_id, &conn_clone, &event_tx_clone, &stats_clone).await;
    });
    
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = ws_sender.send(msg).await {
                warn!("[BridgeConn] Send error for {}: {:?}", conn_id_clone, e);
                break;
            }
        }
    });
    
    let _ = tokio::try_join!(receive_task, send_task);
    
    Ok(())
}

async fn handle_incoming_message(
    msg: Message,
    conn_id: &str,
    connections: &Arc<RwLock<HashMap<String, ActiveConnection>>>,
    event_tx: &broadcast::Sender<BridgeEvent>,
    stats: &Arc<RwLock<BridgeStats>>,
) -> Result<(), anyhow::Error> {
    match msg {
        Message::Text(text) => {
            debug!("[BridgeConn] Text from {}: {} chars", conn_id, text.len());
            
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                if parsed.get("type").and_then(|v| v.as_str()) == Some("heartbeat") {
                    update_heartbeat(conn_id, connections).await;
                    
                    let _ = event_tx.send(BridgeEvent {
                        event_type: BridgeEventType::HeartbeatReceived,
                        timestamp: Utc::now(),
                        connection_id: Some(conn_id.to_string()),
                        data: parsed,
                    });
                    
                    return Ok(());
                }
                
                if parsed.get("type").and_then(|v| v.as_str()) == Some("ide_info") {
                    register_ide_client(conn_id, &parsed, connections, event_tx).await;
                }
            }
            
            let _ = event_tx.send(BridgeEvent {
                event_type: BridgeEventType::MessageReceived,
                timestamp: Utc::now(),
                connection_id: Some(conn_id.to_string()),
                data: serde_json::json!({"size": text.len(), "type": "text"}),
            });
            
            let mut s = stats.write().await;
            s.total_messages_received += 1;
        }
        Message::Binary(data) => {
            debug!("[BridgeConn] Binary from {}: {} bytes", conn_id, data.len());
            
            let mut s = stats.write().await;
            s.total_messages_received += 1;
            s.total_bytes_transferred += data.len() as u64;
        }
        Message::Close(_) => {
            info!("[BridgeConn] Close frame from {}", conn_id);
        }
        Message::Ping(_) | Message::Pong(_) => {}
        Message::Frame(_) => {}  // Raw frame, ignore
    }
    
    Ok(())
}

async fn update_heartbeat(
    conn_id: &str,
    connections: &Arc<RwLock<HashMap<String, ActiveConnection>>>,
) {
    let mut conns = connections.write().await;
    if let Some(conn) = conns.get_mut(conn_id) {
        conn.last_heartbeat = Utc::now();
    }
}

async fn register_ide_client(
    conn_id: &str,
    info: &serde_json::Value,
    connections: &Arc<RwLock<HashMap<String, ActiveConnection>>>,
    event_tx: &broadcast::Sender<BridgeEvent>,
) {
    let ide_type = info.get("ideType")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    
    let workspace = info.get("workspacePath")
        .and_then(|v| v.as_str())
        .map(String::from);
    
    let ext_ver = info.get("extensionVersion")
        .and_then(|v| v.as_str())
        .map(String::from);
    
    let caps_raw = info.get("capabilities");
    let capabilities = if let Some(caps) = caps_raw {
        serde_json::from_value(caps.clone()).unwrap_or_default()
    } else {
        IdeCapabilities::default()
    };
    
    let mut conns = connections.write().await;
    if let Some(conn) = conns.get_mut(conn_id) {
        conn.info.ide_type = ide_type;
        conn.info.workspace_path = workspace;
        conn.info.extension_version = ext_ver;
        conn.info.capabilities = capabilities;
        conn.state = ConnectionState::Authenticated;
        conn.last_activity_at = Utc::now();
    }
    
    let _ = event_tx.send(BridgeEvent {
        event_type: BridgeEventType::ClientAuthenticated,
        timestamp: Utc::now(),
        connection_id: Some(conn_id.to_string()),
        data: info.clone(),
    });
    
    info!("[BridgeConn] {} registered as IDE client", conn_id);
}

async fn cleanup_connection(
    conn_id: &str,
    connections: &Arc<RwLock<HashMap<String, ActiveConnection>>>,
    event_tx: &broadcast::Sender<BridgeEvent>,
    stats: &Arc<RwLock<BridgeStats>>,
) {
    {
        let mut conns = connections.write().await;
        conns.remove(conn_id);
        
        let mut s = stats.write().await;
        s.active_connections = conns.len() as u64;
    }
    
    let _ = event_tx.send(BridgeEvent {
        event_type: BridgeEventType::ClientDisconnected,
        timestamp: Utc::now(),
        connection_id: Some(conn_id.to_string()),
        data: serde_json::json!(null),
    });
    
    info!("[BridgeConn] {} disconnected", conn_id);
}
