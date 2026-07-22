#![cfg_attr(test, allow(clippy::items_after_test_module))]

use super::{
    App, ContentBlock, DisplayMessage, Message, ProcessingStatus, Role, SendAction, SkillRegistry,
    commands, ctrl_bracket_fallback_to_esc, is_context_limit_error,
    is_request_payload_too_large_error, remote,
};
use crate::bus::{
    Bus, BusEvent, ClipboardPasteCompleted, ClipboardPasteContent, ClipboardPasteKind,
    InputShellCompleted,
};
use crate::util::truncate_str;
use anyhow::Result;
use base64::Engine;
use crossterm::event::{EventStream, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

const INPUT_SHELL_MAX_OUTPUT_LEN: usize = 30_000;

/// Remove reasoning-marked lines from committed transcript text. Reasoning lines
/// are wrapped in emphasis containing the invisible [`REASONING_SENTINEL`]
/// (see `jcode_tui_markdown::reasoning_line_markup`). Trailing blank lines left
/// behind are trimmed so the remaining answer renders cleanly.
pub(super) fn strip_reasoning_lines(content: &str) -> String {
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    let mut out_lines: Vec<&str> = Vec::new();
    for line in content.split('\n') {
        if line.contains(sentinel) {
            continue;
        }
        out_lines.push(line);
    }
    // Collapse runs of blank lines created by removed reasoning blocks, and trim
    // leading/trailing blank lines.
    let mut result = String::with_capacity(content.len());
    let mut prev_blank = true; // suppress leading blanks
    for line in out_lines {
        let is_blank = line.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line);
        prev_blank = is_blank;
    }
    result.trim_end().to_string()
}

fn mission_turn_reminder(session_id: &str) -> Option<String> {
    crate::mission::active_system_reminder(session_id)
        .map_err(|err| crate::logging::warn(&format!("failed to load active mission: {err}")))
        .ok()
        .flatten()
}

fn merge_turn_reminders(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{}\n\n{}", a, b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub(super) fn extract_input_shell_command(input: &str) -> Option<&str> {
    input.trim().strip_prefix('!').map(str::trim)
}

fn build_input_shell_command(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg(command);
        cmd
    }
}

fn combine_shell_output(stdout: &[u8], stderr: &[u8]) -> (String, bool) {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let mut output = String::new();

    if !stdout.is_empty() {
        output.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("[stderr]\n");
        output.push_str(&stderr);
    }

    let truncated = if output.len() > INPUT_SHELL_MAX_OUTPUT_LEN {
        output = truncate_str(&output, INPUT_SHELL_MAX_OUTPUT_LEN).to_string();
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("… output truncated");
        true
    } else {
        false
    };

    (output, truncated)
}

fn spawn_input_shell_command(session_id: String, command: String, cwd: Option<String>) {
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut cmd = build_input_shell_command(&command);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(dir) = cwd.as_ref() {
            cmd.current_dir(dir);
        }

        let event = match cmd.output() {
            Ok(output) => {
                let (combined_output, truncated) =
                    combine_shell_output(&output.stdout, &output.stderr);
                InputShellCompleted {
                    session_id,
                    result: crate::message::InputShellResult {
                        command,
                        cwd,
                        output: combined_output,
                        exit_code: output.status.code(),
                        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        truncated,
                        failed_to_start: false,
                    },
                }
            }
            Err(error) => InputShellCompleted {
                session_id,
                result: crate::message::InputShellResult {
                    command,
                    cwd,
                    output: format!("Failed to run command: {}", error),
                    exit_code: None,
                    duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    truncated: false,
                    failed_to_start: true,
                },
            },
        };

        Bus::global().publish(BusEvent::InputShellCompleted(event));
    });
}

pub(super) struct PreparedInput {
    pub raw_input: String,
    pub expanded: String,
    pub images: Vec<(String, String)>,
}

// Roughly 500k English words at ~6 bytes/word including spaces. This is still
// low enough to avoid multi-megabyte submit-path hangs while allowing very large logs.
pub(super) const MAX_SUBMITTED_TEXT_BYTES: usize = 3 * 1024 * 1024;

fn oversized_message_notice(size: usize) -> String {
    format!(
        "Message is too large to send ({} bytes). Save it as a file or attach it instead. Your input was preserved.",
        crate::util::format_number(size)
    )
}

fn input_exceeds_submit_limit(input: &str) -> Option<String> {
    let size = input.len();
    (size > MAX_SUBMITTED_TEXT_BYTES).then(|| oversized_message_notice(size))
}

pub(super) fn paste_from_clipboard(app: &mut App) {
    app.set_status_notice("Reading clipboard...");
    spawn_clipboard_paste(app, ClipboardPasteKind::Smart);
}

fn is_clipboard_paste_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('v' | 'V'))
        && modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META,
        )
}

fn active_clipboard_session_id(app: &App) -> String {
    app.active_client_session_id()
        .unwrap_or(app.session.id.as_str())
        .to_string()
}

fn publish_clipboard_result(
    session_id: String,
    kind: ClipboardPasteKind,
    content: ClipboardPasteContent,
) {
    Bus::global().publish(BusEvent::ClipboardPasteCompleted(ClipboardPasteCompleted {
        session_id,
        kind,
        content,
    }));
}

fn spawn_clipboard_paste(app: &App, kind: ClipboardPasteKind) {
    let session_id = active_clipboard_session_id(app);
    let task_kind = kind.clone();
    spawn_blocking_or_thread(move || {
        let content = read_clipboard_for_paste(&task_kind);
        publish_clipboard_result(session_id, task_kind, content);
    });
}

fn spawn_blocking_or_thread<F>(task: F)
where
    F: FnOnce() + Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(task);
    } else {
        std::thread::spawn(task);
    }
}

fn read_clipboard_text() -> Option<String> {
    if std::env::var("WAYLAND_DISPLAY").is_ok()
        && let Some(text) = read_wayland_clipboard_text()
    {
        return Some(text);
    }

    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return None;
    };
    clipboard.get_text().ok()
}

fn read_wayland_clipboard_text() -> Option<String> {
    let types_output = std::process::Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .ok()?;
    if !types_output.status.success() {
        return None;
    }

    let types = String::from_utf8_lossy(&types_output.stdout);
    let wl_type = preferred_wayland_text_type(&types)?;
    let output = std::process::Command::new("wl-paste")
        .args(["--type", wl_type, "--no-newline"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

fn preferred_wayland_text_type(types: &str) -> Option<&'static str> {
    let has_type = |needle: &str| types.lines().any(|line| line.trim() == needle);
    if has_type("text/plain;charset=utf-8") {
        Some("text/plain;charset=utf-8")
    } else if has_type("text/plain") {
        Some("text/plain")
    } else if has_type("UTF8_STRING") {
        Some("UTF8_STRING")
    } else if has_type("TEXT") {
        Some("TEXT")
    } else if has_type("STRING") {
        Some("STRING")
    } else {
        None
    }
}

fn image_content(media_type: String, base64_data: String) -> ClipboardPasteContent {
    ClipboardPasteContent::Image {
        media_type,
        base64_data,
    }
}

fn download_image_url_content(url: &str) -> Option<ClipboardPasteContent> {
    super::download_image_url(url)
        .map(|(media_type, base64_data)| image_content(media_type, base64_data))
}

fn read_clipboard_for_paste(kind: &ClipboardPasteKind) -> ClipboardPasteContent {
    read_clipboard_for_paste_with(
        kind,
        read_clipboard_text,
        super::clipboard_image,
        download_image_url_content,
    )
}

fn read_clipboard_for_paste_with<ReadText, ReadImage, DownloadImageUrl>(
    kind: &ClipboardPasteKind,
    mut read_text: ReadText,
    mut read_image: ReadImage,
    mut download_image_url: DownloadImageUrl,
) -> ClipboardPasteContent
where
    ReadText: FnMut() -> Option<String>,
    ReadImage: FnMut() -> Option<(String, String)>,
    DownloadImageUrl: FnMut(&str) -> Option<ClipboardPasteContent>,
{
    match kind {
        ClipboardPasteKind::Smart => {
            // Only treat the clipboard as text when it has *non-empty* text.
            // Image-only clipboards (especially on Wayland/arboard) frequently
            // expose an empty text target, which previously short-circuited the
            // image path and produced a silent "0 char" paste.
            if let Some(text) = read_text().filter(|t| !t.trim().is_empty()) {
                if let Some(url) = super::extract_image_url(&text)
                    && let Some(content) = download_image_url(&url)
                {
                    return content;
                }
                return ClipboardPasteContent::Text(text);
            }
            if let Some((media_type, base64_data)) = read_image() {
                return image_content(media_type, base64_data);
            }
            ClipboardPasteContent::Empty
        }
        ClipboardPasteKind::ImageOnly => {
            if let Some((media_type, base64_data)) = read_image() {
                return image_content(media_type, base64_data);
            }
            if let Some(text) = read_text() {
                if let Some(url) = super::extract_image_url(&text) {
                    return download_image_url(&url).unwrap_or_else(|| {
                        ClipboardPasteContent::Error("Failed to download image".to_string())
                    });
                }
                return ClipboardPasteContent::Text(text);
            }
            ClipboardPasteContent::Empty
        }
        ClipboardPasteKind::ImageUrl { fallback_text } => {
            let Some(url) = fallback_text.as_deref().and_then(super::extract_image_url) else {
                return ClipboardPasteContent::Empty;
            };
            download_image_url(&url).unwrap_or_else(|| {
                ClipboardPasteContent::Error("Failed to download image".to_string())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClipboardPasteContent, ClipboardPasteKind, dropped_image_files,
        is_clipboard_paste_shortcut, parse_dropped_paths, preferred_wayland_text_type,
        read_clipboard_for_paste_with, shifted_printable_fallback, text_input_for_key,
    };
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn dropped_paths_accept_quotes_shell_escapes_and_file_urls() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first image.png");
        let second = dir.path().join("second.jpg");
        std::fs::write(&first, b"png").unwrap();
        std::fs::write(&second, b"jpeg").unwrap();

        let quoted = parse_dropped_paths(&format!("'{}'", first.display())).unwrap();
        assert_eq!(quoted, vec![first.clone()]);
        let escaped =
            parse_dropped_paths(&first.display().to_string().replace(' ', "\\ ")).unwrap();
        assert_eq!(escaped, vec![first.clone()]);
        let url = url::Url::from_file_path(&second).unwrap();
        assert_eq!(parse_dropped_paths(url.as_str()).unwrap(), vec![second]);
    }

    #[test]
    fn dropped_images_load_all_supported_files_and_reject_mixed_text() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        let jpeg = dir.path().join("b.jpeg");
        std::fs::write(&png, b"png bytes").unwrap();
        std::fs::write(&jpeg, b"jpeg bytes").unwrap();

        let images =
            dropped_image_files(&format!("'{}' '{}'", png.display(), jpeg.display())).unwrap();
        assert_eq!(images[0], ("image/png".to_string(), b"png bytes".to_vec()));
        assert_eq!(
            images[1],
            ("image/jpeg".to_string(), b"jpeg bytes".to_vec())
        );
        assert!(dropped_image_files("ordinary pasted text").is_none());
    }

    #[test]
    fn smart_paste_prefers_normal_text_when_clipboard_has_text() {
        let content = read_clipboard_for_paste_with(
            &ClipboardPasteKind::Smart,
            || Some("plain text".to_string()),
            || Some(("image/png".to_string(), "base64".to_string())),
            |_| None,
        );

        match content {
            ClipboardPasteContent::Text(text) => assert_eq!(text, "plain text"),
            other => panic!("expected text paste, got {other:?}"),
        }
    }

    #[test]
    fn smart_paste_uses_image_only_when_no_text_is_available() {
        let content = read_clipboard_for_paste_with(
            &ClipboardPasteKind::Smart,
            || None,
            || Some(("image/png".to_string(), "base64".to_string())),
            |_| None,
        );

        match content {
            ClipboardPasteContent::Image {
                media_type,
                base64_data,
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(base64_data, "base64");
            }
            other => panic!("expected image paste, got {other:?}"),
        }
    }

    #[test]
    fn smart_paste_empty_clipboard_stays_empty_not_dictation() {
        let content =
            read_clipboard_for_paste_with(&ClipboardPasteKind::Smart, || None, || None, |_| None);

        assert!(
            matches!(content, ClipboardPasteContent::Empty),
            "expected empty paste, got {content:?}"
        );
    }

    #[test]
    fn smart_paste_uses_image_when_text_target_is_blank() {
        // Image-only clipboards can advertise an empty text target; the image
        // must still be pasted instead of producing a silent empty text paste.
        let content = read_clipboard_for_paste_with(
            &ClipboardPasteKind::Smart,
            || Some("   ".to_string()),
            || Some(("image/png".to_string(), "base64".to_string())),
            |_| None,
        );

        match content {
            ClipboardPasteContent::Image {
                media_type,
                base64_data,
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(base64_data, "base64");
            }
            other => panic!("expected image paste, got {other:?}"),
        }
    }

    #[test]
    fn paste_shortcut_accepts_control_alt_command_and_meta_v() {
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
            KeyModifiers::META,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        ] {
            assert!(
                is_clipboard_paste_shortcut(KeyCode::Char('v'), modifiers),
                "{modifiers:?}+v should paste clipboard contents"
            );
            assert!(
                is_clipboard_paste_shortcut(KeyCode::Char('V'), modifiers),
                "{modifiers:?}+V should paste clipboard contents"
            );
        }

        assert!(!is_clipboard_paste_shortcut(
            KeyCode::Char('v'),
            KeyModifiers::empty()
        ));
    }

    #[test]
    fn wayland_text_type_prefers_utf8_plain_text() {
        let types = "text/plain\ntext/plain;charset=utf-8\nTEXT\nSTRING\nUTF8_STRING\n";

        assert_eq!(
            preferred_wayland_text_type(types),
            Some("text/plain;charset=utf-8")
        );
    }

    #[test]
    fn shifted_printable_fallback_uppercases_ascii_letters() {
        assert_eq!(shifted_printable_fallback('a', KeyModifiers::SHIFT), 'A');
        assert_eq!(shifted_printable_fallback('z', KeyModifiers::SHIFT), 'Z');
    }

    #[test]
    fn shifted_printable_fallback_preserves_terminal_translated_symbols() {
        assert_eq!(shifted_printable_fallback('/', KeyModifiers::SHIFT), '/');
        assert_eq!(shifted_printable_fallback('?', KeyModifiers::SHIFT), '?');
        assert_eq!(shifted_printable_fallback('(', KeyModifiers::SHIFT), '(');
        assert_eq!(shifted_printable_fallback('&', KeyModifiers::SHIFT), '&');
    }

    #[test]
    fn shifted_printable_fallback_does_not_synthesize_us_symbol_layout() {
        assert_eq!(shifted_printable_fallback('7', KeyModifiers::SHIFT), '7');
        assert_eq!(shifted_printable_fallback('8', KeyModifiers::SHIFT), '8');
        assert_eq!(shifted_printable_fallback('=', KeyModifiers::SHIFT), '=');
    }

    #[test]
    fn text_input_for_shifted_symbols_preserves_layout_translated_char() {
        for c in ['/', '?', '(', ')', '&', '=', '"'] {
            assert_eq!(
                text_input_for_key(KeyCode::Char(c), KeyModifiers::SHIFT),
                Some(c.to_string()),
                "shifted {c:?} should be treated as terminal/layout-translated text"
            );
        }
    }

    #[test]
    fn text_input_for_altgr_symbols_preserves_layout_translated_char() {
        let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;

        for c in ['@', '{', '}', '\\', '€', 'ą'] {
            assert_eq!(
                text_input_for_key(KeyCode::Char(c), altgr),
                Some(c.to_string()),
                "AltGr-style {c:?} should be treated as terminal/layout-translated text"
            );
        }
    }

    #[test]
    fn text_input_for_control_shortcut_letters_stays_non_text() {
        assert_eq!(
            text_input_for_key(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            ),
            None
        );
        assert_eq!(
            text_input_for_key(KeyCode::Char('@'), KeyModifiers::CONTROL),
            None
        );
    }
}

pub(super) fn cut_input_line_to_clipboard(app: &mut App) -> bool {
    cut_input_line_to_clipboard_with(app, super::copy_to_clipboard)
}

pub(super) fn cut_input_line_to_clipboard_with<F>(app: &mut App, mut copy_text: F) -> bool
where
    F: FnMut(&str) -> bool,
{
    if app.input.is_empty() {
        return false;
    }

    if !copy_text(&app.input) {
        app.set_status_notice("Failed to copy input line");
        return false;
    }

    app.remember_input_undo_state();
    app.input.clear();
    app.cursor_pos = 0;
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
    app.set_status_notice("✂ Cut input line");
    true
}

mod paste_guard;
#[cfg(test)]
pub(in crate::tui::app) use paste_guard::expire_for_test as paste_guard_expire_for_test;
use paste_guard::image_media_type;

pub(super) fn handle_paste(app: &mut App, text: String) {
    paste_guard::note_paste();
    // Note: clipboard_image() is NOT checked here. Bracketed paste events from the
    // terminal always deliver text. Checking clipboard_image() here caused a bug where
    // text pastes were misidentified as images when the clipboard also had image data
    // (common on Wayland where apps advertise multiple MIME types). Image pasting is
    // handled by explicit clipboard shortcuts instead (Ctrl+V/Alt+V/Cmd+V smart-paste).
    if let Some(paths) = parse_dropped_paths(&text) {
        let item_count = paths.len();
        let mut image_count = 0;
        let mut file_count = 0;

        for (index, path) in paths.into_iter().enumerate() {
            if index > 0 {
                insert_input_text(app, " ");
            }

            if let Some(media_type) = image_media_type(&path)
                && let Ok(data) = std::fs::read(&path)
            {
                attach_image(
                    app,
                    media_type.to_string(),
                    base64::engine::general_purpose::STANDARD.encode(data),
                );
                image_count += 1;
            } else {
                insert_input_text(app, &format_dropped_path(&path, item_count > 1));
                file_count += 1;
            }
        }

        let notice = match (image_count, file_count) {
            (images, 0) => format!(
                "Dropped {images} image{}",
                if images == 1 { "" } else { "s" }
            ),
            (0, files) => format!("Dropped {files} file{}", if files == 1 { "" } else { "s" }),
            (images, files) => format!(
                "Dropped {images} image{} and {files} file{}",
                if images == 1 { "" } else { "s" },
                if files == 1 { "" } else { "s" }
            ),
        };
        app.set_status_notice(notice);
    } else if let Some(url) = super::extract_image_url(&text) {
        crate::logging::info(&format!("Downloading image from pasted URL: {}", url));
        app.set_status_notice("Downloading image...");
        let session_id = active_clipboard_session_id(app);
        spawn_blocking_or_thread(move || {
            let content = download_image_url_content(&url).unwrap_or_else(|| {
                ClipboardPasteContent::Error("Failed to download image".to_string())
            });
            publish_clipboard_result(
                session_id,
                ClipboardPasteKind::ImageUrl {
                    fallback_text: Some(text),
                },
                content,
            );
        });
    } else {
        handle_text_paste(app, text);
    }
}

fn format_dropped_path(path: &std::path::Path, quote_whitespace: bool) -> String {
    let value = path.to_string_lossy();
    if quote_whitespace && value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.into_owned()
    }
}

fn dropped_image_files(text: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let paths = parse_dropped_paths(text)?;
    paths
        .into_iter()
        .map(|path| {
            let media_type = image_media_type(&path)?;
            let data = std::fs::read(path).ok()?;
            Some((media_type.to_string(), data))
        })
        .collect()
}

/// Terminal emulators normally send file drops as bracketed paste, but some send
/// the path as ordinary key events. Promote a complete image-path-only composer
/// value before command/skill routing so an absolute `/...` path is never treated
/// as a slash command.
pub(super) fn promote_dropped_images(app: &mut App) -> bool {
    let Some(images) = dropped_image_files(&app.input) else {
        return false;
    };
    let count = images.len();
    app.input.clear();
    app.cursor_pos = 0;
    for (media_type, data) in images {
        attach_image(
            app,
            media_type,
            base64::engine::general_purpose::STANDARD.encode(data),
        );
    }
    app.set_status_notice(format!(
        "Dropped {count} image{}",
        if count == 1 { "" } else { "s" }
    ));
    true
}

fn parse_dropped_paths(text: &str) -> Option<Vec<PathBuf>> {
    let trimmed = text.trim();
    let literal_path = PathBuf::from(trimmed);
    if literal_path.is_file() {
        return Some(vec![literal_path]);
    }

    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in trimmed.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                token.push(ch);
            }
        } else if ch.is_whitespace() && quote.is_none() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(ch);
        }
    }
    if escaped || quote.is_some() {
        return None;
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    if tokens.is_empty() {
        return None;
    }

    tokens
        .into_iter()
        .map(|token| {
            let path = if token.starts_with("file://") {
                url::Url::parse(&token).ok()?.to_file_path().ok()?
            } else {
                PathBuf::from(token)
            };
            path.is_file().then_some(path)
        })
        .collect()
}

