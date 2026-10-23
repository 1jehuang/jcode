#[test]
fn panel_actions_roundtrip_with_expected_session_identity() -> Result<()> {
    let request = Request::PanelAction {
        id: 81,
        session_id: "attached-session".into(),
        action: "reopen".into(),
        page_id: "plan".into(),
    };
    assert_eq!(request.id(), 81);
    assert!(!request.is_lightweight_control_request());
    let decoded: Request = serde_json::from_value(serde_json::to_value(request)?)?;
    assert!(
        matches!(decoded, Request::PanelAction { id: 81, session_id, action, page_id }
        if session_id == "attached-session" && action == "reopen" && page_id == "plan")
    );
    Ok(())
}

#[test]
fn legacy_panel_snapshots_default_retention_metadata() -> Result<()> {
    let snapshot: jcode_side_panel_types::SidePanelSnapshot = serde_json::from_value(
        serde_json::json!({
            "focused_page_id": "legacy",
            "pages": [{"id":"legacy","title":"Legacy","file_path":"legacy.md","content":"# Kept","updated_at_ms":1}]
        }),
    )?;
    assert_eq!(snapshot.session_id, None);
    assert_eq!(snapshot.revision, 0);
    assert!(snapshot.archived.is_empty());
    assert!(snapshot.unavailable.is_none());
    assert_eq!(
        snapshot
            .focused_page()
            .unwrap()
            .status
            .last_successful_update_ms,
        0
    );
    Ok(())
}

#[test]
fn empty_authoritative_and_unavailable_panel_metadata_survive_history_encoding() -> Result<()> {
    for metadata in [
        serde_json::json!({"session_id":"s1","revision":3}),
        serde_json::json!({"session_id":"s1","unavailable":"index temporarily unavailable"}),
        serde_json::json!({"session_id":"s1","archived":[{"id":"closed","title":"Closed","file_path":"closed.md","archived_at_ms":7}]}),
    ] {
        let event: ServerEvent = serde_json::from_value(serde_json::json!({
            "type":"history", "id":82, "session_id":"s1", "messages":[],
            "provider_name":null,"provider_model":null,"total_tokens":null,"all_sessions":[],
            "side_panel":metadata
        }))?;
        let encoded: serde_json::Value = serde_json::from_str(encode_event(&event).trim())?;
        assert!(
            encoded.get("side_panel").is_some(),
            "retention metadata must not be omitted: {encoded}"
        );
        let decoded: ServerEvent = serde_json::from_value(encoded)?;
        let ServerEvent::History { side_panel, .. } = decoded else {
            panic!("history")
        };
        assert_eq!(side_panel.session_id.as_deref(), Some("s1"));
    }
    Ok(())
}

#[test]
fn side_panel_state_roundtrips_saved_copy_freshness_without_archived_bodies() -> Result<()> {
    let event: ServerEvent = serde_json::from_value(serde_json::json!({
        "type":"side_panel_state", "snapshot":{
            "session_id":"s1", "revision":4, "focused_page_id":"linked",
            "pages":[{"id":"linked","title":"Linked","file_path":"source.md","content":"# Last good", "status":{
                "last_successful_update_ms":1234,"source_revision":5678,"stale_reason":"Source unavailable", "last_opened_at_ms":9012
            }}],
            "archived":[{"id":"closed","title":"Closed","file_path":"closed.md","status":{},"archived_at_ms":77}]
        }
    }))?;
    let wire = encode_event(&event);
    let decoded: ServerEvent = serde_json::from_str(wire.trim())?;
    let ServerEvent::SidePanelState { snapshot } = decoded else {
        panic!("panel state")
    };
    assert_eq!(snapshot.focused_page().unwrap().content, "# Last good");
    assert_eq!(
        snapshot
            .focused_page()
            .unwrap()
            .status
            .last_successful_update_ms,
        1234
    );
    assert_eq!(snapshot.archived.len(), 1);
    let json: serde_json::Value = serde_json::from_str(wire.trim())?;
    assert!(json["snapshot"]["archived"][0].get("content").is_none());
    assert!(json["snapshot"]["archived"][0].get("pdf_data").is_none());
    Ok(())
}
