//! Resolving dropped and pasted file paths into composer text or attachments.
//!
//! Terminals hand us paths in several shapes: bare, quoted, backslash-escaped,
//! or as a `file://` URL. The escaping rules differ per platform, which is the
//! whole reason this lives in one place.

use super::*;

pub(crate) fn format_dropped_path(path: &std::path::Path, quote_for_batch: bool) -> String {
    let value = path.to_string_lossy();
    if quote_for_batch
        && value
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '\\' | '\'' | '"'))
    {
        // On Windows the backslash is the separator, not an escape, so doubling
        // it would produce a path that no longer round-trips through the parser.
        let escaped = if cfg!(windows) {
            value.replace('"', "\\\"")
        } else {
            value.replace('\\', "\\\\").replace('"', "\\\"")
        };
        format!("\"{escaped}\"")
    } else {
        value.into_owned()
    }
}

pub(crate) fn dropped_image_files(text: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let paths = parse_dropped_paths(text)?;
    paths
        .into_iter()
        .map(|path| load_dropped_image(&path))
        .collect()
}

/// Terminal emulators normally send file drops as bracketed paste, but some send
/// the path as ordinary key events. Prepare a complete path-only composer before
/// command/skill routing: attach images, or resolve quoting for ordinary files.
pub(crate) fn promote_dropped_images(app: &mut App) -> bool {
    if attach_dropped_images(app) {
        return true;
    }
    let Some(paths) = parse_dropped_paths(&app.input) else {
        return false;
    };
    let normalized = paths
        .iter()
        .map(|path| format_dropped_path(path, paths.len() > 1))
        .collect::<Vec<_>>()
        .join(" ");
    if normalized == app.input {
        return false;
    }
    app.remember_input_undo_state();
    app.input = normalized;
    app.cursor_pos = app.input.len();
    app.reset_tab_completion();
    true
}

pub(crate) fn attach_dropped_images(app: &mut App) -> bool {
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

pub(crate) fn parse_dropped_paths(text: &str) -> Option<Vec<PathBuf>> {
    if crate::tui::is_ssh_remote() {
        return None;
    }
    let trimmed = text.trim();
    let literal_path = PathBuf::from(trimmed);
    if literal_path.is_file() {
        return Some(vec![literal_path]);
    }

    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut chars = trimmed.chars().peekable();
    while let Some(ch) = chars.next() {
        if escaped {
            token.push(ch);
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') && backslash_escapes(chars.peek().copied()) {
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

/// Whether a backslash at this position is an escape rather than a path separator.
///
/// On Windows `\` separates path components, so treating every one as an escape
/// mangles `C:\Users\...` into `C:Users...`. Shells on Windows still escape a
/// space that way (Git Bash, MSYS), so `\ ` is honoured and every other backslash
/// stays literal. Elsewhere the backslash is an escape character as usual.
fn backslash_escapes(next: Option<char>) -> bool {
    if cfg!(windows) {
        next == Some(' ')
    } else {
        true
    }
}