pub(super) fn handle_text_paste(app: &mut App, text: String) {
    crate::logging::info(&format!(
        "Text paste: {} chars, {} lines",
        text.len(),
        text.lines().count()
    ));

    let line_count = text.lines().count().max(1);
    if line_count < 5 {
        insert_input_text(app, &text);
    } else {
        app.pasted_contents.push(text);
        let placeholder = format!(
            "[pasted {} line{}]",
            line_count,
            if line_count == 1 { "" } else { "s" }
        );
        insert_input_text(app, &placeholder);
    }
}

impl App {
    pub(in crate::tui::app) fn handle_clipboard_paste_completed(
        &mut self,
        result: ClipboardPasteCompleted,
    ) -> bool {
        if self.active_client_session_id() != Some(result.session_id.as_str()) {
            return false;
        }

        match result.content {
            ClipboardPasteContent::Image {
                media_type,
                base64_data,
            } => {
                attach_image(self, media_type, base64_data);
                true
            }
            ClipboardPasteContent::Text(text) => {
                handle_text_paste(self, text);
                true
            }
            ClipboardPasteContent::Empty => {
                match result.kind {
                    ClipboardPasteKind::Smart => {
                        self.set_status_notice("No text or image in clipboard");
                    }
                    ClipboardPasteKind::ImageOnly => {
                        self.set_status_notice("No image in clipboard")
                    }
                    ClipboardPasteKind::ImageUrl { fallback_text } => {
                        if let Some(text) = fallback_text {
                            handle_text_paste(self, text);
                        } else {
                            self.set_status_notice("Failed to download image");
                        }
                    }
                }
                true
            }
            ClipboardPasteContent::Error(message) => {
                if let ClipboardPasteKind::ImageUrl {
                    fallback_text: Some(text),
                } = result.kind
                {
                    self.set_status_notice(message);
                    handle_text_paste(self, text);
                } else {
                    self.set_status_notice(message);
                }
                true
            }
        }
    }
}

pub(super) fn insert_input_text(app: &mut App, text: &str) {
    if text.is_empty() {
        return;
    }

    // Composer insertions should behave the same in local and remote modes.
    // Unless the user explicitly enabled typing scroll lock, beginning or
    // continuing a draft resumes tail-follow before the input height can change.
    // Keeping this at the shared insertion boundary also covers paste and
    // Shift+Enter, rather than relying on individual key dispatchers to remember
    // to reconcile the transcript viewport.
    app.follow_chat_bottom_for_typing();

    let at_end = app.cursor_pos == app.input.len();

    // A habitual space typed after an auto-inserted picker separator would
    // only add noise. Swallow it so command + space + filter still produces
    // a single separator.
    if text == " " && at_end && matches!(app.input.trim_start(), "/login " | "/model " | "/models ")
    {
        return;
    }

    app.remember_input_undo_state();

    // After a picker command is fully typed (or completed without a trailing
    // space), the next printable character starts its filter. Insert the
    // separator instead of extending the command token and closing the picker.
    if at_end
        && matches!(app.input.trim_start(), "/login" | "/model" | "/models")
        && !text.starts_with(char::is_whitespace)
    {
        app.input.push(' ');
        app.cursor_pos = app.input.len();
    }

    app.input.insert_str(app.cursor_pos, text);
    app.cursor_pos += text.len();

    // Typing the final command character immediately arms picker filtering.
    // Without this, users can keep typing the command token or press Enter
    // without realizing the visible picker is ready to filter.
    if app.cursor_pos == app.input.len()
        && matches!(app.input.trim_start(), "/login" | "/model" | "/models")
    {
        app.input.push(' ');
        app.cursor_pos = app.input.len();
    }

    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
}

pub(super) fn handle_text_input(app: &mut App, text: &str) -> bool {
    if text.is_empty() {
        return false;
    }

    let onboarding_suggestions = matches!(
        app.onboarding_phase(),
        Some(crate::tui::app::onboarding_flow::OnboardingPhase::Suggestions)
    );
    if app.input.is_empty()
        && !app.is_processing
        && (app.display_messages.is_empty() || onboarding_suggestions)
    {
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && let Some(digit) = c.to_digit(10)
        {
            let suggestions = app.suggestion_prompts();
            let idx = digit as usize;
            if idx >= 1 && idx <= suggestions.len() {
                let (_label, prompt) = &suggestions[idx - 1];
                if !prompt.starts_with('/') {
                    app.remember_input_undo_state();
                    app.input = prompt.clone();
                    app.cursor_pos = app.input.len();
                    app.follow_chat_bottom_for_typing();
                    app.submit_input();
                    return true;
                }
            }
        }
    }

    insert_input_text(app, text);
    promote_dropped_images(app);
    true
}

fn visible_prompt_history(app: &App) -> Vec<String> {
    app.display_messages
        .iter()
        .filter(|message| message.role == "user")
        .map(|message| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
        .collect()
}

fn byte_offset_for_line_column(
    input: &str,
    line_start: usize,
    line_end: usize,
    column: usize,
) -> usize {
    let mut offset = line_end;
    for (idx, (byte_offset, _)) in input[line_start..line_end].char_indices().enumerate() {
        if idx == column {
            offset = line_start + byte_offset;
            break;
        }
    }
    offset
}

pub(super) fn handle_multiline_input_navigation(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if !modifiers.is_empty()
        || !matches!(code, KeyCode::Up | KeyCode::Down)
        || !app.input.contains('\n')
    {
        return false;
    }

    let input = app.input.as_str();
    let cursor = app.cursor_pos.min(input.len());
    let line_start = input[..cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let line_end = input[cursor..]
        .find('\n')
        .map(|idx| cursor + idx)
        .unwrap_or(input.len());
    let column = input[line_start..cursor].chars().count();

    let target = match code {
        KeyCode::Up => {
            if line_start == 0 {
                return false;
            }
            let previous_line_end = line_start - 1;
            let previous_line_start = input[..previous_line_end]
                .rfind('\n')
                .map(|idx| idx + 1)
                .unwrap_or(0);
            byte_offset_for_line_column(input, previous_line_start, previous_line_end, column)
        }
        KeyCode::Down => {
            if line_end >= input.len() {
                return false;
            }
            let next_line_start = line_end + 1;
            let next_line_end = input[next_line_start..]
                .find('\n')
                .map(|idx| next_line_start + idx)
                .unwrap_or(input.len());
            byte_offset_for_line_column(input, next_line_start, next_line_end, column)
        }
        _ => return false,
    };

    app.cursor_pos = target;
    true
}

/// True when `modifiers` is exactly one of Ctrl, Alt(Option) or Cmd(Super),
/// the set of single modifiers we treat as "recall queued prompts / browse
/// history" when combined with Up/Down. Shift or any combination is excluded so
/// it doesn't shadow selection-extension or other chords.
pub(super) fn is_prompt_recall_modifier(modifiers: KeyModifiers) -> bool {
    matches!(
        modifiers,
        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER
    )
}

pub(super) fn handle_prompt_history_navigation(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let explicit_history = modifiers == KeyModifiers::CONTROL;
    if !(modifiers.is_empty() || explicit_history) || !matches!(code, KeyCode::Up | KeyCode::Down) {
        return false;
    }

    let history = visible_prompt_history(app);
    if history.is_empty() {
        return false;
    }

    let target = if app.input.is_empty() {
        match code {
            KeyCode::Up => Some(history.len() - 1),
            KeyCode::Down => None,
            _ => None,
        }
    } else {
        let Some(current_index) = history.iter().rposition(|prompt| prompt == &app.input) else {
            if explicit_history && matches!(code, KeyCode::Up) {
                return history
                    .last()
                    .map(|prompt| {
                        app.input = prompt.clone();
                        app.cursor_pos = app.input.len();
                        app.reset_tab_completion();
                        app.sync_model_picker_preview_from_input();
                    })
                    .is_some();
            }
            return false;
        };
        match code {
            KeyCode::Up => Some(current_index.saturating_sub(1)),
            KeyCode::Down if current_index + 1 < history.len() => Some(current_index + 1),
            KeyCode::Down => {
                app.input.clear();
                app.cursor_pos = 0;
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
                return true;
            }
            _ => None,
        }
    };

    let Some(target) = target else {
        return false;
    };
    let Some(prompt) = history.get(target) else {
        return false;
    };
    app.input = prompt.clone();
    app.cursor_pos = app.input.len();
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
    true
}

fn associated_text_for_key_event(_event: &KeyEvent) -> Option<String> {
    // Future hook: prefer terminal-provided associated text when crossterm exposes it.
    // Today crossterm does not surface this on KeyEvent even though the kitty protocol
    // defines a REPORT_ASSOCIATED_TEXT flag.
    None
}

pub(super) fn text_input_for_key_event(event: &KeyEvent) -> Option<String> {
    associated_text_for_key_event(event)
        .filter(|text| !text.is_empty())
        .or_else(|| text_input_for_key(event.code, event.modifiers))
}

pub(super) fn text_input_for_key(code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
    let KeyCode::Char(c) = code else {
        return None;
    };

    if !modifiers_allow_printable_text(c, modifiers) {
        return None;
    }

    Some(shifted_printable_fallback(c, modifiers).to_string())
}

fn modifiers_allow_printable_text(c: char, modifiers: KeyModifiers) -> bool {
    if modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META) {
        return false;
    }

    let has_control = modifiers.contains(KeyModifiers::CONTROL);
    let has_alt = modifiers.contains(KeyModifiers::ALT);
    match (has_control, has_alt) {
        (false, false) => true,
        // Some terminals report AltGr/layout-generated symbols as Ctrl+Alt plus the final
        // printable character. Preserve that character when it cannot be confused with normal
        // Ctrl/Alt letter shortcuts. If the terminal only reports the physical base key, we still
        // refuse to synthesize a layout-specific character we cannot know.
        (_, true) => is_layout_modified_text_char(c),
        (true, false) => false,
    }
}

fn is_layout_modified_text_char(c: char) -> bool {
    !c.is_control() && c != ' ' && !c.is_ascii_alphanumeric()
}

fn shifted_printable_fallback(c: char, modifiers: KeyModifiers) -> char {
    if modifiers.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
        return c.to_ascii_uppercase();
    }

    c
}

