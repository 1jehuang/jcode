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
    let definition = definitions
        .iter()
        .find(|d| d.name == "panel")
        .expect("model-visible panel definition");
    assert!(definition.description.contains("desktop panels"));
    let mut events = crate::bus::Bus::global().subscribe();
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
    assert!(
        matches!(events.try_recv().unwrap(), crate::bus::BusEvent::SidePanelUpdated(update) if update.snapshot == first_state)
    );
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
    std::fs::write(
        &path,
        b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF",
    )
    .unwrap();
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
