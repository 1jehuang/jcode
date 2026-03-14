use crate::tui::info_widget::GitInfo;
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

fn resumed_window_title(session_id: &str) -> String {
    let session_name = crate::id::extract_session_name(session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.to_string());
    let icon = crate::id::session_icon(&session_name);
    format!("{} jcode {}", icon, session_name)
}

#[cfg(unix)]
pub(super) fn spawn_in_new_terminal(
    exe: &Path,
    session_id: &str,
    cwd: &Path,
    socket: Option<&str>,
) -> anyhow::Result<bool> {
    use std::process::{Command, Stdio};

    let mut candidates: Vec<String> = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        if !term.trim().is_empty() {
            candidates.push(term);
        }
    }
    candidates.extend(
        [
            "kitty",
            "wezterm",
            "alacritty",
            "gnome-terminal",
            "konsole",
            "xterm",
            "foot",
        ]
        .iter()
        .map(|s| s.to_string()),
    );

    for term in candidates {
        let mut cmd = Command::new(&term);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match term.as_str() {
            "kitty" => {
                let title = resumed_window_title(session_id);
                cmd.args(["--title", &title, "-e"])
                    .arg(exe)
                    .args(resume_invocation_args(session_id, socket));
            }
            "wezterm" => {
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    exe.to_string_lossy().as_ref(),
                ]);
                cmd.args(resume_invocation_args(session_id, socket));
            }
            "alacritty" => {
                cmd.args(["-e"])
                    .arg(exe)
                    .args(resume_invocation_args(session_id, socket));
            }
            "gnome-terminal" => {
                cmd.args(["--", exe.to_string_lossy().as_ref()]);
                cmd.args(resume_invocation_args(session_id, socket));
            }
            "konsole" => {
                cmd.args(["-e"])
                    .arg(exe)
                    .args(resume_invocation_args(session_id, socket));
            }
            "xterm" => {
                cmd.args(["-e"])
                    .arg(exe)
                    .args(resume_invocation_args(session_id, socket));
            }
            "foot" => {
                cmd.args(["-e"])
                    .arg(exe)
                    .args(resume_invocation_args(session_id, socket));
            }
            _ => continue,
        }

        if crate::platform::spawn_detached(&mut cmd).is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
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
        extract_bracketed_system_message, partition_queued_messages, resume_invocation_args,
    };

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
