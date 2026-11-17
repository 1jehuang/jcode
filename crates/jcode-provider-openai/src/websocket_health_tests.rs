use super::*;
use serde_json::Value;

#[test]
fn shared_classification_preserves_wire_semantics() {
    let phrase = WEBSOCKET_FALLBACK_NOTICE;
    let mut cases = vec![
        "not json".to_owned(),
        "null".to_owned(),
        "[]".to_owned(),
        "42".to_owned(),
        r#"{"type":"response.output_text.delta","delta":"hello"}"#.to_owned(),
        r#"{"type":"response.output_text.delta","delta":"error response.completed"}"#.to_owned(),
        r#"{"type":"error"}"#.to_owned(),
        r#"{"type":"ping"}"#.to_owned(),
        r#"{"type":""}"#.to_owned(),
        r#"{"type":null}"#.to_owned(),
        r#"{"type":{}}"#.to_owned(),
        r#"{"nested":{"type":"response.completed"}}"#.to_owned(),
        r#"{"type":"response.created","type":"ping"}"#.to_owned(),
        r#"{"type":"ping","type":"response.created"}"#.to_owned(),
        r#"{"type":"error","type":false}"#.to_owned(),
        r#"{"type":false,"type":"error"}"#.to_owned(),
        r#"{"ty\u0070e":"response.created"}"#.to_owned(),
        r#"{"type":"response.created","bad":1e999}"#.to_owned(),
        r#"{"type":"response.created"} trailing"#.to_owned(),
        r#"{"type":"response.created","delta":"unterminated}"#.to_owned(),
        format!(
            "{{\"type\":\"response.created\",\"nested\":{}0{}}}",
            "[".repeat(130),
            "]".repeat(130)
        ),
        phrase.to_owned(),
        phrase.to_uppercase(),
        phrase.replace("back", "bacK"),
    ];
    for kind in ["response.output_text.delta", "error", "ping", ""] {
        cases.push(serde_json::json!({"type":kind,"delta":phrase}).to_string());
    }
    cases.push(format!(r#"{{"type":"response.created","delta":"{phrase}""#));
    cases.push(format!(
        r#"{{"type":"response.created","type":null,"delta":"{phrase}"}}"#
    ));
    for data in cases {
        let legacy = serde_json::from_str::<Value>(&data).ok();
        let kind = legacy
            .as_ref()
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str);
        let activity = kind.is_some_and(|kind| kind.starts_with("response.") || kind == "error");
        let first = kind.is_some_and(|kind| !kind.is_empty());
        let fallback = !activity && data.to_lowercase().contains(phrase);
        let event = WebsocketEventClassification::parse(&data);
        assert_eq!(event.kind(), kind, "{data}");
        assert_eq!(event.is_activity(), activity, "{data}");
        assert_eq!(event.is_first_activity(), first, "{data}");
        assert_eq!(event.is_fallback_notice(&data), fallback, "{data}");
        assert_eq!(is_websocket_fallback_notice(&data), fallback, "{data}");
    }
}
