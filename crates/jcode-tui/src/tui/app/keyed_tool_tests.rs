use super::*;

fn tool(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "read".into(),
        input: serde_json::Value::Null,
        intent: None,
        thought_signature: None,
    }
}

#[test]
fn keyed_tool_inputs_interleave_and_finish_out_of_order() {
    let mut pending = PendingStreamingTools::default();
    pending.start(tool("a"));
    pending.input(Some("a"), r#"{"file_path":"a","intent":"Read "#);
    pending.start(tool("b"));
    pending.input(Some("b"), r#"{"file_path":"b","intent":"Read B"}"#);
    pending.input(Some("a"), r#"A"}"#);
    pending.signature("a", "signature-a");
    let b = pending.finish(Some("b")).unwrap();
    assert_eq!(b.input["file_path"], "b");
    assert_eq!(b.intent.as_deref(), Some("Read B"));
    assert_eq!(b.thought_signature, None);
    let a = pending.finish(Some("a")).unwrap();
    assert_eq!(a.input["file_path"], "a");
    assert_eq!(a.intent.as_deref(), Some("Read A"));
    assert_eq!(a.thought_signature.as_deref(), Some("signature-a"));
    assert!(pending.is_empty());
}

#[test]
fn keyed_tool_inputs_preserve_unkeyed_fallback_and_ignore_unknown_ids() {
    let mut pending = PendingStreamingTools::default();
    pending.start(tool("legacy"));
    pending.input(None, r#"{"file_path":"legacy"}"#);
    pending.input(Some("unknown"), "corruption");
    assert!(pending.finish(Some("unknown")).is_none());
    assert_eq!(pending.finish(None).unwrap().input["file_path"], "legacy");
    assert!(pending.finish(None).is_none());
}

#[test]
fn keyed_tool_inputs_rollback_and_interruption_cover_all_pending_calls() {
    let mut pending = PendingStreamingTools::default();
    pending.start(tool("a"));
    pending.input(None, r#"{"file_path":"a"}"#);
    pending.start(tool("b"));
    pending.input(None, r#"{"file_path":"b"}"#);
    let calls: Vec<_> = pending.drain().collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].input["file_path"], "a");
    assert_eq!(calls[1].input["file_path"], "b");
    assert!(pending.is_empty());
    pending.start(tool("old"));
    pending.clear();
    pending.start(tool("new"));
    pending.input(Some("old"), "stale");
    pending.input(None, r#"{"file_path":"new"}"#);
    assert!(pending.finish(Some("old")).is_none());
    assert_eq!(pending.finish(None).unwrap().input["file_path"], "new");
}
