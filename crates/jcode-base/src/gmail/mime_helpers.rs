use super::*;

/// Build a raw RFC 5322 message, optionally `multipart/mixed` with file
/// attachments. Returns the full message including headers, suitable for
/// base64url-encoding into the Gmail API `raw` field.
pub(super) fn build_raw_mime(
    recipients: &Recipients<'_>,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
    attachments: &[std::path::PathBuf],
) -> Result<String> {
    let address_headers = recipients.header_lines()?;
    let subject = encode_header_value(subject);
    let mut reply_headers = String::new();
    if let Some(reply_to) = in_reply_to {
        // The In-Reply-To/References headers require an RFC 5322 Message-ID in
        // angle brackets. Callers should already have resolved Gmail API IDs.
        let reply_to = if reply_to.starts_with('<') {
            reply_to.to_string()
        } else {
            format!("<{}>", reply_to)
        };
        reply_headers.push_str(&format!(
            "In-Reply-To: {}\r\nReferences: {}\r\n",
            reply_to, reply_to
        ));
    }

    if attachments.is_empty() {
        return Ok(format!(
            "{}Subject: {}\r\n{}MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{}",
            address_headers, subject, reply_headers, body
        ));
    }

    let boundary = format!(
        "jcode_boundary_{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let mut raw = format!(
        "{}Subject: {}\r\n{}MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"{}\"\r\n\r\n",
        address_headers, subject, reply_headers, boundary
    );

    // Body part.
    raw.push_str(&format!("--{}\r\n", boundary));
    raw.push_str(
        "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n",
    );
    raw.push_str(body);
    raw.push_str("\r\n");

    // Attachment parts.
    for path in attachments {
        let data = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("Failed to read attachment {}: {}", path.display(), e))?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment");
        let mime_type = guess_mime_type(path);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);

        raw.push_str(&format!("--{}\r\n", boundary));
        raw.push_str(&format!(
            "Content-Type: {}; name=\"{}\"\r\n",
            mime_type, file_name
        ));
        raw.push_str("Content-Transfer-Encoding: base64\r\n");
        raw.push_str(&format!(
            "Content-Disposition: attachment; filename=\"{}\"\r\n\r\n",
            file_name
        ));
        // Wrap base64 at 76 chars per RFC 2045.
        for chunk in encoded.as_bytes().chunks(76) {
            raw.push_str(std::str::from_utf8(chunk).unwrap_or(""));
            raw.push_str("\r\n");
        }
    }

    raw.push_str(&format!("--{}--\r\n", boundary));
    Ok(raw)
}

/// RFC 2047 encode a header value when it contains non-ASCII text.
///
/// Gmail stores the `raw` payload verbatim, so a bare UTF-8 subject line gets
/// reinterpreted as Latin-1 by receiving clients (mojibake like "Ã¢Â€Â”") and
/// defeats Gmail's subject-based fallback threading. Encoded words are chunked
/// so each stays within the RFC 2047 limit of 75 characters.
pub(super) fn encode_header_value(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    // 45 input bytes -> 60 base64 chars -> 72 chars with the =?UTF-8?B?...?=
    // wrapper, under the 75-char encoded-word limit.
    const MAX_CHUNK_BYTES: usize = 45;
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if current.len() + ch.len_utf8() > MAX_CHUNK_BYTES && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
        .iter()
        .map(|chunk| {
            format!(
                "=?UTF-8?B?{}?=",
                base64::engine::general_purpose::STANDARD.encode(chunk.as_bytes())
            )
        })
        .collect::<Vec<_>>()
        // Folding whitespace between encoded words is not rendered.
        .join("\r\n ")
}

/// Best-effort MIME type from a file extension for email attachments.
pub(super) fn guess_mime_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("txt") | Some("md") => "text/plain",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        Some("doc") => "application/msword",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => "application/octet-stream",
    }
}
