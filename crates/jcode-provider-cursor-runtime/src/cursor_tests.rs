use super::*;
use std::io::Read;

#[test]
fn available_models_include_composer_models() {
    let provider = CursorCliProvider::new();
    let models = provider.available_models();
    assert!(models.contains(&"composer-2"));
    assert!(models.contains(&"composer-2.5"));
}

#[test]
fn available_models_display_includes_custom_current_model() {
    let provider = CursorCliProvider::new();
    provider.set_model("future-cursor-model").unwrap();

    let models = provider.available_models_display();
    assert!(models.contains(&"future-cursor-model".to_string()));
}

#[test]
fn available_models_display_prefers_fetched_cursor_models() {
    let provider = CursorCliProvider::new();
    *provider.fetched_models.write().unwrap() = vec![
        "claude-4-sonnet-thinking".to_string(),
        "gpt-5.2".to_string(),
    ];

    let models = provider.available_models_display();
    assert_eq!(
        models.first().map(|model| model.as_str()),
        Some("claude-4-sonnet-thinking")
    );
    assert!(models.iter().any(|model| model == "gpt-5.2"));
    assert!(models.iter().any(|model| model == "composer-2.5"));
}

#[test]
fn merge_cursor_models_deduplicates_dynamic_entries() {
    let models = merge_cursor_models(
        &[
            "composer-2".to_string(),
            "claude-4-sonnet-thinking".to_string(),
            "claude-4-sonnet-thinking".to_string(),
        ],
        "claude-4-sonnet-thinking",
    );

    assert_eq!(
        models
            .iter()
            .filter(|model| model.as_str() == "claude-4-sonnet-thinking")
            .count(),
        1
    );
    assert!(models.iter().any(|model| model == "composer-2"));
}

