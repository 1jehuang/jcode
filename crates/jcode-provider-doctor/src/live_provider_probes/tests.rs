use super::*;
use jcode_base::provider_catalog::resolve_openai_compatible_profile;
use jcode_provider_metadata::{GEMINI_OPENAI_COMPAT_PROFILE, OPENAI_NATIVE_OPENAI_COMPAT_PROFILE};

#[test]
fn gemini_openai_compat_strips_models_prefix_from_catalog_ids() {
    let resolved = resolve_openai_compatible_profile(GEMINI_OPENAI_COMPAT_PROFILE);
    assert_eq!(
        normalize_openai_compatible_model_id(&resolved, "models/gemini-2.5-flash"),
        "gemini-2.5-flash"
    );
    // Already-bare ids pass through unchanged.
    assert_eq!(
        normalize_openai_compatible_model_id(&resolved, "gemini-2.5-pro"),
        "gemini-2.5-pro"
    );
}

#[test]
fn non_gemini_openai_compat_leaves_model_ids_untouched() {
    let resolved = resolve_openai_compatible_profile(OPENAI_NATIVE_OPENAI_COMPAT_PROFILE);
    // A leading `models/` segment on a non-Gemini host is not stripped.
    assert_eq!(
        normalize_openai_compatible_model_id(&resolved, "models/gpt-5.1"),
        "models/gpt-5.1"
    );
    assert_eq!(
        normalize_openai_compatible_model_id(&resolved, "gpt-5.1"),
        "gpt-5.1"
    );
}

fn tool_call_with_signature(signature: Option<&str>) -> NativeClaudeToolCall {
    NativeClaudeToolCall {
        id: "call_1".to_string(),
        name: "read".to_string(),
        input_json: "{}".to_string(),
        thought_signature: signature.map(str::to_string),
    }
}

#[test]
fn reasoning_capability_classifies_streamed_when_reasoning_text_present() {
    let outcome = NativeClaudeStreamOutcome {
        reasoning_text_len: 42,
        saw_message_end: true,
        ..Default::default()
    };
    assert_eq!(outcome.reasoning_capability(), "streamed");
}

#[test]
fn reasoning_capability_classifies_opaque_from_thinking_signature() {
    // No reasoning text, but a ThinkingSignatureDelta-style signal: opaque.
    let outcome = NativeClaudeStreamOutcome {
        saw_reasoning_signal: true,
        saw_message_end: true,
        ..Default::default()
    };
    assert_eq!(outcome.reasoning_capability(), "opaque");
}

#[test]
fn reasoning_capability_classifies_opaque_from_tool_thought_signature() {
    // A Gemini-3 tool call carrying a thought_signature is an opaque signal
    // even when no reasoning text streamed.
    let outcome = NativeClaudeStreamOutcome {
        tool_calls: vec![tool_call_with_signature(Some("SIG_ABC"))],
        saw_message_end: true,
        ..Default::default()
    };
    assert_eq!(outcome.reasoning_capability(), "opaque");
}

#[test]
fn reasoning_capability_classifies_none_without_any_signal() {
    // A tool call with no signature is not a reasoning signal.
    let outcome = NativeClaudeStreamOutcome {
        tool_calls: vec![tool_call_with_signature(None)],
        saw_message_end: true,
        ..Default::default()
    };
    assert_eq!(outcome.reasoning_capability(), "none");
}

#[test]
fn reasoning_capability_prefers_streamed_over_opaque() {
    // Streamed reasoning text wins even when an opaque signal is also present.
    let outcome = NativeClaudeStreamOutcome {
        reasoning_text_len: 10,
        saw_reasoning_signal: true,
        tool_calls: vec![tool_call_with_signature(Some("SIG"))],
        saw_message_end: true,
        ..Default::default()
    };
    assert_eq!(outcome.reasoning_capability(), "streamed");
}

#[test]
fn parallel_tool_use_replays_every_signature_in_one_assistant_message() {
    let calls = vec![
        NativeClaudeToolCall {
            id: "a".to_string(),
            name: "read".to_string(),
            input_json: "{\"file_path\":\"/tmp/a\"}".to_string(),
            thought_signature: Some("SIG_A".to_string()),
        },
        NativeClaudeToolCall {
            id: "b".to_string(),
            name: "read".to_string(),
            input_json: "{\"file_path\":\"/tmp/b\"}".to_string(),
            thought_signature: Some("SIG_B".to_string()),
        },
    ];
    let assistant = assistant_parallel_tool_uses(&calls);
    assert!(matches!(assistant.role, Role::Assistant));
    // One assistant message must carry BOTH tool_use blocks, each with its
    // own signature preserved.
    assert_eq!(assistant.content.len(), 2);
    let sigs: Vec<Option<String>> = assistant
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::ToolUse {
                thought_signature, ..
            } => thought_signature.clone(),
            other => panic!("expected ToolUse, got {other:?}"),
        })
        .collect();
    assert_eq!(
        sigs,
        vec![Some("SIG_A".to_string()), Some("SIG_B".to_string())]
    );

    // The results message must answer every call with a matching id.
    let results = parallel_tool_results(&calls);
    assert!(matches!(results.role, Role::User));
    assert_eq!(results.content.len(), 2);
    let ids: Vec<String> = results
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.clone(),
            other => panic!("expected ToolResult, got {other:?}"),
        })
        .collect();
    assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
}
