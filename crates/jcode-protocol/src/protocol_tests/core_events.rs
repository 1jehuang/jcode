#[test]
fn test_request_roundtrip() -> Result<()> {
    let req = Request::Message {
        id: 1,
        content: "hello".to_string(),
        images: vec![],
        system_reminder: None,
    };
    let json = serde_json::to_string(&req)?;
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 1);
    Ok(())
}

#[test]
fn test_compacted_history_request_roundtrip() -> Result<()> {
    let req = Request::GetCompactedHistory {
        id: 7,
        visible_messages: 64,
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"get_compacted_history\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 7);
    let Request::GetCompactedHistory {
        visible_messages, ..
    } = decoded
    else {
        return Err(anyhow!("wrong request type"));
    };
    assert_eq!(visible_messages, 64);
    Ok(())
}

#[test]
fn test_rewind_request_roundtrip() -> Result<()> {
    let req = Request::Rewind {
        id: 8,
        message_index: 3,
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"rewind\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 8);
    let Request::Rewind { message_index, .. } = decoded else {
        return Err(anyhow!("wrong request type"));
    };
    assert_eq!(message_index, 3);
    Ok(())
}

#[test]
fn test_rewind_undo_request_roundtrip() -> Result<()> {
    let req = Request::RewindUndo { id: 9 };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"rewind_undo\""));
    let decoded = parse_request_json(&json)?;
    assert_eq!(decoded.id(), 9);
    let Request::RewindUndo { .. } = decoded else {
        return Err(anyhow!("wrong request type"));
    };
    Ok(())
}

