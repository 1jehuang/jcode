use super::*;

#[test]
fn stored_actionable_reminders_render_as_short_system_notices() {
    let prefix = "You have 1 incomplete todo. Continue working, or update the todo tool.\nActionable todos:\n- capture: \"Capture input\"";
    for suffix in [
        "",
        "\nLeave todos waiting on still-running background tasks alone until their dependencies finish. Leave todos marked Absolutely requires user action alone until the user performs the required action and the condition is cleared. Continue independent actionable work.",
        "\nLeave todos marked Completely blocked alone until the condition is cleared. Continue independent actionable work.",
    ] {
        let stored = crate::session::StoredMessage {
            id: "saved-reminder".into(),
            role: Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: format!("{prefix}{suffix}"),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        };
        let display = display_message_from_stored_message(&stored)
            .expect("restored reminder should have a compact notice");
        assert_eq!(display.role, DisplayMessage::system(String::new()).role);
        assert_eq!(display.content, "Auto-poking: incomplete todos...");
    }
}
