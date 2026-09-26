#![allow(clippy::await_holding_lock)]
use super::*;

fn context(session: &str, call: &str) -> ToolContext {
    ToolContext {
        session_id: session.into(),
        message_id: "msg".into(),
        tool_call_id: call.into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    }
}

struct Home(Option<std::ffi::OsString>, #[allow(dead_code)] tempfile::TempDir);
impl Drop for Home {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => crate::env::set_var("JCODE_HOME", v),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}
fn temp_home() -> Home {
    let temp = tempfile::tempdir().unwrap();
    let old = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    Home(old, temp)
}

#[test]
fn ids_are_slugs_with_hex() {
    let id = generate_instance_id("Sales Chart!");
    assert!(id.starts_with("sales-chart-"), "{id}");
    assert_eq!(id.len(), "sales-chart-".len() + 4);
    assert!(generate_instance_id("!!").starts_with("applet-"));
}

#[tokio::test]
async fn mount_patch_wait_and_close() {
    let _guard = crate::storage::lock_test_env();
    let _home = temp_home();
    let tool = AppletTool::new();
    let registry = crate::tool::Registry::empty();
    assert!(
        crate::tool::Registry::base_tools(&registry.skills).contains_key("applet"),
        "applet must be registered"
    );

    let out = tool
        .execute(
            json!({"title":"Pick","view":{"type":"button","label":"Go","on_press":{"action":"go"}}}),
            context("s1", "call-1"),
        )
        .await
        .unwrap();
    assert!(out.output.contains("revision 1"), "{}", out.output);
    let snap = crate::applets::snapshot_for_session("s1").unwrap();
    let inst = &snap.instances[0];
    assert!(inst.id.starts_with("pick-"));
    assert_eq!(
        inst.placement,
        Placement::Inline {
            session_id: "s1".into(),
            anchor: Anchor::ToolCall {
                call_id: "call-1".into()
            }
        }
    );
    let id = inst.id.clone();

    // Invalid views are rejected without changing anything.
    let err = tool
        .execute(
            json!({"action":"update","instance":id,"view":{"type":"image","source":{"path":"/etc/x"},"alt":"x"}}),
            context("s1", "call-2"),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid"), "{err}");

    tool.execute(
        json!({"action":"patch","instance":id,"ops":[{"op":"replace","path":"/title","value":"P2"}]}),
        context("s1", "call-3"),
    )
    .await
    .unwrap();
    assert_eq!(
        crate::applets::snapshot_for_session("s1").unwrap().instances[0].document.title,
        "P2"
    );

    let wid = id.clone();
    let waiter = tokio::spawn(async move {
        AppletTool::new()
            .execute(
                json!({"action":"update","instance":wid,"wait":true,"placement":"panel"}),
                context("s1", "call-4"),
            )
            .await
            .unwrap()
    });
    let mut delivered = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if deliver_to_waiter("s1", &id, "[applet action] go".into()) {
            delivered = true;
            break;
        }
    }
    assert!(delivered);
    assert!(waiter.await.unwrap().output.contains("[applet action] go"));

    tool.execute(json!({"action":"close","instance":id}), context("s1", "c"))
        .await
        .unwrap();
    assert!(crate::applets::snapshot_for_session("s1").unwrap().instances.is_empty());
}

#[test]
fn action_message_is_compact() {
    let msg = format_action_message(
        "chart-3f2a",
        "Title",
        &jcode_applet_types::Action {
            action: "select".into(),
            args: json!({"id":2}),
        },
        &json!({"q":"x"}),
        None,
    );
    assert_eq!(
        msg,
        "[applet action] instance `chart-3f2a` (\"Title\"): `select` args {\"id\":2}\nstate: {\"q\":\"x\"}"
    );
}
