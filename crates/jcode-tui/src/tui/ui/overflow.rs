use super::*;

pub(super) fn transcript_definitely_overflows(
    messages: &[DisplayMessage],
    available_rows: usize,
) -> bool {
    let mut rows = 0usize;
    for message in messages {
        let role = message.effective_role();
        // Tool results can be hidden (notably pinned todos). These three roles
        // do not add the usual inter-message separator either.
        if rows > 0 && messages::needs_message_separator(message) {
            rows = rows.saturating_add(1);
        }
        rows = rows.saturating_add(match role {
            // Voice/transcription normalization can remove source lines, but
            // even an empty user prompt still emits its numbered first row.
            "user" | "meta" | "error" => 1,
            "spacer" => message
                .content
                .trim()
                .parse::<usize>()
                .unwrap_or(0)
                .saturating_sub(1),
            _ => 0,
        });
        if rows > available_rows {
            return true;
        }
    }
    false
}
