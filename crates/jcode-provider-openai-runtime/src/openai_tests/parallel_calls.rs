#[test]
fn parallel_calls_stream_interleaved_arguments_keep_call_ids() {
    let mut saw_text = false;
    let mut saw_thinking = false;
    let mut calls = HashMap::new();
    let mut completed = HashSet::new();
    let mut pending = VecDeque::new();
    let mut parse = |event: Value| {
        parse_openai_response_event(
            &event.to_string(),
            &mut saw_text,
            &mut saw_thinking,
            &mut calls,
            &mut completed,
            &mut pending,
        )
        .expect("expected keyed tool event")
    };
    for (item, call) in [("fc_a", "call_a"), ("fc_b", "call_b")] {
        let event = parse(
            serde_json::json!({"type":"response.output_item.added", "item":{
                "id":item, "type":"function_call", "call_id":call, "name":"read", "arguments":""
            }}),
        );
        assert!(
            matches!(event, StreamEvent::ToolUseStart { id, name } if id == call && name == "read")
        );
    }
    for (item, call, fragment) in [
        ("fc_b", "call_b", "{\"path\":\"b\"}"),
        ("fc_a", "call_a", "{\"path\":\"a\"}"),
    ] {
        let event = parse(
            serde_json::json!({"type":"response.function_call_arguments.delta", "item_id":item, "delta":fragment}),
        );
        assert!(
            matches!(event, StreamEvent::ToolInputDeltaFor { id, delta } if id == call && delta == fragment)
        );
        let event = parse(
            serde_json::json!({"type":"response.function_call_arguments.done", "item_id":item, "arguments":fragment}),
        );
        assert!(matches!(event, StreamEvent::ToolUseEndFor { id } if id == call));
    }
    assert!(calls.is_empty());
    assert!(completed.contains("fc_a"));
    assert!(completed.contains("fc_b"));
    assert!(pending.is_empty());
}

#[test]
fn parallel_calls_policy_respects_model_and_hosted_tools() {
    let functions = vec![serde_json::json!({"type":"function", "name":"read"})];
    let mut mixed = functions.clone();
    mixed.push(serde_json::json!({"type":"image_generation"}));
    mixed.push(serde_json::json!({"type":"web_search"}));
    for model in ["gpt-5.4", "gpt-5.3-codex", "gpt-5.6-sol", "GPT-6"] {
        assert!(parallel_tool_calls_enabled(model, &functions, true));
        assert!(parallel_tool_calls_enabled(model, &mixed, true));
        assert!(!parallel_tool_calls_enabled(model, &mixed, false));
    }
    assert!(parallel_tool_calls_enabled("gpt-4.1", &functions, true));
    assert!(!parallel_tool_calls_enabled("gpt-4.1", &mixed, true));
    assert!(!parallel_tool_calls_enabled("unknown", &mixed, true));
    assert!(!parallel_tool_calls_enabled(
        "gpt-4.1-nano-2025-04-14",
        &functions,
        true
    ));
}

#[test]
fn parallel_calls_follow_config_and_environment_override() {
    const CHILD: &str = "JCODE_TEST_PARALLEL_CALLS_CHILD";
    if let Ok(expected) = std::env::var(CHILD) {
        let expected = expected == "true";
        for oauth in [false, true] {
            let request =
                build_test_response_request("gpt-5.4", oauth, None, None, None, None, None, None);
            assert_eq!(request["parallel_tool_calls"], expected);
            let continuation =
                openai_stream_runtime::build_continuation_request(&request, "resp_parallel", &[]);
            assert_eq!(continuation["parallel_tool_calls"], expected);
        }
        return;
    }
    // Fresh processes avoid mutating/reusing the global reloadable config cache.
    for (config, override_value, expected) in [
        (None, None, false),
        (Some(""), None, false),
        (Some("[tools]\n"), None, false),
        (Some("[tools]\nparallel = true\n"), None, true),
        (Some("[tools]\nparallel = false\n"), None, false),
        (Some("[tools]\nparallel = true\n"), Some("0"), false),
        (Some("[tools]\nparallel = false\n"), Some("1"), true),
        (None, Some("1"), true),
        (Some("[tools]\n"), Some("1"), true),
    ] {
        let home = tempfile::tempdir().unwrap();
        if let Some(config) = config {
            std::fs::write(home.path().join("config.toml"), config).unwrap();
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--nocapture",
            ])
            .env(CHILD, expected.to_string())
            .env("JCODE_HOME", home.path())
            .env_remove("JCODE_PARALLEL_TOOLS");
        if let Some(value) = override_value {
            child.env("JCODE_PARALLEL_TOOLS", value);
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "config={config:?}, override={override_value:?}, expected={expected}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn parallel_calls_replay_keeps_distinct_ids_and_complete_followups() {
    let mut assistant =
        assistant_tool_use("call_first", "read", serde_json::json!({"path":"first"}));
    assistant.content.extend(
        assistant_tool_use("call_second", "read", serde_json::json!({"path":"second"})).content,
    );
    let messages = vec![
        user_text("read both"),
        assistant,
        // Results may finish in reverse order. IDs, not function names or order,
        // must associate each result with its original call.
        ChatMessage::tool_result("call_second", "second result", false),
        ChatMessage::tool_result("call_first", "first result", false),
    ];
    let input = build_responses_input(&messages);
    for (id, output) in [
        ("call_first", "first result"),
        ("call_second", "second result"),
    ] {
        assert_eq!(function_call_outputs(&input, id), vec![output]);
        assert!(
            function_call_pos(&input, id).unwrap() < function_call_output_pos(&input, id).unwrap()
        );
        assert_eq!(
            input
                .iter()
                .filter(|item| response_item_type(item) == Some("function_call")
                    && response_item_call_id(item) == Some(id))
                .count(),
            1
        );
    }
    let first_output = input
        .iter()
        .position(|item| response_item_type(item) == Some("function_call_output"))
        .unwrap();
    let (delta, _) = persistent_ws_incremental_items(&input, first_output);
    for enabled in [false, true] {
        let request = serde_json::json!({"model":"gpt-5.4", "parallel_tool_calls":enabled, "stream":true, "background":true});
        let continuation =
            openai_stream_runtime::build_continuation_request(&request, "resp_parallel", &delta);
        assert_eq!(continuation["parallel_tool_calls"], enabled);
        assert_eq!(continuation["previous_response_id"], "resp_parallel");
        assert!(continuation.get("stream").is_none());
        assert!(continuation.get("background").is_none());
        let outputs = continuation["input"].as_array().unwrap();
        assert_eq!(
            function_call_outputs(outputs, "call_first"),
            vec!["first result"]
        );
        assert_eq!(
            function_call_outputs(outputs, "call_second"),
            vec!["second result"]
        );
    }
}
