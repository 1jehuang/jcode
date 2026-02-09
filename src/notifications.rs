//! Notification dispatcher for ambient mode.
//!
//! Sends notifications via:
//! - ntfy.sh (push notifications to phone)
//! - Desktop notifications (notify-send)
//! - Email (SMTP via lettre)
//!
//! All sends are fire-and-forget: errors are logged, never block.

use crate::config::{config, SafetyConfig};
use crate::logging;
use crate::safety::AmbientTranscript;

/// Notification priority levels (maps to ntfy priority header).
#[derive(Debug, Clone, Copy)]
pub enum Priority {
    /// Routine cycle summaries
    Default,
    /// Permission requests, errors
    High,
    /// Critical safety issues
    Urgent,
}

impl Priority {
    fn ntfy_value(self) -> &'static str {
        match self {
            Priority::Default => "3",
            Priority::High => "4",
            Priority::Urgent => "5",
        }
    }

    fn ntfy_tags(self) -> &'static str {
        match self {
            Priority::Default => "robot",
            Priority::High => "warning",
            Priority::Urgent => "rotating_light",
        }
    }
}

/// Dispatcher that sends notifications through all configured channels.
#[derive(Clone)]
pub struct NotificationDispatcher {
    client: reqwest::Client,
    config: SafetyConfig,
}

