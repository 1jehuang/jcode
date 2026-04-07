use crate::todo::TodoItem;
use crate::tui::info_widget::{AmbientWidgetData, GitInfo, MemoryInfo};
use crate::tui::session_picker::ResumeTarget;
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

static AMBIENT_INFO_CACHE: Mutex<Option<(std::time::Instant, bool, Option<AmbientWidgetData>)>> =
    Mutex::new(None);

#[derive(Clone)]
pub(super) struct CachedContextInfo {
    pub session_key: String,
    pub is_remote: bool,
    pub display_messages_version: u64,
    pub message_count: usize,
    pub context_info: crate::prompt::ContextInfo,
}

pub(super) fn extract_bracketed_system_message(message: &str) -> Option<String> {
    let trimmed = message.trim();
    let body = trimmed.strip_prefix("[SYSTEM:")?.trim_start();
    let body = body.strip_suffix(']').unwrap_or(body).trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

pub(super) fn launch_client_executable() -> PathBuf {
    crate::build::client_update_candidate(crate::cli::selfdev::client_selfdev_requested())
        .map(|(path, _label)| path)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("jcode"))
}

pub(super) fn partition_queued_messages(
    messages: Vec<String>,
    reminders: Vec<String>,
) -> (Vec<String>, Option<String>, Vec<String>) {
    let mut user_messages = Vec::new();
    let mut display_system_messages = Vec::new();
    let mut reminder_parts = reminders;

    for message in messages {
        if let Some(system_message) = extract_bracketed_system_message(&message) {
            reminder_parts.push(system_message.clone());
            display_system_messages.push(system_message);
        } else {
            user_messages.push(message);
        }
    }

    let reminder = if reminder_parts.is_empty() {
        None
    } else {
        Some(reminder_parts.join("\n\n"))
    };

    (user_messages, reminder, display_system_messages)
}

#[cfg(target_os = "macos")]
pub(super) fn ctrl_bracket_fallback_to_esc(code: &mut KeyCode, modifiers: &mut KeyModifiers) {
    if !modifiers.contains(KeyModifiers::CONTROL) {
        return;
    }
    match code {
        KeyCode::Esc => {
            *code = KeyCode::Char('[');
        }
        KeyCode::Char('5') => {
            // Legacy tty mapping for Ctrl+]
            *code = KeyCode::Char(']');
        }
        _ => {}
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn ctrl_bracket_fallback_to_esc(_code: &mut KeyCode, _modifiers: &mut KeyModifiers) {}

/// Debug command file path
pub(super) fn debug_cmd_path() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_DEBUG_CMD_PATH") {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("jcode_debug_cmd")
}

/// Debug response file path
pub(super) fn debug_response_path() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_DEBUG_RESPONSE_PATH") {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("jcode_debug_response")
}

/// Parse rate limit reset time from error message
/// Returns the Duration until rate limit resets, if this is a rate limit error
pub(super) fn parse_rate_limit_error(error: &str) -> Option<Duration> {
    let error_lower = error.to_lowercase();

    if !error_lower.contains("rate limit")
        && !error_lower.contains("rate_limit")
        && !error_lower.contains("429")
        && !error_lower.contains("too many requests")
        && !error_lower.contains("hit your limit")
    {
        return None;
    }

    if let Some(idx) = error_lower.find("retry") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            if let Ok(secs) = word
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
            {
                if secs > 0 && secs < 86400 {
                    return Some(Duration::from_secs(secs));
                }
            }
        }
    }

    if let Some(idx) = error_lower.find("resets") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            let word = word.trim_matches(|c: char| c == '·' || c == ' ');
            if word.ends_with("am") || word.ends_with("pm") {
                if let Some(duration) = parse_clock_time_to_duration(word) {
                    return Some(duration);
                }
            }
        }
    }

    if let Some(idx) = error_lower.find("reset") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            if let Ok(secs) = word
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
            {
                if secs > 0 && secs < 86400 {
                    return Some(Duration::from_secs(secs));
                }
            }
        }
    }

    None
}

pub(super) fn is_context_limit_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("max context")
        || lower.contains("token limit")
        || lower.contains("too many tokens")
        || lower.contains("prompt is too long")
        || lower.contains("input is too long")
        || lower.contains("request too large")
        || lower.contains("length limit")
        || lower.contains("maximum tokens")
        || (lower.contains("exceeded") && lower.contains("tokens"))
}