#[test]
fn available_models_display_seeds_from_persisted_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    jcode_base::env::set_var("JCODE_HOME", temp.path());

    let path = CursorCliProvider::persisted_catalog_path().expect("catalog path");
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec!["cursor-disk-model".to_string()],
            fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
        },
    )
    .expect("write persisted catalog");

    let provider = CursorCliProvider::new();
    assert!(
        provider
            .available_models_display()
            .contains(&"cursor-disk-model".to_string())
    );

    if let Some(prev_home) = prev_home {
        jcode_base::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_base::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn set_model_accepts_composer_models() {
    let provider = CursorCliProvider::new();

    provider.set_model("composer-2").unwrap();
    assert_eq!(provider.model(), "composer-2");

    provider.set_model("composer-2.5").unwrap();
    assert_eq!(provider.model(), "composer-2.5");
}

#[test]
fn runtime_cursor_api_key_reads_env() {
    let previous = std::env::var_os("CURSOR_API_KEY");
    jcode_base::env::set_var("CURSOR_API_KEY", "cursor-env-test");

    assert_eq!(runtime_cursor_api_key().as_deref(), Some("cursor-env-test"));

    if let Some(previous) = previous {
        jcode_base::env::set_var("CURSOR_API_KEY", previous);
    } else {
        jcode_base::env::remove_var("CURSOR_API_KEY");
    }
}

// ==========================================================================
// Phase 07 Wire Codec & Contract Tests (Plan 07-01)
// ==========================================================================

#[test]
fn test_encode_mcp_tools() {
    let tools = vec![
        ToolDefinition {
            name: "bash".to_string(),
            description: "Execute a shell command in bash".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout": { "type": "integer" }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read contents of a local file".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        },
    ];

    let mcp_tools_bytes = wire::encode_mcp_tools(&tools).expect("encode_mcp_tools");
    assert!(!mcp_tools_bytes.is_empty());

    // Inspect decoded McpTools message
    let mut definitions = Vec::new();
    for field in wire::iter_fields(&mcp_tools_bytes) {
        assert_eq!(field.field, 1); // repeated McpToolDefinition mcp_tools = 1
        assert_eq!(field.wire, 2);

        let mut name = None;
        let mut desc = None;
        let mut schema_val = None;
        let mut provider_id = None;
        let mut tool_name = None;

        for def_field in wire::iter_fields(field.data) {
            match def_field.field {
                1 => name = std::str::from_utf8(def_field.data).ok().map(str::to_string),
                2 => desc = std::str::from_utf8(def_field.data).ok().map(str::to_string),
                3 => {
                    schema_val = wire::decode_google_protobuf_value(def_field.data, 0).ok();
                }
                4 => provider_id = std::str::from_utf8(def_field.data).ok().map(str::to_string),
                5 => tool_name = std::str::from_utf8(def_field.data).ok().map(str::to_string),
                _ => {}
            }
        }

        definitions.push((name, desc, schema_val, provider_id, tool_name));
    }

    assert_eq!(definitions.len(), 2);

    // Verify tool 1: bash
    let (name, desc, schema, prov, bare) = &definitions[0];
    assert_eq!(name.as_deref(), Some("mcp_jcode__bash"));
    assert_eq!(desc.as_deref(), Some("Execute a shell command in bash"));
    assert_eq!(prov.as_deref(), Some("jcode"));
    assert_eq!(bare.as_deref(), Some("bash"));
    assert_eq!(schema.as_ref().unwrap()["type"], "object");
    assert_eq!(schema.as_ref().unwrap()["required"][0], "command");

    // Verify tool 2: read_file
    let (name, desc, schema, prov, bare) = &definitions[1];
    assert_eq!(name.as_deref(), Some("mcp_jcode__read_file"));
    assert_eq!(desc.as_deref(), Some("Read contents of a local file"));
    assert_eq!(prov.as_deref(), Some("jcode"));
    assert_eq!(bare.as_deref(), Some("read_file"));
    assert_eq!(
        schema.as_ref().unwrap()["properties"]["path"]["type"],
        "string"
    );
}

#[test]
fn test_google_protobuf_value_roundtrip() {
    let test_values = vec![
        serde_json::Value::Null,
        serde_json::json!(true),
        serde_json::json!(false),
        serde_json::json!(12345),
        serde_json::json!(42.5),
        serde_json::json!("Hello, Cursor wire!"),
        serde_json::json!(["apple", "banana", 100, true, null]),
        serde_json::json!({
            "nested": {
                "inner_num": 99.25,
                "inner_str": "deep",
                "inner_list": [1, 2, 3]
            },
            "status": "active",
            "count": 7
        }),
    ];

    for original in test_values {
        let encoded = wire::encode_google_protobuf_value(&original, 0).expect("encode Value");
        let decoded = wire::decode_google_protobuf_value(&encoded, 0).expect("decode Value");

        if original.is_number() {
            assert_eq!(
                original.as_f64().unwrap(),
                decoded.as_f64().unwrap(),
                "Number equality failed"
            );
        } else {
            assert_eq!(original, decoded, "Roundtrip failed for {original:?}");
        }
    }
}

#[test]
fn test_google_protobuf_value_bounds() {
    // 1. Test recursion depth exceeding MAX_VALUE_DEPTH (32)
    let mut deep_val = serde_json::json!("deepest");
    for _ in 0..=35 {
        deep_val = serde_json::json!({ "child": deep_val });
    }

    let encode_err = wire::encode_google_protobuf_value(&deep_val, 0);
    assert!(
        encode_err.is_err(),
        "Exceeding MAX_VALUE_DEPTH must fail safely without stack overflow"
    );
    let err_str = encode_err.unwrap_err().to_string();
    assert!(
        err_str.contains("depth limit"),
        "Error must report depth limit, got: {err_str}"
    );

    // 2. Test byte size limit exceeding MAX_VALUE_BYTES
    let oversized_bytes = vec![0u8; wire::MAX_VALUE_BYTES + 1024];
    let decode_err = wire::decode_google_protobuf_value(&oversized_bytes, 0);
    assert!(
        decode_err.is_err(),
        "Exceeding MAX_VALUE_BYTES must fail safely"
    );
    assert!(
        decode_err
            .unwrap_err()
            .to_string()
            .contains("byte size limit")
    );
}

#[test]
fn test_decode_mcp_args_payload() {
    let name = "mcp_jcode__bash";
    let tool_call_id = "call_dummy_001";
    let provider_identifier = "jcode";
    let tool_name = "bash";

    // Construct args map with command="cargo test" and timeout=120
    let cmd_val_bytes =
        wire::encode_google_protobuf_value(&serde_json::json!("cargo test"), 0).unwrap();
    let mut entry1 = wire::field_str(1, "command");
    entry1.extend(wire::field_ld(2, &cmd_val_bytes));

    let timeout_val_bytes = wire::encode_google_protobuf_value(&serde_json::json!(120), 0).unwrap();
    let mut entry2 = wire::field_str(1, "timeout");
    entry2.extend(wire::field_ld(2, &timeout_val_bytes));

    let mut mcp_args_payload = wire::field_str(1, name);
    mcp_args_payload.extend(wire::field_ld(2, &entry1));
    mcp_args_payload.extend(wire::field_ld(2, &entry2));
    mcp_args_payload.extend(wire::field_str(3, tool_call_id));
    mcp_args_payload.extend(wire::field_str(4, provider_identifier));
    mcp_args_payload.extend(wire::field_str(5, tool_name));

    // Wrap in ExecServerMessage (id=101, exec_id="exec_999", mcp_args=11)
    let mut exec_msg_bytes = wire::field_varint(1, 101);
    exec_msg_bytes.extend(wire::field_str(15, "exec_999"));
    exec_msg_bytes.extend(wire::field_ld(11, &mcp_args_payload));

    let decoded =
        wire::decode_exec_server_message(&exec_msg_bytes).expect("decode ExecServerMessage");
    assert_eq!(decoded.id, 101);
    assert_eq!(decoded.exec_id, "exec_999");

    match decoded.variant {
        wire::ExecServerMessageVariant::Mcp(args) => {
            assert_eq!(args.name, "mcp_jcode__bash");
            assert_eq!(args.tool_name, "bash");
            assert_eq!(args.tool_call_id, "call_dummy_001");
            assert_eq!(args.provider_identifier, "jcode");
            assert_eq!(args.args["command"], "cargo test");
            assert_eq!(args.args["timeout"], 120.0);
        }
        other => panic!("Expected Mcp variant, got: {other:?}"),
    }
}

#[test]
fn test_bidi_exec_codecs() {
    fn make_exec_server_msg(id: u32, exec_id: &str, field_num: u64, payload: &[u8]) -> Vec<u8> {
        let mut bytes = wire::field_varint(1, id as u64);
        if !exec_id.is_empty() {
            bytes.extend(wire::field_str(15, exec_id));
        }
        bytes.extend(wire::field_ld(field_num, payload));
        bytes
    }

    // 1. ShellArgs (2)
    let mut shell_data = wire::field_str(1, "echo hi");
    shell_data.extend(wire::field_str(2, "/tmp"));
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(1, "e1", 2, &shell_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Shell(wire::ShellArgs {
            command: "echo hi".into(),
            working_directory: "/tmp".into()
        })
    );

    // 2. WriteArgs (3)
    let mut write_data = wire::field_str(1, "foo.txt");
    write_data.extend(wire::field_str(2, "content"));
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(2, "e2", 3, &write_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Write(wire::WriteArgs {
            path: "foo.txt".into(),
            file_text: "content".into(),
            file_bytes: vec![]
        })
    );

    // 3. DeleteArgs (4)
    let del_data = wire::field_str(1, "bar.txt");
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(3, "e3", 4, &del_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Delete(wire::DeleteArgs {
            path: "bar.txt".into()
        })
    );

    // 4. GrepArgs (5)
    let mut grep_data = wire::field_str(1, "fn main");
    grep_data.extend(wire::field_str(2, "src"));
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(4, "e4", 5, &grep_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Grep(wire::GrepArgs {
            pattern: "fn main".into(),
            path: "src".into(),
            case_insensitive: false,
            glob: "".into()
        })
    );

    // 5. ReadArgs (7)
    let read_data = wire::field_str(1, "README.md");
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(5, "e5", 7, &read_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Read(wire::ReadArgs {
            path: "README.md".into()
        })
    );

    // 6. LsArgs (8)
    let ls_data = wire::field_str(1, "/home/user");
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(6, "e6", 8, &ls_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Ls(wire::LsArgs {
            path: "/home/user".into()
        })
    );

    // 7. DiagnosticsArgs (9)
    let diag_data = wire::field_str(1, "main.rs");
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(7, "e7", 9, &diag_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Diagnostics(wire::DiagnosticsArgs {
            path: "main.rs".into()
        })
    );

    // 8. RequestContextArgs (10)
    let msg = wire::decode_exec_server_message(&make_exec_server_msg(8, "e8", 10, &[])).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::RequestContext(wire::RequestContextArgs {})
    );

    // 9. FetchArgs (20)
    let mut fetch_data = wire::field_str(1, "https://api.example.com");
    fetch_data.extend(wire::field_str(2, "call_fetch_01"));
    let msg =
        wire::decode_exec_server_message(&make_exec_server_msg(9, "e9", 20, &fetch_data)).unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::Fetch(wire::FetchArgs {
            url: "https://api.example.com".into(),
            tool_call_id: "call_fetch_01".into()
        })
    );

    // 10. WriteShellStdinArgs (23)
    let mut stdin_data = wire::field_varint(1, 42);
    stdin_data.extend(wire::field_str(2, "yes\n"));
    let msg = wire::decode_exec_server_message(&make_exec_server_msg(10, "e10", 23, &stdin_data))
        .unwrap();
    assert_eq!(
        msg.variant,
        wire::ExecServerMessageVariant::WriteShellStdin(wire::WriteShellStdinArgs {
            shell_id: 42,
            stdin: "yes\n".into()
        })
    );

    // Verify client framing
    let client_exec = wire::encode_agent_client_exec_message(&[1, 2, 3]);
    let mut fields = wire::iter_fields(&client_exec);
    let f = fields.next().unwrap();
    assert_eq!(f.field, 2); // ExecClientMessage = field 2 in AgentClientMessage
    assert_eq!(f.data, &[1, 2, 3]);

    let client_close = wire::encode_agent_client_stream_close(99);
    let mut fields = wire::iter_fields(&client_close);
    let f = fields.next().unwrap();
    assert_eq!(f.field, 5); // ExecClientControlMessage = field 5 in AgentClientMessage
}

