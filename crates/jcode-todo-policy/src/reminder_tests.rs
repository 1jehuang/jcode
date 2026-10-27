use super::*;

#[test]
fn actionable_reminder_contains_only_header_and_list() {
    let todos = [
        ("standalone-capture", "Capture standalone native source CLI"),
        ("standalone-gates", "Validate standalone capture and ship"),
        (
            "complete-input-capture",
            "Complete consistent native input capture",
        ),
    ]
    .map(|(id, content)| TodoItem {
        id: id.into(),
        content: content.into(),
        ..Default::default()
    });
    let message = build_actionable_poke_message(&todos);
    assert_eq!(
        message,
        "You have 3 incomplete todos. Continue working, or update the todo tool.\nActionable todos:\n- standalone-capture: \"Capture standalone native source CLI\"\n- standalone-gates: \"Validate standalone capture and ship\"\n- complete-input-capture: \"Complete consistent native input capture\""
    );
    assert!(is_auto_poke_message(&message));
}

#[test]
fn saved_actionable_reminders_remain_synthetic_with_legacy_guidance() {
    let prefix = "You have 1 incomplete todo. Continue working, or update the todo tool.\nActionable todos:\n- capture: \"Capture input\"";
    for suffix in [
        "",
        "\nLeave todos waiting on still-running background tasks alone until their dependencies finish. Leave externally blocked todos alone until their review deadlines or an explicit blocker update. Leave todos marked Absolutely requires user action alone until the user performs the required action and the condition is cleared. Continue independent actionable work.",
        "\nLeave todos waiting on still-running background tasks alone until their dependencies finish. Leave externally blocked todos alone until their review deadlines or an explicit blocker update. Leave todos marked Completely blocked alone until the condition is cleared. Continue independent actionable work.",
    ] {
        let message = format!("{prefix}{suffix}");
        assert!(is_auto_poke_message(&message));
        assert_eq!(
            auto_poke_display_summary(&message),
            Some("Auto-poking: incomplete todos...")
        );
    }
}