/// Parse a clock time like "5am" or "12:30pm" and return duration until that time
pub(super) fn parse_clock_time_to_duration(time_str: &str) -> Option<Duration> {
    let time_lower = time_str.to_lowercase();
    let is_pm = time_lower.ends_with("pm");
    let time_part = time_lower.trim_end_matches("am").trim_end_matches("pm");

    let (hour, minute) = if time_part.contains(':') {
        let parts: Vec<&str> = time_part.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let h: u32 = parts[0].parse().ok()?;
        let m: u32 = parts[1].parse().ok()?;
        (h, m)
    } else {
        let h: u32 = time_part.parse().ok()?;
        (h, 0)
    };

    let hour_24 = if is_pm && hour != 12 {
        hour + 12
    } else if !is_pm && hour == 12 {
        0
    } else {
        hour
    };

    if hour_24 >= 24 || minute >= 60 {
        return None;
    }

    let now = chrono::Local::now();
    let today = now.date_naive();
    let target_time = chrono::NaiveTime::from_hms_opt(hour_24, minute, 0)?;
    let mut target_datetime = today.and_time(target_time);

    if target_datetime <= now.naive_local() {
        target_datetime = (today + chrono::Duration::days(1)).and_time(target_time);
    }

    let duration_secs = (target_datetime - now.naive_local()).num_seconds();
    if duration_secs > 0 {
        Some(Duration::from_secs(duration_secs as u64))
    } else {
        None
    }
}

pub(super) fn format_cache_footer(
    read_tokens: Option<u64>,
    write_tokens: Option<u64>,
) -> Option<String> {
    let _ = (read_tokens, write_tokens);
    None
}

/// Format token count for display (e.g., 63000 -> "63K")
pub(super) fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

/// Copy text to clipboard, trying wl-copy first (Wayland), then arboard as fallback.
pub(super) fn copy_to_clipboard(text: &str) -> bool {
    if let Ok(mut child) = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            if stdin.write_all(text.as_bytes()).is_ok() {
                drop(child.stdin.take());
                return child.wait().map(|s| s.success()).unwrap_or(false);
            }
        }
    }
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(text.to_string()))
        .is_ok()
}

pub(super) fn effort_display_label(effort: &str) -> &str {
    match effort {
        "xhigh" => "Max",
        "high" => "High",
        "medium" => "Medium",
        "low" => "Low",
        "none" => "None",
        other => other,
    }
}

pub(super) fn effort_bar(index: usize, total: usize) -> String {
    let mut bar = String::new();
    for i in 0..total {
        if i == index {
            bar.push('●');
        } else {
            bar.push('○');
        }
    }
    bar
}

pub(super) fn service_tier_display_label(service_tier: &str) -> &str {
    match service_tier {
        "priority" => "Fast",
        "flex" => "Flex",
        other => other,
    }
}

pub(super) fn fast_mode_success_message(
    enabled: bool,
    label: &str,
    applies_next_request: bool,
) -> String {
    let status = if enabled { "on" } else { "off" };
    if applies_next_request {
        format!(
            "✓ Fast mode {} ({})\nApplies to the next request/turn. The current in-flight request keeps its existing tier.",
            status, label
        )
    } else {
        format!("✓ Fast mode {} ({})", status, label)
    }
}

pub(super) fn fast_mode_status_notice(enabled: bool, applies_next_request: bool) -> String {
    let status = if enabled { "on" } else { "off" };
    if applies_next_request {
        format!("Fast: {} (next request)", status)
    } else {
        format!("Fast: {}", status)
    }
}

pub(super) fn fast_mode_overview_message(
    enabled: bool,
    current_label: &str,
    default_enabled: bool,
    default_label: &str,
) -> String {
    format!(
        "Fast mode is {}.\nCurrent tier: {}\nSaved default: {} ({})\nUse `/fast on`, `/fast off`, or `/fast default on|off`.",
        if enabled { "on" } else { "off" },
        current_label,
        if default_enabled { "on" } else { "off" },
        default_label
    )
}

pub(super) fn fast_mode_default_message(default_enabled: bool, default_label: &str) -> String {
    format!(
        "Saved default fast mode is {}.\nDefault tier: {}\nUse `/fast default on` or `/fast default off`.",
        if default_enabled { "on" } else { "off" },
        default_label
    )
}

pub(super) fn mask_email(email: &str) -> String {
    let trimmed = email.trim();
    let Some((local, domain)) = trimmed.split_once('@') else {
        return trimmed.to_string();
    };

    if local.is_empty() {
        return format!("***@{}", domain);
    }

    let mut chars = local.chars();
    let first = chars.next().unwrap_or('*');
    let last = chars.last().unwrap_or(first);

    let masked_local = if local.chars().count() <= 2 {
        format!("{}*", first)
    } else {
        format!("{}***{}", first, last)
    };

    format!("{}@{}", masked_local, domain)
}

/// Spawn a new terminal window that resumes a jcode session.
/// Returns Ok(true) if a terminal was successfully launched, Ok(false) if no terminal found.
fn resume_invocation_args(session_id: &str, socket: Option<&str>) -> Vec<String> {
    let mut args = vec!["--resume".to_string(), session_id.to_string()];
    if let Some(socket) = socket.filter(|s| !s.trim().is_empty()) {
        args.push("--socket".to_string());
        args.push(socket.to_string());
    }
    args
}