pub(super) fn clear_input_for_escape(app: &mut App) {
    let had_input = !app.input.is_empty();
    if had_input {
        app.remember_input_undo_state();
    }
    app.input.clear();
    app.cursor_pos = 0;
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
    if had_input {
        app.set_status_notice("Input cleared - Ctrl+Z to restore");
    }
}

pub(super) fn expand_paste_placeholders(app: &mut App, input: &str) -> String {
    let mut result = input.to_string();
    for content in app.pasted_contents.iter().rev() {
        let placeholder = paste_placeholder(content);
        if let Some(pos) = result.rfind(&placeholder) {
            result.replace_range(pos..pos + placeholder.len(), content);
        }
    }
    result
}

pub(super) fn queue_message(app: &mut App) {
    let prepared = take_prepared_input(app);
    app.queued_messages.push(prepared.expanded);
}

pub(super) fn retrieve_pending_message_for_edit(app: &mut App) -> bool {
    if !app.input.is_empty() {
        return false;
    }

    let mut parts: Vec<String> = Vec::new();
    let mut had_pending = false;

    if !app.pending_soft_interrupts.is_empty() {
        parts.extend(std::mem::take(&mut app.pending_soft_interrupts));
        app.pending_soft_interrupt_requests.clear();
        had_pending = true;
    }
    if let Some(msg) = app.interleave_message.take()
        && !msg.is_empty()
    {
        parts.push(msg);
        had_pending = true;
    }
    if !app.queued_messages.is_empty() {
        parts.extend(std::mem::take(&mut app.queued_messages));
        if !app.has_queued_followups() {
            app.pending_queued_dispatch = false;
        }
        had_pending = true;
    }

    if !parts.is_empty() {
        app.input = parts.join("\n\n");
        app.cursor_pos = app.input.len();
        let count = parts.len();
        app.set_status_notice(format!(
            "Retrieved {} pending message{} for editing",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    had_pending
}

pub(super) fn send_action(app: &App, alternate_shortcut: bool) -> SendAction {
    if !app.is_processing {
        return SendAction::Submit;
    }
    if app.input.trim().starts_with('/') || app.input.trim().starts_with('!') {
        return SendAction::Submit;
    }
    if alternate_shortcut {
        if app.queue_mode {
            SendAction::Interleave
        } else {
            SendAction::Queue
        }
    } else if app.queue_mode {
        SendAction::Queue
    } else {
        SendAction::Interleave
    }
}

pub(super) fn handle_shift_enter(app: &mut App) {
    insert_input_text(app, "\n");
}

impl App {
    pub(super) fn has_queued_followups(&self) -> bool {
        self.interleave_message.is_some()
            || !self.queued_messages.is_empty()
            || !self.hidden_queued_system_messages.is_empty()
    }

    /// True when a startup submission is staged and ready to auto-send.
    ///
    /// Headed spawns (and reloads with a resume prompt) stage their initial
    /// prompt into `self.input` and set `submit_input_on_startup`, rather than
    /// pushing onto `queued_messages`. The post-connect dispatcher must treat
    /// this as pending work so the prompt is actually submitted once the remote
    /// session history loads. See issues #267/#268/#76.
    pub(super) fn has_pending_startup_submission(&self) -> bool {
        self.submit_input_on_startup
            && (!self.input.trim().is_empty() || !self.pending_images.is_empty())
    }

    /// Folds this turn's guardrail-stop flag into the consecutive counter.
    /// Call once per finished turn, before scheduling automatic follow-ups.
    /// Returns true when automatic continuation (auto-poke/overnight poke)
    /// must stop because the provider keeps refusing: a guardrail refusal is
    /// deterministic for the same request, so re-poking loops forever
    /// (observed live as one refused API call per auto-poke, every ~7s).
    pub(super) fn guardrail_stops_exhausted_at_turn_end(&mut self) -> bool {
        let stopped = std::mem::take(&mut self.turn_guardrail_stopped);
        if !stopped {
            self.consecutive_guardrail_stops = 0;
            return false;
        }
        self.consecutive_guardrail_stops = self.consecutive_guardrail_stops.saturating_add(1);
        self.consecutive_guardrail_stops >= Self::GUARDRAIL_STOP_MAX_CONSECUTIVE
    }

    /// Disarm auto-poke and overnight poke after repeated guardrail refusals
    /// and tell the user why, instead of silently re-sending the refused
    /// request on every turn end.
    pub(super) fn stop_auto_continuation_after_guardrail(&mut self) {
        let had_overnight = self.overnight_auto_poke.take().is_some();
        if !self.auto_poke_incomplete_todos && !had_overnight {
            return;
        }
        let cleared = super::commands::disable_auto_poke(self);
        crate::logging::warn(&format!(
            "Stopping auto-poke after {} consecutive provider guardrail stops (cleared {} queued poke message(s), overnight={})",
            self.consecutive_guardrail_stops, cleared, had_overnight
        ));
        self.push_display_message(DisplayMessage::system(format!(
            "🛑 Auto-poke stopped: the provider guardrail refused {} turns in a row. Re-poking the same request will keep getting refused. Rephrase or narrow the task, then run /poke again to resume.",
            self.consecutive_guardrail_stops
        )));
        self.set_status_notice("Poke stopped: provider guardrail");
    }

    /// Turn-end entry point for automatic continuations. Applies the
    /// guardrail circuit breaker first, then tries auto-poke and overnight
    /// poke scheduling. Returns true when a follow-up was queued.
    pub(super) fn schedule_turn_end_followups(&mut self) -> bool {
        if self.guardrail_stops_exhausted_at_turn_end() {
            self.stop_auto_continuation_after_guardrail();
            return false;
        }
        self.schedule_auto_poke_followup_if_needed()
            || self.schedule_overnight_poke_followup_if_needed()
    }

    pub(super) fn schedule_auto_poke_followup_if_needed(&mut self) -> bool {
        if !self.auto_poke_incomplete_todos
            || self.pending_queued_dispatch
            || self.pending_turn
            || self.has_queued_followups()
        {
            return false;
        }

        let todos = super::commands::poke_todos(self);
        let incomplete: Vec<_> = todos
            .iter()
            .filter(|todo| super::commands::is_incomplete_poke_todo(todo))
            .cloned()
            .collect();
        if incomplete.is_empty() {
            if todos.is_empty() {
                self.auto_poke_incomplete_todos = false;
                return false;
            }
            let confidence_summary = super::commands::todo_confidence_summary(&todos);
            let confidence_label =
                super::commands::format_todo_completion_confidence(confidence_summary);
            let needs_spike_challenge = confidence_summary.confidence_spike_detected
                && !self.todo_confidence_spike_challenged;
            let gate_budget_left =
                self.todo_completion_gate_attempts < Self::TODO_COMPLETION_GATE_MAX_ATTEMPTS;
            if (confidence_summary.completion_confidence_needs_validation || needs_spike_challenge)
                && gate_budget_left
            {
                self.todo_completion_gate_attempts =
                    self.todo_completion_gate_attempts.saturating_add(1);
                let notice = if confidence_summary.completion_confidence_needs_validation {
                    crate::telemetry::record_todo_gate(crate::telemetry::TodoGateKind::Completion);
                    "🛑 Todo completion gate: completion confidence needs stronger validation."
                } else {
                    self.todo_confidence_spike_challenged = true;
                    crate::telemetry::record_todo_gate(
                        crate::telemetry::TodoGateKind::ConfidenceSpike,
                    );
                    "🛑 Todo completion gate: abrupt confidence increase needs independent validation."
                };
                self.push_display_message(DisplayMessage::system(notice));
                self.hidden_queued_system_messages.push(
                    super::commands::build_todo_confidence_summary_message(&todos),
                );
                self.pending_queued_dispatch = true;
                return true;
            }
            if (confidence_summary.completion_confidence_needs_validation || needs_spike_challenge)
                && !gate_budget_left
            {
                // The gate keeps failing but the model is no longer making
                // progress on it. Nudging again would loop forever, burning an
                // API call per turn (observed live: an unattended session
                // resent the same continuation every ~5s). Stop the cycle and
                // surface the stall instead.
                crate::logging::warn(&format!(
                    "Todo completion gate exhausted after {} attempts; stopping auto-poke to avoid an infinite continuation loop",
                    self.todo_completion_gate_attempts
                ));
                self.push_display_message(DisplayMessage::system(
                    "⚠️ Todo completion gate: validation still failing after repeated nudges. Auto-poke stopped; review the remaining todos manually.",
                ));
                self.auto_poke_incomplete_todos = false;
                self.todo_confidence_spike_challenged = false;
                self.todo_completion_gate_attempts = 0;
                self.pending_queued_dispatch = false;
                return false;
            }
            self.auto_poke_incomplete_todos = false;
            self.todo_confidence_spike_challenged = false;
            self.todo_completion_gate_attempts = 0;
            self.push_display_message(DisplayMessage::system(format!(
                "✅ Todos complete. Completion confidence: {}.",
                confidence_label
            )));
            self.pending_queued_dispatch = false;
            return false;
        }

        self.push_display_message(DisplayMessage::system(format!(
            "👉 Auto-poking: {} incomplete todo{}. /poke off to stop.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
        )));
        // Open todos mean the model is still iterating; completion-gate
        // exhaustion should only trip when the gate itself stops moving.
        self.todo_completion_gate_attempts = 0;
        self.queued_messages
            .push(super::commands::build_poke_message(&incomplete));
        self.pending_queued_dispatch = true;
        true
    }

    pub(super) fn schedule_queued_dispatch_after_interrupt(&mut self) {
        if self.has_queued_followups() {
            self.pending_queued_dispatch = true;
        }
    }

    pub(crate) fn toggle_next_prompt_new_session_routing(&mut self) {
        self.route_next_prompt_to_new_session = !self.route_next_prompt_to_new_session;
        if self.route_next_prompt_to_new_session {
            self.set_status_notice("Next prompt → new session");
        } else {
            self.set_status_notice("Next-prompt new session canceled");
        }
    }

    /// Whether the configured `keybindings.new_terminal` chord matches this key.
    pub(crate) fn new_terminal_key_matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.new_terminal_key
            .binding
            .as_ref()
            .map(|binding| binding.matches(code, modifiers))
            .unwrap_or(false)
    }

    /// Whether the configured `keybindings.open_resume` chord matches this key.
    pub(crate) fn open_resume_key_matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.open_resume_key
            .binding
            .as_ref()
            .map(|binding| binding.matches(code, modifiers))
            .unwrap_or(false)
    }

    /// Whether the configured `keybindings.fallback_switch` chord matches this key.
    pub(crate) fn fallback_switch_key_matches(
        &self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        self.fallback_switch_key
            .binding
            .as_ref()
            .map(|binding| binding.matches(code, modifiers))
            .unwrap_or(false)
    }

    /// Spawn a brand-new jcode session in a new terminal window.
    pub(crate) fn handle_new_terminal_hotkey(&mut self) {
        let cwd = commands::active_working_dir(self)
            .filter(|path| path.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        match super::spawn_fresh_session_in_new_terminal(&cwd) {
            Ok(true) => self.set_status_notice("↗ New terminal opened"),
            Ok(false) => {
                self.set_status_notice("No supported terminal found; run `jcode` manually")
            }
            Err(error) => self.set_status_notice(format!("New terminal failed: {}", error)),
        }
    }
}

pub(super) fn is_next_prompt_new_session_hotkey(code: KeyCode, modifiers: KeyModifiers) -> bool {
    if code != KeyCode::Char(' ') {
        return false;
    }
    // Accept either Command/Super+Space (macOS Cmd, often eaten by Spotlight) or
    // Option/Alt+Space (macOS Option) so the fork-to-new-session arming hotkey is
    // reachable across terminals. Reject Ctrl/Hyper combos so other chords still
    // route to their own handlers.
    let has_super = modifiers.contains(KeyModifiers::SUPER);
    let has_alt = modifiers.contains(KeyModifiers::ALT);
    (has_super || has_alt) && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::HYPER)
}

fn input_routes_to_new_session(app: &App) -> bool {
    if !app.route_next_prompt_to_new_session || app.input.is_empty() {
        return false;
    }
    let trimmed = app.input.trim_start();
    !trimmed.starts_with('/') && extract_input_shell_command(trimmed).is_none()
}

fn route_prompt_to_new_session_local(app: &mut App) -> bool {
    if !input_routes_to_new_session(app) {
        return false;
    }

    app.route_next_prompt_to_new_session = false;
    let prepared = take_prepared_input(app);
    let restored_raw = prepared.raw_input.clone();
    let restored_images = prepared.images.clone();
    match commands::launch_prompt_in_new_session_local(app, prepared.expanded, prepared.images) {
        Ok(_) => true,
        Err(error) => {
            app.input = restored_raw;
            app.cursor_pos = app.input.len();
            app.pending_images = restored_images;
            app.set_status_notice("Prompt launch failed");
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to launch prompt in a new session: {}",
                error
            )));
            true
        }
    }
}

