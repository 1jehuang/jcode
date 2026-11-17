use super::*;

/// Convert stored messages into the mobile wire format. One stored message
/// becomes at most one wire message; `seq` is the stored index so uploads are
/// stable and idempotent.
pub fn wire_messages(session: &Session, start: usize) -> Vec<WireMessage> {
    session
        .messages
        .iter()
        .enumerate()
        .skip(start)
        .filter_map(|(seq, message)| {
            // Match desktop history visibility, including images accompanying
            // model-only generated-image visual context reminders.
            if message
                .content
                .iter()
                .find_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.trim_start()),
                    _ => None,
                })
                .is_some_and(|text| text.starts_with("<system-reminder>"))
            {
                return None;
            }
            let mut images = attached_images(&message.content);
            let mut text = BoundedText::new(MAX_TEXT_CHARS);
            let mut has_parts = false;
            let mut is_tool_result = false;
            for block in &message.content {
                // Even after the text limit, later tool results change the role.
                if matches!(block, ContentBlock::ToolResult { .. }) {
                    is_tool_result = true;
                }
                if text.truncated {
                    continue;
                }
                let part = match block {
                    ContentBlock::Text { text, .. }
                        if !text.trim_start().is_empty()
                            && !text.trim_start().starts_with("<system-reminder>") =>
                    {
                        std::borrow::Cow::Borrowed(text.as_str())
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        if has_parts {
                            text.push("\n\n");
                        }
                        append_tool_summary(&mut text, name, input);
                        has_parts = true;
                        continue;
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        std::borrow::Cow::Owned(truncate(content, MAX_TOOL_RESULT_CHARS))
                    }
                    _ => continue,
                };
                if has_parts {
                    text.push("\n\n");
                }
                text.push(&part);
                has_parts = true;
            }
            if !has_parts && images.is_empty() {
                return None;
            }
            let role = if is_tool_result {
                "tool"
            } else if message.display_role.is_some() {
                "system"
            } else {
                match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
            };
            let structured = structured_tools(&message.content);
            let (display_text, tool_calls, tool_results) = match structured {
                Some((prose, calls, results)) => (Some(prose), calls, results),
                None => (None, Vec::new(), Vec::new()),
            };
            if role == "assistant" && images.len() < 4 {
                images.extend(
                    generated_images(&text.text)
                        .into_iter()
                        .take(4 - images.len()),
                );
            }
            Some(WireMessage {
                seq,
                role,
                display_text,
                tool_calls,
                tool_results,
                images,
                text: text.text,
                created_at: message.timestamp.map(|t| t.to_rfc3339()),
            })
        })
        .collect()
}

pub(super) fn message_chunks(messages: &[WireMessage]) -> Vec<&[WireMessage]> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (index, message) in messages.iter().enumerate() {
        let size = serde_json::to_vec(message).map_or(0, |v| v.len());
        if index > start
            && (bytes + size > 3 * 1024 * 1024 || index - start >= MAX_MESSAGES_PER_UPLOAD)
        {
            chunks.push(&messages[start..index]);
            start = index;
            bytes = 0;
        }
        bytes += size;
    }
    if start < messages.len() {
        chunks.push(&messages[start..]);
    }
    chunks
}

// Only explicit markdown image destinations under the generated-image store
// are eligible. Canonicalization rejects traversal and symlink escapes.
pub(super) fn generated_images(text: &str) -> Vec<WireImage> {
    let Ok(root) =
        storage::jcode_dir().and_then(|p| Ok(p.join("generated-images").canonicalize()?))
    else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut images = Vec::new();
    for fragment in text.split("![").skip(1).take(64) {
        let Some((_, destination)) = fragment.split_once("](") else {
            continue;
        };
        let Some((destination, _)) = destination.split_once(')') else {
            continue;
        };
        let path = Path::new(destination.trim().trim_start_matches("file://"));
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        if !path.starts_with(&root) || !seen.insert(path.clone()) {
            continue;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Do not follow a file swapped to a symlink after canonicalization,
            // and never let a special file block the transcript sync worker.
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let Ok(file) = options.open(&path) else {
            continue;
        };
        let Ok(meta) = file.metadata() else {
            continue;
        };
        if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
            continue;
        }
        if let Some(thumbnail) = image_thumbnail(std::io::BufReader::new(file)) {
            images.push(thumbnail);
        }
        if images.len() == 4 {
            break;
        }
    }
    images
}

// ToolOutput images are persisted as Image blocks following their ToolResult,
// exactly like user attachments. Never resolve attachment data as a local path.
pub(super) fn attached_images(blocks: &[ContentBlock]) -> Vec<WireImage> {
    let mut images = Vec::new();
    for block in blocks {
        let ContentBlock::Image { media_type, data } = block else {
            continue;
        };
        if !matches!(
            media_type.as_str(),
            "image/jpeg" | "image/png" | "image/webp" | "image/gif"
        ) || data.len() > 4 * (16 * 1024 * 1024_usize).div_ceil(3)
        {
            continue;
        }
        if let Some(image) = thumbnail_cache::attachment_thumbnail(data) {
            images.push(image);
            if images.len() == 4 {
                break;
            }
        }
    }
    images
}

pub(super) fn image_thumbnail(reader: impl std::io::BufRead + std::io::Seek) -> Option<WireImage> {
    use base64::Engine;
    let mut reader = match image::ImageReader::new(reader).with_guessed_format() {
        Ok(reader) => reader,
        Err(error) => {
            crate::logging::debug(&format!("Cannot identify mobile thumbnail format: {error}"));
            return None;
        }
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut thumbnail = match reader.decode() {
        Ok(image) => image.thumbnail(1024, 1024).to_rgb8(),
        Err(error) => {
            crate::logging::debug(&format!("Cannot decode mobile thumbnail: {error}"));
            return None;
        }
    };
    loop {
        let mut bytes = Vec::new();
        if let Err(error) = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
            .encode_image(&thumbnail)
        {
            crate::logging::debug(&format!("Cannot encode mobile thumbnail: {error}"));
            return None;
        }
        if bytes.len() <= 256 * 1024 {
            return Some(WireImage {
                media_type: "image/jpeg".into(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }
        thumbnail = image::imageops::thumbnail(
            &thumbnail,
            (thumbnail.width() / 2).max(1),
            (thumbnail.height() / 2).max(1),
        );
    }
}
