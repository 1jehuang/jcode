use super::*;

pub(in crate::tui::app) fn handle_paste(app: &mut App, text: String) {
    if app.paste_account_manager_prompt(&text) {
        return;
    }
    if app.append_ssh_login_input(&text) {
        return;
    }
    paste_guard::note_paste();
    if crate::tui::is_ssh_remote() {
        // Paths belong to the remote machine, not the client's filesystem.
        handle_text_paste(app, text);
        app.set_status_notice("SSH paste: text only. File paths refer to the remote host; paste image bytes from the clipboard to attach an image.");
        return;
    }
    // Bracketed paste delivers text. Clipboard images use explicit shortcuts.
    if let Some(paths) = parse_dropped_paths(&text) {
        let item_count = paths.len();
        let mut image_count = 0;
        let mut file_count = 0;
        for (index, path) in paths.into_iter().enumerate() {
            if index > 0 {
                insert_input_text(app, " ");
            }
            if let Some((media_type, data)) = load_dropped_image(&path) {
                attach_image(
                    app,
                    media_type,
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
    } else if let Some(url) = super::super::extract_image_url(&text) {
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