pub(super) fn handle_alternate_enter(app: &mut App) {
    if app.activate_picker_from_preview() {
        return;
    }

    if app.input.is_empty() {
        return;
    }

    if route_prompt_to_new_session_local(app) {
        return;
    }

    match send_action(app, true) {
        SendAction::Submit => app.submit_input(),
        SendAction::Queue => queue_message(app),
        SendAction::Interleave => {
            let prepared = take_prepared_input(app);
            stage_local_interleave(app, prepared.expanded);
        }
    }
}

pub(super) fn handle_control_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('u') => {
            delete_input_to_start(app);
            true
        }
        KeyCode::Char('k') => {
            delete_input_to_end(app);
            true
        }
        KeyCode::Char('z') => {
            app.undo_input_change();
            true
        }
        KeyCode::Char('x') => {
            cut_input_line_to_clipboard(app);
            true
        }
        KeyCode::Char('a') => {
            app.cursor_pos = 0;
            true
        }
        KeyCode::Char('e') => {
            app.cursor_pos = app.input.len();
            true
        }
        KeyCode::Char('b') => {
            if app.cursor_pos > 0 {
                app.cursor_pos = app.find_word_boundary_back();
            }
            true
        }
        KeyCode::Char('f') => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = app.find_word_boundary_forward();
            }
            true
        }
        KeyCode::Char('w') | KeyCode::Char('\u{8}') | KeyCode::Backspace => {
            delete_input_word_back(app);
            true
        }
        KeyCode::Char('s') => {
            app.toggle_input_stash();
            true
        }
        KeyCode::Char('p') => {
            super::commands::toggle_auto_poke_hotkey_local(app);
            true
        }
        KeyCode::Char('v') => {
            paste_from_clipboard(app);
            true
        }
        KeyCode::Tab | KeyCode::Char('t') => {
            app.queue_mode = !app.queue_mode;
            let mode_str = if app.queue_mode {
                "Queue mode: messages wait until response completes"
            } else {
                "Immediate mode: messages send next (no interrupt)"
            };
            app.set_status_notice(mode_str);
            true
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = app.find_word_boundary_back();
            }
            true
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = app.find_word_boundary_forward();
            }
            true
        }
        KeyCode::Up => {
            retrieve_pending_message_for_edit(app);
            true
        }
        _ => false,
    }
}

pub(super) fn delete_input_to_start(app: &mut App) {
    if app.cursor_pos > 0 {
        app.remember_input_undo_state();
    }
    app.input.drain(..app.cursor_pos);
    app.cursor_pos = 0;
    app.sync_model_picker_preview_from_input();
}

pub(super) fn delete_input_to_end(app: &mut App) {
    if app.cursor_pos < app.input.len() {
        app.remember_input_undo_state();
    }
    app.input.truncate(app.cursor_pos);
    app.sync_model_picker_preview_from_input();
}

pub(super) fn handle_super_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        // Cmd+5 toggles the onboarding simulator (a dev aid for walking through
        // every first-run onboarding screen without touching real auth state).
        KeyCode::Char('5') => {
            app.toggle_onboarding_simulator();
            true
        }
        // macOS terminals that forward Command may report Command+Delete as Super+Backspace,
        // Super+Delete, or Super+DEL. Treat all of them as delete-the-previous-word, matching
        // the requested Cmd+Backspace = delete-by-word behavior.
        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('\u{7f}') => {
            delete_input_word_back(app);
            true
        }
        KeyCode::Left | KeyCode::Home | KeyCode::Char('a') => {
            app.cursor_pos = 0;
            true
        }
        KeyCode::Right | KeyCode::End | KeyCode::Char('e') => {
            app.cursor_pos = app.input.len();
            true
        }
        KeyCode::Char('z') => {
            app.undo_input_change();
            true
        }
        KeyCode::Char('x') => {
            cut_input_line_to_clipboard(app);
            true
        }
        KeyCode::Char('v') => {
            paste_from_clipboard(app);
            true
        }
        _ => false,
    }
}

pub(super) fn delete_input_word_back(app: &mut App) {
    let start = app.find_word_boundary_back();
    if start < app.cursor_pos {
        app.remember_input_undo_state();
    }
    app.input.drain(start..app.cursor_pos);
    app.cursor_pos = start;
    app.sync_model_picker_preview_from_input();
}

pub(super) fn handle_alt_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        // Alt/Option+Left/Right move by word, matching Alt+B / Alt+F.
        KeyCode::Left | KeyCode::Char('b') => {
            app.cursor_pos = app.find_word_boundary_back();
            true
        }
        KeyCode::Right | KeyCode::Char('f') => {
            app.cursor_pos = app.find_word_boundary_forward();
            true
        }
        KeyCode::Char('d') => {
            let end = app.find_word_boundary_forward();
            if app.cursor_pos < end {
                app.remember_input_undo_state();
            }
            app.input.drain(app.cursor_pos..end);
            app.sync_model_picker_preview_from_input();
            true
        }
        // macOS terminals vary between Backspace, Delete, and DEL for Option+Delete.
        // Keep all aliases on word-delete-back so the documented Alt/Option+Backspace works.
        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('\u{7f}') => {
            delete_input_word_back(app);
            true
        }
        KeyCode::Char('v') => {
            paste_from_clipboard(app);
            true
        }
        KeyCode::Char('a') if app.input.is_empty() => {
            app.copy_chat_viewport_context_to_clipboard();
            true
        }
        _ => false,
    }
}

pub(super) fn handle_navigation_shortcuts(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if let Some(amount) = app.scroll_keys.scroll_amount(code, modifiers) {
        if amount < 0 {
            app.scroll_up((-amount) as usize);
        } else {
            app.scroll_down(amount as usize);
        }
        return true;
    }

    if let Some(dir) = app.scroll_keys.prompt_jump(code, modifiers) {
        if dir < 0 {
            app.scroll_to_prev_prompt();
        } else {
            app.scroll_to_next_prompt();
        }
        return true;
    }

    if let Some(ratio) = App::ctrl_side_panel_ratio_preset(&code, modifiers) {
        app.set_side_panel_ratio_preset(ratio);
        return true;
    }

    if let Some(rank) = App::ctrl_prompt_rank(&code, modifiers) {
        app.scroll_to_recent_prompt_rank(rank);
        return true;
    }

    if app.scroll_keys.is_bookmark(code, modifiers) {
        app.toggle_scroll_bookmark();
        return true;
    }

    if app.toggle_keys.diff_mode_cycle.matches(code, modifiers) {
        app.diff_mode = app.diff_mode.cycle();
        if !app.diff_pane_visible() {
            app.diff_pane_focus = false;
        }
        let status = format!("Diffs: {}", app.diff_mode.label());
        app.set_status_notice(&status);
        return true;
    }

    false
}

pub(super) fn is_scroll_only_key(app: &App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let mut code = code;
    let mut modifiers = modifiers;
    ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

    if app.scroll_keys.scroll_amount(code, modifiers).is_some()
        || app.scroll_keys.prompt_jump(code, modifiers).is_some()
        || App::ctrl_side_panel_ratio_preset(&code, modifiers).is_some()
        || App::ctrl_prompt_rank(&code, modifiers).is_some()
        || app.scroll_keys.is_bookmark(code, modifiers)
        || (modifiers.contains(KeyModifiers::ALT)
            && matches!(code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'g')))
    {
        return true;
    }

    if app.diff_pane_focus && !modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('j')
            | KeyCode::Down
            | KeyCode::Char('k')
            | KeyCode::Up
            | KeyCode::Char('d')
            | KeyCode::PageDown
            | KeyCode::Char('u')
            | KeyCode::PageUp
            | KeyCode::Char('g')
            | KeyCode::Home
            | KeyCode::Char('G')
            | KeyCode::End
            | KeyCode::Char('+')
            | KeyCode::Char('=')
            | KeyCode::Char('-')
            | KeyCode::Char('0')
            | KeyCode::Esc => return true,
            _ => {}
        }
    }

    let diagram_available = app.diagram_available();
    if diagram_available && app.diagram_focus && !modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('h')
            | KeyCode::Left
            | KeyCode::Char('l')
            | KeyCode::Right
            | KeyCode::Char('k')
            | KeyCode::Up
            | KeyCode::Char('j')
            | KeyCode::Down
            | KeyCode::Char('+')
            | KeyCode::Char('=')
            | KeyCode::Char('-')
            | KeyCode::Char('_')
            | KeyCode::Char(']')
            | KeyCode::Char('[')
            | KeyCode::Char('o')
            | KeyCode::Esc => return true,
            _ => {}
        }
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        if diagram_available {
            match code {
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                    return true;
                }
                _ => {}
            }
        }
        if app.diff_pane_visible() {
            match code {
                KeyCode::Char('h') | KeyCode::Char('l') => return true,
                _ => {}
            }
        }
    }

    false
}

pub(super) fn handle_pre_control_shortcuts(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    // Plain Ctrl+K kills to end of line (emacs habit). Ctrl+Shift+K must fall
    // through to the scroll handler: with the Kitty keyboard protocol enabled,
    // terminals report Ctrl+Shift+K as Char('k') + CONTROL|SHIFT, so without the
    // Shift guard this would swallow the scroll-up chord and wipe the draft.
    if modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::SHIFT)
        && matches!(code, KeyCode::Char('k'))
        && !app.input.is_empty()
    {
        delete_input_to_end(app);
        return true;
    }

    if is_clipboard_paste_shortcut(code, modifiers) {
        paste_from_clipboard(app);
        return true;
    }

    let macos_option_shortcut =
        crate::tui::keybind::shortcut_char_for_macos_option_key(code, modifiers);
    if app.toggle_keys.copy_selection.matches(code, modifiers) {
        app.toggle_copy_selection_mode();
        return true;
    }

    if handle_visible_copy_shortcut(app, code, modifiers) {
        return true;
    }

    if app.toggle_keys.side_panel.matches(code, modifiers) {
        app.toggle_side_panel();
        return true;
    }
    if app.toggle_keys.diagram_pane.matches(code, modifiers) {
        app.toggle_diagram_pane_position();
        return true;
    }
    if app.toggle_keys.typing_scroll_lock.matches(code, modifiers) {
        app.toggle_typing_scroll_lock();
        return true;
    }
    if app.toggle_keys.info_widget.matches(code, modifiers) {
        crate::tui::info_widget::toggle_enabled();
        let status = if crate::tui::info_widget::is_enabled() {
            "Info widget: ON"
        } else {
            "Info widget: OFF"
        };
        app.set_status_notice(status);
        return true;
    }
    if app.toggle_keys.todo_card.matches(code, modifiers) {
        app.toggle_todo_card();
        return true;
    }
    if app.dictation_key_matches(code, modifiers) {
        app.handle_dictation_trigger();
        return true;
    }

    // Swarm views: Alt+N cycles chat → inline controls → full live page → chat.
    // Selection/open/prompt controls stay available in both active views, while
    // plain typing continues to flow to the chat input.
    if app.toggle_keys.swarm_panel_focus.matches(code, modifiers) {
        match app.cycle_swarm_panel_view() {
            super::tui_state::SwarmPanelView::Chat => {
                app.set_status_notice("Swarm view closed");
            }
            super::tui_state::SwarmPanelView::Controls => {
                app.set_status_notice("Swarm: alt+n full page · alt+↑/↓ select · alt+o open · esc");
            }
            super::tui_state::SwarmPanelView::FullPage => {
                app.set_status_notice("Swarm page: alt+n chat · alt+↑/↓ select · alt+o open · esc");
            }
        }
        return true;
    }
    {
        use crate::tui::TuiState as _;
        if app.swarm_panel_focused() && app.handle_swarm_panel_key(code, modifiers) {
            return true;
        }
    }
    if app.new_terminal_key_matches(code, modifiers) {
        app.handle_new_terminal_hotkey();
        return true;
    }
    if app.open_resume_key_matches(code, modifiers) {
        app.record_keybinding_fast(super::shortcut_hints::LearnableAction::Resume);
        app.open_session_picker();
        return true;
    }
    if let Some(direction) = app.model_switch_keys.direction_for(code, modifiers) {
        app.record_keybinding_fast(super::shortcut_hints::LearnableAction::ModelSwitch);
        app.cycle_model(direction);
        return true;
    }
    if let Some(direction) = app.effort_switch_keys.direction_for(code, modifiers) {
        app.record_keybinding_fast(super::shortcut_hints::LearnableAction::EffortCycle);
        app.cycle_effort(direction);
        return true;
    }
    if cfg!(target_os = "macos")
        && !matches!(app.status, ProcessingStatus::RunningTool(_))
        && let Some(direction) = app
            .effort_switch_keys
            .macos_option_arrow_escape_direction_for(code, modifiers)
    {
        app.cycle_effort(direction);
        return true;
    }
    if app.centered_toggle_keys.matches(code, modifiers) {
        app.record_keybinding_fast(super::shortcut_hints::LearnableAction::Alignment);
        app.toggle_centered_mode();
        return true;
    }

    app.normalize_diagram_state();
    let diagram_available = app.diagram_available();
    if app.handle_diagram_focus_key(code, modifiers, diagram_available) {
        return true;
    }
    if app.handle_diff_pane_focus_key(code, modifiers) {
        return true;
    }
    if modifiers.contains(KeyModifiers::ALT) && handle_alt_key(app, code) {
        return true;
    }
    if let Some(shortcut) = macos_option_shortcut
        && handle_alt_key(app, KeyCode::Char(shortcut))
    {
        return true;
    }
    if modifiers.contains(KeyModifiers::SUPER) && handle_super_key(app, code) {
        return true;
    }

    handle_navigation_shortcuts(app, code, modifiers)
}

