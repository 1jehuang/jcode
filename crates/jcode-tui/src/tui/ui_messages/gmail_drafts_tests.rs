fn gmail_draft_message(content: &str, input: serde_json::Value) -> DisplayMessage {
    DisplayMessage {
        tool_duration_ms: None,
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_gmail_draft".to_string(),
            name: "gmail".to_string(),
            input,
            intent: None,
            thought_signature: None,
        }),
    }
}

#[test]
fn render_tool_message_shows_gmail_draft_card() {
    let msg = gmail_draft_message(
        "Draft created successfully.\nDraft ID: draft_123\nTo: bob@example.com\nSubject: Project update",
        serde_json::json!({
            "action": "draft",
            "to": "bob@example.com",
            "subject": "Project update",
            "body": "Hi Bob,\n\nThe release is ready for review."
        }),
    );

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Gmail draft created · draft_123"), "{plain}");
    assert!(
        !plain.contains('✉'),
        "draft card should not show an icon: {plain}"
    );
    assert!(plain.contains("To: bob@example.com"), "{plain}");
    assert!(plain.contains("Subject: Project update"), "{plain}");
    assert!(
        plain.contains("The release is ready for review."),
        "{plain}"
    );
    assert!(
        !plain.contains("\"body\""),
        "must not leak raw JSON: {plain}"
    );
}

#[test]
fn render_gmail_draft_failure_keeps_normal_header_without_redundant_card() {
    let msg = gmail_draft_message(
        "Error: Gmail draft creation failed",
        serde_json::json!({ "action": "draft", "body": "" }),
    );

    let lines = render_tool_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(lines.len(), 2, "{plain}");
    assert!(plain.contains("✗ gmail draft"), "{plain}");
    assert!(plain.contains("╰─Gmail draft creation failed"), "{plain}");
    assert!(!plain.contains("(recipient missing)"), "{plain}");
    assert!(!plain.contains("(no subject)"), "{plain}");
    assert!(!plain.contains("(empty body)"), "{plain}");
}

#[test]
fn render_gmail_draft_card_wraps_attachments_and_shows_complete_long_body() {
    let body = (1..=30)
        .map(|index| format!("body line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let msg = gmail_draft_message(
        "Draft created successfully.\nDraft ID: draft_long",
        serde_json::json!({
            "action": "draft",
            "to": "a-very-long-recipient-address@example.com",
            "subject": "A subject that should wrap cleanly in a narrow transcript",
            "body": body,
            "attachments": [
                "/tmp/a-very-long-quarterly-report-filename.pdf",
                "/tmp/notes.txt"
            ]
        }),
    );

    let lines = render_tool_message(&msg, 48, crate::config::DiffDisplayMode::Off);
    let rendered = lines.iter().map(extract_line_text).collect::<Vec<_>>();
    let plain = rendered.join("\n");
    let compact = without_whitespace(&plain.replace('│', ""));

    assert!(
        compact.contains("To:a-very-long-recipient-address@example.com"),
        "{plain}"
    );
    assert!(
        compact.contains("Subject:Asubjectthatshouldwrapcleanlyinanarrowtranscript"),
        "{plain}"
    );
    assert!(
        compact
            .contains("Attachments:/tmp/a-very-long-quarterly-report-filename.pdf,/tmp/notes.txt"),
        "{plain}"
    );
    assert!(plain.contains("body line 18"), "{plain}");
    assert!(plain.contains("body line 19"), "{plain}");
    assert!(plain.contains("body line 30"), "{plain}");
    assert!(
        !plain.contains("more lines"),
        "body must not be truncated: {plain}"
    );
    assert!(
        lines.iter().all(|line| line.width() <= 47),
        "draft card exceeded row width: {rendered:?}"
    );
}

#[test]
fn render_gmail_draft_card_preserves_html_like_body_text() {
    let msg = gmail_draft_message(
        "Draft created successfully.\nDraft ID: draft_html",
        serde_json::json!({
            "action": "draft",
            "to": "web@example.com",
            "subject": "HTML-ish content",
            "body": "<p>Hello <strong>team</strong></p>"
        }),
    );

    let plain = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("<p>Hello <strong>team</strong></p>"),
        "{plain}"
    );
}

#[test]
fn render_batch_tool_message_shows_nested_gmail_draft_card() {
    let msg = DisplayMessage {
        tool_duration_ms: None,
        role: "tool".to_string(),
        content: "--- [1] gmail ---\nDraft created successfully.\nDraft ID: nested_123\nTo: nested@example.com\nSubject: Nested\n\nCompleted: 1 succeeded, 0 failed".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "call_batch_gmail".to_string(),
            name: "batch".to_string(),
            input: serde_json::json!({
                "tool_calls": [{
                    "tool": "gmail",
                    "parameters": {
                        "action": "draft",
                        "to": "nested@example.com",
                        "subject": "Nested",
                        "body": "Created inside a batch"
                    }
                }]
            }),
            intent: None,
            thought_signature: None,
        }),
    };

    let lines = render_tool_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let plain = lines
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plain.contains("Gmail draft created · nested_123"),
        "{plain}"
    );
    assert!(plain.contains("Created inside a batch"), "{plain}");
}