#[test]
fn test_rejection_encoders() {
    // Ensure all rejection functions return their matching response oneof (CURS-03)
    let cases: Vec<(Vec<u8>, u64, &str)> = vec![
        (
            wire::reject_shell_exec(1, "e1", "rm -rf /", "/tmp", "denied"),
            2,
            "shell_result",
        ),
        (
            wire::reject_write_exec(2, "e2", "/etc/hosts", "denied"),
            3,
            "write_result",
        ),
        (
            wire::reject_delete_exec(3, "e3", "/etc/passwd", "denied"),
            4,
            "delete_result",
        ),
        (
            wire::reject_grep_exec(4, "e4", "disabled"),
            5,
            "grep_result",
        ),
        (
            wire::reject_read_exec(5, "e5", "/secret", "disabled"),
            7,
            "read_result",
        ),
        (
            wire::reject_ls_exec(6, "e6", "/root", "disabled"),
            8,
            "ls_result",
        ),
        (
            wire::reject_diagnostics_exec(7, "e7", "foo.rs", "unsupported"),
            9,
            "diagnostics_result",
        ),
        (
            wire::reject_shell_stream_exec(8, "e8", "disabled"),
            14,
            "shell_stream",
        ),
        (
            wire::reject_background_shell_spawn_exec(9, "e9", "sleep", "/tmp", "disabled"),
            16,
            "background_shell_spawn_result",
        ),
        (
            wire::reject_list_mcp_resources_exec(10, "e10", "disabled"),
            17,
            "list_mcp_resources_exec_result",
        ),
        (
            wire::reject_read_mcp_resource_exec(11, "e11", "res://1", "disabled"),
            18,
            "read_mcp_resource_exec_result",
        ),
        (
            wire::reject_fetch_exec(12, "e12", "http://bad", "disabled"),
            20,
            "fetch_result",
        ),
        (
            wire::reject_record_screen_exec(13, "e13", "disabled"),
            21,
            "record_screen_result",
        ),
        (
            wire::reject_computer_use_exec(14, "e14", "disabled"),
            22,
            "computer_use_result",
        ),
        (
            wire::reject_write_shell_stdin_exec(15, "e15", "disabled"),
            23,
            "write_shell_stdin_result",
        ),
    ];

    for (bytes, expected_field, name) in cases {
        let fields: Vec<_> = wire::iter_fields(&bytes).collect();
        assert!(fields.iter().any(|f| f.field == 1), "Missing id for {name}");
        assert!(
            fields.iter().any(|f| f.field == 15),
            "Missing exec_id for {name}"
        );

        // Must contain expected oneof field
        assert!(
            fields.iter().any(|f| f.field == expected_field),
            "{name} must encode response oneof field {expected_field}"
        );

        // ANTI-PATTERN CHECK: Must NEVER return mcp_result (field 11)
        assert!(
            !fields.iter().any(|f| f.field == 11),
            "{name} MUST NEVER return mcp_result (field 11)!"
        );
    }
}

