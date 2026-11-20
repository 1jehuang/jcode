use super::*;

#[test]
fn opaque_rejection_uses_validated_top_level_type() {
    for data in [
        r#"{"type":"ping","delta":"response.created"}"#,
        r#"{"type":"ping","nested":{"type":"response.created"}}"#,
        r#"{"type":"response.created","type":null}"#,
        r#"{"type":"response.created"} trailing"#,
        r#"{"type":"response.created","invalid":1e999}"#,
    ] {
        let mut started = false;
        assert!(!opaque_ws_rejection(data, &mut started));
        assert!(!started, "{data}");
    }
    let mut started = false;
    assert!(!opaque_ws_rejection(
        r#"{"type":"ping","ty\u0070e":"response.created"}"#,
        &mut started
    ));
    assert!(started);
    let mut started = false;
    assert!(opaque_ws_rejection(
        r#"{"type":"response.created","type":"error","error":{"type":"invalid_request_error","message":"Invalid request"}}"#,
        &mut started
    ));
    assert!(!started);
}

#[test]
fn opaque_rejection_requires_exact_shape_and_no_response_activity() {
    let generic = serde_json::json!({"type":"error","error":{"type":"invalid_request_error","message":"Invalid request"}});
    assert!(opaque_ws_rejection(&generic.to_string(), &mut false));
    for key in ["code", "param", "detail"] {
        let mut detailed = generic.clone();
        detailed["error"][key] = Value::String("specific validation failure".into());
        assert!(!opaque_ws_rejection(&detailed.to_string(), &mut false));
    }
    let mut metadata = generic.clone();
    metadata["sequence_number"] = serde_json::json!(0);
    metadata["event_id"] = serde_json::json!("event_fixture");
    metadata["request_id"] = serde_json::json!("request_fixture");
    metadata["response"] = Value::Null;
    metadata["error"]["code"] = Value::Null;
    assert!(opaque_ws_rejection(&metadata.to_string(), &mut false));
    metadata["status"] = serde_json::json!(400);
    assert!(opaque_ws_rejection(&metadata.to_string(), &mut false));
    metadata["status"] = serde_json::json!(429);
    assert!(!opaque_ws_rejection(&metadata.to_string(), &mut false));
    metadata["status"] = serde_json::json!(400);
    metadata["response"] = serde_json::json!({"id":"resp_started"});
    assert!(!opaque_ws_rejection(&metadata.to_string(), &mut false));
    metadata["response"] = Value::Null;
    metadata["detail"] = serde_json::json!("specific validation failure");
    assert!(!opaque_ws_rejection(&metadata.to_string(), &mut false));
    for kind in [
        "response.created",
        "response.output_text.delta",
        "response.reasoning_summary_text.delta",
        "response.function_call_arguments.delta",
    ] {
        let mut started = false;
        assert!(!opaque_ws_rejection(
            &serde_json::json!({"type":kind}).to_string(),
            &mut started
        ));
        assert!(!opaque_ws_rejection(&generic.to_string(), &mut started));
    }
}
