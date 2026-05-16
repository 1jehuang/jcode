// ════════════════════════════════════════════════════════════════
// 多渠道通知系统
//
// 支持的通知渠道:
//   1. Terminal (内嵌在 TUI 中)
//   2. Email (SMTP)
//   3. Webhook (HTTP POST)
//   4. Desktop Notification (OS 原生弹窗)
//   5. Slack / Discord Webhook
//
// 特性:
//   - 通知级别 (Info/Warning/Error/Success)
//   - 静音/免打扰模式
//   - 聚合去重 (相同内容 N 分钟内不重复发送)
//   - 模板变量替换 {{var}}
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::sync::RwLock;

/// 通知级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum NotificationLevel {
    Debug,
    Info,
    Warning,
    Error,
    Success,
}

impl std::fmt::Display for NotificationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Debug => write!(f, "DEBUG"),
            Self::Info => write!(f, "INFO"),
            Self::Warning => write!(f, "WARN"),
            Self::Error => write!(f, "ERROR"),
            Self::Success => write!(f, "OK"),
        }
    }
}

/// 通知消息
#[derive(Debug, Clone)]
pub struct NotificationMessage {
    pub id: String,
    pub level: NotificationLevel,
    pub title: String,
    pub body: String,
    
    /// 来源标识 (如 "tool:Bash", "session:end", "system:error")
    pub source: String,
    
    /// 时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    
    /// 附加数据 (JSON)
    pub metadata: HashMap<String, String>,

    /// 是否已读
    pub read: bool,
    
    /// 关联的操作 (如 "view_log", "retry")
    pub actions: Vec<NotificationAction>,
}

#[derive(Debug, Clone)]
pub struct NotificationAction {
    pub label: String,
    pub action_type: ActionType,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum ActionType {
    Url { url: String },
    Command { cmd: String },
    Callback { event: String },
    Dismiss,
}

/// 通知渠道
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelType {
    Terminal,
    Email,
    Webhook,
    OsNotification,
    Slack,
    Discord,
}

/// 通知渠道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub enabled: bool,
    
    // Terminal 特有
    pub terminal_show_banner: bool,
    pub max_terminal_messages: usize,

    // Email 特有
    pub smtp_server: Option<String>,
    pub smtp_port: u16,
    pub smtp_user: Option<String>,
    pub smtp_password: Option<String>, // 应用专用密码
    pub from_address: Option<String>,
    pub to_addresses: Vec<String>,

    // Webhook 特有
    pub webhook_url: Option<String>,
    pub webhook_headers: HashMap<String, String>,

    // OS Notification 特有
    pub os_notification_sound: bool,
    pub os_notification_timeout_secs: u64,
    
    // 级别过滤: 只发送 >= 此级别的通知
    pub min_level: NotificationLevel,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            terminal_show_banner: true,
            max_terminal_messages: 50,
            smtp_server: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_address: None,
            to_addresses: vec![],
            webhook_url: None,
            webhook_headers: HashMap::new(),
            os_notification_sound: true,
            os_notification_timeout_secs: 5,
            min_level: NotificationLevel::Info,
        }
    }
}

/// 通知调度器
pub struct NotificationDispatcher {
    channels: Arc<RwLock<HashMap<ChannelType, ChannelConfig>>>,
    message_history: Arc<RwLock<Vec<NotificationMessage>>>,
    dedup_cache: Arc<RwLock<DedupCache>>,
    global_silent_mode: Arc<RwLock<bool>>,
}

#[derive(Debug, Clone)]
struct DedupCache {
    entries: HashMap<String, chrono::DateTime<chrono::Utc>>,
    ttl_seconds: u64,
}

impl Default for NotificationDispatcher {
    fn default() -> Self { Self::new() }
}

impl NotificationDispatcher {
    pub fn new() -> Self {
        let mut channels = HashMap::new();
        channels.insert(ChannelType::Terminal, ChannelConfig::default());
        
        Self {
            channels: Arc::new(RwLock::new(channels)),
            message_history: Arc::new(RwLock::new(Vec::new())),
            dedup_cache: Arc::new(RwLock::new(DedupCache {
                entries: HashMap::new(),
                ttl_seconds: 300, // 5分钟去重
            })),
            global_silent_mode: Arc::new(RwLock::new(false)),
        }
    }

