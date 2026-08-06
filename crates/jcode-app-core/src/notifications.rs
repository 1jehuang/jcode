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
// Desktop (cross-platform, fire-and-forget)
// ---------------------------------------------------------------------------

/// Send a local desktop notification without blocking.
///
/// Uses Notification Center via `osascript` on macOS and `notify-send` on
/// Linux. The child process is spawned detached and never waited on; failures
/// are ignored (a missing notifier is not an error).
pub fn send_desktop_notification(title: &str, body: &str) {
    send_desktop_notification_rich(title, None, body, None);
}

/// Escape a string for embedding in an AppleScript double-quoted literal.
#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Build the AppleScript that posts a Notification Center banner.
///
/// When `bundle_id` is set the `display notification` is wrapped in a
/// `tell application id` block so macOS attributes the banner to that app.
/// Clicking the banner then activates the terminal hosting the jcode session
/// instead of Script Editor, which owns bare `osascript` notifications.
#[cfg(target_os = "macos")]
fn build_macos_notification_script(
    title: &str,
    subtitle: Option<&str>,
    body: &str,
    sound: Option<&str>,
    bundle_id: Option<&str>,
) -> String {
    let mut script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        applescript_escape(title)
    );
    if let Some(subtitle) = subtitle.filter(|s| !s.trim().is_empty()) {
        script.push_str(&format!(" subtitle \"{}\"", applescript_escape(subtitle)));
    }
    if let Some(sound) = sound.filter(|s| !s.trim().is_empty()) {
        script.push_str(&format!(" sound name \"{}\"", applescript_escape(sound)));
    }
    match bundle_id.filter(|id| !id.trim().is_empty()) {
        Some(bundle_id) => format!(
            "tell application id \"{}\" to {}",
            applescript_escape(bundle_id),
            script
        ),
        None => script,
    }
}

/// Map a terminal key from the terminal detector to its macOS bundle id.
#[cfg(target_os = "macos")]
fn macos_terminal_bundle_id(terminal: &str) -> Option<&'static str> {
    Some(match terminal {
        "handterm" => "com.jcode.handterm",
        "ghostty" => "com.mitchellh.ghostty",
        "kitty" => "net.kovidgoyal.kitty",
        "wezterm" => "com.github.wez.wezterm",
        "alacritty" => "org.alacritty",
        "iterm2" => "com.googlecode.iterm2",
        "terminal" => "com.apple.Terminal",
        _ => return None,
    })
}

/// Bundle identifier of the terminal app hosting this jcode process, if it can
/// be identified. Notifications posted via `tell application id "..."` are
/// attributed to that app, so clicking the banner activates the terminal
/// window with the session instead of Script Editor.
#[cfg(target_os = "macos")]
fn macos_host_terminal_bundle_id() -> Option<String> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let launching = std::env::var_os("__CFBundleIdentifier");
            resolve_macos_host_terminal_bundle_id(
                launching.as_deref().and_then(std::ffi::OsStr::to_str),
                jcode_base::terminal_launch::detected_resume_terminal().as_deref(),
            )
        })
        .clone()
}

/// Resolve the host terminal bundle id from the launching app's bundle id
/// (`__CFBundleIdentifier`, exported by every GUI-launched process) with the
/// terminal detector as the fallback. Split out from env access so it is
/// directly unit-testable.
#[cfg(target_os = "macos")]
fn resolve_macos_host_terminal_bundle_id(
    launching_bundle_id: Option<&str>,
    detected_terminal: Option<&str>,
) -> Option<String> {
    // `osascript` run from a plain shell reports Script Editor's helper id,
    // which is exactly the attribution we are trying to avoid.
    if let Some(explicit) = launching_bundle_id
        .map(str::trim)
        .filter(|v| !v.is_empty() && *v != "com.apple.Terminal.osascript")
    {
        return Some(explicit.to_string());
    }
    detected_terminal
        .and_then(macos_terminal_bundle_id)
        .map(str::to_string)
}

/// Send a local desktop notification with optional macOS subtitle and sound.
///
/// `subtitle` renders as a second bold line on macOS (ignored elsewhere).
/// `sound` is a Notification Center sound name such as "Glass" or "Ping"
/// (macOS only). Both are best-effort; a missing notifier is not an error.
pub fn send_desktop_notification_rich(
    title: &str,
    subtitle: Option<&str>,
    body: &str,
    sound: Option<&str>,
) {
    #[cfg(target_os = "macos")]
    {
        let script = build_macos_notification_script(
            title,
            subtitle,
            body,
            sound,
            macos_host_terminal_bundle_id().as_deref(),
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (subtitle, sound);
        let _ = std::process::Command::new("notify-send")
            .arg("--app-name=jcode")
            .arg(title)
            .arg(body)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, subtitle, body, sound);
    }
}

// ---------------------------------------------------------------------------
// Desktop (notify-send)
// ---------------------------------------------------------------------------

