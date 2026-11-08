use super::{
    AppliedHistoryFingerprint, history_images_match_retained, history_payload_fingerprint,
    should_skip_identical_history_payload,
};
use crate::protocol::HistoryMessage;
use crate::session::{RenderedImage, RenderedImageSource};

fn message(role: &str, content: &str) -> HistoryMessage {
    HistoryMessage {
        response_stats: None,
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_data: None,
    }
}

fn image(data: &str) -> RenderedImage {
    RenderedImage {
        history_message_index: None,
        media_type: "image/png".to_string(),
        data: data.to_string(),
        label: None,
        source: RenderedImageSource::UserInput,
        anchor: None,
    }
}

#[test]
fn identical_payloads_fingerprint_equal() {
    let a = vec![message("user", "hi"), message("assistant", "hello")];
    let b = vec![message("user", "hi"), message("assistant", "hello")];
    assert_eq!(
        history_payload_fingerprint(&a),
        history_payload_fingerprint(&b)
    );
}

#[test]
fn fingerprint_changes_on_content_role_count_and_tool_data() {
    let base = vec![message("user", "hi"), message("assistant", "hello")];
    let fp = history_payload_fingerprint(&base);

    let content = vec![message("user", "hi"), message("assistant", "hello!")];
    assert_ne!(fp, history_payload_fingerprint(&content));

    let role = vec![message("user", "hi"), message("system", "hello")];
    assert_ne!(fp, history_payload_fingerprint(&role));

    let count = vec![message("user", "hi")];
    assert_ne!(fp, history_payload_fingerprint(&count));

    let mut tool = base.clone();
    tool[1].tool_data = Some(super::ToolCall {
        id: "t1".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({"command": "ls"}),
        intent: None,
        thought_signature: None,
    });
    assert_ne!(fp, history_payload_fingerprint(&tool));

    // Same tool call with different input must differ too.
    let mut tool_other = tool.clone();
    tool_other[1].tool_data.as_mut().unwrap().input = serde_json::json!({"command": "pwd"});
    assert_ne!(
        history_payload_fingerprint(&tool),
        history_payload_fingerprint(&tool_other)
    );
}

#[test]
fn skip_decision_requires_same_session_same_fingerprint_and_intact_display() {
    let entry = AppliedHistoryFingerprint {
        session_id: "ses_a".to_string(),
        fingerprint: 42,
    };

    // Exact match with intact display and unchanged session -> skip.
    assert!(should_skip_identical_history_payload(
        false,
        false,
        Some(&entry),
        "ses_a",
        42
    ));
    // Session switch must always re-apply.
    assert!(!should_skip_identical_history_payload(
        true,
        false,
        Some(&entry),
        "ses_a",
        42
    ));
    // A cleared display must be repopulated even for an identical payload.
    assert!(!should_skip_identical_history_payload(
        false,
        true,
        Some(&entry),
        "ses_a",
        42
    ));
    // Different session id -> re-apply.
    assert!(!should_skip_identical_history_payload(
        false,
        false,
        Some(&entry),
        "ses_b",
        42
    ));
    // Different payload (e.g. rewind truncation) -> re-apply.
    assert!(!should_skip_identical_history_payload(
        false,
        false,
        Some(&entry),
        "ses_a",
        43
    ));
    // Nothing applied yet -> re-apply.
    assert!(!should_skip_identical_history_payload(
        false, false, None, "ses_a", 42
    ));
}

#[test]
fn images_match_retained_compares_count_and_lengths() {
    let retained = vec![image("aaaa"), image("bbbbbb")];
    let same = vec![image("aaaa"), image("bbbbbb")];
    assert!(history_images_match_retained(&same, &retained));
    // Length-only comparison: equal lengths count as identical.
    let same_len = vec![image("cccc"), image("dddddd")];
    assert!(history_images_match_retained(&same_len, &retained));

    assert!(!history_images_match_retained(&[], &retained));
    let fewer = vec![image("aaaa")];
    assert!(!history_images_match_retained(&fewer, &retained));
    let diff_len = vec![image("aaaa"), image("bbbbb")];
    assert!(!history_images_match_retained(&diff_len, &retained));
    let mut diff_meta = vec![image("aaaa"), image("bbbbbb")];
    diff_meta[0].media_type = "image/jpeg".to_string();
    assert!(!history_images_match_retained(&diff_meta, &retained));
    assert!(history_images_match_retained(&[], &[]));
}