fn command_display(program: &Path, args: &[String]) -> String {
    std::iter::once(program.to_string_lossy().to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn build_resume_command(
    target: &ResumeTarget,
    socket: Option<&str>,
) -> (PathBuf, Vec<String>, String) {
    match target {
        ResumeTarget::JcodeSession { session_id } => {
            let exe = launch_client_executable();
            let args = resume_invocation_args(session_id, socket);
            let title = resumed_window_title(session_id);
            (exe, args, title)
        }
        ResumeTarget::ClaudeCodeSession { session_id } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_claude_code_session_id(session_id);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!("🧵 Claude Code {}", &session_id[..session_id.len().min(8)]);
            (exe, args, title)
        }
        ResumeTarget::CodexSession { session_id } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_codex_session_id(session_id);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!("🧠 Codex {}", &session_id[..session_id.len().min(8)]);
            (exe, args, title)
        }
        ResumeTarget::PiSession { session_path } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_pi_session_id(session_path);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!(
                "π Pi {}",
                Path::new(session_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("session")
            );
            (exe, args, title)
        }
        ResumeTarget::OpenCodeSession { session_id } => {
            let exe = launch_client_executable();
            let imported_id = crate::import::imported_opencode_session_id(session_id);
            let args = resume_invocation_args(&imported_id, socket);
            let title = format!("◌ OpenCode {}", &session_id[..session_id.len().min(8)]);
            (exe, args, title)
        }
    }
}

pub(super) fn resume_target_manual_command(target: &ResumeTarget, socket: Option<&str>) -> String {
    let (exe, args, _) = build_resume_command(target, socket);
    command_display(&exe, &args)
}

fn spawn_command_in_new_terminal(
    program: &Path,
    args: &[String],
    title: &str,
    cwd: &Path,
) -> anyhow::Result<bool> {
    use std::process::{Command, Stdio};

    let mut last_spawn_error: Option<std::io::Error> = None;

    #[cfg(unix)]
    let resume_terminal_candidates = resume_terminal_candidates_unix();
    #[cfg(not(unix))]
    let resume_terminal_candidates = resume_terminal_candidates_windows();

    for term in resume_terminal_candidates {
        let mut cmd = Command::new(&term);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match term.as_str() {
            #[cfg(unix)]
            "handterm" => {
                let command = shell_command(
                    &std::iter::once(program.to_string_lossy().into_owned())
                        .chain(args.iter().cloned())
                        .collect::<Vec<_>>(),
                );
                cmd.args(["--standalone", "--backend", "gpu", "--exec", &command]);
            }
            "kitty" => {
                cmd.args(["--title", title, "-e"]).arg(program).args(args);
            }
            "wezterm" => {
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    program.to_string_lossy().as_ref(),
                ]);
                cmd.args(args);
            }
            "alacritty" | "konsole" | "xterm" | "foot" => {
                if term == "alacritty" {
                    cmd.args(["--title", title, "-e"]).arg(program).args(args);
                } else {
                    cmd.args(["-e"]).arg(program).args(args);
                }
            }
            "gnome-terminal" => {
                cmd.arg("--title").arg(title);
                cmd.arg("--").arg(program).args(args);
            }
            #[cfg(target_os = "macos")]
            "iterm2" => {
                cmd = Command::new("osascript");
                cmd.args([
                    "-e",
                    &format!(
                        r#"tell application \"iTerm2\"
                            create window with default profile command \"{}\"
                        end tell"#,
                        shell_command(
                            &std::iter::once(program.to_string_lossy().into_owned())
                                .chain(args.iter().cloned())
                                .collect::<Vec<_>>()
                        )
                    ),
                ]);
            }
            #[cfg(target_os = "macos")]
            "terminal" => {
                cmd = Command::new("open");
                cmd.args([
                    "-a",
                    "Terminal",
                    program.to_str().unwrap_or("jcode"),
                    "--args",
                ]);
                cmd.args(args);
            }
            _ => continue,
        }

        match crate::platform::spawn_detached(&mut cmd) {
            Ok(_) => return Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => last_spawn_error = Some(err),
        }
    }

    if let Some(err) = last_spawn_error {
        Err(err.into())
    } else {
        Ok(false)
    }
}

pub(super) fn spawn_resume_target_in_new_terminal(
    target: &ResumeTarget,
    cwd: &Path,
    socket: Option<&str>,
) -> anyhow::Result<bool> {
    let (program, args, title) = build_resume_command(target, socket);
    spawn_command_in_new_terminal(&program, &args, &title, cwd)
}

fn resumed_window_title(session_id: &str) -> String {
    let session_name = crate::id::extract_session_name(session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.to_string());
    let icon = crate::id::session_icon(&session_name);
    if let Some(server_info) =
        crate::registry::find_server_by_socket_sync(&crate::server::socket_path())
    {
        format!("{} jcode/{} {}", icon, server_info.name, session_name)
    } else {
        format!("{} jcode {}", icon, session_name)
    }
}

