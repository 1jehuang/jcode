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
        let safe_body =
            "An ambient action needs your approval. Open jcode to review.".to_string();
        let detailed_body = format!(
            "Action: {}\n{}\n\nRequest ID: {}\nReview in jcode to approve or deny.",
            action, description, request_id
        );

        self.send_all(&title, &safe_body, &detailed_body, Priority::High, None);
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
            let priority = priority;
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
                let body = detailed_body.to_string();
                let cycle_id = cycle_id.map(|s| s.to_string());
                tokio::spawn(async move {
                    if let Err(e) = send_email(
                        &host,
                        port,
                        &from,
                        &to,
                        password.as_deref(),
                        &title,
                        &body,
                        cycle_id.as_deref(),
                    )
                    .await
                    {
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
    cycle_id: Option<&str>,
) -> anyhow::Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let html_body = markdown_to_html_email(body);

    let mut builder = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject(subject)
        .header(ContentType::TEXT_HTML);

    // Add Message-ID for reply tracking (format: <ambient-{id}@jcode>)
    if let Some(cid) = cycle_id {
        let msg_id = format!("<ambient-{}@jcode>", cid);
        builder = builder.message_id(Some(msg_id));
    }

    let email = builder.body(html_body)?;

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
// Markdown → HTML email
// ---------------------------------------------------------------------------

/// Convert markdown text to a styled HTML email body.
fn markdown_to_html_email(markdown: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(markdown, options);
    let mut html_content = String::new();
    html::push_html(&mut html_content, parser);

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  body {{
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    color: #1a1a1a;
    line-height: 1.6;
    max-width: 640px;
    margin: 0 auto;
    padding: 20px;
    background: #f5f5f5;
  }}
  .container {{
    background: #ffffff;
    border-radius: 8px;
    padding: 24px 28px;
    border: 1px solid #e0e0e0;
  }}
  h1, h2, h3 {{
    color: #2d2d2d;
    margin-top: 1.2em;
    margin-bottom: 0.4em;
  }}
  h1 {{ font-size: 1.3em; border-bottom: 2px solid #6366f1; padding-bottom: 6px; }}
  h2 {{ font-size: 1.1em; }}
  strong {{ color: #111; }}
  ul, ol {{ padding-left: 1.4em; }}
  li {{ margin-bottom: 4px; }}
  code {{
    background: #f0f0f0;
    padding: 2px 5px;
    border-radius: 3px;
    font-size: 0.9em;
  }}
  pre {{
    background: #1e1e2e;
    color: #cdd6f4;
    padding: 12px 16px;
    border-radius: 6px;
    overflow-x: auto;
    font-size: 0.85em;
  }}
  pre code {{
    background: none;
    padding: 0;
    color: inherit;
  }}
  table {{
    border-collapse: collapse;
    width: 100%;
    margin: 1em 0;
  }}
  th, td {{
    border: 1px solid #ddd;
    padding: 6px 10px;
    text-align: left;
  }}
  th {{ background: #f8f8f8; font-weight: 600; }}
  .footer {{
    margin-top: 20px;
    padding-top: 12px;
    border-top: 1px solid #e0e0e0;
    font-size: 0.8em;
    color: #888;
  }}
</style>
</head>
<body>
<div class="container">
{html_content}
</div>
<div class="footer">
  Sent by jcode ambient mode
</div>
</body>
</html>"#
    )
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
            Ok(Ok(count)) => {
                if count > 0 {
                    logging::info(&format!("IMAP: processed {} email replies", count));
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

fn poll_imap_once(host: &str, port: u16, user: &str, pass: &str) -> anyhow::Result<usize> {
    let tls = native_tls::TlsConnector::builder().build()?;
    let client = imap::connect((host, port), host, &tls)?;
    let mut session = client
        .login(user, pass)
        .map_err(|(e, _)| anyhow::anyhow!("IMAP login failed: {}", e))?;

    session.select("INBOX")?;

    // Search for unseen replies to our ambient emails
    let search = session.search("UNSEEN HEADER In-Reply-To \"@jcode>\"")?;

    let mut processed = 0;
    if search.is_empty() {
        session.logout()?;
        return Ok(0);
    }

    // Build sequence set from search results
    let seq_set: String = search
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let messages = session.fetch(&seq_set, "RFC822")?;
    for message in messages.iter() {
        if let Some(body) = message.body() {
            if let Some(parsed) = mail_parser::MessageParser::default().parse(body) {
                // Extract the cycle ID from In-Reply-To header
                let in_reply_to = parsed
                    .in_reply_to()
                    .as_text()
                    .unwrap_or("")
                    .to_string();

                // Parse cycle_id from "<ambient-{id}@jcode>"
                let cycle_id = in_reply_to
                    .trim_start_matches("<ambient-")
                    .trim_end_matches("@jcode>")
                    .to_string();

                // Get reply body text (strip quoted content)
                let body_text = parsed
                    .body_text(0)
                    .map(|s| strip_quoted_reply(&s))
                    .unwrap_or_default();

                if !body_text.trim().is_empty() {
                    if let Err(e) =
                        crate::ambient::add_directive(body_text.trim().to_string(), cycle_id)
                    {
                        logging::error(&format!("Failed to save directive: {}", e));
                    }
                    processed += 1;
                }
            }
        }
    }

    // Mark all processed as seen
    if let Err(e) = session.store(&seq_set, "+FLAGS (\\Seen)") {
        logging::warn(&format!("IMAP: failed to mark messages as seen: {}", e));
    }

    session.logout()?;
    Ok(processed)
}

/// Strip quoted reply lines (lines starting with ">") and email signatures.
fn strip_quoted_reply(text: &str) -> String {
    text.lines()
        .take_while(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('>')
                && trimmed != "--"
                && trimmed != "-- "
                && !trimmed.starts_with("On ") // "On Mon, Jan 1, 2025 ... wrote:"
                    || trimmed.is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Sanitized body for potentially public channels (ntfy.sh).
/// Only includes counts and status — no model-generated text.
fn format_cycle_body_safe(transcript: &AmbientTranscript) -> String {
    let mut lines = Vec::new();

    lines.push(format!("Status: {:?}", transcript.status));
    lines.push(format!("Memories modified: {}", transcript.memories_modified));
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
        };

        let body = format_cycle_body_detailed(&transcript);
        // Detailed body SHOULD include the summary
        assert!(body.contains("Cleaned up 3 stale memories."));
        assert!(body.contains("**Memories:** 3"));
        assert!(body.contains("claude"));
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

        let safe = format_cycle_body_safe(&transcript);
        assert!(safe.contains("2 permission request(s) pending"));
        assert!(safe.contains("Check jcode for full details"));

        let detailed = format_cycle_body_detailed(&transcript);
        assert!(detailed.contains("2 permission request(s) pending"));
    }

    #[test]
    fn test_markdown_to_html_email() {
        let md = "**Ambient Cycle Summary:**\n\n- Cleaned 3 memories\n- Status: Complete\n";
        let html = markdown_to_html_email(md);
        assert!(html.contains("<strong>Ambient Cycle Summary:</strong>"));
        assert!(html.contains("<li>"));
        assert!(html.contains("jcode ambient mode"));
    }

    #[test]
    fn test_strip_quoted_reply() {
        let email = "Thanks, please clean up the test data.\n\n> On Mon, Feb 9, 2026 jcode wrote:\n> Ambient cycle complete.\n";
        let stripped = strip_quoted_reply(email);
        assert!(stripped.contains("clean up the test data"));
        assert!(!stripped.contains("Ambient cycle complete"));
    }

    #[test]
    fn test_strip_quoted_reply_signature() {
        let email = "Focus on memory gardening.\n--\nJeremy\n";
        let stripped = strip_quoted_reply(email);
        assert!(stripped.contains("Focus on memory gardening"));
        assert!(!stripped.contains("Jeremy"));
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
