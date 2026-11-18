use super::*;

/// "To: ...\nCc: ...\nBcc: ..." lines for whichever recipient headers are
/// present. Bcc only appears on the sender's own copy and on drafts.
pub fn format_recipient_lines(msg: &Message, indent: &str) -> String {
    ["To", "Cc", "Bcc"]
        .iter()
        .filter_map(|name| {
            msg.header(name)
                .filter(|v| !v.trim().is_empty())
                .map(|v| format!("{}{}: {}", indent, name, v))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_message_summary(msg: &Message) -> String {
    let from = msg.from().unwrap_or("(unknown)");
    let subject = msg.subject().unwrap_or("(no subject)");
    let date = msg.date().unwrap_or("");
    let snippet = msg.snippet.as_deref().unwrap_or("");
    let labels = msg
        .label_ids
        .as_ref()
        .map(|l| l.join(", "))
        .unwrap_or_default();

    let recipients = format_recipient_lines(msg, "");
    let recipients = if recipients.is_empty() {
        String::new()
    } else {
        format!("{}\n", recipients)
    };
    format!(
        "From: {}\n{}Subject: {}\nDate: {}\nLabels: {}\nSnippet: {}\nID: {}",
        from, recipients, subject, date, labels, snippet, msg.id
    )
}

pub fn format_message_full(msg: &Message) -> String {
    let mut out = format_message_summary(msg);
    let attachments = msg.attachments();
    if !attachments.is_empty() {
        out.push_str(&format!("\nAttachments ({}):\n", attachments.len()));
        out.push_str(&format_attachment_lines(&attachments));
    }
    if let Some(body) = msg.body_text() {
        out.push_str("\n\n--- Body ---\n");
        out.push_str(&body);
    }
    out
}

/// One "  - name (mime, size)" line per attachment.
pub fn format_attachment_lines(attachments: &[AttachmentInfo]) -> String {
    attachments
        .iter()
        .map(|a| {
            let mut details = Vec::new();
            if let Some(ref mime) = a.mime_type {
                details.push(mime.clone());
            }
            if let Some(size) = a.size {
                details.push(format_size(size));
            }
            if details.is_empty() {
                format!("  - {}", a.filename)
            } else {
                format!("  - {} ({})", a.filename, details.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn format_size(bytes: u32) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}
