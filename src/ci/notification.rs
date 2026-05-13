use std::sync::Arc;
use tokio::sync::RwLock;

/// Notification channel
#[derive(Debug, Clone)]
pub enum NotificationChannel {
    Console,
    Log,
}

/// Notification level
#[derive(Debug, Clone)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
    Success,
}

/// Pipeline notification
#[derive(Debug, Clone)]
pub struct PipelineNotification {
    pub title: String,
    pub message: String,
    pub level: NotificationLevel,
    pub pipeline_name: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Notification service for pipeline events
#[derive(Debug, Clone)]
pub struct NotificationService {
    notifications: Arc<RwLock<Vec<PipelineNotification>>>,
    channels: Vec<NotificationChannel>,
}

impl NotificationService {
    pub fn new() -> Self {
        NotificationService {
            notifications: Arc::new(RwLock::new(vec![])),
            channels: vec![NotificationChannel::Console],
        }
    }

    pub fn with_channel(mut self, channel: NotificationChannel) -> Self {
        self.channels.push(channel);
        self
    }

    pub async fn notify(&self, title: &str, message: &str, level: NotificationLevel, pipeline_name: &str) {
        let notification = PipelineNotification {
            title: title.to_string(),
            message: message.to_string(),
            level,
            pipeline_name: pipeline_name.to_string(),
            timestamp: chrono::Utc::now(),
        };

        for channel in &self.channels {
            match channel {
                NotificationChannel::Console => {
                    let prefix = match &notification.level {
                        NotificationLevel::Info => "[CI]",
                        NotificationLevel::Warning => "[CI WARN]",
                        NotificationLevel::Error => "[CI ERROR]",
                        NotificationLevel::Success => "[CI OK]",
                    };
                    eprintln!("{} {}: {}", prefix, notification.title, notification.message);
                }
                NotificationChannel::Log => {}
            }
        }

        self.notifications.write().await.push(notification);
    }

    pub async fn get_notifications(&self) -> Vec<PipelineNotification> {
        self.notifications.read().await.clone()
    }

    pub async fn clear(&self) {
        self.notifications.write().await.clear();
    }
}