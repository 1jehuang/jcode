//! A shared server's commands carry the identity of the client whose turn ran
//! them, never that of the pane that happened to start the server.

use super::*;

const PROBE: &str = "printf '%s' \"${YARAN_SESSION_ID-unset}\"";

/// Run `input` the way a turn does: under the client's terminal identity, in
/// a spawned session task that a task-local does not reach on its own.
async fn run_as_client(ctx: ToolContext, agent: Option<&str>, input: Value) -> ToolOutput {
    let env = agent
        .into_iter()
        .map(|name| ("YARAN_SESSION_ID".into(), name.into()))
        .collect();
    crate::hooks::with_client_terminal_env(env, async move {
        let session = ctx.session_id.clone();
        crate::runtime_activity::spawn_session(&session, async move {
            BashTool::new().execute(input, ctx).await
        })
        .expect("the session accepts the tool task")
        .await
        .expect("the tool task finishes")
        .expect("bash runs")
    })
    .await
}

#[tokio::test]
async fn foreground_commands_speak_as_their_own_client() {
    let mut a = make_ctx(None);
    a.session_id = "client-identity-a".into();
    let mut b = make_ctx(None);
    b.session_id = "client-identity-b".into();
    let (a, b) = tokio::join!(
        run_as_client(a, Some("agent-a"), json!({"command": PROBE})),
        run_as_client(b, Some("agent-b"), json!({"command": PROBE})),
    );
    assert!(
        a.output.contains("agent-a") && !a.output.contains("agent-b"),
        "{}",
        a.output
    );
    assert!(
        b.output.contains("agent-b") && !b.output.contains("agent-a"),
        "{}",
        b.output
    );
}

#[cfg(unix)]
#[tokio::test]
async fn reload_persistable_commands_speak_as_their_client() {
    let mut ctx = make_agent_ctx(jcode_agent_runtime::InterruptSignal::new());
    ctx.session_id = "client-identity-detached".into();
    let result = run_as_client(ctx, Some("agent-detached"), json!({"command": PROBE})).await;
    assert!(
        result.output.contains("agent-detached"),
        "{}",
        result.output
    );
}

#[tokio::test]
async fn background_commands_speak_as_their_client() {
    let mut ctx = make_ctx(None);
    ctx.session_id = "client-identity-background".into();
    let result = run_as_client(
        ctx,
        Some("agent-background"),
        json!({"command": PROBE, "run_in_background": true, "notify": false, "wake": false}),
    )
    .await;
    let id = result.metadata.as_ref().unwrap()["task_id"]
        .as_str()
        .unwrap();
    let mut finished = false;
    for _ in 0..100 {
        if let Some(status) = crate::background::global().status(id).await
            && status.status != BackgroundTaskStatus::Running
        {
            finished = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(finished, "background command did not finish");
    let output = crate::background::global().output(id).await.unwrap();
    assert!(output.contains("agent-background"), "{output}");
}

#[tokio::test]
async fn client_identity_overrides_saved_environment_and_an_empty_client_clears_it() {
    for (agent, expected) in [(Some("current-agent"), "current-agent"), (None, "unset")] {
        let mut ctx = make_ctx(None);
        ctx.session_id = format!("client-identity-saved-{expected}");
        crate::session_env::set_session_env(
            &ctx.session_id,
            std::collections::HashMap::from([(
                "YARAN_SESSION_ID".into(),
                "stale-session-agent".into(),
            )]),
        )
        .unwrap();
        let session = ctx.session_id.clone();
        let output = run_as_client(ctx, agent, json!({"command": PROBE})).await;
        crate::session_env::remove_session_env(&session);
        assert!(output.output.contains(expected), "{}", output.output);
        assert!(!output.output.contains("stale-session-agent"));
    }
}
