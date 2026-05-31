//! Notification dispatcher for ambient mode.
//!
//! Sends notifications via:
//! - ntfy.sh (push notifications to phone)
//! - Desktop notifications (notify-send)
//! - Email (SMTP via lettre)
//!
//! All sends are fire-and-forget: errors are logged, never block.

use crate::config::{SafetyConfig, config};
use crate::logging;
use crate::safety::AmbientTranscript;

use jcode_notify_email::{
    ReplyAction, SendEmailRequest, build_permission_email_html, poll_imap_once, send_email,
};
pub use jcode_notify_email::{extract_permission_id, parse_permission_reply};

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
    channels: crate::channel::ChannelRegistry,
}

impl Default for NotificationDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationDispatcher {
    pub fn new() -> Self {
        let cfg = config().safety.clone();
        Self {
            client: crate::provider::shared_http_client(),
            channels: crate::channel::ChannelRegistry::from_config(&cfg),
            config: cfg,
        }
    }

    #[cfg(test)]
    pub fn from_config(config: SafetyConfig) -> Self {
        Self {
            client: crate::provider::shared_http_client(),
            channels: crate::channel::ChannelRegistry::from_config(&config),
            config,
        }
    }

    /// Send a cycle summary notification (after ambient cycle completes).
    pub fn dispatch_cycle_summary(&self, transcript: &AmbientTranscript) {
        let title = format!(
            "Ambient cycle: {} memories, {} compactions",
            transcript.memories_modified, transcript.compactions
        );
        let safe_body = format_cycle_body_safe(transcript);
        let detailed_body = format_cycle_body_detailed(transcript);

        let priority = if transcript.pending_permissions > 0 {
            Priority::High
        } else {
            Priority::Default
        };

        self.send_all(
            &title,
            &safe_body,
            &detailed_body,
            priority,
            Some(&transcript.session_id),
        );
    }

    /// Send a permission request notification (high priority).
    pub fn dispatch_permission_request(&self, action: &str, description: &str, request_id: &str) {
        let title = format!("jcode: permission needed ({})", action);
        let safe_body = "An ambient action needs your approval. Open jcode to review.".to_string();
        let detailed_body = format!(
            "Action: {}\n{}\n\nRequest ID: {}\nReview in jcode to approve or deny.",
            action, description, request_id
        );

        // Build rich HTML email with approve/deny buttons
        let reply_to = self
            .config
            .email_from
            .as_deref()
            .unwrap_or("jcode@localhost");
        let email_html = build_permission_email_html(action, description, request_id, reply_to);

        self.send_all_with_email_override(
            &title,
            &safe_body,
            &detailed_body,
            Priority::High,
            Some(request_id),
            Some(&email_html),
        );
    }

    /// Send through all configured channels (fire-and-forget).
    ///
    /// `safe_body` is sanitized (no secrets) — used for ntfy (potentially public).
    /// `detailed_body` includes full info — used for email and desktop (private channels).
    /// `cycle_id` is embedded as Message-ID in emails for reply tracking.
    fn send_all(
        &self,
        title: &str,
        safe_body: &str,
        detailed_body: &str,
        priority: Priority,
        cycle_id: Option<&str>,
    ) {
        self.send_all_with_email_override(
            title,
            safe_body,
            detailed_body,
            priority,
            cycle_id,
            None,
        );
    }

