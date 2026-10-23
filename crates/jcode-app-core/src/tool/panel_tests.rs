#![cfg_attr(test, allow(clippy::await_holding_lock))]
use super::*;
use crate::side_panel::SidePanelSnapshot;

fn context(dir: &std::path::Path, session: &str) -> ToolContext {
    ToolContext {
        session_id: session.into(),
        message_id: "msg".into(),
        tool_call_id: uuid::Uuid::new_v4().to_string(),
        working_dir: Some(dir.into()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    }
}

#[tokio::test]
async fn panel_visibility_follows_implementation_not_registry_name() {
    let registry = super::super::Registry::empty();
    registry
        .register(
            "legacy_alias".into(),
            std::sync::Arc::new(super::super::side_panel::SidePanelTool::new()),
        )
        .await;
    registry
        .register("side_panel".into(), std::sync::Arc::new(PanelTool::new()))
        .await;
    let definitions = registry.definitions(None).await;
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "side_panel");
}

#[tokio::test]
async fn panel_registered_lifecycle_and_validation() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let old = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(v) = &self.0 {
                crate::env::set_var("JCODE_HOME", v);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }
    let _restore = Restore(old);
    let registry = super::super::Registry::empty();
    // Exercise the actual production registration map rather than manually inserting PanelTool.
    *registry.tools.write().await = super::super::Registry::base_tools(&registry.skills);
    assert!(registry.tools.read().await.contains_key("side_panel"));
    assert!(registry.tools.read().await.contains_key("panel"));
    let definitions = registry.definitions(None).await;
    assert!(!definitions.iter().any(|d| d.name == "side_panel"));
    let allowed = ["panel".to_string(), "side_panel".to_string()]
        .into_iter()
        .collect();
    let allowed_definitions = registry.definitions(Some(&allowed)).await;
    assert_eq!(allowed_definitions.len(), 1);
    assert_eq!(allowed_definitions[0].name, "panel");
    assert!(
        !registry
            .definitions_for_session("panel-test")
            .await
            .iter()
            .any(|d| d.name == "side_panel")
    );
    let definition = definitions
        .iter()
        .find(|d| d.name == "panel")
        .expect("model-visible panel definition");
    assert!(definition.description.contains("session panels"));
    assert!(definition.description.contains("TUI and desktop"));
    let mut events = crate::bus::Bus::global().subscribe();
    // Another session's event may arrive before this test's panel write.
    crate::bus::Bus::global().publish(crate::bus::BusEvent::SidePanelUpdated(
        crate::bus::SidePanelUpdated {
            session_id: "other-panel-test".into(),
            snapshot: SidePanelSnapshot::default(),
        },
    ));
    let first = registry
        .execute(
            "panel",
            json!({"action":null,"content":"# One", "intent":"Show notes", "accept_large_output":false}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let first_state: SidePanelSnapshot = serde_json::from_value(first.metadata.unwrap()).unwrap();
    let id = &first_state.pages[0].id;
    assert!(first.output.contains(&format!(
        "panel_id: {id}\nidentity: side-panel://panel-test/{id}"
    )));
    // The bus is process-global: concurrent tests publish on it too, so search, don't take the head.
    let mut saw_first = false;
    while let Ok(event) = events.try_recv() {
        if let crate::bus::BusEvent::SidePanelUpdated(update) = event {
            saw_first |= update.snapshot == first_state;
        }
    }
    assert!(saw_first, "the first panel write must publish its snapshot");
    let second = registry
        .execute(
            "panel",
            json!({"content":"# Two"}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let second_state: SidePanelSnapshot = serde_json::from_value(second.metadata.unwrap()).unwrap();
    assert_eq!(second_state.pages.len(), 2);
    assert_ne!(second_state.focused_page_id, first_state.focused_page_id);
    let path = temp.path().join("report.pdf");
    std::fs::write(&path, synthetic_pdf("Panel lifecycle")).unwrap();
    let updated = registry
        .execute(
            "panel",
            json!({"action":"update", "panel_id":id, "file_path":"report.pdf"}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let updated: SidePanelSnapshot = serde_json::from_value(updated.metadata.unwrap()).unwrap();
    assert_eq!(updated.focused_page_id, second_state.focused_page_id);
    assert!(
        updated
            .pages
            .iter()
            .find(|p| &p.id == id)
            .unwrap()
            .pdf_data
            .is_none()
    );
    let mut saw_pdf_event = false;
    while let Ok(event) = events.try_recv() {
        if let crate::bus::BusEvent::SidePanelUpdated(update) = event {
            saw_pdf_event |= update
                .snapshot
                .pages
                .iter()
                .any(|p| &p.id == id && p.pdf_data.is_some());
        }
    }
    assert!(
        saw_pdf_event,
        "full PDF bytes must reach the desktop event, not tool metadata"
    );
    for input in [
        json!({"action":"update","panel_id":"missing","content":"no"}),
        json!({"content":"one","file_path":"report.pdf"}),
        json!({}),
        json!({"content":"no","panel_id":id}),
    ] {
        assert!(
            registry
                .execute("panel", input, context(temp.path(), "panel-test"))
                .await
                .is_err()
        );
    }
    assert!(
        registry
            .execute(
                "panel",
                json!({"action":"update","panel_id":id,"content":"no"}),
                context(temp.path(), "other-session")
            )
            .await
            .is_err()
    );
    let focused = registry
        .execute(
            "panel",
            json!({"action":"focus","panel_id":id}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    assert_eq!(focused.metadata.unwrap()["focused_page_id"], *id);
    std::fs::remove_file(&path).unwrap();
    let failed_refresh = registry
        .execute(
            "panel",
            json!({"action":"refresh", "panel_id":id}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let failed_refresh: SidePanelSnapshot =
        serde_json::from_value(failed_refresh.metadata.unwrap()).unwrap();
    let failed_page = failed_refresh
        .pages
        .iter()
        .find(|page| &page.id == id)
        .unwrap();
    assert!(failed_page.status.stale_reason.is_some());
    assert!(failed_page.content.is_empty() && failed_page.pdf_data.is_none());
    assert!(
        crate::side_panel::snapshot_for_session("panel-test")
            .unwrap()
            .pages
            .iter()
            .any(|page| &page.id == id && page.pdf_data.is_some())
    );
    std::fs::write(&path, synthetic_pdf("Refreshed panel")).unwrap();
    let refreshed = registry
        .execute(
            "panel",
            json!({"action":"refresh", "panel_id":id}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let refreshed: SidePanelSnapshot = serde_json::from_value(refreshed.metadata.unwrap()).unwrap();
    assert!(
        refreshed
            .pages
            .iter()
            .find(|page| &page.id == id)
            .unwrap()
            .status
            .stale_reason
            .is_none()
    );
    for input in [
        json!({"action":"refresh"}),
        json!({"action":"refresh", "panel_id":id, "content":"invalid"}),
        json!({"action":"refresh", "panel_id":"missing"}),
    ] {
        assert!(
            registry
                .execute("panel", input, context(temp.path(), "panel-test"))
                .await
                .is_err()
        );
    }
    assert!(
        registry
            .execute(
                "panel",
                json!({"action":"refresh", "panel_id":id}),
                context(temp.path(), "other-session")
            )
            .await
            .is_err()
    );
    registry
        .execute(
            "panel",
            json!({"action":"close","panel_id":id}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let listed = registry
        .execute(
            "panel",
            json!({"action":"list"}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let listed: SidePanelSnapshot = serde_json::from_value(listed.metadata.unwrap()).unwrap();
    assert_eq!(listed.pages.len(), 1);
    assert!(listed.pages.iter().all(|p| &p.id != id));
    assert_eq!(listed.archived.len(), 1);
    assert_eq!(&listed.archived[0].id, id);
    assert!(
        listed
            .pages
            .iter()
            .all(|p| p.content.is_empty() && p.pdf_data.is_none())
    );
    let reopened = registry
        .execute(
            "panel",
            json!({"action":"reopen", "panel_id":id}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let reopened: SidePanelSnapshot = serde_json::from_value(reopened.metadata.unwrap()).unwrap();
    assert!(reopened.archived.is_empty());
    assert_eq!(reopened.focused_page_id.as_ref(), Some(id));
    assert!(reopened.pages.iter().all(|p| p.pdf_data.is_none()));
    assert!(
        crate::side_panel::snapshot_for_session("panel-test")
            .unwrap()
            .pages
            .iter()
            .any(|p| &p.id == id && p.pdf_data.is_some())
    );
    let history = registry
        .execute(
            "panel",
            json!({"action":"history"}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    let history: SidePanelSnapshot = serde_json::from_value(history.metadata.unwrap()).unwrap();
    assert!(
        history
            .pages
            .iter()
            .all(|p| p.content.is_empty() && p.pdf_data.is_none())
    );
    assert!(
        registry
            .execute(
                "panel",
                json!({"action":"purge", "panel_id":id}),
                context(temp.path(), "other-session")
            )
            .await
            .is_err()
    );
    registry
        .execute(
            "panel",
            json!({"action":"purge", "panel_id":id}),
            context(temp.path(), "panel-test"),
        )
        .await
        .unwrap();
    assert!(
        registry
            .execute(
                "panel",
                json!({"action":"reopen", "panel_id":id}),
                context(temp.path(), "panel-test")
            )
            .await
            .is_err()
    );
    assert!(path.exists());
    let legacy = registry
        .execute(
            "side_panel",
            json!({"action":"load", "file_path":"report.pdf"}),
            context(temp.path(), "legacy-pdf"),
        )
        .await
        .unwrap();
    let legacy: SidePanelSnapshot = serde_json::from_value(legacy.metadata.unwrap()).unwrap();
    assert!(legacy.pages[0].pdf_data.is_none());
    assert!(
        crate::side_panel::snapshot_for_session("legacy-pdf")
            .unwrap()
            .pages[0]
            .pdf_data
            .is_some()
    );
    for accept_large_output in [Value::Null, json!(false), json!(true)] {
        registry
            .execute(
                "panel",
                json!({
                    "action": "list",
                    "intent": "Inspect panels",
                    "accept_large_output": accept_large_output,
                }),
                context(temp.path(), "panel-test"),
            )
            .await
            .expect("framework-injected fields must be accepted");
    }
}

/// A complete one-page PDF with real object offsets and a cross-reference table.
fn synthetic_pdf(label: &str) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 50 100 Td ({label}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
    ];
    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    pdf
}

#[test]
fn panel_history_schema_and_sendable_future() {
    let tool = PanelTool::new();
    let schema = tool.parameters_schema();
    for action in [
        "spawn", "update", "focus", "close", "list", "history", "reopen", "purge", "refresh",
    ] {
        assert!(
            schema["properties"]["action"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!(action))
        );
    }
    assert_eq!(schema["additionalProperties"], false);
    fn assert_send<T: Send>(_: T) {}
    assert_send(tool.execute(
        json!({"action":"history"}),
        context(std::path::Path::new("."), "synthetic"),
    ));
}
