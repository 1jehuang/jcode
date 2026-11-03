use super::*;

#[tokio::test]
async fn session_env_foreground_sessions_are_isolated() {
    use crate::session_env::{remove_session_env, set_session_env};
    for (sid, value) in [("env-bash-a", "proxy-a"), ("env-bash-b", "proxy-b")] {
        set_session_env(
            sid,
            std::collections::HashMap::from([("HTTPS_PROXY".into(), value.into())]),
        )
        .unwrap();
    }
    let mut a = make_ctx(None);
    a.session_id = "env-bash-a".into();
    let mut b = make_ctx(None);
    b.session_id = "env-bash-b".into();
    let tool = BashTool::new();
    let command = json!({"command": "printf '%s:%s' \"$HTTPS_PROXY\" \"$JCODE_SESSION_ID\""});
    let (a, b) = tokio::join!(tool.execute(command.clone(), a), tool.execute(command, b));
    let a = a.unwrap().output;
    let b = b.unwrap().output;
    assert!(a.contains("proxy-a:env-bash-a"), "{a}");
    assert!(!a.contains("proxy-b"));
    assert!(b.contains("proxy-b:env-bash-b"), "{b}");
    assert!(!b.contains("proxy-a"));
    remove_session_env("env-bash-a");
    remove_session_env("env-bash-b");
}

#[cfg(unix)]
#[tokio::test]
async fn session_env_reload_persistable_command_receives_overrides() {
    let mut ctx = make_agent_ctx(jcode_agent_runtime::InterruptSignal::new());
    ctx.session_id = "env-bash-detached".into();
    crate::session_env::set_session_env(
        &ctx.session_id,
        std::collections::HashMap::from([("HTTPS_PROXY".into(), "proxy-detached".into())]),
    )
    .unwrap();
    let result = BashTool::new()
        .execute(
            json!({"command": "printf '%s:%s' \"$HTTPS_PROXY\" \"$JCODE_SESSION_ID\""}),
            ctx,
        )
        .await
        .unwrap();
    assert!(
        result.output.contains("proxy-detached:env-bash-detached"),
        "{}",
        result.output
    );
    crate::session_env::remove_session_env("env-bash-detached");
}

#[tokio::test]
async fn session_env_updates_affect_only_new_launches_and_invalid_updates_keep_bash_working() {
    use crate::session_env::{remove_session_env, set_session_env};
    let sid = "env-bash-updates";
    let mut ctx = make_ctx(None);
    ctx.session_id = sid.into();
    let tool = BashTool::new();
    let input = json!({"command": "printf '%s' \"${SESSION_ENV_REGRESSION_PROBE-unset}\""});
    for value in ["first", "second"] {
        set_session_env(
            sid,
            std::collections::HashMap::from([(
                "SESSION_ENV_REGRESSION_PROBE".into(),
                value.into(),
            )]),
        )
        .unwrap();
        assert!(
            set_session_env(
                sid,
                std::collections::HashMap::from([(
                    "SESSION_ENV_REGRESSION_PROBE".into(),
                    "invalid\0value".into()
                ),])
            )
            .is_err()
        );
        let result = tool.execute(input.clone(), ctx.clone()).await.unwrap();
        assert!(result.output.contains(value), "{}", result.output);
    }
    set_session_env(sid, std::collections::HashMap::new()).unwrap();
    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("unset"), "{}", result.output);
    remove_session_env(sid);
}

#[tokio::test]
async fn session_env_background_snapshots_before_deferred_launch() {
    use crate::session_env::{remove_session_env, set_session_env};
    for clear in [false, true] {
        let sid = format!("env-bash-race-{clear}");
        let original =
            std::collections::HashMap::from([("HTTPS_PROXY".into(), "proxy-original".into())]);
        set_session_env(&sid, original).unwrap();
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        SESSION_ENV_LAUNCH_GATES
            .lock()
            .unwrap()
            .insert(sid.clone(), (reached_tx, release_rx));
        let mut ctx = make_ctx(None);
        ctx.session_id = sid.clone();
        let result = BashTool::new()
            .execute(
                json!({
                    "command": "printf '%s:%s\\n' \"$HTTPS_PROXY\" \"$JCODE_SESSION_ID\"",
                    "run_in_background": true, "notify": false,
                }),
                ctx,
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
            .await
            .unwrap()
            .unwrap();
        if clear {
            set_session_env(&sid, std::collections::HashMap::new()).unwrap();
        } else {
            set_session_env(
                &sid,
                std::collections::HashMap::from([("HTTPS_PROXY".into(), "proxy-replaced".into())]),
            )
            .unwrap();
        }
        // Teardown must not erase the snapshot of this already-accepted job.
        remove_session_env(&sid);
        release_tx.send(()).unwrap();
        let metadata = result.metadata.unwrap();
        let task_id = metadata["task_id"].as_str().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let status = crate::background::global().status(task_id).await.unwrap();
                if status.status != BackgroundTaskStatus::Running {
                    assert_eq!(status.status, BackgroundTaskStatus::Completed);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let output = crate::background::global().output(task_id).await.unwrap();
        assert!(
            output.contains(&format!("proxy-original:{sid}")),
            "{output}"
        );
        assert!(!output.contains("proxy-replaced"));
    }
}