fn send_desktop(title: &str, body: &str, urgency: &str) {
    // On macOS notify-send does not exist; route through Notification Center.
    #[cfg(target_os = "macos")]
    {
        let _ = urgency;
        send_desktop_notification(title, body);
    }
    #[cfg(not(target_os = "macos"))]
    {
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

    /// A bare `display notification` is attributed to Script Editor, so
    /// clicking the banner opens Script Editor instead of the user's session.
    /// Every macOS notification must be wrapped in `tell application id`.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_script_is_attributed_to_host_terminal() {
        let script = build_macos_notification_script(
            "jcode: done",
            Some("my-session"),
            "turn complete",
            Some("Glass"),
            Some("com.mitchellh.ghostty"),
        );
        assert_eq!(
            script,
            "tell application id \"com.mitchellh.ghostty\" to \
             display notification \"turn complete\" with title \"jcode: done\" \
             subtitle \"my-session\" sound name \"Glass\""
        );
    }

    /// Without a resolvable terminal we must still post the notification,
    /// just unattributed, rather than emitting a broken `tell` block.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_script_without_bundle_id_is_unwrapped() {
        let script = build_macos_notification_script("t", None, "b", None, None);
        assert_eq!(script, "display notification \"b\" with title \"t\"");
        let blank = build_macos_notification_script("t", None, "b", None, Some("  "));
        assert_eq!(blank, "display notification \"b\" with title \"t\"");
    }

    /// Quotes/backslashes in session names or assistant text must not be able
    /// to break out of the AppleScript string literal.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_script_escapes_injection_attempts() {
        let script = build_macos_notification_script(
            "ti\"tle",
            None,
            "bo\\dy\" & (do shell script \"boom\")",
            None,
            Some("com.apple.Terminal"),
        );
        assert!(script.starts_with("tell application id \"com.apple.Terminal\" to "));
        assert!(script.contains("\\\"tle"));
        assert!(script.contains("bo\\\\dy\\\""));
        // The real invariant: only the six literal delimiters (bundle id,
        // body, title) survive as unescaped quotes, so hostile text cannot
        // terminate a string and append its own AppleScript.
        let delimiters = script
            .replace("\\\\", "")
            .replace("\\\"", "")
            .matches('"')
            .count();
        assert_eq!(delimiters, 6, "unbalanced quotes in script: {script}");
    }

    /// End-to-end guard: the generated script must actually compile as
    /// AppleScript, proving the `tell` wrapper and escaping are well formed.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_script_compiles_as_applescript() {
        let script = build_macos_notification_script(
            "ti\"tle",
            Some("sub\\title"),
            "bo\"dy & (do shell script \"boom\")",
            Some("Glass"),
            Some("com.apple.Terminal"),
        );
        // `osacompile` parses without executing, so no notification is posted.
        let output = std::process::Command::new("osacompile")
            .args(["-o", "/dev/null", "-e", &script])
            .output();
        let Ok(output) = output else {
            return; // osacompile unavailable; nothing to assert.
        };
        assert!(
            output.status.success(),
            "script failed to compile: {script}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The launching app's bundle id wins, since it names the exact terminal
    /// that started this process.
    #[cfg(target_os = "macos")]
    #[test]
    fn host_terminal_bundle_id_prefers_launching_app() {
        assert_eq!(
            resolve_macos_host_terminal_bundle_id(Some("com.mitchellh.ghostty"), Some("terminal")),
            Some("com.mitchellh.ghostty".to_string())
        );
    }

    /// `osascript` launched from a shell reports Script Editor's helper id.
    /// Trusting it would reintroduce the Script Editor bug, so it is ignored
    /// in favor of terminal detection.
    #[cfg(target_os = "macos")]
    #[test]
    fn host_terminal_bundle_id_ignores_script_editor_helper() {
        assert_eq!(
            resolve_macos_host_terminal_bundle_id(
                Some("com.apple.Terminal.osascript"),
                Some("ghostty")
            ),
            Some("com.mitchellh.ghostty".to_string())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn host_terminal_bundle_id_falls_back_to_detected_terminal() {
        for (terminal, expected) in [
            ("ghostty", "com.mitchellh.ghostty"),
            ("kitty", "net.kovidgoyal.kitty"),
            ("wezterm", "com.github.wez.wezterm"),
            ("alacritty", "org.alacritty"),
            ("iterm2", "com.googlecode.iterm2"),
            ("terminal", "com.apple.Terminal"),
            ("handterm", "com.jcode.handterm"),
        ] {
            assert_eq!(
                resolve_macos_host_terminal_bundle_id(None, Some(terminal)),
                Some(expected.to_string()),
                "terminal {terminal} should map to {expected}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn host_terminal_bundle_id_is_none_when_unknown() {
        assert_eq!(
            resolve_macos_host_terminal_bundle_id(Some(""), Some("some-unknown-term")),
            None
        );
        assert_eq!(resolve_macos_host_terminal_bundle_id(None, None), None);
    }

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