#[cfg(unix)]
fn sh_escape(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn shell_command(args: &[String]) -> String {
    args.iter()
        .map(|arg| sh_escape(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn push_unique_terminal(candidates: &mut Vec<String>, term: impl Into<String>) {
    let term = term.into();
    if term.trim().is_empty() {
        return;
    }
    if !candidates.iter().any(|candidate| candidate == &term) {
        candidates.push(term);
    }
}

#[cfg(unix)]
fn detected_resume_terminal() -> Option<&'static str> {
    if std::env::var("HANDTERM_SESSION").is_ok() || std::env::var("HANDTERM_PID").is_ok() {
        return Some("handterm");
    }
    if std::env::var("TERM_PROGRAM")
        .ok()
        .map(|value| value.eq_ignore_ascii_case("handterm"))
        .unwrap_or(false)
    {
        return Some("handterm");
    }
    if std::env::var("KITTY_PID").is_ok() {
        return Some("kitty");
    }
    if std::env::var("WEZTERM_EXECUTABLE").is_ok() || std::env::var("WEZTERM_PANE").is_ok() {
        return Some("wezterm");
    }
    if std::env::var("ALACRITTY_WINDOW_ID").is_ok() {
        return Some("alacritty");
    }

    #[cfg(target_os = "macos")]
    {
        let term_program = std::env::var("TERM_PROGRAM")
            .ok()
            .map(|value| value.to_ascii_lowercase());
        return match term_program.as_deref() {
            Some("kitty") => Some("kitty"),
            Some("wezterm") => Some("wezterm"),
            Some("alacritty") => Some("alacritty"),
            Some("iterm.app") | Some("iterm2") => Some("iterm2"),
            Some("apple_terminal") | Some("terminal") => Some("terminal"),
            _ => None,
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(unix)]
fn resume_terminal_candidates_unix() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        push_unique_terminal(&mut candidates, term);
    }
    if let Some(term) = detected_resume_terminal() {
        push_unique_terminal(&mut candidates, term);
    }

    #[cfg(target_os = "macos")]
    {
        for term in ["kitty", "wezterm", "alacritty", "iterm2", "terminal"] {
            push_unique_terminal(&mut candidates, term);
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        for term in [
            "handterm",
            "kitty",
            "wezterm",
            "alacritty",
            "gnome-terminal",
            "konsole",
            "xterm",
            "foot",
        ] {
            push_unique_terminal(&mut candidates, term);
        }
    }

    candidates
}

#[cfg(unix)]
pub(super) fn spawn_in_new_terminal(
    exe: &Path,
    session_id: &str,
    cwd: &Path,
    socket: Option<&str>,
) -> anyhow::Result<bool> {
    let title = resumed_window_title(session_id);
    let args = resume_invocation_args(session_id, socket);
    spawn_command_in_new_terminal(exe, &args, &title, cwd)
}

#[cfg(not(unix))]
pub(super) fn spawn_in_new_terminal(
    _exe: &Path,
    _session_id: &str,
    _cwd: &Path,
    _socket: Option<&str>,
) -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{
        build_resume_command, detected_resume_terminal, extract_bracketed_system_message,
        format_countdown_until, gather_ambient_info, partition_queued_messages,
        resume_invocation_args, shell_command,
    };
    use crate::ambient::{AmbientManager, Priority, ScheduleRequest, ScheduleTarget};
    use crate::tui::session_picker::ResumeTarget;
    use chrono::{Duration as ChronoDuration, Utc};

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set_value(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }

        fn set_path(key: &'static str, value: &std::path::Path) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = self.prev.take() {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn extract_bracketed_system_message_strips_wrapper() {
        let parsed = extract_bracketed_system_message(
            "[SYSTEM: Your session was interrupted. Continue immediately.]",
        );
        assert_eq!(
            parsed.as_deref(),
            Some("Your session was interrupted. Continue immediately.")
        );
    }

    #[test]
    fn partition_queued_messages_moves_system_messages_into_reminders() {
        let (user_messages, reminder, display_system_messages) = partition_queued_messages(
            vec![
                "[SYSTEM: Continue where you left off.]".to_string(),
                "normal user input".to_string(),
            ],
            vec!["hidden reminder".to_string()],
        );

        assert_eq!(user_messages, vec!["normal user input"]);
        assert_eq!(
            display_system_messages,
            vec!["Continue where you left off."]
        );
        assert_eq!(
            reminder.as_deref(),
            Some("hidden reminder\n\nContinue where you left off.")
        );
    }

    #[cfg(unix)]
    #[test]
    fn detected_resume_terminal_recognizes_handterm_term_program() {
        let _guard = EnvVarGuard::set_value("TERM_PROGRAM", "handterm");
        assert_eq!(detected_resume_terminal(), Some("handterm"));
    }

    #[cfg(unix)]
    #[test]
    fn shell_command_quotes_single_quotes_for_handterm_exec() {
        let command = shell_command(&[
            "/tmp/jcode binary".to_string(),
            "--resume".to_string(),
            "session'quote".to_string(),
        ]);
        assert_eq!(
            command,
            "'/tmp/jcode binary' '--resume' 'session'\"'\"'quote'"
        );
    }

    #[test]
    fn resume_invocation_args_includes_socket_when_present() {
        let args = resume_invocation_args("ses_123", Some("/tmp/jcode-test.sock"));
        assert_eq!(
            args,
            vec![
                "--resume".to_string(),
                "ses_123".to_string(),
                "--socket".to_string(),
                "/tmp/jcode-test.sock".to_string()
            ]
        );
    }

    #[test]
    fn resume_invocation_args_omits_blank_socket() {
        let args = resume_invocation_args("ses_123", Some("   "));
        assert_eq!(args, vec!["--resume".to_string(), "ses_123".to_string()]);
    }

    #[test]
    fn build_resume_command_uses_imported_jcode_session_for_claude_code() {
        let (program, args, title) = build_resume_command(
            &ResumeTarget::ClaudeCodeSession {
                session_id: "claude-session-123".to_string(),
            },
            None,
        );

        assert_eq!(
            program.file_name().and_then(|name| name.to_str()),
            Some("jcode")
        );
        assert_eq!(
            args,
            vec![
                "--resume".to_string(),
                crate::import::imported_claude_code_session_id("claude-session-123")
            ]
        );
        assert!(title.contains("Claude Code"));
        assert!(title.contains("claude-s"));
    }

    #[test]
    fn build_resume_command_uses_imported_jcode_session_for_codex() {
        let (program, args, title) = build_resume_command(
            &ResumeTarget::CodexSession {
                session_id: "codex-session-123".to_string(),
            },
            None,
        );

        assert_eq!(
            program.file_name().and_then(|name| name.to_str()),
            Some("jcode")
        );
        assert_eq!(
            args,
            vec![
                "--resume".to_string(),
                crate::import::imported_codex_session_id("codex-session-123")
            ]
        );
        assert!(title.contains("Codex"));
    }

    #[test]
    fn format_countdown_until_handles_subminute_and_minutes() {
        let soon = Utc::now() + ChronoDuration::seconds(25);
        let medium = Utc::now() + ChronoDuration::minutes(2) + ChronoDuration::seconds(15);

        let soon_text = format_countdown_until(soon);
        let medium_text = format_countdown_until(medium);

        assert!(soon_text.starts_with("in "));
        assert!(soon_text.ends_with('s'));
        assert!(medium_text.starts_with("in 2m"));
    }

    #[test]
    fn gather_ambient_info_filters_to_session_reminders_when_ambient_disabled() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

        let mut manager = AmbientManager::new().expect("ambient manager");
        let first_due = Utc::now() + ChronoDuration::minutes(5);
        let second_due = Utc::now() + ChronoDuration::minutes(10);

        manager
            .schedule(ScheduleRequest {
                wake_in_minutes: None,
                wake_at: Some(first_due),
                context: "ambient context".to_string(),
                priority: Priority::Normal,
                target: ScheduleTarget::Ambient,
                created_by_session: "ambient".to_string(),
                working_dir: None,
                task_description: Some("ambient work".to_string()),
                relevant_files: Vec::new(),
                git_branch: None,
                additional_context: None,
            })
            .expect("schedule ambient item");
        manager
            .schedule(ScheduleRequest {
                wake_in_minutes: None,
                wake_at: Some(first_due),
                context: "first context".to_string(),
                priority: Priority::Normal,
                target: ScheduleTarget::Session {
                    session_id: "session_1".to_string(),
                },
                created_by_session: "session_1".to_string(),
                working_dir: None,
                task_description: Some("first reminder".to_string()),
                relevant_files: Vec::new(),
                git_branch: None,
                additional_context: None,
            })
            .expect("schedule first reminder");
        manager
            .schedule(ScheduleRequest {
                wake_in_minutes: None,
                wake_at: Some(second_due),
                context: "second context".to_string(),
                priority: Priority::Normal,
                target: ScheduleTarget::Session {
                    session_id: "session_1".to_string(),
                },
                created_by_session: "session_1".to_string(),
                working_dir: None,
                task_description: Some("second reminder".to_string()),
                relevant_files: Vec::new(),
                git_branch: None,
                additional_context: None,
            })
            .expect("schedule second reminder");

        let info = gather_ambient_info(false).expect("ambient info");
        assert!(info.show_widget);
        assert_eq!(info.queue_count, 3);
        assert_eq!(info.reminder_count, 2);
        assert_eq!(
            info.next_reminder_preview.as_deref(),
            Some("first reminder")
        );
        assert!(
            info.next_reminder_wake
                .as_deref()
                .is_some_and(|text| text.starts_with("in 4m") || text.starts_with("in 5m"))
        );
    }
}

/// Try to get an image from the system clipboard.
///
/// Returns `Some((media_type, base64_data))` if an image is available.
/// Uses `wl-paste` on Wayland, `osascript` on macOS, falls back to `arboard::get_image()`.
pub(super) fn clipboard_image() -> Option<(String, String)> {
    use base64::Engine;

    // Try wl-paste first (native Wayland - better image format support)
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        if let Ok(output) = std::process::Command::new("wl-paste")
            .arg("--list-types")
            .output()
        {
            let types = String::from_utf8_lossy(&output.stdout);
            crate::logging::info(&format!(
                "clipboard_image: wl-paste types: {:?}",
                types.trim()
            ));
            let (mime, wl_type) = if types.lines().any(|t| t.trim() == "image/png") {
                ("image/png", "image/png")
            } else if types.lines().any(|t| t.trim() == "image/jpeg") {
                ("image/jpeg", "image/jpeg")
            } else if types.lines().any(|t| t.trim() == "image/webp") {
                ("image/webp", "image/webp")
            } else if types.lines().any(|t| t.trim() == "image/gif") {
                ("image/gif", "image/gif")
            } else {
                ("", "")
            };

            if !mime.is_empty() {
                if let Ok(img_output) = std::process::Command::new("wl-paste")
                    .args(["--type", wl_type, "--no-newline"])
                    .output()
                {
                    if img_output.status.success() && !img_output.stdout.is_empty() {
                        let b64 =
                            base64::engine::general_purpose::STANDARD.encode(&img_output.stdout);
                        return Some((mime.to_string(), b64));
                    }
                }
            }

            // Fallback: check text/html for <img> tags (Discord copies HTML with image URLs)
            if types.lines().any(|t| t.trim() == "text/html") {
                if let Ok(html_output) = std::process::Command::new("wl-paste")
                    .args(["--type", "text/html"])
                    .output()
                {
                    if html_output.status.success() && !html_output.stdout.is_empty() {
                        let html = String::from_utf8_lossy(&html_output.stdout);
                        crate::logging::info(&format!(
                            "clipboard_image: checking HTML for img tags ({} bytes)",
                            html.len()
                        ));
                        if let Some(url) = extract_image_url(&html) {
                            crate::logging::info(&format!(
                                "clipboard_image: found image URL in HTML: {}",
                                &url[..url.len().min(80)]
                            ));
                            if let Some(result) = download_image_url(&url) {
                                return Some(result);
                            }
                        }
                    }
                }
            }
        }
    }

    // macOS: use osascript to check clipboard for images and save as PNG via temp file
    #[cfg(target_os = "macos")]
    {
        let temp_path = std::env::temp_dir().join("jcode_clipboard.png");
        let script = format!(
            r#"use framework \"AppKit\"
            set pb to current application's NSPasteboard's generalPasteboard()
            set imgClasses to current application's NSArray's arrayWithObject:(current application's NSImage)
            if (pb's canReadObjectForClasses:imgClasses options:(missing value)) then
                set imgList to pb's readObjectsForClasses:imgClasses options:(missing value)
                set img to item 1 of imgList
                set tiffData to img's TIFFRepresentation()
                set bitmapRep to current application's NSBitmapImageRep's imageRepWithData:tiffData
                set pngData to bitmapRep's representationUsingType:(current application's NSBitmapImageFileTypePNG) properties:(missing value)
                pngData's writeToFile:\"{}\" atomically:true
                return \"ok\"
            else
                return \"none\"
            end if"#,
            temp_path.to_string_lossy()
        );
        if let Ok(output) = std::process::Command::new("osascript")
            .args(["-l", "AppleScript", "-e", &script])
            .output()
        {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if result == "ok" {
                if let Ok(data) = std::fs::read(&temp_path) {
                    let _ = std::fs::remove_file(&temp_path);
                    if !data.is_empty() {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                        return Some(("image/png".to_string(), b64));
                    }
                }
            }
        }
    }

    // Fallback: arboard (works on X11/XWayland and macOS via NSPasteboard)
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(img) = clipboard.get_image() {
            // img.bytes is RGBA pixel data - encode as PNG
            if let Some(png_data) = encode_rgba_as_png(img.width, img.height, &img.bytes) {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png_data);
                return Some(("image/png".to_string(), b64));
            }
        }
    }

    None
}

