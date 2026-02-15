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
        // If email_html_override is provided, send it directly as HTML.
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
                let html_override = email_html_override.map(|s| s.to_string());
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
                        html_override.as_deref(),
                    )
                    .await
                    {
                        logging::error(&format!("Email notification failed: {}", e));
                    }
                });
            }
        }

        // Telegram — uses DETAILED body (sent to your bot chat, private)
        if self.config.telegram_enabled {
            if let (Some(ref token), Some(ref chat_id)) = (
                &self.config.telegram_bot_token,
                &self.config.telegram_chat_id,
            ) {
                let client = self.client.clone();
                let token = token.clone();
                let chat_id = chat_id.clone();
                let title = title.to_string();
                let body = detailed_body.to_string();
                tokio::spawn(async move {
                    let text = format!("*{}*\n\n{}", title, body);
                    if let Err(e) =
                        crate::telegram::send_message(&client, &token, &chat_id, &text).await
                    {
                        logging::error(&format!("Telegram notification failed: {}", e));
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
    html_override: Option<&str>,
) -> anyhow::Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let html_body = match html_override {
        Some(html) => html.to_string(),
        None => markdown_to_html_email(body),
    };

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
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(smtp_host)?.port(smtp_port);

    if let Some(pw) = password {
        transport_builder =
            transport_builder.credentials(Credentials::new(from.to_string(), pw.to_string()));
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

    // Search for unseen replies to ambient emails AND button-generated permission emails.
    // Two patterns:
    //   1. Replies: In-Reply-To header contains "@jcode>"
    //   2. Button emails: Subject contains "[jcode-perm:" (from mailto: buttons)
    let reply_search = session.search("UNSEEN HEADER In-Reply-To \"@jcode>\"")?;
    let button_search = session.search("UNSEEN SUBJECT \"[jcode-perm:\"")?;

    // Merge and deduplicate sequence numbers
    let mut all_seqs: Vec<u32> = reply_search.into_iter().chain(button_search).collect();
    all_seqs.sort_unstable();
    all_seqs.dedup();

    let mut processed = 0;
    if all_seqs.is_empty() {
        session.logout()?;
        return Ok(0);
    }

    // Build sequence set from search results
    let seq_set: String = all_seqs
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let messages = session.fetch(&seq_set, "RFC822")?;
    for message in messages.iter() {
        if let Some(body) = message.body() {
            if let Some(parsed) = mail_parser::MessageParser::default().parse(body) {
                // Try to extract request/cycle ID from two sources:
                //   1. In-Reply-To header: "<ambient-{id}@jcode>"
                //   2. Subject: "[jcode-perm:{id}] ..."
                let in_reply_to = parsed.in_reply_to().as_text().unwrap_or("").to_string();
                let subject = parsed.subject().unwrap_or("");

                let cycle_id = if in_reply_to.contains("@jcode>") {
                    // Reply path: extract from In-Reply-To
                    in_reply_to
                        .trim_start_matches("<ambient-")
                        .trim_end_matches("@jcode>")
                        .to_string()
                } else if let Some(start) = subject.find("[jcode-perm:") {
                    // Button path: extract request ID from subject
                    let rest = &subject[start + "[jcode-perm:".len()..];
                    rest.split(']').next().unwrap_or("").to_string()
                } else {
                    continue; // Neither pattern matched — skip
                };

                // Get reply body text (strip quoted content)
                let body_text = parsed
                    .body_text(0)
                    .map(|s| strip_quoted_reply(&s))
                    .unwrap_or_default();

                // For button-generated emails, also check subject for approval intent
                // (the body may just be "Approved" or "Denied" from the mailto: pre-fill)
                let effective_text = if body_text.trim().is_empty() {
                    // Fall back to subject for intent
                    subject.to_string()
                } else {
                    body_text
                };

                if !effective_text.trim().is_empty() {
                    if cycle_id.starts_with("req_") {
                        // Reply to a permission request — parse approve/deny
                        let (approved, message) = parse_permission_reply(effective_text.trim());
                        if let Err(e) = crate::safety::record_permission_via_file(
                            &cycle_id,
                            approved,
                            "email_reply",
                            message,
                        ) {
                            logging::error(&format!(
                                "Failed to record permission decision for {}: {}",
                                cycle_id, e
                            ));
                        } else {
                            logging::info(&format!(
                                "Permission {} via email: {}",
                                if approved { "approved" } else { "denied" },
                                cycle_id
                            ));
                        }
                    } else {
                        // Normal directive reply to a cycle notification
                        if let Err(e) = crate::ambient::add_directive(
                            effective_text.trim().to_string(),
                            cycle_id,
                        ) {
                            logging::error(&format!("Failed to save directive: {}", e));
                        }
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

// ---------------------------------------------------------------------------
// Telegram reply polling
// ---------------------------------------------------------------------------

/// Run a Telegram Bot API polling loop checking for replies.
/// Should be spawned as a tokio task alongside the ambient runner.
/// Messages are injected directly into the active ambient agent session
/// via soft interrupts. If no cycle is running, a new cycle is triggered.
pub async fn telegram_reply_loop(
    config: SafetyConfig,
    runner: crate::ambient_runner::AmbientRunnerHandle,
) {
    let token = match config.telegram_bot_token.as_ref() {
        Some(t) => t.clone(),
        None => {
            logging::error("Telegram reply loop: no bot_token configured");
            return;
        }
    };
    let expected_chat_id = match config.telegram_chat_id.as_ref() {
        Some(c) => c.clone(),
        None => {
            logging::error("Telegram reply loop: no chat_id configured");
            return;
        }
    };

    logging::info("Telegram reply loop: starting");

    let client = reqwest::Client::new();
    let mut offset: Option<i64> = None;

    loop {
        match crate::telegram::get_updates(&client, &token, offset, 30).await {
            Ok(updates) => {
                for update in updates {
                    offset = Some(update.update_id + 1);

                    let msg = match update.message {
                        Some(m) => m,
                        None => continue,
                    };

                    if msg.chat.id.to_string() != expected_chat_id {
                        continue;
                    }

                    let text = match msg.text {
                        Some(t) => t,
                        None => continue,
                    };

                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Check if this is a permission reply (e.g. "approve req_abc123")
                    if let Some(req_id) = extract_permission_id(trimmed) {
                        let (approved, message) = parse_permission_reply(trimmed);
                        if let Err(e) = crate::safety::record_permission_via_file(
                            &req_id,
                            approved,
                            "telegram_reply",
                            message,
                        ) {
                            logging::error(&format!(
                                "Failed to record permission from Telegram for {}: {}",
                                req_id, e
                            ));
                        } else {
                            logging::info(&format!(
                                "Permission {} via Telegram: {}",
                                if approved { "approved" } else { "denied" },
                                req_id
                            ));
                            let _ = crate::telegram::send_message(
                                &client,
                                &token,
                                &expected_chat_id,
                                &format!(
                                    "✅ Permission {} for `{}`",
                                    if approved { "approved" } else { "denied" },
                                    req_id
                                ),
                            )
                            .await;
                        }
                    } else {
                        let injected = runner.inject_telegram_message(trimmed).await;
                        let ack = if injected {
                            format!("💬 Message sent to active session: _{}_", trimmed)
                        } else {
                            format!("📋 Message queued, waking agent: _{}_", trimmed)
                        };
                        let _ = crate::telegram::send_message(
                            &client,
                            &token,
                            &expected_chat_id,
                            &ack,
                        )
                        .await;
                    }
                }
            }
            Err(e) => {
                logging::error(&format!("Telegram poll error: {}", e));
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        }
    }
}

/// Extract a permission request ID from a Telegram message.
/// Matches patterns like "approve req_abc123" or "deny req_abc123".
fn extract_permission_id(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for word in lower.split_whitespace() {
        if word.starts_with("req_") {
            return Some(word.to_string());
        }
    }
    None
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

/// Parse an email reply body for permission approve/deny intent.
/// Returns `(approved, optional_message)`.
///
/// Checks the first line for approve keywords vs deny keywords.
/// Defaults to deny if ambiguous (fail-safe).
fn parse_permission_reply(text: &str) -> (bool, Option<String>) {
    let lower = text.to_lowercase();
    let first_line = lower.lines().next().unwrap_or("").trim();

    let approve_words = [
        "approve", "approved", "yes", "lgtm", "go ahead", "ok", "sure",
    ];
    let deny_words = ["deny", "denied", "no", "reject", "rejected", "stop", "nope"];

    let has_approve = approve_words.iter().any(|w| first_line.contains(w));
    let has_deny = deny_words.iter().any(|w| first_line.contains(w));

    // If both or neither match, default to deny (fail-safe)
    let approved = has_approve && !has_deny;

    let message = if text.trim().len() > 20 {
        Some(text.trim().to_string())
    } else {
        None
    };

    (approved, message)
}

// ---------------------------------------------------------------------------
// Permission email HTML builder
// ---------------------------------------------------------------------------

/// Build a styled HTML email for a permission request, with Approve/Deny mailto: buttons.
///
/// The buttons create a new email to `reply_to` with a subject containing the request ID
/// (pattern: `[jcode-perm:req_xxx] Approved/Denied`) so the IMAP poller can match them.
fn build_permission_email_html(
    action: &str,
    description: &str,
    request_id: &str,
    reply_to: &str,
) -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y-%m-%d %H:%M:%S UTC").to_string();

    // URL-encode components for mailto: links
    let approve_subj_raw = format!("[jcode-perm:{}] Approved", request_id);
    let deny_subj_raw = format!("[jcode-perm:{}] Denied", request_id);
    let approve_subject = urlencoding::encode(&approve_subj_raw);
    let deny_subject = urlencoding::encode(&deny_subj_raw);
    let approve_body = urlencoding::encode("Approved");
    let deny_body = urlencoding::encode("Denied");

    let approve_href = format!(
        "mailto:{}?subject={}&body={}",
        reply_to, approve_subject, approve_body
    );
    let deny_href = format!(
        "mailto:{}?subject={}&body={}",
        reply_to, deny_subject, deny_body
    );

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
  h1 {{
    font-size: 1.3em;
    color: #2d2d2d;
    border-bottom: 2px solid #f59e0b;
    padding-bottom: 6px;
    margin-top: 0;
  }}
  .field {{
    margin-bottom: 12px;
  }}
  .field-label {{
    font-weight: 600;
    color: #555;
    font-size: 0.85em;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }}
  .field-value {{
    margin-top: 2px;
    color: #1a1a1a;
  }}
  .request-id {{
    font-family: monospace;
    background: #f0f0f0;
    padding: 2px 6px;
    border-radius: 3px;
    font-size: 0.85em;
  }}
  .buttons {{
    margin-top: 24px;
    text-align: center;
  }}
  .btn {{
    display: inline-block;
    padding: 12px 32px;
    border-radius: 6px;
    text-decoration: none;
    font-weight: 600;
    font-size: 1em;
    margin: 0 8px;
  }}
  .btn-approve {{
    background: #22c55e;
    color: #ffffff;
  }}
  .btn-deny {{
    background: #ef4444;
    color: #ffffff;
  }}
  .timestamp {{
    margin-top: 16px;
    font-size: 0.8em;
    color: #888;
  }}
  .hint {{
    margin-top: 8px;
    font-size: 0.8em;
    color: #999;
    font-style: italic;
  }}
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
  <h1>Permission Request</h1>
  <div class="field">
    <div class="field-label">Action</div>
    <div class="field-value"><strong>{action}</strong></div>
  </div>
  <div class="field">
    <div class="field-label">Description</div>
    <div class="field-value">{description}</div>
  </div>
  <div class="field">
    <div class="field-label">Request ID</div>
    <div class="field-value"><span class="request-id">{request_id}</span></div>
  </div>
  <div class="buttons">
    <a href="{approve_href}" class="btn btn-approve">Approve</a>
    <a href="{deny_href}" class="btn btn-deny">Deny</a>
  </div>
  <div class="hint">Clicking opens a pre-filled email — just hit Send.</div>
  <div class="hint">Or reply to this email with "Approved" or "Denied".</div>
  <div class="timestamp">Sent at {timestamp}</div>
</div>
<div class="footer">
  Sent by jcode ambient mode
</div>
</body>
</html>"#
    )
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

    #[test]
    fn test_parse_permission_reply_approve() {
        let (approved, _) = parse_permission_reply("Yes, go ahead");
        assert!(approved);

        let (approved, _) = parse_permission_reply("Approved");
        assert!(approved);

        let (approved, _) = parse_permission_reply("LGTM");
        assert!(approved);

        let (approved, _) = parse_permission_reply("sure thing");
        assert!(approved);

        let (approved, _) = parse_permission_reply("ok");
        assert!(approved);
    }

    #[test]
    fn test_parse_permission_reply_deny() {
        let (approved, _) = parse_permission_reply("No, too risky");
        assert!(!approved);

        let (approved, _) = parse_permission_reply("Denied");
        assert!(!approved);

        let (approved, _) = parse_permission_reply("reject this");
        assert!(!approved);

        let (approved, _) = parse_permission_reply("nope");
        assert!(!approved);

        let (approved, _) = parse_permission_reply("Stop, don't do that");
        assert!(!approved);
    }

    #[test]
    fn test_parse_permission_reply_ambiguous_defaults_deny() {
        let (approved, _) = parse_permission_reply("hmm let me think about it");
        assert!(!approved);

        let (approved, _) = parse_permission_reply("");
        assert!(!approved);
    }

    #[test]
    fn test_parse_permission_reply_message() {
        // Short replies: no message
        let (_, message) = parse_permission_reply("yes");
        assert!(message.is_none());

        // Longer replies: message included
        let (_, message) =
            parse_permission_reply("Approved, but please use a feature branch for this");
        assert!(message.is_some());
    }
}