#[test]
fn test_event_roundtrip() -> Result<()> {
    let event = ServerEvent::TextDelta {
        text: "hello".to_string(),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::TextDelta { text } = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(text, "hello");
    Ok(())
}

#[test]
fn test_interrupted_event_decodes_from_json() -> Result<()> {
    let json = r#"{"type":"interrupted"}"#;
    let decoded = parse_event_json(json)?;
    let ServerEvent::Interrupted = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    Ok(())
}

#[test]
fn test_connection_type_event_roundtrip() -> Result<()> {
    let event = ServerEvent::ConnectionType {
        connection: "websocket".to_string(),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::ConnectionType { connection } = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(connection, "websocket");
    Ok(())
}

#[test]
fn test_status_detail_event_roundtrip() -> Result<()> {
    let event = ServerEvent::StatusDetail {
        detail: "reusing websocket".to_string(),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::StatusDetail { detail } = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(detail, "reusing websocket");
    Ok(())
}

#[test]
fn test_generated_image_event_roundtrip() -> Result<()> {
    let event = ServerEvent::GeneratedImage {
        id: "ig_123".to_string(),
        path: "/tmp/generated.png".to_string(),
        metadata_path: Some("/tmp/generated.json".to_string()),
        output_format: "png".to_string(),
        revised_prompt: Some("A polished image prompt".to_string()),
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"generated_image\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::GeneratedImage {
        id,
        path,
        metadata_path,
        output_format,
        revised_prompt,
    } = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(id, "ig_123");
    assert_eq!(path, "/tmp/generated.png");
    assert_eq!(metadata_path.as_deref(), Some("/tmp/generated.json"));
    assert_eq!(output_format, "png");
    assert_eq!(revised_prompt.as_deref(), Some("A polished image prompt"));
    Ok(())
}

#[test]
fn test_interrupted_event_roundtrip() -> Result<()> {
    let event = ServerEvent::Interrupted;
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"interrupted\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::Interrupted = decoded else {
        return Err(anyhow!("wrong event type"));
    };
    Ok(())
}

#[test]
fn test_history_event_decodes_without_compaction_mode_for_older_servers() -> Result<()> {
    let json = r#"{
            "type":"history",
            "id":1,
            "session_id":"ses_test_123",
            "messages":[],
            "provider_name":"openai",
            "provider_model":"gpt-5.4",
            "available_models":["gpt-5.4"],
            "connection_type":"websocket"
        }"#;
    let decoded = parse_event_json(json)?;
    let ServerEvent::History {
        provider_name,
        provider_model,
        available_models,
        connection_type,
        compaction_mode,
        side_panel,
        ..
    } = decoded
    else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(provider_name.as_deref(), Some("openai"));
    assert_eq!(provider_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(available_models, vec!["gpt-5.4"]);
    assert_eq!(connection_type.as_deref(), Some("websocket"));
    assert_eq!(compaction_mode, jcode_config_types::CompactionMode::Reactive);
    assert!(!side_panel.has_pages());
    Ok(())
}

#[test]
fn test_history_event_roundtrip_preserves_side_panel_snapshot() -> Result<()> {
    let event = ServerEvent::History {
        id: 101,
        session_id: "ses_test_456".to_string(),
        messages: vec![HistoryMessage {
            role: "assistant".to_string(),
            content: "hello".to_string(),
            tool_calls: None,
            tool_data: None,
        }],
        images: Vec::new(),
        provider_name: Some("openai".to_string()),
        provider_model: Some("gpt-5.4".to_string()),
        available_models: vec!["gpt-5.4".to_string()],
        available_model_routes: Vec::new(),
        mcp_servers: Vec::new(),
        skills: Vec::new(),
        total_tokens: None,
        all_sessions: Vec::new(),
        client_count: None,
        is_canary: None,
        reload_recovery: None,
        server_version: None,
        server_name: None,
        server_icon: None,
        server_has_update: None,
        was_interrupted: None,
        connection_type: Some("websocket".to_string()),
        status_detail: None,
        upstream_provider: None,
        reasoning_effort: None,
        service_tier: None,
        subagent_model: None,
        autoreview_enabled: None,
        autojudge_enabled: None,
        compaction_mode: jcode_config_types::CompactionMode::Reactive,
        activity: None,
        side_panel: jcode_side_panel_types::SidePanelSnapshot {
            focused_page_id: Some("page-1".to_string()),
            pages: vec![jcode_side_panel_types::SidePanelPage {
                id: "page-1".to_string(),
                title: "Notes".to_string(),
                file_path: "/tmp/notes.md".to_string(),
                format: jcode_side_panel_types::SidePanelPageFormat::Markdown,
                source: jcode_side_panel_types::SidePanelPageSource::Managed,
                content: "# Notes".to_string(),
                updated_at_ms: 42,
            }],
        },
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::History {
        id,
        side_panel,
        messages,
        provider_name,
        provider_model,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected History event"));
    };
    assert_eq!(id, 101);
    assert_eq!(provider_name.as_deref(), Some("openai"));
    assert_eq!(provider_model.as_deref(), Some("gpt-5.4"));
    assert_eq!(messages.len(), 1);
    assert_eq!(side_panel.focused_page_id.as_deref(), Some("page-1"));
    assert_eq!(side_panel.pages.len(), 1);
    assert_eq!(side_panel.pages[0].title, "Notes");
    assert_eq!(side_panel.pages[0].content, "# Notes");
    Ok(())
}

#[test]
fn test_compacted_history_event_roundtrip() -> Result<()> {
    let event = ServerEvent::CompactedHistory {
        id: 77,
        session_id: "ses_compact_123".to_string(),
        messages: vec![HistoryMessage {
            role: "assistant".to_string(),
            content: "older response".to_string(),
            tool_calls: None,
            tool_data: None,
        }],
        images: Vec::new(),
        compacted_total: 128,
        compacted_visible: 64,
        compacted_remaining: 64,
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"compacted_history\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::CompactedHistory {
        id,
        session_id,
        messages,
        compacted_total,
        compacted_visible,
        compacted_remaining,
        ..
    } = decoded
    else {
        return Err(anyhow!("expected CompactedHistory event"));
    };
    assert_eq!(id, 77);
    assert_eq!(session_id, "ses_compact_123");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "older response");
    assert_eq!(compacted_total, 128);
    assert_eq!(compacted_visible, 64);
    assert_eq!(compacted_remaining, 64);
    Ok(())
}

#[test]
fn test_side_panel_state_event_roundtrip() -> Result<()> {
    let event = ServerEvent::SidePanelState {
        snapshot: jcode_side_panel_types::SidePanelSnapshot {
            focused_page_id: Some("page-1".to_string()),
            pages: vec![jcode_side_panel_types::SidePanelPage {
                id: "page-1".to_string(),
                title: "Notes".to_string(),
                file_path: "/tmp/notes.md".to_string(),
                format: jcode_side_panel_types::SidePanelPageFormat::Markdown,
                source: jcode_side_panel_types::SidePanelPageSource::Managed,
                content: "updated".to_string(),
                updated_at_ms: 99,
            }],
        },
    };
    let json = encode_event(&event);
    assert!(json.contains("\"type\":\"side_panel_state\""));
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::SidePanelState { snapshot } = decoded else {
        return Err(anyhow!("expected SidePanelState event"));
    };
    assert_eq!(snapshot.focused_page_id.as_deref(), Some("page-1"));
    assert_eq!(snapshot.pages.len(), 1);
    assert_eq!(snapshot.pages[0].title, "Notes");
    assert_eq!(snapshot.pages[0].content, "updated");
    Ok(())
}

#[test]
fn test_error_event_retry_after_roundtrip() -> Result<()> {
    let event = ServerEvent::Error {
        id: 42,
        message: "rate limited".to_string(),
        retry_after_secs: Some(17),
    };
    let json = encode_event(&event);
    let decoded = parse_event_json(json.trim())?;
    let ServerEvent::Error {
        id,
        message,
        retry_after_secs,
    } = decoded
    else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(id, 42);
    assert_eq!(message, "rate limited");
    assert_eq!(retry_after_secs, Some(17));
    Ok(())
}

#[test]
fn test_error_event_retry_after_back_compat_default() -> Result<()> {
    let json = r#"{"type":"error","id":7,"message":"oops"}"#;
    let decoded = parse_event_json(json)?;
    let ServerEvent::Error {
        id,
        message,
        retry_after_secs,
    } = decoded
    else {
        return Err(anyhow!("wrong event type"));
    };
    assert_eq!(id, 7);
    assert_eq!(message, "oops");
    assert_eq!(retry_after_secs, None);
    Ok(())
}
