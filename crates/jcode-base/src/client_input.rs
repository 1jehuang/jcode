//! Client-input handoff files (`~/.jcode/client-input-<session>`).
//!
//! When a client queues a startup submission for a session (e.g. a headed
//! spawn or a reload), it is persisted to a small JSON file that the next
//! client process restores on launch. This writer lives below the TUI layer so
//! non-TUI producers (notably the server, when preparing a visible spawn) can
//! stage a startup message without depending on `tui::App`.

/// Persist a startup submission for `session_id` to be restored on next launch.
///
/// No-op when both `input` and `pending_images` are empty. Best-effort: any
/// filesystem error is silently ignored (the submission is simply not staged).
pub fn save_startup_submission_for_session(
    session_id: &str,
    input: String,
    pending_images: Vec<(String, String)>,
) {
    if input.trim().is_empty() && pending_images.is_empty() {
        return;
    }
    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
        let path = jcode_dir.join(format!("client-input-{}", session_id));
        let data = serde_json::json!({
            "cursor": input.len(),
            "input": input,
            "pending_images": pending_images.iter().map(|(media_type, data)| serde_json::json!({
                "media_type": media_type,
                "data": data,
            })).collect::<Vec<_>>(),
            "submit_on_restore": true,
            "queued_messages": [],
            "hidden_queued_system_messages": [],
            "startup_status_notice": "Startup prompt queued",
            "startup_display_message_title": serde_json::Value::Null,
            "startup_display_message": serde_json::Value::Null,
            "interleave_message": serde_json::Value::Null,
            "pending_soft_interrupts": [],
            "pending_soft_interrupt_resend": [],
            "rate_limit_pending_message": serde_json::Value::Null,
            "rate_limit_reset_in_ms": serde_json::Value::Null,
            "observe_mode_enabled": false,
            "observe_page_markdown": "",
            "observe_page_updated_at_ms": 0,
            "split_view_enabled": false,
            "todos_view_enabled": false,
        });
        let _ = std::fs::write(&path, data.to_string());
    }
}