/// Extract an image URL from text that looks like an HTML img tag or a bare image URL.
/// Returns the URL if found.
pub(super) fn extract_image_url(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Check for <img src="..."> pattern (Discord web copies)
    if let Some(start) = trimmed.find("<img") {
        if let Some(src_start) = trimmed[start..].find("src=\"") {
            let url_start = start + src_start + 5;
            if let Some(url_end) = trimmed[url_start..].find('"') {
                let url = &trimmed[url_start..url_start + url_end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
        if let Some(src_start) = trimmed[start..].find("src='") {
            let url_start = start + src_start + 5;
            if let Some(url_end) = trimmed[url_start..].find('\'') {
                let url = &trimmed[url_start..url_start + url_end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
    }

    // Check for bare image URL
    if trimmed.starts_with("http")
        && (trimmed.contains(".png")
            || trimmed.contains(".jpg")
            || trimmed.contains(".jpeg")
            || trimmed.contains(".gif")
            || trimmed.contains(".webp"))
    {
        // Strip query params for extension check but return full URL
        return Some(trimmed.to_string());
    }

    None
}

/// Download an image from a URL and return (media_type, base64_data).
/// Uses curl for simplicity (available on all platforms).
pub(super) fn download_image_url(url: &str) -> Option<(String, String)> {
    use base64::Engine;

    let output = std::process::Command::new("curl")
        .args(["-sL", "--max-time", "10", "--max-filesize", "10000000", url])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    // Detect image type from magic bytes
    let data = &output.stdout;
    let media_type = if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"RIFF") && data.len() > 12 && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };

    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    Some((media_type.to_string(), b64))
}

/// Encode raw RGBA pixel data as PNG bytes.
pub(super) fn encode_rgba_as_png(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    use image::{ImageBuffer, RgbaImage};
    use std::io::Cursor;

    let img: RgbaImage = ImageBuffer::from_raw(width as u32, height as u32, rgba.to_vec())?;
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

pub(super) fn gather_git_info() -> Option<GitInfo> {
    use std::sync::Mutex;
    use std::time::Instant;

    static CACHE: Mutex<Option<(Instant, Option<GitInfo>)>> = Mutex::new(None);

    const TTL: Duration = Duration::from_secs(5);

    if let Ok(guard) = CACHE.lock() {
        if let Some((ts, ref cached)) = *guard {
            if ts.elapsed() < TTL {
                return cached.clone();
            }
        }
    }

    let result = gather_git_info_inner();

    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((Instant::now(), result.clone()));
    }

    result
}

pub(super) fn gather_todos_for_session(session_id: Option<&str>) -> Vec<TodoItem> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    use std::time::Instant;

    static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Vec<TodoItem>)>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    const TTL: Duration = Duration::from_secs(1);

    let Some(session_id) = session_id else {
        return Vec::new();
    };

    if let Ok(cache) = CACHE.lock() {
        if let Some((ts, todos)) = cache.get(session_id) {
            if ts.elapsed() < TTL {
                return todos.clone();
            }
        }
    }

    let todos = crate::todo::load_todos(session_id).unwrap_or_default();

    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(session_id.to_string(), (Instant::now(), todos.clone()));
    }

    todos
}

pub(super) fn gather_memory_info(memory_enabled: bool) -> Option<MemoryInfo> {
    use std::sync::Mutex;
    use std::time::Instant;

    static CACHE: Mutex<Option<(Instant, Option<MemoryInfo>)>> = Mutex::new(None);
    const TTL: Duration = Duration::from_secs(2);

    if !memory_enabled {
        return None;
    }

    let activity = crate::memory::get_activity();
    let sidecar_model = if crate::memory::memory_sidecar_enabled() {
        let sidecar = crate::sidecar::Sidecar::new();
        Some(format!(
            "{} · {}",
            sidecar.backend_name(),
            sidecar.model_name()
        ))
    } else {
        None
    };

    if let Ok(guard) = CACHE.lock() {
        if let Some((ts, ref cached)) = *guard {
            if ts.elapsed() < TTL {
                return match cached.clone() {
                    Some(mut info) => {
                        info.activity = activity.clone();
                        info.sidecar_model = sidecar_model.clone();
                        Some(info)
                    }
                    None => activity.clone().map(|activity| MemoryInfo {
                        sidecar_available: crate::memory::memory_sidecar_enabled(),
                        sidecar_model: sidecar_model.clone(),
                        activity: Some(activity),
                        ..Default::default()
                    }),
                };
            }
        }
    }

    use crate::memory::MemoryManager;

    let manager = MemoryManager::new();
    let project_graph = manager.load_project_graph().ok();
    let global_graph = manager.load_global_graph().ok();

    let (project_count, global_count, by_category) = {
        let mut by_category = std::collections::HashMap::new();
        let project_count = project_graph
            .as_ref()
            .map(|p| {
                for entry in p.memories.values() {
                    *by_category.entry(entry.category.to_string()).or_insert(0) += 1;
                }
                p.memory_count()
            })
            .unwrap_or(0);
        let global_count = global_graph
            .as_ref()
            .map(|g| {
                for entry in g.memories.values() {
                    *by_category.entry(entry.category.to_string()).or_insert(0) += 1;
                }
                g.memory_count()
            })
            .unwrap_or(0);
        (project_count, global_count, by_category)
    };

    let total_count = project_count + global_count;
    let (graph_nodes, graph_edges) = crate::tui::info_widget::build_graph_topology(
        project_graph.as_ref(),
        global_graph.as_ref(),
    );

    let result = if total_count > 0 || activity.is_some() || sidecar_model.is_some() {
        Some(MemoryInfo {
            total_count,
            project_count,
            global_count,
            by_category,
            sidecar_available: crate::memory::memory_sidecar_enabled(),
            sidecar_model,
            activity,
            graph_nodes,
            graph_edges,
        })
    } else {
        None
    };

    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((Instant::now(), result.clone()));
    }

    result
}