pub(super) fn handle_visible_copy_shortcut(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let Some(c) = visible_copy_shortcut_key(code, modifiers) else {
        return false;
    };

    // Many terminals encode Alt+Shift+<letter> as just Alt + uppercase letter
    // instead of reporting an explicit Shift modifier. Accept either form so the
    // on-screen [Alt] [⇧] copy badges behave consistently.
    let explicit_shift = modifiers.contains(KeyModifiers::SHIFT);
    let implicit_shift = c.is_ascii_uppercase();
    let macos_option_shift =
        crate::tui::keybind::shortcut_char_for_macos_option_shift_key(code, modifiers).is_some();
    if !explicit_shift && !implicit_shift && !macos_option_shift {
        // Some terminals report Alt+Shift+E as Alt+lowercase `e` with no
        // explicit SHIFT modifier. Keep the relaxed fallback scoped to the
        // expand badge so plain Alt+letter copy shortcuts do not become active.
        if c.eq_ignore_ascii_case(&'e') && handle_expand_edit_badge_shortcut(app, c) {
            return true;
        }
        return false;
    }

    if handle_expand_edit_badge_shortcut(app, c) {
        return true;
    }

    if handle_inline_image_toggle_shortcut(app, c) {
        return true;
    }

    if let Some(target) = crate::tui::ui::recent_flicker_copy_target_for_key(c)
        .or_else(|| crate::tui::ui::visible_copy_target_for_key(c))
    {
        let success = super::copy_to_clipboard(&target.content);
        app.record_copy_badge_key_press(c);
        app.record_copy_badge_feedback(c, success);
        if success {
            app.set_status_notice(target.copied_notice);
        } else {
            app.set_status_notice(format!("Failed to copy {}", target.kind_label));
        }
        return true;
    }

    false
}

fn visible_copy_shortcut_key(code: KeyCode, modifiers: KeyModifiers) -> Option<char> {
    if let Some(key) =
        crate::tui::keybind::shortcut_char_for_macos_option_shift_key(code, modifiers)
    {
        return Some(key);
    }

    let KeyCode::Char(c) = code else {
        return None;
    };

    modifiers.contains(KeyModifiers::ALT).then_some(c)
}

/// Alt+Shift+I toggles inline transcript images between expanded and
/// collapsed label stubs. Only active when the transcript actually has
/// inline images, so the chord stays inert otherwise.
fn handle_inline_image_toggle_shortcut(app: &mut App, key: char) -> bool {
    if !key.eq_ignore_ascii_case(&'i') {
        return false;
    }
    use crate::tui::TuiState as _;
    if app.side_pane_images_signature().0 == 0 {
        return false;
    }
    app.record_copy_badge_key_press('i');
    app.toggle_inline_images();
    true
}

fn handle_expand_edit_badge_shortcut(app: &mut App, key: char) -> bool {
    if !key.eq_ignore_ascii_case(&'e') {
        return false;
    }

    let visible_expand_badge = crate::tui::ui::visible_expand_edit_badge();
    let has_edit_tool_message = app.display_edit_tool_message_count > 0
        || app.display_messages.iter().any(|message| {
            message
                .tool_data
                .as_ref()
                .map(|tool| crate::tui::ui::tools_ui::is_edit_tool_name(&tool.name))
                .unwrap_or(false)
        });

    // The inline edit badge is rendered from the inline diff mode itself, while
    // opening it from other diff modes requires at least one edit tool message.
    // Keep this predicate in one place so the [Alt] [⇧] [E] badge uses the same
    // shortcut path as visible copy badges without falling through to copy key E.
    if !visible_expand_badge && !app.diff_mode.is_inline() && !has_edit_tool_message {
        return false;
    }

    if app.diff_mode.is_full_inline() {
        return false;
    }

    app.diff_mode = crate::config::DiffDisplayMode::FullInline;
    app.record_copy_badge_key_press('e');
    app.copy_badge_ui.expand_feedback_until =
        Some(std::time::Instant::now() + std::time::Duration::from_millis(1100));
    app.copy_badge_ui.expand_feedback_line = crate::tui::ui::visible_expand_edit_badge_line();
    app.set_status_notice(format!(
        "Expanded edit diffs · Diffs: {}",
        app.diff_mode.label()
    ));
    true
}

pub(super) fn handle_modal_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<bool> {
    if app.changelog_scroll.is_some() {
        app.handle_changelog_key(code)?;
        return Ok(true);
    }

    if app.help_scroll.is_some() {
        app.handle_help_key(code)?;
        return Ok(true);
    }

    if app.model_status_scroll.is_some() {
        app.handle_model_status_key(code)?;
        return Ok(true);
    }

    if app.session_picker_overlay.is_some() {
        app.handle_session_picker_key(code, modifiers)?;
        return Ok(true);
    }

    if app.login_picker_overlay.is_some() {
        app.handle_login_picker_key(code, modifiers)?;
        return Ok(true);
    }

    if app.account_picker_overlay.is_some() {
        if let Some(command) = app.next_account_picker_action(code, modifiers)? {
            app.handle_account_picker_command(command);
        }
        return Ok(true);
    }

    if app.copy_selection_mode {
        if modifiers.contains(KeyModifiers::CONTROL)
            && matches!(code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            // Ctrl+C over an active selection is universal copy muscle
            // memory. Falling through here used to reach the global handler,
            // which quits when idle, so trying to copy an error message
            // closed jcode and lost the error (issue #497). Only fall
            // through (interrupt/quit) when nothing is selected.
            if code == KeyCode::Char('c')
                && app
                    .current_copy_selection_text()
                    .is_some_and(|text| !text.is_empty())
            {
                app.copy_current_selection_to_clipboard();
                return Ok(true);
            }
            return Ok(false);
        }

        let _ = app.handle_copy_selection_key(code, modifiers)
            || handle_navigation_shortcuts(app, code, modifiers);
        return Ok(true);
    }

    if let Some(ref picker) = app.inline_interactive_state
        && !picker.preview
    {
        app.handle_inline_interactive_key(code, modifiers)?;
        return Ok(true);
    }

    if app.handle_inline_interactive_preview_key(&code, modifiers)? {
        return Ok(true);
    }

    Ok(false)
}

pub(super) fn handle_global_control_shortcuts(
    app: &mut App,
    code: KeyCode,
    diagram_available: bool,
) -> bool {
    if app.handle_diagram_ctrl_key(code, diagram_available) {
        return true;
    }

    match code {
        KeyCode::Char('c') | KeyCode::Char('d') => {
            if app.is_processing {
                app.cancel_requested = true;
                app.interleave_message = None;
                app.pending_soft_interrupts.clear();
                app.pending_soft_interrupt_requests.clear();
                if app.cancel_overnight_for_interrupt() {
                    app.set_status_notice("Interrupting... Overnight cancelled");
                } else {
                    app.set_status_notice("Interrupting...");
                }
            } else {
                app.handle_quit_request();
            }
            true
        }
        KeyCode::Char('r') => {
            app.recover_session_without_tools();
            true
        }
        KeyCode::Char('a') if app.input.is_empty() => {
            app.copy_chat_viewport_context_to_clipboard();
            true
        }
        KeyCode::Char('l') => true,
        _ => handle_control_key(app, code),
    }
}

pub(super) fn handle_enter(app: &mut App) -> bool {
    promote_dropped_images(app);
    if app.activate_picker_from_preview() {
        return true;
    }
    if !app.input.is_empty() {
        if route_prompt_to_new_session_local(app) {
            return true;
        }
        match send_action(app, false) {
            SendAction::Submit => app.submit_input(),
            SendAction::Queue => queue_message(app),
            SendAction::Interleave => {
                let prepared = take_prepared_input(app);
                stage_local_interleave(app, prepared.expanded);
            }
        }
    }
    true
}

pub(super) fn handle_basic_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char(c) => handle_text_input(app, &c.to_string()),
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                let prev = crate::tui::core::prev_char_boundary(&app.input, app.cursor_pos);
                app.remember_input_undo_state();
                app.input.drain(prev..app.cursor_pos);
                app.cursor_pos = prev;
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
            true
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                let next = crate::tui::core::next_char_boundary(&app.input, app.cursor_pos);
                app.remember_input_undo_state();
                app.input.drain(app.cursor_pos..next);
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
            true
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = crate::tui::core::prev_char_boundary(&app.input, app.cursor_pos);
            } else {
                // Opt-in: Left on an empty input opens the active sessions
                // manager (no-op unless display.active_sessions_manager).
                app.maybe_open_active_sessions_on_left();
            }
            true
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = crate::tui::core::next_char_boundary(&app.input, app.cursor_pos);
            }
            true
        }
        KeyCode::Home => {
            app.cursor_pos = 0;
            true
        }
        KeyCode::End => {
            app.cursor_pos = app.input.len();
            true
        }
        KeyCode::Tab => {
            app.autocomplete();
            true
        }
        KeyCode::Up | KeyCode::PageUp => {
            let inc = if code == KeyCode::PageUp { 10 } else { 1 };
            app.scroll_up(inc);
            true
        }
        KeyCode::Down | KeyCode::PageDown => {
            let dec = if code == KeyCode::PageDown { 10 } else { 1 };
            app.scroll_down(dec);
            true
        }
        KeyCode::Esc => {
            if app
                .inline_interactive_state
                .as_ref()
                .map(|p| p.preview)
                .unwrap_or(false)
            {
                app.inline_interactive_state = None;
                app.inline_view_state = None;
                clear_input_for_escape(app);
            } else if app.inline_view_state.is_some() {
                app.inline_view_state = None;
                clear_input_for_escape(app);
            } else if app.is_processing {
                let disabled_auto_poke = app.auto_poke_incomplete_todos
                    || app
                        .queued_messages
                        .iter()
                        .any(|message| super::commands::is_poke_message(message));
                app.cancel_requested = true;
                app.interleave_message = None;
                app.pending_soft_interrupts.clear();
                app.pending_soft_interrupt_requests.clear();
                let cancelled_overnight = app.cancel_overnight_for_interrupt();
                if disabled_auto_poke {
                    super::commands::disable_auto_poke(app);
                    if cancelled_overnight {
                        app.set_status_notice("Interrupting... Auto-poke OFF, overnight cancelled");
                    } else {
                        app.set_status_notice("Interrupting... Auto-poke OFF");
                    }
                } else if cancelled_overnight {
                    app.set_status_notice("Interrupting... Overnight cancelled");
                } else {
                    app.set_status_notice("Interrupting...");
                }
            } else {
                app.follow_chat_bottom();
                clear_input_for_escape(app);
            }
            true
        }
        _ => false,
    }
}

pub(super) fn take_prepared_input(app: &mut App) -> PreparedInput {
    let raw_input = std::mem::take(&mut app.input);
    let expanded = expand_paste_placeholders(app, &raw_input);
    app.pasted_contents.clear();
    let images = std::mem::take(&mut app.pending_images);
    app.cursor_pos = 0;
    app.clear_input_undo_history();
    PreparedInput {
        raw_input,
        expanded,
        images,
    }
}

pub(super) fn stage_local_interleave(app: &mut App, content: String) {
    app.interleave_message = Some(content);
    app.set_status_notice("⏭ Sending now (interleave)");
}

fn attach_image(app: &mut App, media_type: String, base64_data: String) {
    let size_kb = base64_data.len() / 1024;
    app.pending_images.push((media_type.clone(), base64_data));
    let placeholder = format!("[image {}]", app.pending_images.len());
    app.remember_input_undo_state();
    app.input.insert_str(app.cursor_pos, &placeholder);
    app.cursor_pos += placeholder.len();
    app.sync_model_picker_preview_from_input();
    app.set_status_notice(format!("Pasted {} ({} KB)", media_type, size_kb));
}

fn paste_placeholder(content: &str) -> String {
    let line_count = content.lines().count().max(1);
    format!(
        "[pasted {} line{}]",
        line_count,
        if line_count == 1 { "" } else { "s" }
    )
}

impl App {
    pub(super) fn handle_key_event(&mut self, event: crossterm::event::KeyEvent) {
        // Record the event if recording is active
        use crate::tui::test_harness::{TestEvent, record_event};
        let modifiers: Vec<String> = {
            let mut mods = vec![];
            if event.modifiers.contains(KeyModifiers::CONTROL) {
                mods.push("ctrl".to_string());
            }
            if event.modifiers.contains(KeyModifiers::ALT) {
                mods.push("alt".to_string());
            }
            if event.modifiers.contains(KeyModifiers::SHIFT) {
                mods.push("shift".to_string());
            }
            mods
        };
        let code_str = format!("{:?}", event.code);
        record_event(TestEvent::Key {
            code: code_str,
            modifiers,
        });

        self.update_copy_badge_key_event(event);
        if matches!(
            event.kind,
            crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat
        ) {
            let _ = self.handle_key_press_event(event);
        }
    }

    pub(super) fn handle_key_press_event(&mut self, event: KeyEvent) -> Result<()> {
        self.handle_key_core(
            event.code,
            event.modifiers,
            text_input_for_key_event(&event),
        )
    }

