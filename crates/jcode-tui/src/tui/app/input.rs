#![cfg_attr(test, allow(clippy::items_after_test_module))]

use super::{
    App, ContentBlock, DisplayMessage, Message, ProcessingStatus, Role, SendAction, SkillRegistry,
    commands, ctrl_bracket_fallback_to_esc, is_context_limit_error, remote,
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
        Err(err) => app.set_status_notice(format!("Failed to open $EDITOR: {err}")),
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

/// Issue #4 follow-up: expand `/<prompt-name>` (or `/<prompt-name> <args>`)
/// into the body of a discovered prompt template, when the name is not a
/// reserved built-in slash command.
///
/// Returns:
///   - `Some(expanded)` when `input` matches `/name[ args]` and a template
///     `name` exists in project- or user-level prompts.
///   - `None` when the input is not a slash invocation, the slash name is a
///     reserved built-in, or no template by that name is discovered.
///
/// Args (after the name) are appended to the template body on a new line as
/// `Args: <rest>`. A future PR can replace this with `{{name}}` / `{{$1}}`
/// substitution; this MVP keeps it explicit so users see what got injected.
pub(super) fn expand_prompt_template_invocation(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let body = trimmed.strip_prefix('/')?;
    if body.is_empty() {
        return None;
    }
    let (name, args) = match body.split_once(char::is_whitespace) {
        Some((n, rest)) => (n, rest.trim()),
        None => (body, ""),
    };

    // Skip reserved built-in slash commands so user templates don't shadow
    // `/help`, `/quit`, `/export`, etc.
    let with_slash = format!("/{name}");
    if super::state_ui_input_helpers::REGISTERED_COMMANDS
        .iter()
        .any(|c| c.name() == with_slash)
    {
        return None;
    }

    let template = crate::prompt_templates::find_by_name(name)?;
    // Issue #4 follow-up: if the template body has YAML frontmatter declaring
    // `args:`, substitute `{{name}}` placeholders in the body with values
    // resolved from user input. Otherwise fall back to the previous "append
    // raw args" behavior so older templates without frontmatter still work.
    let (frontmatter, body, decls) = crate::prompt_templates::parse_frontmatter(&template.body);

    // Drop the rendered body if frontmatter exists; otherwise use the
    // original (which may itself begin with --- intended as content).
    let body = if frontmatter.is_some() {
        body
    } else {
        template.body.clone()
    };

    let mut out = if !decls.is_empty() {
        let (positional, named) = crate::prompt_templates::parse_user_args(args);
        let (bindings, missing) = crate::prompt_templates::bind_args(&decls, &positional, &named);
        let mut rendered = crate::prompt_templates::substitute_placeholders(&body, &bindings);
        if !missing.is_empty() {
            rendered.push_str(&format!(
                "\n\n[/{name}: missing required arg(s): {}]",
                missing.join(", ")
            ));
        }
        rendered.trim_end().to_string()
    } else {
        // Backwards-compat: no declared args, use legacy "Args: <rest>" trailer.
        let mut legacy = body.trim_end().to_string();
        if !args.is_empty() {
            legacy.push_str("\n\nArgs: ");
            legacy.push_str(args);
            legacy.push('\n');
        }
        legacy
    };

    // Strip trailing whitespace consistently.
    while out.ends_with(['\n', ' ', '\t']) {
        out.pop();
    }
    Some(out)
}

#[cfg(test)]
mod prompt_expansion_tests {
    use super::*;

    fn write_template(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.md")), body).unwrap();
    }

    #[test]
    fn expansion_replaces_slash_with_body() {
        // Use the user-level prompts dir under JCODE_HOME so the test is
        // independent of cwd.
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        let prompts = temp.path().join("prompts");
        write_template(&prompts, "qa-checklist", "Please run the QA checklist.");

        let expanded = expand_prompt_template_invocation("/qa-checklist").expect("expanded");
        assert!(expanded.contains("Please run the QA checklist."));

        let with_args = expand_prompt_template_invocation("/qa-checklist the auth module").unwrap();
        assert!(with_args.contains("Please run the QA checklist."));
        assert!(with_args.contains("Args: the auth module"));

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    // Issue #4 follow-up: when frontmatter declares args, expansion must
    // substitute {{name}} placeholders with bound values.
    #[test]
    fn expansion_substitutes_frontmatter_args() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        let prompts = temp.path().join("prompts");

        write_template(
            &prompts,
            "review-it",
            "---\n\
             description: review with focus + target\n\
             args:\n\
             - name: focus\n  required: false\n  default: bugs\n\
             - name: target\n  required: true\n\
             ---\n\n\
             Please review the {{target}} for {{focus}}.\n",
        );

        // Positional fills declared order: focus, target.
        let out = expand_prompt_template_invocation("/review-it auth src/lib.rs").unwrap();
        assert!(
            out.contains("Please review the src/lib.rs for auth."),
            "expected substitution; got: {out}"
        );

        // Named overrides positional.
        let out = expand_prompt_template_invocation("/review-it focus=security target=src/auth.rs")
            .unwrap();
        assert!(out.contains("Please review the src/auth.rs for security."));

        // Default kicks in when focus is absent (positional is target).
        // 1 positional → fills focus first; target then unbound + required → missing arg notice.
        let out = expand_prompt_template_invocation("/review-it src/foo.rs").unwrap();
        assert!(out.contains("Please review the {{target}} for src/foo.rs."));
        assert!(out.contains("missing required arg(s): target"));

        // No args → focus uses default "bugs", target is missing.
        let out = expand_prompt_template_invocation("/review-it").unwrap();
        assert!(out.contains("for bugs."));
        assert!(out.contains("missing required arg(s): target"));

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn expansion_legacy_template_without_frontmatter_still_appends_args() {
        // Backwards-compat: templates without `---` frontmatter use the old
        // `Args: <rest>` trailer.
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        let prompts = temp.path().join("prompts");
        write_template(&prompts, "old-style", "No frontmatter here.");

        let out = expand_prompt_template_invocation("/old-style hello world").unwrap();
        assert!(out.contains("No frontmatter here."));
        assert!(out.contains("Args: hello world"));

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn expansion_skips_builtin_commands() {
        // Even if a template named "help" existed, the built-in /help wins.
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        write_template(&temp.path().join("prompts"), "help", "shadowed body");

        assert!(expand_prompt_template_invocation("/help").is_none());

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn expansion_returns_none_for_non_slash_input() {
        assert!(expand_prompt_template_invocation("hello").is_none());
        assert!(expand_prompt_template_invocation("/").is_none());
        assert!(expand_prompt_template_invocation("").is_none());
    }

    #[test]
    fn expansion_returns_none_for_unknown_template() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());
        assert!(expand_prompt_template_invocation("/no-such-template").is_none());

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
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
            if let Some(text) = read_text() {
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
    app.reset_input_history_browse();
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
    expand_at_path_references(&result, app.session.working_dir.as_deref())
}

/// Maximum size (in bytes) of a single `@path` reference jcode is willing to
/// inline before sending to the model. Larger files are kept as a literal
/// `@path` token plus a one-line note so the agent can still ask for them
/// via the `read` tool — better than blowing the context window silently.
const AT_PATH_MAX_BYTES: u64 = 200 * 1024;

/// Maximum directory entries inlined for an `@folder/` mention.
/// Matches Claude Code's `MAX_DIR_ENTRIES = 1000`.
const AT_FOLDER_MAX_ENTRIES: usize = 1000;

/// Expand `@<path>` references in user input.
///
/// Supported forms:
///   - `@path/to/file` — inline full content (cap: `AT_PATH_MAX_BYTES`)
///   - `@"path with spaces"` — quoted path; same expansion rules
///   - `@path/to/file#L10` or `@path/to/file#L10-20` — line range slice
///   - `@path/to/folder/` — directory listing (1 level, cap:
///     `AT_FOLDER_MAX_ENTRIES`)
///
/// Email-like tokens (`user@example.com`) are left untouched.
/// Multiple mentions in a single message are all expanded.
pub fn expand_at_path_references(input: &str, working_dir: Option<&str>) -> String {
    if !input.contains('@') {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        // Fast path: copy non-`@` chars verbatim.
        if bytes[i] != b'@' {
            // Find the next `@` or end of input and copy that slice.
            let next = bytes[i + 1..]
                .iter()
                .position(|&b| b == b'@')
                .map(|p| i + 1 + p)
                .unwrap_or(bytes.len());
            out.push_str(&input[i..next]);
            i = next;
            continue;
        }

        // We're at an `@`. Validate token-start (start of input or
        // preceded by whitespace) — anything else (including `,@`,
        // `(@`, `user@`) is left as a literal `@`.
        let valid_start = i == 0 || (bytes[i - 1] as char).is_ascii_whitespace();
        if !valid_start {
            out.push('@');
            i += 1;
            continue;
        }

        // Parse the token after `@`. Two forms:
        //   - quoted: `@"..."` — content can include spaces; ends at
        //     matching `"` or end-of-input.
        //   - bare:   `@path` — ends at first whitespace.
        let token_start = i + 1;
        let (raw_token, token_end, was_quoted) = if bytes.get(token_start) == Some(&b'"') {
            let inner_start = token_start + 1;
            let mut j = inner_start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            let closed = j < bytes.len();
            let token = input[inner_start..j].to_string();
            let end = if closed { j + 1 } else { j };
            (token, end, true)
        } else {
            let mut j = token_start;
            while j < bytes.len() && !(bytes[j] as char).is_ascii_whitespace() {
                j += 1;
            }
            (input[token_start..j].to_string(), j, false)
        };

        // Strip trailing punctuation that's clearly not part of a path
        // (only for unquoted tokens — quoted paths are taken verbatim).
        let (token, trail_punct) = if was_quoted {
            (raw_token.as_str(), "")
        } else {
            let trimmed = raw_token.trim_end_matches(|c: char| {
                matches!(c, '.' | ',' | ')' | ']' | '!' | '?' | ':' | ';')
            });
            let trail = &raw_token[trimmed.len()..];
            (trimmed, trail)
        };

        if token.is_empty() {
            // Lone `@` — emit as-is and continue past it.
            out.push_str(&input[i..token_end]);
            out.push_str(trail_punct);
            i = token_end;
            continue;
        }

        // Parse optional `#L<n>` or `#L<a>-<b>` line range. The hash must
        // be in the unquoted suffix; quoted paths can contain `#` literally.
        let (path_part, line_range, range_was_invalid) = if was_quoted {
            (token.to_string(), None, false)
        } else {
            split_line_range(token)
        };

        match try_read_at_path(&path_part, working_dir, line_range) {
            ResolveOutcome::File {
                display_path,
                body,
                range,
            } => {
                let header = match range {
                    Some((a, b)) if a == b => format!("@{display_path} (line {a})"),
                    Some((a, b)) => format!("@{display_path} (lines {a}-{b})"),
                    None => format!("@{display_path}"),
                };
                let mut suffix = String::new();
                if range_was_invalid {
                    suffix.push_str(" _(note: ignored invalid line range, showing full file)_");
                }
                out.push_str(&format!(
                    "\n\n--- {header}{suffix} ---\n{body}\n--- end @{display_path} ---\n\n"
                ));
                out.push_str(trail_punct);
            }
            ResolveOutcome::Folder {
                display_path,
                listing,
                total_entries,
                shown_entries,
            } => {
                let header = if total_entries > shown_entries {
                    format!(
                        "@{display_path} (directory listing, {shown_entries} of {total_entries} entries)"
                    )
                } else {
                    format!("@{display_path} (directory listing, {total_entries} entries)")
                };
                out.push_str(&format!(
                    "\n\n--- {header} ---\n{listing}--- end @{display_path} ---\n\n"
                ));
                out.push_str(trail_punct);
            }
            ResolveOutcome::TooLarge { display_path, size } => {
                out.push_str(&format!(
                    "@{display_path} _(skipped — {size} bytes > {AT_PATH_MAX_BYTES} byte cap; use the read tool)_"
                ));
                out.push_str(trail_punct);
            }
            ResolveOutcome::Missing => {
                // Leave the original token verbatim (incl. quotes/range).
                out.push_str(&input[i..token_end]);
                out.push_str(trail_punct);
            }
        }
        i = token_end;
    }
    out
}

enum ResolveOutcome {
    File {
        display_path: String,
        body: String,
        range: Option<(usize, usize)>,
    },
    Folder {
        display_path: String,
        listing: String,
        total_entries: usize,
        shown_entries: usize,
    },
    TooLarge {
        display_path: String,
        size: u64,
    },
    Missing,
}

/// Split a bare token into (path, optional_range, range_was_invalid).
///
/// Recognized forms (case-insensitive `L`):
///   - `path#L10`     → ("path", Some((10, 10)), false)
///   - `path#L10-20`  → ("path", Some((10, 20)), false)
///   - `path#L`       → ("path#L", None, true)        — invalid, treat as path
///   - `path#Lbad`    → ("path#Lbad", None, true)     — invalid
///
/// If no `#L` suffix is present, returns `(path, None, false)`.
fn split_line_range(token: &str) -> (String, Option<(usize, usize)>, bool) {
    // Find the LAST `#L` so paths containing `#` work (`@path#with#L1`).
    let Some(hash_pos) = token.rfind("#L") else {
        return (token.to_string(), None, false);
    };
    let path = &token[..hash_pos];
    let spec = &token[hash_pos + 2..];
    if spec.is_empty() {
        return (token.to_string(), None, true);
    }
    if let Some((a_str, b_str)) = spec.split_once('-') {
        match (a_str.parse::<usize>(), b_str.parse::<usize>()) {
            (Ok(a), Ok(b)) if a >= 1 && b >= a => {
                return (path.to_string(), Some((a, b)), false);
            }
            _ => return (token.to_string(), None, true),
        }
    }
    match spec.parse::<usize>() {
        Ok(n) if n >= 1 => (path.to_string(), Some((n, n)), false),
        _ => (token.to_string(), None, true),
    }
}

fn try_read_at_path(
    token: &str,
    working_dir: Option<&str>,
    range: Option<(usize, usize)>,
) -> ResolveOutcome {
    let path = std::path::Path::new(token);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(cwd) = working_dir {
        std::path::Path::new(cwd).join(path)
    } else {
        path.to_path_buf()
    };
    let Ok(meta) = std::fs::metadata(&resolved) else {
        return ResolveOutcome::Missing;
    };

    if meta.is_dir() {
        return read_folder_listing(&resolved, token);
    }

    if !meta.is_file() {
        return ResolveOutcome::Missing;
    }

    let size = meta.len();
    if size > AT_PATH_MAX_BYTES {
        return ResolveOutcome::TooLarge {
            display_path: token.to_string(),
            size,
        };
    }

    let body = match std::fs::read_to_string(&resolved) {
        Ok(b) => b,
        Err(_) => return ResolveOutcome::Missing,
    };

    let (sliced, applied_range) = match range {
        Some((a, b)) => {
            let lines: Vec<&str> = body.lines().collect();
            // Clamp to actual line count; preserve user's reported range.
            let end = b.min(lines.len()).max(a.min(lines.len()));
            if a > lines.len() {
                (String::new(), Some((a, b)))
            } else {
                let slice = lines[a - 1..end].join("\n");
                (slice, Some((a, b)))
            }
        }
        None => (body, None),
    };

    ResolveOutcome::File {
        display_path: token.to_string(),
        body: sliced,
        range: applied_range,
    }
}

/// Build a 1-level directory listing in the format Claude Code uses.
/// Sort: directories first (alphabetical), then files (alphabetical).
/// Each directory entry has a trailing `/`.
fn read_folder_listing(abs: &std::path::Path, display_token: &str) -> ResolveOutcome {
    let entries = match std::fs::read_dir(abs) {
        Ok(e) => e,
        Err(_) => return ResolveOutcome::Missing,
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            dirs.push(format!("{name_str}/"));
        } else {
            files.push(name_str);
        }
    }
    dirs.sort();
    files.sort();
    let total = dirs.len() + files.len();

    let mut listing = String::new();
    let mut shown = 0usize;
    for entry in dirs.iter().chain(files.iter()) {
        if shown >= AT_FOLDER_MAX_ENTRIES {
            break;
        }
        listing.push_str(entry);
        listing.push('\n');
        shown += 1;
    }
    if total > shown {
        listing.push_str(&format!("… and {} more entries\n", total - shown));
    }

    // Always present folder paths with a trailing `/` in the marker so
    // model can distinguish folder mentions from file mentions.
    let display_path = if display_token.ends_with('/') {
        display_token.to_string()
    } else {
        format!("{display_token}/")
    };

    ResolveOutcome::Folder {
        display_path,
        listing,
        total_entries: total,
        shown_entries: shown,
    }
}

#[cfg(test)]
mod at_path_tests {
    use super::*;

    #[test]
    fn passes_through_input_with_no_at() {
        assert_eq!(
            expand_at_path_references("hello world", None),
            "hello world"
        );
    }

    #[test]
    fn does_not_expand_email_like_tokens() {
        let out = expand_at_path_references("ping me at user@example.com please", None);
        assert!(out.contains("user@example.com"));
        assert!(!out.contains("--- @example.com"));
    }

    #[test]
    fn expands_real_file_relative_to_working_dir() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("hello.txt");
        std::fs::write(&file, "FILE_BODY\nLINE 2").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();

        let out = expand_at_path_references("review @hello.txt for issues", Some(&cwd));
        assert!(out.contains("--- @hello.txt ---"));
        assert!(out.contains("FILE_BODY"));
        assert!(out.contains("LINE 2"));
        assert!(out.contains("--- end @hello.txt ---"));
        // Trailing words are preserved.
        assert!(out.contains(" for issues"));
    }

    #[test]
    fn missing_file_leaves_at_token_literal() {
        let temp = tempfile::TempDir::new().expect("temp");
        let cwd = temp.path().to_string_lossy().to_string();
        let out = expand_at_path_references("look at @nope.txt", Some(&cwd));
        assert_eq!(out, "look at @nope.txt");
    }

    #[test]
    fn trailing_punctuation_after_path_is_preserved() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("a.rs");
        std::fs::write(&file, "fn main() {}").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        // "@a.rs." => path is "a.rs", trailing "." preserved.
        let out = expand_at_path_references("see @a.rs.", Some(&cwd));
        assert!(out.contains("--- @a.rs ---"));
        assert!(out.trim_end().ends_with("."));
    }

    // ---- Phase 5: quoted paths, line ranges, folder listings ----

    #[test]
    fn quoted_path_with_space_expands() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("my file.txt");
        std::fs::write(&file, "hello world").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let out = expand_at_path_references("@\"my file.txt\"", Some(&cwd));
        assert!(out.contains("--- @my file.txt ---"), "got: {out}");
        assert!(out.contains("hello world"));
    }

    #[test]
    fn line_range_single_line() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("a.rs");
        std::fs::write(&file, "line1\nline2\nline3\nline4").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let out = expand_at_path_references("@a.rs#L2", Some(&cwd));
        assert!(out.contains("--- @a.rs (line 2) ---"), "got: {out}");
        assert!(out.contains("line2"));
        assert!(!out.contains("line1"));
        assert!(!out.contains("line3"));
    }

    #[test]
    fn line_range_multi_line() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("a.rs");
        std::fs::write(&file, "a\nb\nc\nd\ne").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let out = expand_at_path_references("@a.rs#L2-4", Some(&cwd));
        assert!(out.contains("--- @a.rs (lines 2-4) ---"), "got: {out}");
        assert!(out.contains("b\nc\nd"));
        assert!(!out.contains("\na\n") && !out.starts_with("a\n"));
    }

    #[test]
    fn invalid_line_range_falls_back_to_full_file_with_note() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("a.rs");
        std::fs::write(&file, "x\ny\nz").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let out = expand_at_path_references("@a.rs#Lbad", Some(&cwd));
        // The path with `#Lbad` doesn't exist, so the token stays literal.
        // (Better to keep it as `@a.rs#Lbad` than silently expand to a
        // different path the user didn't ask for.)
        assert!(out.contains("@a.rs#Lbad"), "got: {out}");
    }

    #[test]
    fn line_range_just_hash_l_treated_as_path() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("a.rs");
        std::fs::write(&file, "x").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        // `@a.rs#L` is invalid range → token is `a.rs#L` → file not found
        // → literal kept.
        let out = expand_at_path_references("@a.rs#L", Some(&cwd));
        assert!(out.contains("@a.rs#L"));
    }

    #[test]
    fn folder_mention_inlines_listing() {
        let temp = tempfile::TempDir::new().expect("temp");
        let cwd = temp.path().to_string_lossy().to_string();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/a.rs"), "").unwrap();
        std::fs::write(temp.path().join("src/b.rs"), "").unwrap();
        std::fs::create_dir(temp.path().join("src/sub")).unwrap();

        let out = expand_at_path_references("review @src/", Some(&cwd));
        assert!(out.contains("--- @src/ (directory listing"), "got: {out}");
        // Sort: directories first, then files, alphabetically.
        let listing_section = out.split("--- @src/ (directory listing").nth(1).unwrap();
        let pos_sub = listing_section.find("sub/").unwrap();
        let pos_a = listing_section.find("a.rs").unwrap();
        let pos_b = listing_section.find("b.rs").unwrap();
        assert!(pos_sub < pos_a, "dirs should come before files");
        assert!(pos_a < pos_b, "files alphabetically sorted");
        // No file content was inlined — only the listing.
        assert!(!out.contains("--- end @src/ ---\n\n--- ") || out.matches("--- end").count() == 1);
    }

    #[test]
    fn folder_listing_caps_at_max_entries() {
        let temp = tempfile::TempDir::new().expect("temp");
        let big = temp.path().join("big");
        std::fs::create_dir(&big).unwrap();
        for n in 0..(super::AT_FOLDER_MAX_ENTRIES + 50) {
            std::fs::write(big.join(format!("f{n:05}.txt")), "").unwrap();
        }
        let cwd = temp.path().to_string_lossy().to_string();
        let out = expand_at_path_references("@big/", Some(&cwd));
        // Header announces shown vs total.
        assert!(
            out.contains(&format!(
                "{} of {} entries",
                super::AT_FOLDER_MAX_ENTRIES,
                super::AT_FOLDER_MAX_ENTRIES + 50
            )),
            "got: {out}"
        );
        // Truncation note present.
        assert!(out.contains("… and 50 more entries"));
    }

    #[test]
    fn multiple_mentions_all_expanded() {
        let temp = tempfile::TempDir::new().expect("temp");
        let cwd = temp.path().to_string_lossy().to_string();
        std::fs::write(temp.path().join("a.rs"), "AAA").unwrap();
        std::fs::write(temp.path().join("b.rs"), "BBB").unwrap();
        std::fs::create_dir(temp.path().join("docs")).unwrap();
        std::fs::write(temp.path().join("docs/x.md"), "").unwrap();

        let out = expand_at_path_references("see @a.rs and @docs/ then @b.rs", Some(&cwd));
        assert!(out.contains("AAA"));
        assert!(out.contains("BBB"));
        assert!(out.contains("--- @docs/ (directory listing"));
    }

    #[test]
    fn email_with_mention_after_only_expands_mention() {
        let temp = tempfile::TempDir::new().expect("temp");
        let cwd = temp.path().to_string_lossy().to_string();
        std::fs::write(temp.path().join("real.rs"), "REAL").unwrap();
        let out = expand_at_path_references("ping user@example.com about @real.rs", Some(&cwd));
        assert!(out.contains("user@example.com"));
        assert!(!out.contains("--- @example.com"));
        assert!(out.contains("REAL"));
    }

    #[test]
    fn folder_without_trailing_slash_still_works() {
        let temp = tempfile::TempDir::new().expect("temp");
        let cwd = temp.path().to_string_lossy().to_string();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        let out = expand_at_path_references("look at @src", Some(&cwd));
        // No trailing `/` in token, but the marker normalizes to `@src/`.
        assert!(out.contains("--- @src/ (directory listing"), "got: {out}");
    }

    #[test]
    fn line_range_clamped_to_file_length() {
        let temp = tempfile::TempDir::new().expect("temp");
        let file = temp.path().join("a.rs");
        std::fs::write(&file, "x\ny\nz").unwrap();
        let cwd = temp.path().to_string_lossy().to_string();
        let out = expand_at_path_references("@a.rs#L1-100", Some(&cwd));
        assert!(out.contains("--- @a.rs (lines 1-100) ---"), "got: {out}");
        assert!(out.contains("x\ny\nz"));
    }

    #[test]
    fn split_line_range_helper() {
        use super::split_line_range;
        assert_eq!(
            split_line_range("file.rs#L10"),
            ("file.rs".into(), Some((10, 10)), false)
        );
        assert_eq!(
            split_line_range("file.rs#L5-7"),
            ("file.rs".into(), Some((5, 7)), false)
        );
        assert_eq!(split_line_range("file.rs"), ("file.rs".into(), None, false));
        // Backwards range → invalid, keep literal.
        assert_eq!(
            split_line_range("file.rs#L7-3"),
            ("file.rs#L7-3".into(), None, true)
        );
        // Zero → invalid.
        assert_eq!(
            split_line_range("file.rs#L0"),
            ("file.rs#L0".into(), None, true)
        );
    }
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
    if modifiers.contains(KeyModifiers::CONTROL)
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
            if code == KeyCode::Up
                && (app.input.is_empty() || app.input_history_index.is_some())
                && app.input_history_up()
            {
                return true;
            }
            let inc = if code == KeyCode::PageUp { 10 } else { 1 };
            app.scroll_up(inc);
            true
        }
        KeyCode::Down | KeyCode::PageDown => {
            if code == KeyCode::Down
                && app.input_history_index.is_some()
                && app.input_history_down()
            {
                return true;
            }
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
    app.reset_input_history_browse();
    app.push_input_history(expanded.clone());
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
        // hotkeys (Ctrl+D, Ctrl+F, Alt+F) to the focused picker handler before the
        // global control shortcuts can claim them (e.g. Ctrl+D as quit). This makes
        // the hotkeys work directly in the preview list the user always sees.
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

        // Shift+Tab: cycle permission mode (Shift+Tab cycles through
        // default → acceptEdits → plan → auto → dontAsk → bypassPermissions → default)
        if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
            self.cycle_permission_mode();
            let label = self.permission_mode_label();
            self.set_status_notice(format!("🔒 {} mode", label));
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
        if self.streaming_text.is_empty() {
            self.replace_streaming_text(prefix);
        } else {
            self.replace_streaming_text(format!("{}{}", prefix, self.streaming_text));
        }
    }

    pub(super) fn append_streaming_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.streaming_text.push_str(text);
        self.refresh_split_view_if_needed();
    }

    pub(super) fn replace_streaming_text(&mut self, text: String) {
        self.streaming_text = text;
        self.refresh_split_view_if_needed();
    }

    pub(super) fn clear_streaming_render_state(&mut self) {
        self.streaming_text.clear();
        self.stream_message_ended = false;
        self.refresh_split_view_if_needed();
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
    }

    pub(super) fn take_streaming_text(&mut self) -> String {
        let content = std::mem::take(&mut self.streaming_text);
        self.stream_message_ended = false;
        self.refresh_split_view_if_needed();
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
        content
    }

    pub(super) fn commit_pending_streaming_assistant_message(&mut self) -> bool {
        if let Some(chunk) = self.stream_buffer.flush() {
            self.append_streaming_text(&chunk);
        }

        if self.streaming_text.is_empty() {
            self.stream_buffer.clear();
            return false;
        }

        let content = self.take_streaming_text();
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
        if self.streaming_tps_collect_output {
            self.streaming_total_output_tokens += delta;
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

        let mut raw_input = std::mem::take(&mut self.input);
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
        self.reset_input_history_browse();
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

        // Issue #4 follow-up: if the user typed `/<name>` (or `/<name> <args>`)
        // and `<name>` is a discovered prompt template, expand the template
        // body in-place so the rest of the submit flow treats it as a normal
        // user message. Built-in slash commands (`/help`, `/quit`, `/export`,
        // etc.) are checked first below, so this only fires for user-defined
        // templates.
        let mut input = expand_prompt_template_invocation(&input).unwrap_or(input);

        // Issue #265 (input history): record submitted input for Up/Down recall.
        // Done after template expansion so the recall menu shows the
        // user-typed string, not the expanded body.
        self.push_input_history(input.clone());

        let trimmed = input.trim();
        let handled = commands::handle_help_command(self, trimmed)
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

                // Issue #125: previously, `/skill-name` activated the skill
                // and returned without sending anything to the model. Some
                // non-Claude providers (zai/GLM, etc.) then stalled on the
                // following turn because the agent had a large skill-injected
                // system prompt with no fresh user message to anchor the
                // conversation.
                //
                // Trigger a real turn now with a short instruction the model
                // can act on. Mirrors what the user would get if they typed
                // `/skill-name please begin.`. The skill body is already
                // wired into the system prompt via active_skill in the agent
                // prompting layer; this just gives the turn a non-empty
                // user message.
                let synthesized = format!(
                    "Please begin applying the `{}` skill that was just activated.",
                    skill.name
                );
                raw_input = synthesized.clone();
                input = synthesized;
                // Fall through to the normal user-message path below, which
                // will push raw_input to display + send `input` to the model.
            } else {
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Unknown skill: /{}", skill_name),
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
        self.stream_buffer.clear();
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.streaming_cache_read_tokens = None;
        self.streaming_cache_creation_tokens = None;
        self.current_api_usage_recorded = false;
        self.upstream_provider = None;
        self.status_detail = None;
        self.streaming_tps_start = None;
        self.streaming_tps_elapsed = Duration::ZERO;
        self.streaming_tps_collect_output = false;
        self.streaming_total_output_tokens = 0;
        self.streaming_tps_observed_output_tokens = 0;
        self.streaming_tps_observed_elapsed = Duration::ZERO;
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
            self.streaming_input_tokens = 0;
            self.streaming_output_tokens = 0;
            self.streaming_cache_read_tokens = None;
            self.streaming_cache_creation_tokens = None;
            self.current_api_usage_recorded = false;
            self.upstream_provider = None;
            self.status_detail = None;
            self.streaming_tps_start = None;
            self.streaming_tps_elapsed = Duration::ZERO;
            self.streaming_tps_collect_output = false;
            self.streaming_total_output_tokens = 0;
            self.streaming_tps_observed_output_tokens = 0;
            self.streaming_tps_observed_elapsed = Duration::ZERO;
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
                }
                Err(e) => {
                    let err_str = crate::util::format_error_chain(&e);
                    if is_context_limit_error(&err_str) {
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