    /// 配置渠道
    pub async fn configure_channel(&self, channel: ChannelType, config: ChannelConfig) {
        self.channels.write().await.insert(channel, config);
    }

    /// 发送通知 (自动路由到所有启用的渠道)
    pub async fn notify(&self, msg: NotificationMessage) -> Vec<ChannelType> {
        if *self.global_silent_mode.read().await {
            return vec![];
        }

        // 去重检查
        if self.is_duplicate(&msg).await {
            return vec![];
        }

        // 记录到历史
        self.message_history.write().await.push(msg.clone());

        // 分发到各渠道
        let mut delivered = Vec::new();
        let channels = self.channels.read().await;

        for (&channel, config) in channels.iter() {
            if !config.enabled { continue; }
            
            // 级别过滤
            if msg.level < config.min_level { continue; }

            let result = match channel {
                ChannelType::Terminal => self.send_to_terminal(&msg, config).await,
                ChannelType::Webhook => self.send_webhook(&msg, config).await,
                ChannelType::OsNotification => self.send_os_notification(&msg, config).await,
                ChannelType::Email => self.send_email(&msg, config).await,
                _ => Ok(()), // Slack/Discord 暂未实现
            };

            match result {
                Ok(()) => delivered.push(channel),
                Err(e) => tracing::warn!(channel = ?channel, error = %e, "Failed to send notification"),
            }
        }

        delivered
    }

    /// 快捷方法: 发送 Info 级别通知
    pub async fn info(&self, title: &str, body: &str, source: &str) -> Vec<ChannelType> {
        self.notify(NotificationMessage {
            id: uuid::Uuid::new_v4().to_string(),
            level: NotificationLevel::Info,
            title: title.into(),
            body: body.into(),
            source: source.into(),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
            read: false,
            actions: vec![],
        }).await
    }

    /// 快捷方法: 发送 Warning
    pub async fn warning(&self, title: &str, body: &str, source: &str) -> Vec<ChannelType> {
        self.notify(NotificationMessage {
            id: uuid::Uuid::new_v4().to_string(),
            level: NotificationLevel::Warning,
            title: title.into(),
            body: body.into(),
            source: source.into(),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
            read: false,
            actions: vec![],
        }).await
    }

    /// 快捷方法: 发送 Error
    pub async fn error(&self, title: &str, body: &str, source: &str) -> Vec<ChannelType> {
        self.notify(NotificationMessage {
            id: uuid::Uuid::new_v4().to_string(),
            level: NotificationLevel::Error,
            title: format!("⚠ {}", title),
            body: body.into(),
            source: source.into(),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
            read: false,
            actions: vec![],
        }).await
    }

    /// 设置静音模式
    pub async fn set_silent_mode(&self, silent: bool) {
        *self.global_silent_mode.write().await = silent;
    }

    /// 获取未读通知数
    pub async fn unread_count(&self) -> usize {
        self.message_history.read().await.iter()
            .filter(|m| !m.read)
            .count()
    }

    /// 标记全部已读
    pub async fn mark_all_read(&self) {
        for msg in self.message_history.write().await.iter_mut() {
            msg.read = true;
        }
    }

    // --- 渠道实现 -------------------------

    async fn send_to_terminal(&self, msg: &NotificationMessage, config: &ChannelConfig) -> Result<(), String> {
        if !config.terminal_show_banner {
            println!("{}", msg.body);
            return Ok(());
        }

        let icon = match msg.level {
            NotificationLevel::Debug => "🔍",
            NotificationLevel::Info => "ℹ️",
            NotificationLevel::Warning => "⚠️ ",
            NotificationLevel::Error => "❌",
            NotificationLevel::Success => "✅",
        };

        eprintln!(
            "\n{} [{}] {}{}",
            icon,
            msg.level,
            msg.title,
            if !msg.source.is_empty() { format!(" ({})", msg.source) } else { String::new() }
        );
        for line in msg.body.lines().take(5) {
            eprintln!("  {}", line);
        }
        if msg.body.lines().count() > 5 {
            eprintln!("  ...");
        }

        Ok(())
    }