    #[cfg(test)]
    pub(super) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        self.handle_key_core(code, modifiers, None)
    }

    fn handle_key_core(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        text_input: Option<String>,
    ) -> Result<()> {
        let mut code = code;
        let mut modifiers = modifiers;
        ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

        // Alt+5 always starts the onboarding simulator from a pristine first
        // screen, even when another modal or a previous sim screen is active.
        if self.handle_onboarding_sim_reset_shortcut(code, modifiers) {
            return Ok(());
        }

        // The onboarding simulator owns all key handling while active so the
        // real onboarding handlers and simulated modal overlays never fire (no
        // real logins/imports or action selection).
        if self.handle_onboarding_sim_key(code, modifiers) {
            return Ok(());
        }

        if handle_modal_key(self, code, modifiers)? {
            return Ok(());
        }

        if self.handle_onboarding_continue_prompt_key(code) {
            return Ok(());
        }

        // Inline hotkey feedback: when a known-but-rarely-used chord is pressed,
        // show "you just pressed X → does Y". Placed after the modal/overlay
        // handlers so overlay-local keys stay silent. Unknown chords are
        // reported at the fall-through points below.
        self.observe_known_hotkey(code, modifiers, false);

        if code == KeyCode::BackTab {
            self.cycle_model_favorite_hotkey();
            return Ok(());
        }

        // While the model picker preview is visible, route its favorite/default
        // hotkeys (Ctrl+O set default, Ctrl+N toggle favorite) to the focused
        // picker handler before the global control shortcuts can claim them. This
        // makes the hotkeys work directly in the preview list the user always
        // sees, without colliding with the readline/tmux keys (Ctrl+B/Ctrl+F).
        if self.model_picker_preview_hotkey(code, modifiers)? {
            return Ok(());
        }

        if self.pending_provider_failover.is_some() && !self.is_processing {
            if code == KeyCode::Esc {
                self.cancel_pending_provider_failover("Provider auto-switch canceled");
                return Ok(());
            }
            if !is_scroll_only_key(self, code, modifiers) {
                self.cancel_pending_provider_failover("Provider auto-switch canceled");
            }
        }

        // Accept an armed post-error fallback offer: switch to the next best
        // model/auth-method and resend the failed turn.
        if self.pending_fallback_offer.is_some()
            && !self.is_processing
            && self.fallback_switch_key_matches(code, modifiers)
        {
            self.apply_pending_fallback_offer();
            return Ok(());
        }

        // Accept an armed "merge the diverged update" offer: spawn a jcode agent
        // to reconcile the branches. Shares the fallback-switch accept key.
        if self.merge_offer_key_matches(code, modifiers) {
            self.accept_update_merge_offer();
            return Ok(());
        }

        if is_next_prompt_new_session_hotkey(code, modifiers) {
            self.toggle_next_prompt_new_session_routing();
            return Ok(());
        }

        if self.handle_command_suggestion_key(code, modifiers) {
            return Ok(());
        }

        if handle_pre_control_shortcuts(self, code, modifiers) {
            return Ok(());
        }

        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();

        // Ctrl / Alt(Option) / Cmd(Super) + Up all recall queued/pending messages
        // for editing and then walk prompt history. We accept any of the three
        // single modifiers so the gesture works regardless of which one a given
        // terminal forwards (some send Option as Alt, some forward Command as
        // Super), without the user having to rebind anything.
        if code == KeyCode::Up && is_prompt_recall_modifier(modifiers) {
            if retrieve_pending_message_for_edit(self) {
                return Ok(());
            }
            // Normalize to CONTROL so handle_prompt_history_navigation takes its
            // explicit-history path (jump straight into history even mid-draft).
            handle_prompt_history_navigation(self, KeyCode::Up, KeyModifiers::CONTROL);
            return Ok(());
        }

        if code == KeyCode::Down && is_prompt_recall_modifier(modifiers) {
            handle_prompt_history_navigation(self, KeyCode::Down, KeyModifiers::CONTROL);
            return Ok(());
        }

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL)
            && handle_global_control_shortcuts(self, code, diagram_available)
        {
            return Ok(());
        }

        // Ctrl+Enter / Cmd+Enter: does opposite of queue_mode during processing
        if code == KeyCode::Enter
            && modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            handle_alternate_enter(self);
            return Ok(());
        }

        // Shift+Enter and Alt/Option+Enter insert a newline in the input box.
        if code == KeyCode::Enter && modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) {
            handle_shift_enter(self);
            return Ok(());
        }

        // When the model picker preview is visible, arrow keys navigate the picker list
        if self
            .inline_interactive_state
            .as_ref()
            .map(|p| p.preview)
            .unwrap_or(false)
        {
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    return self.handle_inline_interactive_key(code, modifiers);
                }
                _ => {}
            }
        }

        if handle_multiline_input_navigation(self, code, modifiers)
            || handle_prompt_history_navigation(self, code, modifiers)
        {
            return Ok(());
        }

        if let Some(text) = text_input.or_else(|| text_input_for_key(code, modifiers)) {
            handle_text_input(self, &text);
            return Ok(());
        }

        // Never fall through and insert literal text for unhandled Ctrl+key chords. This stays
        // after text_input so Ctrl+Alt/AltGr symbols delivered as final printable text still work.
        if modifiers.contains(KeyModifiers::CONTROL) {
            self.note_unrecognized_hotkey(code, modifiers, false);
            return Ok(());
        }

        if code == KeyCode::Enter {
            // Stray post-paste Enter from some terminals is not a submit (#544).
            if paste_guard::consume_paste_trailing_enter() {
                return Ok(());
            }
            // During the onboarding model-selection phase, Enter on an empty
            // prompt opens the model picker instead of submitting nothing.
            if self.input.trim().is_empty()
                && matches!(
                    self.onboarding_phase(),
                    Some(crate::tui::app::onboarding_flow::OnboardingPhase::ModelSelect)
                )
            {
                self.open_model_picker();
                return Ok(());
            }
            handle_enter(self);
            return Ok(());
        }

        // A modified chord (or function key) that reached this point is not
        // bound to anything; tell the user instead of silently swallowing it or
        // inserting a surprise character.
        self.note_unrecognized_hotkey(code, modifiers, false);

        if handle_basic_key(self, code) {
            return Ok(());
        }

        Ok(())
    }

    pub(super) fn request_full_redraw(&mut self) {
        self.force_full_redraw = true;
    }

    /// Arm a full re-emit of every cell on the next frame without an
    /// intermediate ED2 clear escape. Prefer this over `request_full_redraw`
    /// when the real screen has not diverged from ratatui's model (e.g. chat
    /// scrolling), so image placeholder cells do not flash blank (issue #404).
    pub(super) fn request_full_repaint(&mut self) {
        self.force_full_repaint = true;
    }

    const RESIZE_REDRAW_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

    fn commit_resize_redraw(&mut self, now: std::time::Instant) -> bool {
        self.last_resize_redraw = Some(now);
        self.resize_redraw_pending = false;
        self.handle_diagram_geometry_change();
        true
    }

    pub(super) fn should_redraw_after_resize(&mut self) -> bool {
        let now = std::time::Instant::now();
        match self.last_resize_redraw {
            Some(last) if now.duration_since(last) < Self::RESIZE_REDRAW_MIN_INTERVAL => {
                self.resize_redraw_pending = true;
                false
            }
            _ => self.commit_resize_redraw(now),
        }
    }

    /// Flush the trailing edge of a debounced resize burst. Without this, the
    /// last resize event can be suppressed after an intermediate frame, leaving
    /// width/height-sensitive Mermaid placeholders and image state stale until
    /// some unrelated UI event happens to redraw the terminal.
    pub(super) fn flush_pending_resize_redraw(&mut self) -> bool {
        if !self.resize_redraw_pending {
            return false;
        }
        let now = std::time::Instant::now();
        match self.last_resize_redraw {
            Some(last) if now.duration_since(last) < Self::RESIZE_REDRAW_MIN_INTERVAL => false,
            _ => self.commit_resize_redraw(now),
        }
    }

    pub(super) fn update_copy_badge_key_event(&mut self, event: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind, ModifierKeyCode};

        self.prune_copy_badge_ui();
        let pulse_until = std::time::Instant::now() + std::time::Duration::from_millis(240);

        match (event.kind, event.code) {
            (KeyEventKind::Press | KeyEventKind::Repeat, KeyCode::Modifier(modifier)) => {
                match modifier {
                    ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => {
                        self.copy_badge_ui.alt_active = true;
                        self.copy_badge_ui.alt_pulse_until = Some(pulse_until);
                    }
                    ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => {
                        self.copy_badge_ui.shift_active = true;
                        self.copy_badge_ui.shift_pulse_until = Some(pulse_until);
                    }
                    _ => {}
                }
            }
            (KeyEventKind::Release, KeyCode::Modifier(modifier)) => match modifier {
                ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => {
                    self.copy_badge_ui.alt_active = false;
                }
                ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => {
                    self.copy_badge_ui.shift_active = false;
                }
                _ => {}
            },
            (KeyEventKind::Press | KeyEventKind::Repeat, KeyCode::Char(c)) => {
                if event.modifiers.contains(KeyModifiers::ALT) {
                    self.copy_badge_ui.alt_pulse_until = Some(pulse_until);
                }
                if event.modifiers.contains(KeyModifiers::SHIFT) || c.is_ascii_uppercase() {
                    self.copy_badge_ui.shift_pulse_until = Some(pulse_until);
                }
                self.record_copy_badge_key_press(c);
            }
            (KeyEventKind::Release, KeyCode::Char(c)) => {
                if let Some((active, _)) = self.copy_badge_ui.key_active
                    && active.eq_ignore_ascii_case(&c)
                {
                    self.copy_badge_ui.key_active = None;
                }
                if !event.modifiers.contains(KeyModifiers::ALT) {
                    self.copy_badge_ui.alt_active = false;
                }
                if !event.modifiers.contains(KeyModifiers::SHIFT) {
                    self.copy_badge_ui.shift_active = false;
                }
            }
            _ => {}
        }
    }

    pub(super) fn record_copy_badge_key_press(&mut self, key: char) {
        let expiry = std::time::Instant::now() + std::time::Duration::from_millis(240);
        self.copy_badge_ui.key_active = Some((key, expiry));
    }

    pub(super) fn record_copy_badge_feedback(&mut self, key: char, success: bool) {
        self.copy_badge_ui.copied_feedback = Some(crate::tui::app::CopyBadgeFeedback {
            key,
            success,
            expires_at: std::time::Instant::now() + std::time::Duration::from_millis(1100),
        });
    }

    pub(super) fn prune_copy_badge_ui(&mut self) {
        let now = std::time::Instant::now();
        if self
            .copy_badge_ui
            .alt_pulse_until
            .map(|expires_at| expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.alt_pulse_until = None;
        }
        if self
            .copy_badge_ui
            .shift_pulse_until
            .map(|expires_at| expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.shift_pulse_until = None;
        }
        if self
            .copy_badge_ui
            .key_active
            .as_ref()
            .map(|(_, expires_at)| *expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.key_active = None;
        }
        if self
            .copy_badge_ui
            .copied_feedback
            .as_ref()
            .map(|feedback| feedback.expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.copied_feedback = None;
        }
        if self
            .copy_badge_ui
            .expand_feedback_until
            .map(|expires_at| expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.expand_feedback_until = None;
            self.copy_badge_ui.expand_feedback_line = None;
        }
    }

    /// Try to paste whatever is in the clipboard.
    /// Prefers text when available, otherwise falls back to image data.
    /// Used by Ctrl+V handlers in both local and remote mode.
    pub(super) fn paste_from_clipboard(&mut self) {
        paste_from_clipboard(self);
    }

    /// Queue a message to be sent later
    /// Handle bracketed paste: store text content (image URLs are still detected inline)
    pub(super) fn handle_paste(&mut self, text: String) {
        handle_paste(self, text);
    }

    /// Expand paste placeholders in input with actual content
    pub(super) fn expand_paste_placeholders(&mut self, input: &str) -> String {
        expand_paste_placeholders(self, input)
    }

    pub(super) fn queue_message(&mut self) {
        queue_message(self);
    }

    /// Send an interleave message immediately to the server as a soft interrupt.
    /// Skips the intermediate buffer stage - goes directly to pending_soft_interrupts.
    pub(super) async fn send_interleave_now(
        &mut self,
        content: String,
        remote: &mut crate::tui::backend::RemoteConnection,
    ) {
        remote::send_interleave_now(self, content, remote).await;
    }

    /// Retrieve all pending unsent messages into the input for editing.
    /// Priority: pending soft interrupts first, then interleave, then queued.
    /// Returns true if pending soft interrupts were retrieved (caller should cancel on server).
    pub(super) fn retrieve_pending_message_for_edit(&mut self) -> bool {
        retrieve_pending_message_for_edit(self)
    }

    pub(super) fn send_action(&self, shift: bool) -> SendAction {
        send_action(self, shift)
    }

    pub(super) fn insert_thought_line(&mut self, line: String) {
        if self.thought_line_inserted || line.is_empty() {
            return;
        }
        self.thought_line_inserted = true;
        let mut prefix = line;
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
        prefix.push('\n');
        if self.streaming.streaming_text.is_empty() {
            self.replace_streaming_text(prefix);
        } else {
            self.replace_streaming_text(format!("{}{}", prefix, self.streaming.streaming_text));
        }
    }

    /// Begin a reasoning region. Reasoning renders as dim, italic text (no
    /// blockquote gutter, no header, no footer). Idempotent while open.
    pub(super) fn open_reasoning_region(&mut self) {
        if self.reasoning_streaming {
            return;
        }
        // Separate the reasoning block from any prior content with a blank line.
        if !self.streaming.streaming_text.is_empty() {
            if self.streaming.streaming_text.ends_with("\n\n") {
                // already separated
            } else if self.streaming.streaming_text.ends_with('\n') {
                self.append_streaming_text("\n");
            } else {
                self.append_streaming_text("\n\n");
            }
        }
        self.reasoning_streaming = true;
        self.reasoning_pending_line.clear();
        self.reasoning_partial_len = 0;
        // Remember where this reasoning block starts in the stream so `current`
        // mode can later slice it back out in place (without disturbing any
        // preceding answer text) once the model starts answering.
        self.reasoning_block_start = Some(self.streaming.streaming_text.len());
    }

    /// Remove the live partial-reasoning tail (the rendered, not-yet-committed
    /// in-progress line) from the streaming buffer so it can be rebuilt. No-op
    /// when there is no live partial.
    fn strip_reasoning_partial_tail(&mut self) {
        if self.reasoning_partial_len > 0 {
            let new_len = self
                .streaming
                .streaming_text
                .len()
                .saturating_sub(self.reasoning_partial_len);
            self.streaming.streaming_text.truncate(new_len);
            self.reasoning_partial_len = 0;
        }
    }

    /// Append streamed reasoning text, rendering the in-progress line live so
    /// reasoning trickles in token-by-token (like normal output) rather than one
    /// whole line at a time. Complete lines (terminated by `\n`) are committed as
    /// dim+italic markdown; the trailing partial line is rendered as a live tail
    /// that is re-emitted in place on each delta. The whole-line emphasis run is
    /// preserved (each line is its own `*…*`) so styling never breaks mid-line.
    pub(super) fn append_reasoning_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.reasoning_streaming {
            self.open_reasoning_region();
        }
        // Drop the previous live tail; we rebuild committed lines + a fresh tail.
        self.strip_reasoning_partial_tail();
        let mut committed = String::new();
        for ch in text.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.reasoning_pending_line);
                committed.push_str(&jcode_tui_markdown::reasoning_line_markup(&line));
            } else {
                self.reasoning_pending_line.push(ch);
            }
        }
        if !committed.is_empty() {
            self.streaming.streaming_text.push_str(&committed);
        }
        // Re-append the live tail for the in-progress (partial) line.
        let partial = jcode_tui_markdown::reasoning_partial_markup(&self.reasoning_pending_line);
        self.reasoning_partial_len = partial.len();
        self.streaming.streaming_text.push_str(&partial);
        self.refresh_split_view_if_needed();
    }

    /// Promote the live partial line to a committed line and end the region. The
    /// `_footer` argument is ignored (the "Thought for Xs" footer was removed);
    /// it is kept for call-site compatibility.
    pub(super) fn close_reasoning_region(&mut self, _footer: Option<String>) {
        if !self.reasoning_streaming {
            return;
        }
        // Replace the live tail with the committed (newline-terminated) line.
        self.strip_reasoning_partial_tail();
        let pending = std::mem::take(&mut self.reasoning_pending_line);
        if !pending.is_empty() {
            self.streaming
                .streaming_text
                .push_str(&jcode_tui_markdown::reasoning_line_markup(&pending));
        }
        self.reasoning_streaming = false;

        // In `current` mode, reasoning is ephemeral: it is never written to the
        // persistent transcript. The closed block is sliced out of the live
        // stream and anchored *in place* as a display-only reasoning message in
        // the transcript flow: it never moves again (no bottom-following, no
        // hoisting), stays readable for the rest of the turn, and is removed
        // when the next user prompt starts a new turn.
        if self.reasoning_current_mode() {
            self.anchor_current_reasoning_block();
            return;
        }

        // Terminate the reasoning block with a blank line so following output
        // renders as a normal paragraph.
        if !self.streaming.streaming_text.ends_with("\n\n") {
            if self.streaming.streaming_text.ends_with('\n') {
                self.streaming.streaming_text.push('\n');
            } else {
                self.streaming.streaming_text.push_str("\n\n");
            }
        }
        self.refresh_split_view_if_needed();
    }

    /// True when the active reasoning-display mode is `current` (live-only,
    /// ephemeral reasoning).
    pub(super) fn reasoning_current_mode(&self) -> bool {
        matches!(
            crate::config::config().display.reasoning_display(),
            crate::config::ReasoningDisplayMode::Current
        )
    }

    /// Slice the just-closed reasoning block out of `streaming_text` and anchor
    /// it as a display-only reasoning message in the transcript flow, exactly
    /// where it streamed. Used in `current` mode: the trace keeps its position
    /// (content below it can only be appended, never inserted above), so the
    /// thought stays readable and anchored until the next user prompt removes
    /// the turn's traces.
    pub(super) fn anchor_current_reasoning_block(&mut self) {
        let block_start = self
            .reasoning_block_start
            .take()
            .unwrap_or(0)
            .min(self.streaming.streaming_text.len());
        // Everything from the block start onward is the reasoning markup. Split it
        // off so the preceding answer text (if any) stays in the live stream.
        let block = self.streaming.streaming_text.split_off(block_start);
        // Drop the separator the open path added before the reasoning block so the
        // surrounding answer text rejoins cleanly.
        while self.streaming.streaming_text.ends_with('\n') {
            self.streaming.streaming_text.pop();
        }
        let block = block.trim_matches('\n').to_string();
        if block.is_empty() {
            self.refresh_split_view_if_needed();
            return;
        }
        // Answer text that streamed *before* the block must commit first so the
        // anchored trace lands after it in the transcript (chronological order).
        if !self.streaming.streaming_text.trim().is_empty() {
            let preceding = self.take_streaming_text();
            let preceding = self.collapse_reasoning_for_commit(preceding);
            if !preceding.trim().is_empty() {
                self.push_display_message(DisplayMessage::assistant(preceding));
            }
        }
        self.turn_reasoning_traces
            .push(crate::tui::app::TurnReasoningTrace {
                display_index: self.display_messages.len(),
                // Snapshot the transcript height when this trace anchors. The trace
                // begins life at the viewport tail; once the transcript grows a
                // full viewport beyond this point the trace is provably off-screen
                // (while tail-following) and can be GC'd without visible motion.
                wrapped_lines_at_anchor: crate::tui::ui::last_total_wrapped_lines(),
            });
        self.push_display_message(DisplayMessage::reasoning(block));
        self.refresh_split_view_if_needed();
    }

    /// Remove the current turn's anchored reasoning traces from the transcript.
    /// Called when the next user prompt is submitted so `current` mode stays
    /// ephemeral across turns: the trace never moves while on screen, it is
    /// simply gone the next time the user acts (a moment when the transcript
    /// reflows anyway).
    pub(super) fn clear_turn_reasoning_traces(&mut self) {
        if self.turn_reasoning_traces.is_empty() {
            return;
        }
        let traces = std::mem::take(&mut self.turn_reasoning_traces);
        let removed = self.remove_reasoning_trace_messages(traces.iter().map(|t| t.display_index));
        if removed > 0 {
            self.bump_display_messages_version();
            self.refresh_split_view_if_needed();
        }
    }

    /// Garbage-collect *stale* reasoning traces (every anchored trace except
    /// the most recent one) that are provably above the tail-following
    /// viewport, so their removal causes zero visible motion. Keeps `current`
    /// mode meaning "the current thought": old thoughts dissolve once they
    /// scroll out of view instead of accumulating across a long agentic turn.
    /// Skipped entirely while the user has scrolled up (their reading position
    /// must not shift).
    pub(super) fn gc_offscreen_reasoning_traces(&mut self) -> bool {
        // Only the traces *before* the most recent one are stale.
        if self.turn_reasoning_traces.len() < 2 {
            return false;
        }
        if self.auto_scroll_paused {
            // User is reading history; never remove anything they might see.
            return false;
        }
        let total = crate::tui::ui::last_total_wrapped_lines();
        let viewport = crate::tui::ui::last_layout_snapshot()
            .map(|l| l.messages_area.height as usize)
            .unwrap_or(0);
        if total == 0 || viewport == 0 {
            return false;
        }
        // A trace anchored when the transcript was `at_anchor` lines tall sits
        // entirely above wrapped line `at_anchor`. While tail-following, the
        // viewport shows the last `viewport` lines, so once the transcript has
        // grown a full viewport past the anchor point (with margin for the
        // separator blank line), the trace cannot be on screen.
        let last = self.turn_reasoning_traces.len() - 1;
        let stale: Vec<usize> = self.turn_reasoning_traces[..last]
            .iter()
            .filter(|t| total.saturating_sub(t.wrapped_lines_at_anchor) > viewport + 2)
            .map(|t| t.display_index)
            .collect();
        if stale.is_empty() {
            return false;
        }
        let removed = self.remove_reasoning_trace_messages(stale.iter().copied());
        if removed > 0 {
            // Re-track surviving traces with adjusted display indices.
            self.turn_reasoning_traces.retain_mut(|t| {
                if stale.contains(&t.display_index) {
                    return false;
                }
                let shift = stale.iter().filter(|&&s| s < t.display_index).count();
                t.display_index -= shift;
                true
            });
            self.bump_display_messages_version();
            self.refresh_split_view_if_needed();
            return true;
        }
        false
    }

    /// Remove reasoning display messages at the given (pre-removal) indices.
    /// Returns how many were removed.
    fn remove_reasoning_trace_messages(&mut self, indices: impl Iterator<Item = usize>) -> usize {
        let mut sorted: Vec<usize> = indices.collect();
        sorted.sort_unstable();
        let mut removed = 0usize;
        for idx in sorted {
            let idx = idx.saturating_sub(removed);
            if idx < self.display_messages.len() && self.display_messages[idx].role == "reasoning" {
                self.display_messages.remove(idx);
                removed += 1;
            }
        }
        removed
    }

    pub(super) fn append_streaming_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // Invariant: answer text is never appended *into* an open reasoning
        // region. If a region is still open when real (non-whitespace) answer
        // text arrives, close it first so the next `open_reasoning_region` still
        // inserts its blank-line separator. Without this, a stale
        // `reasoning_streaming` flag makes `open_reasoning_region` early-return
        // and the answer tail gets glued directly onto the next reasoning run
        // (e.g. `...patch + build.Ah, I see...`). Whitespace-only appends (the
        // separators emitted by the reasoning helpers themselves) never trip
        // this. `open_reasoning_region` only appends its separator *before*
        // setting the flag, so this cannot recurse.
        if self.reasoning_streaming && !text.trim().is_empty() {
            self.close_reasoning_region(None);
        }
        self.streaming.streaming_text.push_str(text);
        self.refresh_split_view_if_needed();
    }

    /// Apply a batch of paced [`StreamOp`]s from the segment-aware
    /// [`StreamBuffer`](crate::tui::stream_buffer::StreamBuffer) to the live
    /// streaming view, preserving arrival order across answer text, reasoning
    /// text, and reasoning-region boundaries. Returns true when anything
    /// visible changed.
    pub(super) fn apply_stream_ops(
        &mut self,
        ops: Vec<crate::tui::stream_buffer::StreamOp>,
    ) -> bool {
        use crate::tui::stream_buffer::StreamOp;
        let mut changed = false;
        for op in ops {
            match op {
                StreamOp::Text(text) => {
                    if !text.is_empty() {
                        // `append_streaming_text` enforces the invariant that real
                        // answer text closes any still-open reasoning region first
                        // (so the region's blank-line separator is preserved). The
                        // buffer also queues an explicit CloseReasoning before
                        // non-whitespace text, so this is normally already closed.
                        self.append_streaming_text(&text);
                        changed = true;
                    }
                }
                StreamOp::Reasoning(text) => {
                    if !text.is_empty() {
                        self.append_reasoning_text(&text);
                        changed = true;
                    }
                }
                StreamOp::CloseReasoning => {
                    if self.reasoning_streaming {
                        self.close_reasoning_region(None);
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// In `current` reasoning display mode, reasoning is shown live but collapsed
    /// once the assistant commits a message or runs a tool. Strip any
    /// reasoning-marked lines (identified by [`REASONING_SENTINEL`]) from text
    /// about to be committed to the transcript. Other modes pass through.
    pub(super) fn collapse_reasoning_for_commit(&self, content: String) -> String {
        if !matches!(
            crate::config::config().display.reasoning_display(),
            crate::config::ReasoningDisplayMode::Current
        ) {
            return content;
        }
        strip_reasoning_lines(&content)
    }

    pub(super) fn replace_streaming_text(&mut self, text: String) {
        self.streaming.streaming_text = text;
        self.refresh_split_view_if_needed();
    }

    pub(super) fn clear_streaming_render_state(&mut self) {
        self.streaming.streaming_text.clear();
        self.stream_message_ended = false;
        self.deferred_stream_done_id = None;
        self.reasoning_streaming = false;
        self.reasoning_pending_line.clear();
        self.reasoning_partial_len = 0;
        // The stream (and any block offset into it) is gone.
        self.reasoning_block_start = None;
        self.refresh_split_view_if_needed();
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
    }

    /// Reset provider-reported usage that belongs to a transcript being fully
    /// discarded. Render-only resets intentionally preserve these counters,
    /// so full session clears must call this separately.
    pub(super) fn clear_live_usage_state(&mut self) {
        self.streaming.streaming_input_tokens = 0;
        self.streaming.streaming_output_tokens = 0;
        self.streaming.streaming_cache_read_tokens = None;
        self.streaming.streaming_cache_creation_tokens = None;
        self.streaming.streaming_context_stale = false;
        self.streaming.streaming_usage_call_reset_pending = false;
        self.kv_cache.current_api_usage_recorded = false;
    }

    /// Discard all client-side render state for the current streaming attempt:
    /// the live streaming buffer, in-progress tool calls, thinking-line state,
    /// and any assistant transcript messages that were already committed
    /// mid-attempt at tool-call boundaries.
    ///
    /// Used when the provider reports a `RetryRollback`: a transient transport
    /// fault interrupted the response mid-stream and the request is being
    /// replayed from the top, so everything from the aborted attempt must
    /// disappear or the replay would render duplicated output.
    pub(super) fn rollback_streaming_attempt(&mut self) {
        self.stream_buffer.clear();
        self.clear_streaming_render_state();
        self.streaming_tool_calls.clear();
        self.batch_progress = None;
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.thinking_start = None;
        // Assistant text committed to the transcript during this attempt (a
        // ToolStart boundary commits the pending streamed text) must also go;
        // the retry re-streams the entire response. `push_display_message`
        // counts the trailing run of assistant messages and resets on any
        // user/tool/system fence, so this removes exactly the current
        // attempt's committed segments and never touches earlier turns.
        let to_remove = self.attempt_committed_assistant_messages;
        for _ in 0..to_remove {
            if self
                .display_messages
                .last()
                .is_some_and(|m| m.role == "assistant")
            {
                let idx = self.display_messages.len() - 1;
                self.remove_display_message(idx);
            } else {
                break;
            }
        }
        self.attempt_committed_assistant_messages = 0;
    }

    pub(super) fn take_streaming_text(&mut self) -> String {
        let content = std::mem::take(&mut self.streaming.streaming_text);
        self.stream_message_ended = false;
        self.deferred_stream_done_id = None;
        self.reasoning_streaming = false;
        self.reasoning_pending_line.clear();
        self.reasoning_partial_len = 0;
        self.reasoning_block_start = None;
        self.refresh_split_view_if_needed();
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
        content
    }

    pub(super) fn commit_pending_streaming_assistant_message(&mut self) -> bool {
        let ops = self.stream_buffer.flush();
        self.apply_stream_ops(ops);
        // A commit is a hard message boundary: end any still-open reasoning
        // region so `current` mode retains/discards the trace correctly.
        if self.reasoning_streaming {
            self.close_reasoning_region(None);
        }

        if self.streaming.streaming_text.is_empty() {
            self.stream_buffer.clear();
            // Tool-only boundary (no answer text): keep the retained trace on
            // screen so the thought stays readable while the tool runs. It
            // folds when superseded by the next trace or at end of turn.
            //
            // The ephemeral mermaid preview slot mirrors the (now empty) live
            // buffer, so any surviving entry here is stale by definition. The
            // buffer can only become empty without the slot being cleared via
            // `replace_streaming_text` (remote TextReplace, debug snapshot
            // restore); `take_streaming_text` and `clear_streaming_render_state`
            // both clear it themselves.
            crate::tui::mermaid::clear_streaming_preview_diagram();
            return false;
        }

        // `take_streaming_text` also clears the streaming mermaid preview
        // slot, so the whitespace-only early return below cannot leak it.
        let content = self.take_streaming_text();
        let content = self.collapse_reasoning_for_commit(content);
        if content.trim().is_empty() {
            // Nothing left after collapsing reasoning-only content; same
            // tool-only situation as above, keep the trace readable.
            self.stream_buffer.clear();
            return false;
        }
        self.push_display_message(DisplayMessage::assistant(content));
        self.stream_buffer.clear();
        true
    }

    pub(super) fn accumulate_streaming_output_tokens(
        &mut self,
        output_tokens: u64,
        call_output_tokens_seen: &mut u64,
    ) {
        let delta = if output_tokens >= *call_output_tokens_seen {
            output_tokens - *call_output_tokens_seen
        } else {
            // Usage snapshots should be monotonic within one API call. If they are not,
            // treat this as a reset and count the full value once.
            output_tokens
        };
        if self.streaming.streaming_tps_collect_output {
            self.streaming.streaming_total_output_tokens += delta;
            if delta > 0 {
                self.snapshot_streaming_tps();
            }
        }
        *call_output_tokens_seen = output_tokens;
    }

    /// Submit input - just sets up message and flags, processing happens in next loop iteration
    pub(super) fn submit_input(&mut self) {
        promote_dropped_images(self);
        if self.activate_picker_from_preview() {
            return;
        }

        let raw_input = std::mem::take(&mut self.input);
        let mut input = self.expand_paste_placeholders(&raw_input);
        if let Some(notice) = input_exceeds_submit_limit(&input) {
            self.input = raw_input;
            self.cursor_pos = self.input.len();
            self.set_status_notice(notice.clone());
            self.push_display_message(DisplayMessage::system(notice));
            return;
        }
        self.pasted_contents.clear();
        self.cursor_pos = 0;
        self.clear_input_undo_history();
        self.follow_chat_bottom(); // Reset to bottom and resume auto-scroll on new input

        // If the previous assistant turn still has visible streamed text that has not yet been
        // committed into chat history, finalize it before inserting the next user turn.
        // Otherwise the new prompt can appear directly under the last tool call, and the final
        // assistant paragraph shows up later out of order.
        self.commit_pending_streaming_assistant_message();

        if let Some(pending) = self.pending_login.take() {
            self.handle_login_input(pending, input);
            return;
        }

        if let Some(pending) = self.pending_account_input.take() {
            self.handle_pending_account_input(pending, input);
            return;
        }

        if let Some(name) = self.pending_ssh_remote_name.take() {
            commands::handle_pending_ssh_remote_target(self, name, input);
            return;
        }

        let trimmed = input.trim();
        let handled = commands::handle_cancel_command(self, trimmed)
            || commands::handle_help_command(self, trimmed)
            || commands::handle_keys_command(self, trimmed)
            || commands::handle_ssh_command(self, trimmed)
            || commands::handle_session_command(self, trimmed)
            || commands::handle_dictation_command(self, trimmed)
            || commands::handle_config_command(self, trimmed)
            || commands::handle_log_command(self, trimmed)
            || commands::handle_diff_command(self, trimmed)
            || commands::handle_model_status_command(self, trimmed)
            || super::debug::handle_debug_command(self, trimmed)
            || super::model_context::handle_model_command(self, trimmed)
            || super::commands::handle_usage_command(self, trimmed)
            || super::productivity::handle_productivity_command(self, trimmed)
            || super::commands::handle_feedback_command(self, trimmed)
            || super::support::handle_support_command(self, trimmed)
            || super::state_ui::handle_info_command(self, trimmed)
            || super::auth::handle_auth_command(self, trimmed)
            || super::tui_lifecycle_runtime::handle_dev_command(self, trimmed);
        if handled {
            if trimmed.starts_with('/') {
                crate::telemetry::record_command_family(trimmed);
            }
            return;
        }

        if let Some(command) = extract_input_shell_command(&input) {
            self.push_display_message(DisplayMessage::user(raw_input));

            if command.is_empty() {
                self.push_display_message(DisplayMessage::system(
                    "Shell command cannot be empty after !.",
                ));
                self.set_status_notice("Shell command is empty");
                return;
            }

            if self.is_remote {
                self.push_display_message(DisplayMessage::system(
                    "Input-line ! shell commands are only available in a local jcode TUI session.",
                ));
                self.set_status_notice("Local shell unavailable in remote mode");
                return;
            }

            self.set_status_notice(format!(
                "Running local shell: {}",
                crate::util::truncate_str(command, 48)
            ));
            spawn_input_shell_command(
                self.session.id.clone(),
                command.to_string(),
                self.session.working_dir.clone(),
            );
            return;
        }

        // A terminal file drop is user input even when its absolute path starts
        // with `/`. Check the filesystem-aware drop parser before slash routing
        // so a real file can never collide with a skill name.
        let skill_invocation = parse_dropped_paths(&input)
            .is_none()
            .then(|| SkillRegistry::parse_invocation(&input))
            .flatten();

        // Check for skill invocation.
        if let Some(invocation) = skill_invocation {
            let skill_name = invocation.name.to_string();
            let trailing_prompt = invocation.prompt.map(str::to_string);
            let mut skill = self.current_skills_snapshot().get(&skill_name).cloned();

            // Remote/minimal TUI clients may start with an empty skill snapshot, and
            // daemon-side `skill_manage reload_all` can update a different process.
            // On a slash miss, synchronously refresh from the active session working
            // directory before reporting Unknown skill so project-local skills such
            // as .jcode/skills/optimization work immediately after reload/build.
            if skill.is_none() {
                self.refresh_skills_snapshot();
                skill = self.current_skills_snapshot().get(&skill_name).cloned();
            }

            if let Some(skill) = skill {
                self.active_skill = Some(skill_name.clone());
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Activated skill: {} - {}", skill.name, skill.description),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                if let Some(prompt) = trailing_prompt {
                    input = prompt;
                } else {
                    return;
                }
            } else {
                // Distinguish an endorsed-but-not-installed skill from a
                // typo: the skill list advertises endorsed skills, so a bare
                // "Unknown skill" for them reads like a bug (issue #445).
                let endorsed_hint = crate::skill::endorsed_skills()
                    .iter()
                    .find(|endorsed| endorsed.name == skill_name)
                    .map(|endorsed| match endorsed.install {
                        Some(install) => format!(
                            "Skill /{} is endorsed but not installed. Install it with `{}`, then run /skills or skill_manage reload_all.",
                            skill_name, install
                        ),
                        None => format!(
                            "Skill /{} is endorsed but not installed (source: {}). Install it into ~/.jcode/skills/{}/SKILL.md.",
                            skill_name, endorsed.source, skill_name
                        ),
                    });
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: endorsed_hint
                        .unwrap_or_else(|| format!("Unknown skill: /{}", skill_name)),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                return;
            }
        }

        // Leaving the preview should happen as soon as the user acts on it.
        self.onboarding_preview_mode = false;

        // Add user message to display (show placeholder to user, not full paste)
        // Remember the typed prompt so we can restore it to the input box if this
        // turn fails (e.g. "token refresh needed"), instead of dropping it.
        self.last_submitted_input = Some(raw_input.clone());
        self.push_display_message(DisplayMessage {
            role: "user".to_string(),
            content: raw_input, // Show placeholder to user (condensed view)
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        // Send expanded content (with actual pasted text) to model
        let images = std::mem::take(&mut self.pending_images);
        if !images.is_empty() {
            crate::logging::info(&format!(
                "Submitting with {} image(s): {}",
                images.len(),
                images
                    .iter()
                    .map(|(t, d)| format!("{} ({}KB)", t, d.len() / 1024))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if images.is_empty() {
            self.current_turn_system_reminder = mission_turn_reminder(&self.session.id);
            self.add_provider_message(Message::user(&input));
            self.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: input.clone(),
                    cache_control: None,
                }],
            );
        } else {
            self.current_turn_system_reminder = mission_turn_reminder(&self.session.id);
            self.add_provider_message(Message::user_with_images(&input, images.clone()));
            let mut blocks: Vec<ContentBlock> = images
                .into_iter()
                .map(|(media_type, data)| ContentBlock::Image { media_type, data })
                .collect();
            blocks.push(ContentBlock::Text {
                text: input.clone(),
                cache_control: None,
            });
            self.session.add_message(Role::User, blocks);
        }
        crate::telemetry::record_turn();
        self.session_save_pending = true;

        // A fresh user turn supersedes any post-error fallback offer from the
        // previous turn; drop it so a stale keypress can't switch+resend.
        self.clear_pending_fallback_offer();
        // Likewise drop any armed "merge the diverged update" offer.
        self.clear_update_merge_offer();

        // Set up processing state - actual processing happens after UI redraws
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.clear_streaming_render_state();
        // A new prompt starts a new turn: the previous turn's anchored
        // reasoning traces leave the transcript (ephemeral `current` mode).
        self.clear_turn_reasoning_traces();
        self.stream_buffer.clear();
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming.streaming_input_tokens = 0;
        self.streaming.streaming_output_tokens = 0;
        self.streaming.streaming_cache_read_tokens = None;
        self.streaming.streaming_cache_creation_tokens = None;
        self.kv_cache.current_api_usage_recorded = false;
        self.upstream_provider = None;
        self.status_detail = None;
        self.streaming.streaming_tps_start = None;
        self.streaming.streaming_tps_elapsed = Duration::ZERO;
        self.streaming.streaming_tps_collect_output = false;
        self.streaming.streaming_total_output_tokens = 0;
        self.streaming.streaming_tps_observed_output_tokens = 0;
        self.streaming.streaming_tps_observed_elapsed = Duration::ZERO;
        self.processing_started = Some(Instant::now());
        self.visible_turn_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Process all queued messages (combined into a single request)
    /// Loops until queue is empty (in case more messages are queued during processing)
    pub(super) async fn process_queued_messages(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) {
        while !self.queued_messages.is_empty() || !self.hidden_queued_system_messages.is_empty() {
            // Combine all currently queued messages into one, treating [SYSTEM: ...]
            // startup continuations as system reminders rather than user turns.
            let queued_messages = std::mem::take(&mut self.queued_messages);
            let hidden_reminders = std::mem::take(&mut self.hidden_queued_system_messages);
            let (messages, reminder, display_system_messages) =
                super::helpers::partition_queued_messages(queued_messages, hidden_reminders);
            let combined = messages.join("\n\n");
            let has_combined = !combined.is_empty();
            let preserve_visible_turn = super::commands::queued_messages_are_only_pokes(&messages);

            self.commit_pending_streaming_assistant_message();

            for msg in display_system_messages {
                self.push_display_message(DisplayMessage::system(msg));
            }

            for msg in &messages {
                if !super::commands::is_poke_message(msg) {
                    self.push_display_message(DisplayMessage::user(msg.clone()));
                }
            }

            self.current_turn_system_reminder =
                merge_turn_reminders(reminder, mission_turn_reminder(&self.session.id));

            if has_combined {
                self.add_provider_message(Message::user(&combined));
                self.session.add_message(
                    Role::User,
                    vec![ContentBlock::Text {
                        text: combined.clone(),
                        cache_control: None,
                    }],
                );
            }
            self.session_save_pending = true;
            self.clear_streaming_render_state();
            self.stream_buffer.clear();
            self.thought_line_inserted = false;
            self.thinking_prefix_emitted = false;
            self.thinking_buffer.clear();
            self.streaming_tool_calls.clear();
            self.streaming.streaming_input_tokens = 0;
            self.streaming.streaming_output_tokens = 0;
            self.streaming.streaming_cache_read_tokens = None;
            self.streaming.streaming_cache_creation_tokens = None;
            self.kv_cache.current_api_usage_recorded = false;
            self.upstream_provider = None;
            self.status_detail = None;
            self.streaming.streaming_tps_start = None;
            self.streaming.streaming_tps_elapsed = Duration::ZERO;
            self.streaming.streaming_tps_collect_output = false;
            self.streaming.streaming_total_output_tokens = 0;
            self.streaming.streaming_tps_observed_output_tokens = 0;
            self.streaming.streaming_tps_observed_elapsed = Duration::ZERO;
            self.processing_started = Some(Instant::now());
            if has_combined {
                if preserve_visible_turn {
                    self.visible_turn_started.get_or_insert_with(Instant::now);
                } else {
                    self.visible_turn_started = Some(Instant::now());
                }
            }
            self.is_processing = true;
            self.status = ProcessingStatus::Sending;

            match self
                .run_turn_interactive(terminal, event_stream, None)
                .await
            {
                Ok(()) => {
                    self.last_stream_error = None;
                    self.last_submitted_input = None;
                }
                Err(e) => {
                    let err_str = crate::util::format_error_chain(&e);
                    if is_request_payload_too_large_error(&err_str) {
                        if !self
                            .try_recover_payload_too_large_and_retry(terminal, event_stream)
                            .await
                        {
                            self.handle_turn_error(err_str);
                        }
                    } else if is_context_limit_error(&err_str) {
                        if self
                            .try_auto_compact_and_retry(terminal, event_stream)
                            .await
                        {
                            // Successfully recovered
                        } else {
                            self.handle_turn_error(err_str);
                        }
                    } else {
                        self.handle_turn_error(err_str);
                    }
                }
            }
            self.current_turn_system_reminder = None;
            // Loop will check if more messages were queued during this turn
        }
    }

    pub(super) fn flush_pending_session_save(&mut self) {
        if !self.session_save_pending {
            return;
        }

        match self.session.save() {
            Ok(()) => {
                self.session_save_pending = false;
            }
            Err(error) => {
                crate::logging::warn(&format!(
                    "Failed to persist pending session save for {}: {}",
                    self.session.id, error
                ));
            }
        }
    }
}