impl NotificationDispatcher {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            config: config().safety.clone(),
        }
    }

    pub fn from_config(config: SafetyConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    /// Send a cycle summary notification (after ambient cycle completes).
    pub fn dispatch_cycle_summary(&self, transcript: &AmbientTranscript) {
        let title = format!(
            "Ambient cycle: {} memories, {} compactions",
            transcript.memories_modified, transcript.compactions
        );
        let body = format_cycle_body(transcript);

        let priority = if transcript.pending_permissions > 0 {
            Priority::High
        } else {
            Priority::Default
        };

        self.send_all(&title, &body, priority);
    }

    /// Send a permission request notification (high priority).
    pub fn dispatch_permission_request(&self, action: &str, description: &str, request_id: &str) {
        let title = format!("Permission needed: {}", action);
        let body = format!(
            "{}\n\nRequest ID: {}\nReview pending permissions in jcode.",
            description, request_id
        );

        self.send_all(&title, &body, Priority::High);
    }

    /// Send through all configured channels (fire-and-forget).
    fn send_all(&self, title: &str, body: &str, priority: Priority) {
        // Guard: only dispatch if inside a tokio runtime
        if tokio::runtime::Handle::try_current().is_err() {
            logging::info("Notification skipped: no tokio runtime");
            return;
        }

        // ntfy.sh
        if let Some(ref topic) = self.config.ntfy_topic {
            let client = self.client.clone();
            let url = format!("{}/{}", self.config.ntfy_server, topic);
            let title = title.to_string();
            let body = body.to_string();
            let priority = priority;
            tokio::spawn(async move {
                if let Err(e) = send_ntfy(&client, &url, &title, &body, priority).await {
                    logging::error(&format!("ntfy notification failed: {}", e));
                }
            });
        }

        // Desktop notification (notify-send)
        if self.config.desktop_notifications {
            let title = title.to_string();
            let body = body.to_string();
            let urgency = match priority {
                Priority::Default => "normal",
                Priority::High | Priority::Urgent => "critical",
            };
            tokio::spawn(async move {
                send_desktop(&title, &body, urgency);
            });
        }

        // Email
        if self.config.email_enabled {
            if let (Some(ref to), Some(ref host), Some(ref from)) = (
                &self.config.email_to,
                &self.config.email_smtp_host,
                &self.config.email_from,
            ) {
                let to = to.clone();
                let host = host.clone();
                let from = from.clone();
                let port = self.config.email_smtp_port;
                let password = self.config.email_password.clone();
                let title = title.to_string();
                let body = body.to_string();
                tokio::spawn(async move {
                    if let Err(e) = send_email(&host, port, &from, &to, password.as_deref(), &title, &body).await {
                        logging::error(&format!("Email notification failed: {}", e));
                    }
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ntfy.sh
// ---------------------------------------------------------------------------

async fn send_ntfy(
    client: &reqwest::Client,
    url: &str,
    title: &str,
    body: &str,
    priority: Priority,
) -> anyhow::Result<()> {
    let resp = client
        .post(url)
        .header("Title", title)
        .header("Priority", priority.ntfy_value())
        .header("Tags", priority.ntfy_tags())
        .body(body.to_string())
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("ntfy returned {}: {}", status, text);
    }

    logging::info(&format!("ntfy notification sent: {}", title));
    Ok(())
}

// ---------------------------------------------------------------------------
// Desktop (notify-send)
// ---------------------------------------------------------------------------

fn send_desktop(title: &str, body: &str, urgency: &str) {
    let result = std::process::Command::new("notify-send")
        .arg("--app-name=jcode")
        .arg(format!("--urgency={}", urgency))
        .arg("--icon=dialog-information")
        .arg(title)
        .arg(body)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match result {
        Ok(status) if status.success() => {
            logging::info(&format!("Desktop notification sent: {}", title));
        }
        Ok(status) => {
            logging::warn(&format!("notify-send exited with {}", status));
        }
        Err(e) => {
            // notify-send not available - not an error, just skip
            logging::info(&format!("notify-send unavailable: {}", e));
        }
    }
}

// ---------------------------------------------------------------------------
// Email (SMTP via lettre)
// ---------------------------------------------------------------------------

async fn send_email(
    smtp_host: &str,
    smtp_port: u16,
    from: &str,
    to: &str,
    password: Option<&str>,
    subject: &str,
    body: &str,
) -> anyhow::Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;

    let mut transport_builder =
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(smtp_host)?
            .port(smtp_port);

    if let Some(pw) = password {
        transport_builder = transport_builder.credentials(Credentials::new(
            from.to_string(),
            pw.to_string(),
        ));
    }

    let transport = transport_builder.build();
    transport.send(email).await?;

    logging::info(&format!("Email notification sent to {}: {}", to, subject));
    Ok(())
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn format_cycle_body(transcript: &AmbientTranscript) -> String {
    let mut lines = Vec::new();

    if let Some(ref summary) = transcript.summary {
        lines.push(summary.clone());
        lines.push(String::new());
    }

    lines.push(format!("Status: {:?}", transcript.status));
    lines.push(format!("Provider: {} ({})", transcript.provider, transcript.model));
    lines.push(format!("Memories modified: {}", transcript.memories_modified));
    lines.push(format!("Compactions: {}", transcript.compactions));

    if transcript.pending_permissions > 0 {
        lines.push(String::new());
        lines.push(format!(
            "⚠ {} permission request(s) pending — review in jcode",
            transcript.pending_permissions
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_cycle_body() {
        let transcript = AmbientTranscript {
            session_id: "test_001".to_string(),
            started_at: chrono::Utc::now(),
            ended_at: Some(chrono::Utc::now()),
            status: crate::safety::TranscriptStatus::Complete,
            provider: "claude".to_string(),
            model: "claude-sonnet-4".to_string(),
            actions: Vec::new(),
            pending_permissions: 0,
            summary: Some("Cleaned up 3 stale memories.".to_string()),
            compactions: 1,
            memories_modified: 3,
        };

        let body = format_cycle_body(&transcript);
        assert!(body.contains("Cleaned up 3 stale memories."));
        assert!(body.contains("Memories modified: 3"));
        assert!(body.contains("Compactions: 1"));
        assert!(!body.contains("permission"));
    }

    #[test]
    fn test_format_cycle_body_with_pending_permissions() {
        let transcript = AmbientTranscript {
            session_id: "test_002".to_string(),
            started_at: chrono::Utc::now(),
            ended_at: Some(chrono::Utc::now()),
            status: crate::safety::TranscriptStatus::Complete,
            provider: "claude".to_string(),
            model: "claude-sonnet-4".to_string(),
            actions: Vec::new(),
            pending_permissions: 2,
            summary: None,
            compactions: 0,
            memories_modified: 0,
        };

        let body = format_cycle_body(&transcript);
        assert!(body.contains("2 permission request(s) pending"));
    }

    #[test]
    fn test_priority_values() {
        assert_eq!(Priority::Default.ntfy_value(), "3");
        assert_eq!(Priority::High.ntfy_value(), "4");
        assert_eq!(Priority::Urgent.ntfy_value(), "5");
    }

    #[test]
    fn test_dispatcher_creation() {
        // Just verify it doesn't panic
        let cfg = SafetyConfig::default();
        let _dispatcher = NotificationDispatcher::from_config(cfg);
    }
}