pub(super) fn gather_ambient_info(ambient_enabled: bool) -> Option<AmbientWidgetData> {
    use std::time::Instant;
    const TTL: Duration = Duration::from_secs(2);

    if let Ok(guard) = AMBIENT_INFO_CACHE.lock() {
        if let Some((ts, cached_enabled, ref cached)) = *guard {
            if cached_enabled == ambient_enabled && ts.elapsed() < TTL {
                return cached.clone();
            }
        }
    }

    let state = crate::ambient::AmbientState::load().unwrap_or_default();
    let manager = crate::ambient::AmbientManager::new().ok();
    let queue_items: Vec<_> = manager
        .as_ref()
        .map(|m| m.queue().items().to_vec())
        .unwrap_or_default();
    let queue_count = queue_items.len();
    let next_queue_item = queue_items.iter().min_by_key(|item| item.scheduled_for);
    let reminder_items: Vec<_> = queue_items
        .iter()
        .filter(|item| matches!(item.target, crate::ambient::ScheduleTarget::Session { .. }))
        .collect();
    let reminder_count = reminder_items.len();
    let next_reminder_item = reminder_items
        .iter()
        .min_by_key(|item| item.scheduled_for)
        .copied();

    if !ambient_enabled && reminder_count == 0 {
        if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock() {
            *guard = Some((Instant::now(), ambient_enabled, None));
        }
        return None;
    }

    let last_run_ago = state.last_run.map(|t| {
        let ago = chrono::Utc::now() - t;
        if ago.num_hours() > 0 {
            format!("{}h ago", ago.num_hours())
        } else {
            format!("{}m ago", ago.num_minutes().max(0))
        }
    });
    let next_wake = match &state.status {
        crate::ambient::AmbientStatus::Scheduled { next_wake } => {
            Some(format_countdown_until(*next_wake))
        }
        _ => None,
    };

    let next_queue_preview = next_queue_item.map(|item| {
        item.task_description
            .as_deref()
            .unwrap_or(&item.context)
            .to_string()
    });
    let next_reminder_preview = next_reminder_item.map(|item| {
        item.task_description
            .as_deref()
            .unwrap_or(&item.context)
            .to_string()
    });

    let result = Some(AmbientWidgetData {
        show_widget: ambient_enabled || reminder_count > 1,
        status: state.status,
        queue_count,
        next_queue_preview,
        reminder_count,
        next_reminder_preview,
        last_run_ago,
        last_summary: state.last_summary,
        next_wake,
        next_reminder_wake: next_reminder_item
            .map(|item| format_countdown_until(item.scheduled_for)),
        budget_percent: None,
    });

    if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock() {
        *guard = Some((Instant::now(), ambient_enabled, result.clone()));
    }

    result
}

