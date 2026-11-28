use super::super as app_mod;
use super::*;

pub(in crate::tui::app) fn paste_from_clipboard(app: &mut App) {
    app.set_status_notice("Reading clipboard...");
    spawn_clipboard_paste(app, ClipboardPasteKind::Smart);
}

pub(super) fn is_clipboard_paste_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('v' | 'V'))
        && modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::META,
        )
}

pub(super) fn active_clipboard_session_id(app: &App) -> String {
    app.active_client_session_id()
        .unwrap_or(app.session.id.as_str())
        .to_string()
}

pub(super) fn publish_clipboard_result(
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

pub(super) fn spawn_blocking_or_thread<F>(task: F)
where
    F: FnOnce() + Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(task);
    } else {
        std::thread::spawn(task);
    }
}

fn image_content(media_type: String, base64_data: String) -> ClipboardPasteContent {
    ClipboardPasteContent::Image {
        media_type,
        base64_data,
    }
}

pub(super) fn download_image_url_content(url: &str) -> Option<ClipboardPasteContent> {
    if crate::tui::is_ssh_remote() {
        return None;
    }
    app_mod::download_image_url(url)
        .map(|(media_type, base64_data)| image_content(media_type, base64_data))
}

fn read_clipboard_for_paste(kind: &ClipboardPasteKind) -> ClipboardPasteContent {
    match kind {
        ClipboardPasteKind::Smart => {
            // Native-only reader first, so copied web text whose HTML carries
            // an <img> stays text. Only when the clipboard offers nothing else
            // fall back to the HTML <img> URL path (e.g. Discord on Wayland).
            match read_clipboard_for_paste_with(
                kind,
                read_clipboard_text,
                app_mod::clipboard_image_native,
                download_image_url_content,
                read_clipboard_file_list,
            ) {
                // The HTML <img> fallback downloads a URL; never do that
                // implicitly over SSH (direct image-URL paste has the same rule).
                ClipboardPasteContent::Empty if !crate::tui::is_ssh_remote() => {
                    app_mod::clipboard_image()
                        .map(|(media_type, base64_data)| image_content(media_type, base64_data))
                        .unwrap_or(ClipboardPasteContent::Empty)
                }
                content => content,
            }
        }
        _ => {
            // ImageOnly and ImageUrl can use full clipboard_image with HTML fallback
            read_clipboard_for_paste_with(
                kind,
                read_clipboard_text,
                app_mod::clipboard_image,
                download_image_url_content,
                read_clipboard_file_list,
            )
        }
    }
}

/// True when clipboard text is just a reference to a picture: a single line
/// that is a local path, `file://` URL, or bare file name with an image
/// extension. Copying an image commonly leaves exactly this on the text
/// target alongside the image bytes. Web URLs are excluded: those go through
/// the explicit image-URL download path instead.
pub(super) fn text_names_an_image(
    text: &str,
    copied_files: impl FnOnce() -> Vec<std::path::PathBuf>,
) -> bool {
    let text = text.trim();
    if text.contains('\n') || text.len() > 4096 {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return false;
    }
    let unquoted = text.trim_matches(['\'', '"']);
    let quoted = unquoted.len() != text.len();
    let path = unquoted
        .strip_prefix("file://")
        .or_else(|| unquoted.strip_prefix("FILE://"))
        .unwrap_or(unquoted);
    if !paste_guard::has_image_extension(std::path::Path::new(path)) {
        return false;
    }
    if !path.chars().any(char::is_whitespace) {
        return true;
    }
    if quoted && (path.starts_with('/') || path.starts_with("~/")) {
        return true;
    }
    if !crate::tui::is_ssh_remote()
        && names_existing_file(
            path,
            dirs::home_dir(),
            match std::env::current_dir() {
                Ok(cwd) => Some(cwd),
                Err(error) => {
                    crate::logging::info(&format!("clipboard path lookup: {error}"));
                    None
                }
            },
        )
    {
        return true;
    }
    copied_files()
        .iter()
        .any(|file| file.as_os_str() == path || file.file_name().is_some_and(|name| name == path))
}

pub(super) fn names_existing_file(
    path: &str,
    home: Option<std::path::PathBuf>,
    cwd: Option<std::path::PathBuf>,
) -> bool {
    let candidate = if let Some(rest) = path.strip_prefix("~/") {
        home.map(|home| home.join(rest))
    } else if std::path::Path::new(path).is_absolute() {
        Some(std::path::PathBuf::from(path))
    } else {
        cwd.map(|cwd| cwd.join(path))
    };
    candidate.is_some_and(|candidate| candidate.is_file())
}

fn read_clipboard_file_list() -> Vec<std::path::PathBuf> {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            crate::logging::info(&format!("clipboard unavailable for file list: {error}"));
            return Vec::new();
        }
    };
    match clipboard.get().file_list() {
        Ok(files) => files,
        Err(error) => {
            crate::logging::info(&format!("clipboard has no file list: {error}"));
            Vec::new()
        }
    }
}

pub(super) fn read_clipboard_for_paste_with<ReadText, ReadImage, DownloadImageUrl>(
    kind: &ClipboardPasteKind,
    mut read_text: ReadText,
    mut read_image: ReadImage,
    mut download_image_url: DownloadImageUrl,
    copied_files: impl FnOnce() -> Vec<std::path::PathBuf>,
) -> ClipboardPasteContent
where
    ReadText: FnMut() -> Option<String>,
    ReadImage: FnMut() -> Option<(String, String)>,
    DownloadImageUrl: FnMut(&str) -> Option<ClipboardPasteContent>,
{
    match kind {
        ClipboardPasteKind::Smart => {
            // Ordinary text must not wait for an image probe.
            let text = read_text().filter(|t| !t.trim().is_empty());
            // Copied images often advertise their filename alongside native bytes.
            let text_is_image_name = text
                .as_deref()
                .is_some_and(|text| text_names_an_image(text, copied_files));
            if (text.is_none() || text_is_image_name)
                && let Some((media_type, base64_data)) = read_image()
            {
                return image_content(media_type, base64_data);
            }
            if let Some(text) = text {
                if let Some(url) = app_mod::extract_image_url(&text)
                    && let Some(content) = download_image_url(&url)
                {
                    return content;
                }
                return ClipboardPasteContent::Text(text);
            }
            ClipboardPasteContent::Empty
        }
        ClipboardPasteKind::ImageOnly => {
            if let Some((media_type, base64_data)) = read_image() {
                return image_content(media_type, base64_data);
            }
            if let Some(text) = read_text() {
                if let Some(url) = app_mod::extract_image_url(&text) {
                    return download_image_url(&url).unwrap_or_else(|| {
                        ClipboardPasteContent::Error("Failed to download image".to_string())
                    });
                }
                return ClipboardPasteContent::Text(text);
            }
            ClipboardPasteContent::Empty
        }
        ClipboardPasteKind::ImageUrl { fallback_text } => {
            let Some(url) = fallback_text
                .as_deref()
                .and_then(app_mod::extract_image_url)
            else {
                return ClipboardPasteContent::Empty;
            };
            download_image_url(&url).unwrap_or_else(|| {
                ClipboardPasteContent::Error("Failed to download image".to_string())
            })
        }
    }
}