#[test]
fn request_context_result_has_required_success_wrapper() {
    let context = wire::field_str(1, "sentinel");
    let encoded = wire::encode_request_context_result(7, "exec", &context);
    let result = wire::iter_fields(&encoded)
        .find(|field| field.field == 10)
        .expect("request context result");
    let success = wire::iter_fields(result.data)
        .find(|field| field.field == 1)
        .expect("success oneof");
    let request_context = wire::iter_fields(success.data)
        .find(|field| field.field == 1)
        .expect("request context wrapper");
    assert_eq!(request_context.data, context);
}

#[test]
fn request_context_result_omits_synthetic_ids() {
    let encoded = wire::encode_request_context_result(7, "exec", &[]);
    let fields: Vec<_> = wire::iter_fields(&encoded).collect();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].field, 2);
    let exec_fields: Vec<_> = wire::iter_fields(fields[0].data).collect();
    assert_eq!(exec_fields.len(), 1);
    assert_eq!(exec_fields[0].field, 10);
}

#[test]
fn large_connect_frames_use_gzip_and_round_trip() {
    let payload = vec![b'x'; 1024];
    let framed = wire::connect_frame(&payload);
    assert_eq!(framed[0], 1, "connect-es compresses payloads at 1024 bytes");
    let length = u32::from_be_bytes(framed[1..5].try_into().unwrap()) as usize;
    assert_eq!(length, framed.len() - 5);
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(&framed[5..])
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn kv_ack_codecs_match_agent_client_message_shape() {
    let set = wire::encode_kv_set_blob_ack(9);
    let set_fields: Vec<_> = wire::iter_fields(&set).collect();
    assert_eq!(set_fields.len(), 1);
    assert_eq!(set_fields[0].field, 3);
    assert!(wire::iter_fields(set_fields[0].data).any(|f| f.field == 1 && f.varint == 9));
    assert!(wire::iter_fields(set_fields[0].data).any(|f| f.field == 3));

    let get = wire::encode_kv_get_blob_result(10, b"blob");
    let get_fields: Vec<_> = wire::iter_fields(&get).collect();
    assert_eq!(get_fields.len(), 1);
    assert_eq!(get_fields[0].field, 3);
    let result = wire::iter_fields(get_fields[0].data)
        .find(|f| f.field == 2)
        .expect("get blob result");
    let blob = wire::iter_fields(result.data)
        .find(|f| f.field == 1)
        .expect("blob data");
    assert_eq!(blob.data, b"blob");
}

#[test]
fn shell_rejection_sets_aborted_on_field_eleven() {
    let encoded = wire::reject_shell_exec(1, "exec", "false", ".", "denied");
    let shell_result = wire::iter_fields(&encoded)
        .find(|field| field.field == 2)
        .expect("shell result");
    let failure = wire::iter_fields(shell_result.data)
        .find(|field| field.field == 2)
        .expect("shell failure");
    let fields: Vec<_> = wire::iter_fields(failure.data).collect();
    assert!(
        fields
            .iter()
            .any(|field| field.field == 11 && field.varint == 1)
    );
    assert!(!fields.iter().any(|field| field.field == 8));
}

#[test]
fn tool_request_id_preserves_colons_in_exec_id() {
    let (id, exec_id) = agent_transport::parse_tool_request_id("stream:42:call:step_1");
    assert_eq!(id, 42);
    assert_eq!(exec_id, "call:step_1");
}

#[test]
fn tool_error_is_encoded_as_mcp_success_with_error_flag() {
    let encoded = wire::encode_mcp_success_result(1, "exec", "failed", true);
    let mcp_result = wire::iter_fields(&encoded)
        .find(|field| field.field == 11)
        .expect("mcp result");
    let success = wire::iter_fields(mcp_result.data)
        .find(|field| field.field == 1)
        .expect("mcp success");
    assert!(wire::iter_fields(success.data).any(|field| field.field == 2 && field.varint == 1));
}

#[test]
fn test_native_exec_rejection_oneofs() {
    let read_rej = wire::reject_read_exec(10, "ex_read", "/test/file", "policy");
    let write_rej = wire::reject_write_exec(11, "ex_write", "/test/file", "policy");
    let shell_rej = wire::reject_shell_exec(12, "ex_shell", "ls", ".", "policy");
    let fetch_rej = wire::reject_fetch_exec(13, "ex_fetch", "https://example.com", "policy");

    let has_field =
        |bytes: &[u8], field_num: u64| wire::iter_fields(bytes).any(|f| f.field == field_num);

    assert!(has_field(&read_rej, 7), "read rejection must be field 7");
    assert!(
        !has_field(&read_rej, 11),
        "read rejection must not be field 11"
    );

    assert!(has_field(&write_rej, 3), "write rejection must be field 3");
    assert!(
        !has_field(&write_rej, 11),
        "write rejection must not be field 11"
    );

    assert!(has_field(&shell_rej, 2), "shell rejection must be field 2");
    assert!(
        !has_field(&shell_rej, 11),
        "shell rejection must not be field 11"
    );

    assert!(
        has_field(&fetch_rej, 20),
        "fetch rejection must be field 20"
    );
    assert!(
        !has_field(&fetch_rej, 11),
        "fetch rejection must not be field 11"
    );

    // In contrast, Mcp results MUST use field 11
    let mcp_success = wire::encode_mcp_success_result(14, "ex_mcp", "ok", false);
    assert!(has_field(&mcp_success, 11), "mcp success must be field 11");
    let mcp_err = wire::encode_mcp_error_result(15, "ex_mcp", "err");
    assert!(has_field(&mcp_err, 11), "mcp error must be field 11");
}

#[test]
fn test_redacted_contract_fixtures() {
    let synthetic_token = "cursor_token_dummy_001";
    let synthetic_account = "dummy_user@example.com";

    let mut exec_bytes = wire::field_varint(1, 1);
    exec_bytes.extend(wire::field_str(15, "exec_redacted_001"));
    exec_bytes.extend(wire::field_ld(11, &wire::field_str(1, synthetic_token)));

    let mcp_args_wire = wire::decode_exec_server_message(&exec_bytes).unwrap();
    assert_eq!(mcp_args_wire.exec_id, "exec_redacted_001");

    // Ensure no real personal accounts or secrets are present in any test data
    assert!(!synthetic_token.contains("ghp_"));
    assert!(!synthetic_token.contains("ey"));
    assert_eq!(synthetic_account, "dummy_user@example.com");
}

#[tokio::test]
async fn test_tool_result_multiplexer_concurrent_dispatch() {
    let mux = ToolResultMultiplexer::new();
    let stream1 = "stream-1111-uuid";
    let stream2 = "stream-2222-uuid";
    let (tx1, mut rx1) = mpsc::channel::<NativeToolResult>(4);
    let (tx2, mut rx2) = mpsc::channel::<NativeToolResult>(4);

    let _guard1 = mux.register(stream1, tx1);
    let _guard2 = mux.register(stream2, tx2);

    let res1 = NativeToolResult::success(format!("{}:10:call_a", stream1), "output 1".to_string());
    let res2 = NativeToolResult::success(format!("{}:11:call_b", stream2), "output 2".to_string());

    mux.dispatch(res1.clone());
    mux.dispatch(res2.clone());

    let got1 = tokio::time::timeout(std::time::Duration::from_millis(500), rx1.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got1.request_id, format!("{}:10:call_a", stream1));
    assert_eq!(got1.result.output.as_deref(), Some("output 1"));

    let got2 = tokio::time::timeout(std::time::Duration::from_millis(500), rx2.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got2.request_id, format!("{}:11:call_b", stream2));
    assert_eq!(got2.result.output.as_deref(), Some("output 2"));
    assert_eq!(got2.msg_type, "native_tool_result");
    assert!(!got2.is_error);

    drop(_guard1);
    let res1_after = NativeToolResult::success(
        format!("{}:12:call_c", stream1),
        "output 1 after drop".to_string(),
    );
    mux.dispatch(res1_after);
    // After dropping _guard1, rx1 channel sender is dropped or removed from routes.
    // Since tx1 was moved into _guard1/routes, rx1 will see channel close (Ok(None)).
    let got1_none = tokio::time::timeout(std::time::Duration::from_millis(100), rx1.recv())
        .await
        .unwrap();
    assert!(got1_none.is_none());
}

#[test]
fn test_thinking_delta_stream_events() {
    let leaf = wire::field_str(1, "pondering the universe");
    let f4 = wire::field_ld(4, &leaf);
    let top = wire::field_ld(1, &f4);
    let thinking = agent_transport::extract_thinking_text(&top);
    assert_eq!(thinking.as_deref(), Some("pondering the universe"));
    let event = jcode_message_types::StreamEvent::ThinkingDelta(thinking.unwrap());
    match event {
        jcode_message_types::StreamEvent::ThinkingDelta(t) => {
            assert_eq!(t, "pondering the universe")
        }
        _ => panic!("Expected ThinkingDelta"),
    }
}

#[test]
fn test_outbound_tool_result_secret_redaction() {
    let raw_secret = "sk-ant-oat01-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let output = format!("access={}", raw_secret);
    let redacted = jcode_base::message::redact_secrets(&output);
    assert!(!redacted.contains(raw_secret));
    assert!(redacted.contains("[REDACTED"));

    let mcp_res_bytes = wire::encode_mcp_success_result(1, "call_1", &redacted, false);
    let hay = String::from_utf8_lossy(&mcp_res_bytes);
    assert!(!hay.contains(raw_secret));
    assert!(hay.contains("[REDACTED"));
}