#[cfg(test)]
pub(crate) fn clear_ambient_info_cache_for_tests() {
    if let Ok(mut guard) = AMBIENT_INFO_CACHE.lock() {
        *guard = None;
    }
}

pub(crate) fn format_countdown_until(target: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (target - chrono::Utc::now()).num_seconds().max(0);
    match secs {
        0..=59 => format!("in {}s", secs),
        60..=3599 => {
            let mins = secs / 60;
            let rem = secs % 60;
            if rem == 0 {
                format!("in {}m", mins)
            } else {
                format!("in {}m {}s", mins, rem)
            }
        }
        _ => {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            if mins == 0 {
                format!("in {}h", hours)
            } else {
                format!("in {}h {}m", hours, mins)
            }
        }
    }
}

fn gather_git_info_inner() -> Option<GitInfo> {
    use std::process::Command;

    let in_repo = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !in_repo {
        return None;
    }

    let branch = Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let b = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if b.is_empty() { None } else { Some(b) }
            } else {
                None
            }
        })
        .unwrap_or_else(|| "HEAD".to_string());

    let mut modified = 0;
    let mut staged = 0;
    let mut untracked = 0;
    let mut dirty_files = Vec::new();

    if let Ok(output) = Command::new("git").args(["status", "--porcelain"]).output() {
        if output.status.success() {
            let status = String::from_utf8_lossy(&output.stdout);
            for line in status.lines() {
                if line.len() < 3 {
                    continue;
                }
                let index_status = line.as_bytes()[0];
                let worktree_status = line.as_bytes()[1];
                let file_path = line[3..].to_string();

                if index_status == b'?' {
                    untracked += 1;
                } else {
                    if index_status != b' ' && index_status != b'?' {
                        staged += 1;
                    }
                    if worktree_status != b' ' && worktree_status != b'?' {
                        modified += 1;
                    }
                }

                if dirty_files.len() < 10 {
                    dirty_files.push(file_path);
                }
            }
        }
    }

    let (ahead, behind) = Command::new("git")
        .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let parts: Vec<&str> = text.split('\t').collect();
                if parts.len() == 2 {
                    let a = parts[0].parse::<usize>().unwrap_or(0);
                    let b = parts[1].parse::<usize>().unwrap_or(0);
                    Some((a, b))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or((0, 0));

    Some(GitInfo {
        branch,
        modified,
        staged,
        untracked,
        ahead,
        behind,
        dirty_files,
    })
}
