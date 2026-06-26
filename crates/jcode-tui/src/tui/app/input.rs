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
use crossterm::event::{EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::DefaultTerminal;
use std::io::{Read, Write};
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

pub(super) fn edit_input_in_external_editor(app: &mut App) {
    match edit_text_in_external_editor(&app.input) {
        Ok(edited) => {
            if edited != app.input {
                app.remember_input_undo_state();
                app.input = edited;
                app.cursor_pos = app.input.len();
                app.sync_model_picker_preview_from_input();
            }
            app.set_status_notice("Prompt edited in $EDITOR");
        }
        Err(err) => app.set_status_notice(&format!("Failed to open $EDITOR: {err}")),
    }
}

fn edit_text_in_external_editor(initial_text: &str) -> Result<String> {
    let mut file = tempfile::Builder::new()
        .prefix("jcode-prompt-")
        .suffix(".md")
        .tempfile()?;
    file.write_all(initial_text.as_bytes())?;
    file.flush()?;
    let path = file.path().to_path_buf();

    let raw_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if raw_was_enabled {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    let _ = crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );

    let status_result = std::process::Command::new("sh")
        .arg("-c")
        .arg("exec ${VISUAL:-${EDITOR:-vi}} \"$@\"")
        .arg("jcode-editor")
        .arg(&path)
        .status();

    let _ = crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        crossterm::cursor::Hide
    );
    if raw_was_enabled {
        let _ = crossterm::terminal::enable_raw_mode();
    }

    let status = status_result?;
    if !status.success() {
        anyhow::bail!("editor exited with status {status}");
    }

    let mut edited = String::new();
    std::fs::File::open(&path)?.read_to_string(&mut edited)?;
    Ok(edited)
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
        ClipboardPasteContent, ClipboardPasteKind, is_clipboard_paste_shortcut,
        preferred_wayland_text_type, read_clipboard_for_paste_with, shifted_printable_fallback,
        text_input_for_key,
    };
    use crossterm::event::{KeyCode, KeyModifiers};

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

pub(super) fn handle_paste(app: &mut App, text: String) {
    // Note: clipboard_image() is NOT checked here. Bracketed paste events from the
    // terminal always deliver text. Checking clipboard_image() here caused a bug where
    // text pastes were misidentified as images when the clipboard also had image data
    // (common on Wayland where apps advertise multiple MIME types). Image pasting is
    // handled by explicit clipboard shortcuts instead (Ctrl+V/Alt+V/Cmd+V smart-paste).
    if let Some(url) = super::extract_image_url(&text) {
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
        return;
    }

    handle_text_paste(app, text);
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

    app.remember_input_undo_state();
    app.input.insert_str(app.cursor_pos, text);
    app.cursor_pos += text.len();
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
            self.auto_poke_incomplete_todos = false;
            if todos.is_empty() {
                return false;
            }
            let confidence_summary = super::commands::todo_confidence_summary(&todos);
            let confidence_label =
                super::commands::format_todo_completion_confidence(confidence_summary);
            self.push_display_message(DisplayMessage::system(format!(
                "✅ Todos complete. Auto-poke finished. Cumulative confidence: {}.",
                confidence_label
            )));
            if confidence_summary.needs_more_work {
                self.hidden_queued_system_messages.push(
                    super::commands::build_todo_confidence_summary_message(&todos),
                );
                self.pending_queued_dispatch = true;
                return true;
            }
            self.pending_queued_dispatch = false;
            return false;
        }

        self.push_display_message(DisplayMessage::system(format!(
            "👉 Auto-poking: {} incomplete todo{}. /poke off to stop.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
        )));
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
}

