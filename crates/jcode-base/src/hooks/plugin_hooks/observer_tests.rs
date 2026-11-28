use super::*;

async fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "observer did not reach barrier"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn observer_workers_queue_payloads_and_identity_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    struct Release {
        path: PathBuf,
        pool: Option<Observers>,
    }
    impl Drop for Release {
        fn drop(&mut self) {
            std::fs::write(&self.path, "").expect("release observer test children");
            let pool = self.pool.take().unwrap();
            drop(pool.sender);
            for worker in pool.workers {
                if worker.join().is_err() {
                    eprintln!("observer test worker panicked");
                }
            }
        }
    }
    let release = Release {
        path: temp.path().join("release"),
        pool: Some(Observers::new().unwrap()),
    };
    for dir in ["hooks", "started", "done"] {
        std::fs::create_dir(temp.path().join(dir)).unwrap();
    }
    std::fs::write(temp.path().join("hooks/hooks.json"), json!({"hooks":{"PostToolUse":[{"hooks":[{
        "type":"command", "timeout":10,
        "command":"printf '%s' \"$YARAN_SESSION_ID\" > \"$CLAUDE_PLUGIN_ROOT/started/$JCODE_HOOK_SESSION_ID\"; while [ ! -f \"$CLAUDE_PLUGIN_ROOT/release\" ]; do sleep 0.01; done; cat > \"$CLAUDE_PLUGIN_ROOT/done/$JCODE_HOOK_SESSION_ID\""
    }]}]}}).to_string()).unwrap();
    let (hooks, _) = load_root(temp.path());
    let observers = release.pool.as_ref().unwrap();
    let event = |id| {
        HookEvent::new("post_tool")
            .cwd(temp.path().to_string_lossy())
            .session_id(format!("{id}"))
            .plugin_tool_payload(json!({"value":"full input"}), json!("x".repeat(20000)))
    };
    for id in 0..WORKERS {
        assert!(
            super::super::super::with_client_terminal_env(
                vec![("YARAN_SESSION_ID".into(), format!("client-{id}"))],
                async { observers.submit(&event(id), hooks.clone()) }
            )
            .await
        );
    }
    wait_for(|| {
        std::fs::read_dir(temp.path().join("started"))
            .unwrap()
            .count()
            == WORKERS
    })
    .await;
    for id in WORKERS..WORKERS + QUEUED_EVENTS {
        assert!(
            super::super::super::with_client_terminal_env(
                vec![("YARAN_SESSION_ID".into(), format!("client-{id}"))],
                async { observers.submit(&event(id), hooks.clone()) }
            )
            .await
        );
    }
    assert!(
        !observers.submit(&event(999), hooks.clone()),
        "saturation drops the newest event"
    );
    assert_eq!(
        std::fs::read_dir(temp.path().join("started"))
            .unwrap()
            .count(),
        WORKERS
    );
    let oversized = event(1000).plugin_tool_payload(json!({}), json!("x".repeat(EVENT_BYTES)));
    assert!(Job::new(&oversized, hooks.clone()).is_none());
    assert!(
        Job::new(
            &HookEvent::new("post_tool").field("OUTPUT", "x".repeat(EVENT_BYTES)),
            hooks.clone()
        )
        .is_none()
    );
    assert!(
        super::super::super::with_client_terminal_env(
            vec![("YARAN_SESSION_ID".into(), "x".repeat(EVENT_BYTES))],
            async { Job::new(&event(1001), hooks.clone()).is_none() }
        )
        .await
    );
    std::fs::write(temp.path().join("release"), "").unwrap();
    wait_for(|| {
        (0..WORKERS + QUEUED_EVENTS).all(|id| {
            std::fs::read(temp.path().join(format!("done/{id}")))
                .is_ok_and(|bytes| serde_json::from_slice::<Value>(&bytes).is_ok())
        })
    })
    .await;
    for id in 0..WORKERS + QUEUED_EVENTS {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(format!("started/{id}"))).unwrap(),
            format!("client-{id}")
        );
        let value: Value =
            serde_json::from_slice(&std::fs::read(temp.path().join(format!("done/{id}"))).unwrap())
                .unwrap();
        assert_eq!(value["tool_input"], json!({"value":"full input"}));
        assert_eq!(value["tool_response"], "x".repeat(20000));
    }
    assert!(!temp.path().join("started/999").exists());
}

#[tokio::test]
async fn queued_observer_rechecks_disable_and_invalid_roots_before_spawning() {
    use super::super::test_fixture::{Fixture, write};
    use crate::plugin::PluginScope;
    let f = Fixture::new();
    let installed = f
        .manager
        .install("registry-fixture", PluginScope::Project)
        .unwrap();
    write(
        &installed.root.join("hooks/hooks.json"),
        &json!({"hooks":{"SessionStart":[{"hooks":[{
            "type":"command", "command":"touch \"$CLAUDE_PLUGIN_ROOT/ran\""
        }]}]}})
        .to_string(),
    );
    let event = HookEvent::new("session_start").cwd(f.project.to_string_lossy());
    let job = Job::new(&event, commands(&event)).unwrap();
    assert!(!job.hooks.is_empty());
    f.manager
        .disable("registry-fixture", PluginScope::Project)
        .unwrap();
    job.run().await;
    assert!(!installed.root.join("ran").exists());
    f.manager
        .enable("registry-fixture", PluginScope::Project)
        .unwrap();
    let job = Job::new(&event, commands(&event)).unwrap();
    write(
        &installed.root.join(".claude-plugin/plugin.json"),
        "corrupt",
    );
    job.run().await;
    assert!(!installed.root.join("ran").exists());
    write(
        &installed.root.join(".claude-plugin/plugin.json"),
        r#"{"name":"registry-fixture"}"#,
    );
    Job::new(&event, commands(&event)).unwrap().run().await;
    assert!(
        installed.root.join("ran").exists(),
        "valid current scope still executes"
    );
}