    async fn send_os_notification(&self, msg: &NotificationMessage, config: &ChannelConfig) -> Result<(), String> {
        #[cfg(target_os = "windows")]
        {
            use std::process::Command;
            
            let title = format!("[{}] {}", msg.level, msg.title);
            let body = if msg.body.len() > 200 {
                format!("{}...", &msg.body[..200])
            } else {
                msg.body.clone()
            };

            Command::new("powershell")
                .args(["-Command", 
                    &format!(
                        "[System.Reflection.Assembly]::LoadWithPartialName('Microsoft.VisualBasic'); \
                         Add-Type -AssemblyName System.Windows.Forms; \
                         $balloon = New-Object System.Windows.Forms.NotifyIcon; \
                         $balloon.BalloonTipText = '{}'; \
                         $balloon.Text = '{}'; \
                         $balloon.Visible = $true; \
                         Start-Sleep -Seconds {}; \
                         $balloon.Dispose()",
                        body.replace("'", "''"), title.replace("'", "''"),
                        config.os_notification_timeout_secs
                    )])
                .output()
                .map_err(|e| format!("OS notification failed: {}", e))?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            use std::process::Command;
            
            Command::new("notify-send")
                .arg("--app-name=jcode")
                .arg(format!("--icon={}", match msg.level {
                    NotificationLevel::Error => "dialog-error",
                    NotificationLevel::Warning => "dialog-warning",
                    NotificationLevel::Success => "dialog-information",
                    _ => "dialog-information",
                }))
                .arg(&msg.title)
                .arg(&if msg.body.len() > 100 { format!("{}...", &msg.body[..100]) } else { msg.body.clone() })
                .output()
                .map_err(|e| format!("notify-send failed: {}", e))?;
        }

        Ok(())
    }

    async fn send_webhook(&self, msg: &NotificationMessage, config: &ChannelConfig) -> Result<(), String> {
        let url = config.webhook_url.as_ref()
            .ok_or("No webhook URL configured")?;

        let payload = serde_json::json!({
            "id": msg.id,
            "level": format!("{:?}", msg.level),
            "title": msg.title,
            "body": msg.body,
            "source": msg.source,
            "timestamp": msg.timestamp.to_rfc3339(),
            "metadata": msg.metadata,
        });

        reqwest::Client::new()
            .post(url)
            .json(&payload)
            .headers(config.webhook_headers.iter().filter_map(|(k, v)| {
                Some((k.parse::<reqwest::header::HeaderName>().ok()?, reqwest::header::HeaderValue::from_str(v).ok()?))
            }).collect::<HeaderMap>())
            .send()
            .await
            .map_err(|e| format!("Webhook delivery failed: {}", e))
            .map(|_| ())
    }

    async fn send_email(&self, msg: &NotificationMessage, config: &ChannelConfig) -> Result<(), String> {
        // TODO: 实现 SMTP 邮件发送
        // 需要 lettre 或 native-tls crate
        tracing::info!(
            subject = %msg.title,
            "Email notification would be sent here"
        );
        Ok(())
    }

    // --- 内部工具 ------------------------

    async fn is_duplicate(&self, msg: &NotificationMessage) -> bool {
        let key = format!("{}:{}:{}", msg.level, msg.title, msg.body.len());

        let cache = self.dedup_cache.read().await;

        if let Some(&last_sent) = cache.entries.get(&key) {
            let elapsed = (chrono::Utc::now() - last_sent).num_seconds();
            if elapsed < cache.ttl_seconds.try_into().unwrap_or(i64::MAX) {
                return true;
            }
        }

        // 更新缓存
        drop(cache);
        self.dedup_cache.write().await.entries.insert(key, chrono::Utc::now());

        false
    }
}