pub(super) fn is_next_prompt_new_session_hotkey(code: KeyCode, modifiers: KeyModifiers) -> bool {
    code == KeyCode::Char(' ')
        && modifiers.contains(KeyModifiers::SUPER)
        && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::HYPER)
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
            edit_input_in_external_editor(app);
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
        KeyCode::Char('b') => {
            app.cursor_pos = app.find_word_boundary_back();
            true
        }
        KeyCode::Char('f') => {
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
    if app.dictation_key_matches(code, modifiers) {
        app.handle_dictation_trigger();
        return true;
    }
    if let Some(direction) = app.model_switch_keys.direction_for(code, modifiers) {
        app.cycle_model(direction);
        return true;
    }
    if let Some(direction) = app.effort_switch_keys.direction_for(code, modifiers) {
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
    if app.centered_toggle_keys.toggle.matches(code, modifiers) {
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

        if handle_modal_key(self, code, modifiers)? {
            return Ok(());
        }

        if self.handle_onboarding_continue_prompt_key(code) {
            return Ok(());
        }

        if code == KeyCode::BackTab {
            self.cycle_model_favorite_hotkey();
            return Ok(());
        }

        // While the model picker preview is visible, route its favorite/default
        // hotkeys (Ctrl+B, Ctrl+F, Alt+F) to the focused picker handler before the
        // global control shortcuts can claim them. This makes the hotkeys work
        // directly in the preview list the user always sees.
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

        if modifiers == KeyModifiers::CONTROL && code == KeyCode::Up {
            if retrieve_pending_message_for_edit(self) {
                return Ok(());
            }
            handle_prompt_history_navigation(self, code, modifiers);
            return Ok(());
        }

        if modifiers == KeyModifiers::CONTROL && code == KeyCode::Down {
            handle_prompt_history_navigation(self, code, modifiers);
            return Ok(());
        }

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL)
            && handle_global_control_shortcuts(self, code, diagram_available)
        {
            return Ok(());
        }

        // Ctrl+Enter: does opposite of queue_mode during processing
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::CONTROL) {
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
            return Ok(());
        }

        if code == KeyCode::Enter {
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

        if handle_basic_key(self, code) {
            return Ok(());
        }

        Ok(())
    }

    pub(super) fn request_full_redraw(&mut self) {
        self.force_full_redraw = true;
    }

    pub(super) fn should_redraw_after_resize(&mut self) -> bool {
        const RESIZE_REDRAW_MIN_INTERVAL: std::time::Duration =
            std::time::Duration::from_millis(33);

        let now = std::time::Instant::now();
        match self.last_resize_redraw {
            Some(last) if now.duration_since(last) < RESIZE_REDRAW_MIN_INTERVAL => false,
            _ => {
                self.last_resize_redraw = Some(now);
                self.handle_diagram_geometry_change();
                true
            }
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
        // persistent transcript. Historically the closed block was sliced straight
        // back out of the live stream, so it vanished the instant the model moved
        // on. With decorative animations we instead *retain* the just-closed trace
        // on screen (as its own dim section above the live stream) until the next
        // trace is fully done, then shrink the previous one away. Tiers without
        // decorative animations keep the original instant-discard behavior.
        if self.reasoning_current_mode() {
            if self.reasoning_animations_enabled() {
                self.retain_current_reasoning_block();
            } else {
                self.discard_current_reasoning_block();
            }
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

    /// True when the retain-then-shrink reasoning animation should drive the
    /// `current`-mode collapse. Disabled on tiers without decorative animations
    /// (SSH/WSL/Minimal), which fall back to instant discard.
    pub(super) fn reasoning_animations_enabled(&self) -> bool {
        crate::perf::tui_policy().enable_decorative_animations
    }

    /// Slice the just-closed reasoning block out of `streaming_text` and keep it as
    /// the retained trace. Any previously retained trace begins its shrink-away
    /// animation (it is "fully done" now that a newer trace has closed). Used in
    /// `current` mode when decorative animations are enabled.
    pub(super) fn retain_current_reasoning_block(&mut self) {
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
        // The previously retained trace is now superseded: fold it away.
        if let Some(prev) = self.reasoning_retained.take() {
            self.start_reasoning_collapse(prev);
        }
        self.reasoning_retained = Some(block);
        self.refresh_split_view_if_needed();
    }

    /// Begin (or restart) the shrink-away animation for a retained reasoning trace.
    fn start_reasoning_collapse(&mut self, markup: String) {
        if markup.trim().is_empty() {
            return;
        }
        self.reasoning_collapse = Some(crate::tui::app::ReasoningCollapse {
            markup,
            started: std::time::Instant::now(),
        });
    }

    /// Drive the reasoning retain/collapse animation forward by one tick. Folds the
    /// last retained trace away once the turn is over, finishes any in-progress
    /// shrink, and reports whether a redraw is still needed. Cheap no-op when no
    /// reasoning animation is active.
    pub(super) fn tick_reasoning_collapse(&mut self) -> bool {
        let mut redraw = false;
        if let Some(collapse) = &self.reasoning_collapse {
            redraw = true;
            if collapse.started.elapsed() >= crate::tui::app::REASONING_COLLAPSE_DURATION {
                self.reasoning_collapse = None;
            }
        }
        // Once the turn finishes, the final retained trace has no successor to wait
        // on, so fold it away too (keeping `current` mode ephemeral).
        if !self.is_processing && self.reasoning_retained.is_some() {
            if let Some(trace) = self.reasoning_retained.take() {
                self.start_reasoning_collapse(trace);
            }
            redraw = true;
        }
        if redraw {
            self.refresh_split_view_if_needed();
        }
        redraw
    }

    /// Whether a retained or collapsing reasoning trace needs animation frames.
    pub(super) fn reasoning_animation_active(&self) -> bool {
        self.reasoning_collapse.is_some()
            || (self.reasoning_retained.is_some() && !self.is_processing)
    }

    /// Drop any retained/collapsing reasoning trace immediately (new turn / reset).
    pub(super) fn clear_retained_reasoning(&mut self) {
        let had = self.reasoning_retained.is_some() || self.reasoning_collapse.is_some();
        self.reasoning_retained = None;
        self.reasoning_collapse = None;
        if had {
            self.refresh_split_view_if_needed();
        }
    }

    /// Slice the just-closed reasoning block out of `streaming_text` in place,
    /// leaving any answer text that streamed *before* it untouched and in order.
    /// Used in `current` mode (animations disabled) so only the live reasoning
    /// block is ever visible and no per-block trace is left behind.
    pub(super) fn discard_current_reasoning_block(&mut self) {
        let block_start = self
            .reasoning_block_start
            .take()
            .unwrap_or(0)
            .min(self.streaming.streaming_text.len());
        // Everything from the block start onward is reasoning markup (plus the
        // separators inserted by open/close). Drop it from the live stream.
        self.streaming.streaming_text.truncate(block_start);
        // Drop the separator the open path added before the reasoning block so the
        // surrounding answer text rejoins cleanly.
        while self.streaming.streaming_text.ends_with('\n') {
            self.streaming.streaming_text.pop();
        }
        self.refresh_split_view_if_needed();
    }

    pub(super) fn append_streaming_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.streaming.streaming_text.push_str(text);
        self.refresh_split_view_if_needed();
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
        self.reasoning_streaming = false;
        self.reasoning_pending_line.clear();
        self.reasoning_partial_len = 0;
        // The stream (and any block offset into it) is gone.
        self.reasoning_block_start = None;
        self.refresh_split_view_if_needed();
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
    }

    pub(super) fn take_streaming_text(&mut self) -> String {
        let content = std::mem::take(&mut self.streaming.streaming_text);
        self.stream_message_ended = false;
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
        if let Some(chunk) = self.stream_buffer.flush() {
            self.append_streaming_text(&chunk);
        }

        if self.streaming.streaming_text.is_empty() {
            self.stream_buffer.clear();
            return false;
        }

        let content = self.take_streaming_text();
        let content = self.collapse_reasoning_for_commit(content);
        if content.trim().is_empty() {
            // Nothing left after collapsing reasoning-only content.
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
        if self.activate_picker_from_preview() {
            return;
        }

        let raw_input = std::mem::take(&mut self.input);
        let input = self.expand_paste_placeholders(&raw_input);
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
        let handled = commands::handle_help_command(self, trimmed)
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

        // Check for skill invocation
        if let Some(skill_name) = SkillRegistry::parse_invocation(&input) {
            let mut skill = self.current_skills_snapshot().get(skill_name).cloned();

            // Remote/minimal TUI clients may start with an empty skill snapshot, and
            // daemon-side `skill_manage reload_all` can update a different process.
            // On a slash miss, synchronously refresh from the active session working
            // directory before reporting Unknown skill so project-local skills such
            // as .jcode/skills/optimization work immediately after reload/build.
            if skill.is_none() {
                let working_dir = self
                    .session
                    .working_dir
                    .as_deref()
                    .map(std::path::Path::new);
                if let Ok(reloaded) = SkillRegistry::load_for_working_dir(working_dir) {
                    skill = reloaded.get(skill_name).cloned();
                    self.skills = std::sync::Arc::new(reloaded.clone());
                    if let Ok(mut shared) = self.registry.skills().try_write() {
                        *shared = reloaded;
                    }
                    self.invalidate_command_candidates_cache();
                }
            }

            if let Some(skill) = skill {
                self.active_skill = Some(skill_name.to_string());
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Activated skill: {} - {}", skill.name, skill.description),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            } else {
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Unknown skill: /{}", skill_name),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            return;
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

        // Set up processing state - actual processing happens after UI redraws
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.clear_streaming_render_state();
        self.clear_retained_reasoning();
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