    /// Like `send_all`, but with an optional pre-built HTML body for the email channel.
    /// When `email_html_override` is Some, it's used directly as the email body instead
    /// of converting `detailed_body` through `markdown_to_html_email`.
    fn send_all_with_email_override(
        &self,
        title: &str,
        safe_body: &str,
        detailed_body: &str,
        priority: Priority,
        cycle_id: Option<&str>,
        email_html_override: Option<&str>,
    ) {
        // Guard: only dispatch if inside a tokio runtime
        if tokio::runtime::Handle::try_current().is_err() {
            logging::info("Notification skipped: no tokio runtime");
            return;
        }

        // ntfy.sh — uses SAFE body (may be publicly readable)
        if let Some(ref topic) = self.config.ntfy_topic {
            let client = self.client.clone();
            let url = format!("{}/{}", self.config.ntfy_server, topic);
            let title = title.to_string();
            let body = safe_body.to_string();
            tokio::spawn(async move {
                if let Err(e) = send_ntfy(&client, &url, &title, &body, priority).await {
                    logging::error(&format!("ntfy notification failed: {}", e));
                }
            });
        }

        // Desktop notification — uses DETAILED body (local machine, private)
        if self.config.desktop_notifications {
            let title = title.to_string();
            let body = detailed_body.to_string();
            let urgency = match priority {
                Priority::Default => "normal",
                Priority::High | Priority::Urgent => "critical",
            };
            tokio::spawn(async move {
                send_desktop(&title, &body, urgency);
            });
        }

        // Email — uses DETAILED body (sent to your own address, private)
        // If email_html_override is provided, send it directly as HTML.
        if self.config.email_enabled
            && let (Some(to), Some(host), Some(from)) = (
                &self.config.email_to,
                &self.config.email_smtp_host,
                &self.config.email_from,
            )
        {
            let to = to.clone();
            let host = host.clone();
            let from = from.clone();
            let port = self.config.email_smtp_port;
            let password = self.config.email_password.clone();
            let title = title.to_string();
            let body = detailed_body.to_string();
            let cycle_id = cycle_id.map(|s| s.to_string());
            let html_override = email_html_override.map(|s| s.to_string());
            tokio::spawn(async move {
                if let Err(e) = send_email(SendEmailRequest {
                    smtp_host: &host,
                    smtp_port: port,
                    from: &from,
                    to: &to,
                    password: password.as_deref(),
                    subject: &title,
                    body: &body,
                    cycle_id: cycle_id.as_deref(),
                    html_override: html_override.as_deref(),
                })
                .await
                {
                    logging::error(&format!("Email notification failed: {}", e));
                } else {
                    logging::info(&format!("Email notification sent to {}: {}", to, title));
                }
            });
        }

        // Message channels (Telegram, Discord, etc.) — uses DETAILED body
        let channel_text = format!("*{}*\n\n{}", title, detailed_body);
        self.channels.send_all(&channel_text);
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
// IMAP reply polling
// ---------------------------------------------------------------------------

/// Run an IMAP polling loop checking for replies to ambient emails.
/// Should be spawned as a tokio task alongside the ambient runner.
pub async fn imap_reply_loop(config: SafetyConfig) {
    let host = match config.email_imap_host.as_ref() {
        Some(h) => h.clone(),
        None => {
            logging::error("IMAP reply loop: no imap_host configured");
            return;
        }
    };
    let port = config.email_imap_port;
    let user = match config.email_from.as_ref() {
        Some(u) => u.clone(),
        None => {
            logging::error("IMAP reply loop: no email_from configured");
            return;
        }
    };
    let pass = match config.email_password.as_ref() {
        Some(p) => p.clone(),
        None => {
            logging::error("IMAP reply loop: no email password configured");
            return;
        }
    };

    logging::info(&format!(
        "IMAP reply loop: starting ({}:{}, user: {})",
        host, port, user
    ));

    loop {
        // Run synchronous IMAP in a blocking task
        let h = host.clone();
        let u = user.clone();
        let p = pass.clone();
        let pt = port;
        let result = tokio::task::spawn_blocking(move || poll_imap_once(&h, pt, &u, &p)).await;

        match result {
            Ok(Ok(actions)) => {
                for action in &actions {
                    match action {
                        ReplyAction::PermissionDecision {
                            request_id,
                            approved,
                            message,
                        } => {
                            if let Err(e) = crate::safety::record_permission_via_file(
                                request_id,
                                *approved,
                                "email_reply",
                                message.clone(),
                            ) {
                                logging::error(&format!(
                                    "Failed to record permission decision for {}: {}",
                                    request_id, e
                                ));
                            } else {
                                logging::info(&format!(
                                    "Permission {} via email: {}",
                                    if *approved { "approved" } else { "denied" },
                                    request_id
                                ));
                            }
                        }
                        ReplyAction::DirectiveReply { cycle_id, text } => {
                            if let Err(e) =
                                crate::ambient::add_directive(text.clone(), cycle_id.clone())
                            {
                                logging::error(&format!("Failed to save directive: {}", e));
                            }
                        }
                    }
                }

                if !actions.is_empty() {
                    logging::info(&format!("IMAP: processed {} email replies", actions.len()));
                }
            }
            Ok(Err(e)) => {
                logging::error(&format!("IMAP poll error: {}", e));
            }
            Err(e) => {
                logging::error(&format!("IMAP poll task panicked: {}", e));
            }
        }

        // Poll every 60 seconds
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Sanitized body for potentially public channels (ntfy.sh).
/// Only includes counts and status — no model-generated text.
fn format_cycle_body_safe(transcript: &AmbientTranscript) -> String {
    let mut lines = Vec::new();

    lines.push(format!("Status: {:?}", transcript.status));
    lines.push(format!(
        "Memories modified: {}",
        transcript.memories_modified
    ));
    lines.push(format!("Compactions: {}", transcript.compactions));

    if transcript.pending_permissions > 0 {
        lines.push(format!(
            "{} permission request(s) pending",
            transcript.pending_permissions
        ));
    }

    lines.push("Check jcode for full details.".to_string());
    lines.join("\n")
}

/// Full detailed body for private channels (email, desktop).
/// Includes the model-generated summary and provider info.
/// Output is markdown — rendered to HTML for email, plain text for desktop.
fn format_cycle_body_detailed(transcript: &AmbientTranscript) -> String {
    let mut lines = Vec::new();

    if let Some(ref summary) = transcript.summary {
        lines.push("# Summary".to_string());
        lines.push(String::new());
        lines.push(summary.clone());
        lines.push(String::new());
    }

    lines.push("---".to_string());
    lines.push(String::new());
    lines.push(format!(
        "**Status:** {:?} · **Provider:** {} ({}) · **Memories:** {} · **Compactions:** {}",
        transcript.status,
        transcript.provider,
        transcript.model,
        transcript.memories_modified,
        transcript.compactions,
    ));

    if transcript.pending_permissions > 0 {
        lines.push(String::new());
        lines.push(format!(
            "**⚠ {} permission request(s) pending** — review in jcode",
            transcript.pending_permissions
        ));
    }

    // Include full conversation transcript if available
    if let Some(ref conversation) = transcript.conversation {
        lines.push(String::new());
        lines.push("---".to_string());
        lines.push(String::new());
        lines.push("# Full Transcript".to_string());
        lines.push(String::new());
        lines.push(conversation.clone());
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_cycle_body_safe() {
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
            conversation: None,
        };

        let body = format_cycle_body_safe(&transcript);
        assert!(body.contains("Memories modified: 3"));
        assert!(body.contains("Compactions: 1"));
        assert!(body.contains("Check jcode for full details"));
        // Safe body must NOT include model-generated summary
        assert!(!body.contains("Cleaned up"));
        assert!(!body.contains("permission"));
    }

    #[test]
    fn test_format_cycle_body_detailed() {
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
            conversation: Some("### User\n\nBegin cycle.\n\n### Assistant\n\nDone.\n".to_string()),
        };

        let body = format_cycle_body_detailed(&transcript);
        // Detailed body SHOULD include the summary
        assert!(body.contains("Cleaned up 3 stale memories."));
        assert!(body.contains("**Memories:** 3"));
        assert!(body.contains("claude"));
        // Should include conversation transcript
        assert!(body.contains("# Full Transcript"));
        assert!(body.contains("### User"));
        assert!(body.contains("Begin cycle."));
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
            conversation: None,
        };

        let safe = format_cycle_body_safe(&transcript);
        assert!(safe.contains("2 permission request(s) pending"));
        assert!(safe.contains("Check jcode for full details"));

        let detailed = format_cycle_body_detailed(&transcript);
        assert!(detailed.contains("2 permission request(s) pending"));
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
