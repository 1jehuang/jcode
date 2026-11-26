use super::*;

#[tokio::test(start_paused = true)]
async fn configured_streaming_budget_and_disabled_handoff() {
    for budget in [Duration::from_millis(25), Duration::from_secs(12)] {
        let bg = InterruptSignal::new();
        let shutdown = InterruptSignal::new();
        let start = tokio::time::Instant::now();
        assert!(wait_for_tool_handoff(&bg, &shutdown, true, budget).await);
        assert_eq!(start.elapsed(), budget);
    }
    for shutdown_requested in [false, true] {
        let bg = InterruptSignal::new();
        let shutdown = InterruptSignal::new();
        tokio::select! {
            biased;
            _ = wait_for_tool_handoff(&bg, &shutdown, true, Duration::ZERO) => {
                panic!("zero budget should disable automatic handoff");
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
        if shutdown_requested {
            shutdown.fire();
        } else {
            bg.fire();
        }
        assert!(!wait_for_tool_handoff(&bg, &shutdown, true, Duration::ZERO).await);
    }
}

#[tokio::test(start_paused = true)]
async fn callback_paths_honor_custom_and_disabled_budgets() {
    // SDK and nonstreaming calls share execute_with_background_budget.
    for (budget, action, backgrounded) in [
        (Duration::from_millis(1250), "status", true),
        (Duration::from_secs(12), "status", false),
        (Duration::ZERO, "status", false),
        (Duration::from_millis(10), "wait", false),
    ] {
        let registry = Registry::empty();
        registry.register("bg".into(), Arc::new(WaitProbe)).await;
        let ctx = ToolContext {
            session_id: format!("configured-handoff-{action}-{}", budget.as_millis()),
            message_id: "message".into(),
            tool_call_id: "configured-probe".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };
        let start = tokio::time::Instant::now();
        let result = execute_with_background_budget(
            registry,
            "bg".into(),
            serde_json::json!({"action": action, "intent": "Run  parser\ntests"}),
            ctx,
            budget,
        )
        .await
        .unwrap();
        if backgrounded {
            assert_eq!(start.elapsed(), budget);
            assert!(result.output.contains("automatically after 1.25 seconds"));
            assert!(result.output.contains("task_id:"));
            let task_id = result
                .output
                .split("task_id: ")
                .nth(1)
                .unwrap()
                .split(')')
                .next()
                .unwrap();
            let finished = crate::background::global()
                .wait(task_id, Duration::from_secs(30), false)
                .await
                .unwrap();
            assert_eq!(
                finished.task.status,
                crate::bus::BackgroundTaskStatus::Completed
            );
            // The intent names the task, so waits do not read "Waiting for bg".
            assert_eq!(
                finished.task.display_name.as_deref(),
                Some("Run parser tests")
            );
            assert_eq!(
                crate::background::global().output(task_id).await.unwrap(),
                "wait finished"
            );
        } else {
            assert_eq!(start.elapsed(), Duration::from_secs(8));
            assert_eq!(result.output, "wait finished");
        }
    }
}

struct WaitProbe;

#[tokio::test(start_paused = true)]
async fn ask_user_waits_past_budget_and_returns_its_answer() {
    let registry = Registry::empty();
    registry
        .register("ask_user".into(), Arc::new(WaitProbe))
        .await;
    let ctx = ToolContext {
        session_id: "foreground-ask-user-probe".into(),
        message_id: "message".into(),
        tool_call_id: "ask-user-probe".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };
    let start = tokio::time::Instant::now();
    let result = execute_with_background_budget(
        registry,
        "ask_user".into(),
        serde_json::json!({"question": "Continue?"}),
        ctx,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(8));
    assert_eq!(result.output, "wait finished");
}

#[tokio::test(start_paused = true)]
async fn streaming_ask_user_wait_is_not_automatically_backgrounded() {
    for input in [
        serde_json::json!({}),
        serde_json::json!({"action": "choose"}),
    ] {
        assert!(!auto_background_allowed("ask_user", &input));
        let bg = InterruptSignal::new();
        let shutdown = InterruptSignal::new();
        tokio::select! {
            biased;
            _ = wait_for_tool_handoff(&bg, &shutdown, auto_background_allowed("ask_user", &input), Duration::from_secs(3)) => {
                panic!("human decision was automatically backgrounded");
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
        shutdown.fire();
        assert!(!wait_for_tool_handoff(&bg, &shutdown, false, Duration::from_secs(3)).await);
    }
}

#[async_trait::async_trait]
impl crate::tool::Tool for WaitProbe {
    fn name(&self) -> &str {
        "bg"
    }
    fn description(&self) -> &str {
        "Wait probe"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    async fn execute(
        &self,
        _: serde_json::Value,
        _: ToolContext,
    ) -> Result<crate::tool::ToolOutput> {
        tokio::time::sleep(Duration::from_secs(8)).await;
        Ok(crate::tool::ToolOutput::new("wait finished"))
    }
}

#[tokio::test(start_paused = true)]
async fn nonstreaming_background_wait_returns_original_result_after_budget() {
    let registry = Registry::empty();
    registry.register("bg".into(), Arc::new(WaitProbe)).await;
    let ctx = ToolContext {
        session_id: "foreground-wait-probe".into(),
        message_id: "message".into(),
        tool_call_id: "wait-probe".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };
    let start = tokio::time::Instant::now();
    let result = execute_with_background(
        registry,
        "bg".into(),
        serde_json::json!({"action":"wait"}),
        ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.output, "wait finished");
    assert_eq!(start.elapsed(), Duration::from_secs(8));
}

#[tokio::test(start_paused = true)]
async fn background_wait_is_not_automatically_backgrounded() {
    let input = serde_json::json!({"action": "wait", "task_id": "task"});
    assert!(!auto_background_allowed("bg", &input));
    assert!(auto_background_allowed(
        "bg",
        &serde_json::json!({"action": "status"})
    ));
    assert!(auto_background_allowed("bash", &input));
    let bg = InterruptSignal::new();
    let shutdown = InterruptSignal::new();
    tokio::select! {
        biased;
        _ = wait_for_tool_handoff(&bg, &shutdown, auto_background_allowed("bg", &input), Duration::from_secs(3)) => {
            panic!("bg wait was backgrounded while waiting for its dependency");
        }
        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
    }
    // Reloads still interrupt a wait safely, without waiting for its deadline.
    shutdown.fire();
    assert!(!wait_for_tool_handoff(&bg, &shutdown, false, Duration::from_secs(3)).await);
}

#[tokio::test(start_paused = true)]
async fn slow_tool_handoff_occurs_after_three_seconds_without_cancelling_work() {
    let bg = InterruptSignal::new();
    let shutdown = InterruptSignal::new();
    let mut work = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        42
    });
    let start = tokio::time::Instant::now();
    tokio::select! {
        biased;
        _ = &mut work => panic!("slow work completed before handoff"),
        automatic = wait_for_tool_handoff(&bg, &shutdown, true, Duration::from_secs(3)) => assert!(automatic),
    }
    assert_eq!(start.elapsed(), Duration::from_secs(3));
    assert!(!work.is_finished());
    assert_eq!(work.await.unwrap(), 42);
}

#[tokio::test(start_paused = true)]
async fn fast_tool_finishes_in_foreground() {
    let bg = InterruptSignal::new();
    let shutdown = InterruptSignal::new();
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_secs(1)) => {},
        _ = wait_for_tool_handoff(&bg, &shutdown, true, Duration::from_secs(3)) => panic!("fast tool backgrounded"),
    }
}

#[tokio::test(start_paused = true)]
async fn manual_background_and_shutdown_do_not_wait_for_budget() {
    for shutdown_requested in [false, true] {
        let bg = InterruptSignal::new();
        let shutdown = InterruptSignal::new();
        if shutdown_requested {
            shutdown.fire();
        } else {
            bg.fire();
        }
        let start = tokio::time::Instant::now();
        assert!(!wait_for_tool_handoff(&bg, &shutdown, true, Duration::from_secs(3)).await);
        assert_eq!(start.elapsed(), Duration::ZERO);
    }
}

#[tokio::test(start_paused = true)]
async fn handoff_preserves_shell_delivery_and_ignores_other_tools_flags() {
    for (name, input, expected) in [
        ("bash", serde_json::json!({}), (true, true)),
        (
            "functions.shell",
            serde_json::json!({"wake": false}),
            (true, false),
        ),
        (
            "Bash",
            serde_json::json!({"notify": false, "wake": null}),
            (false, false),
        ),
        (
            "remote_exec",
            serde_json::json!({"notify": false}),
            (false, false),
        ),
        (
            "communicate",
            serde_json::json!({"notify": false, "wake": false}),
            (true, true),
        ),
    ] {
        let registry = Registry::empty();
        registry
            .register(
                Registry::resolve_tool_name(name).into(),
                Arc::new(WaitProbe),
            )
            .await;
        let ctx = ToolContext {
            session_id: format!("handoff-delivery-{name}"),
            message_id: "message".into(),
            tool_call_id: "delivery-probe".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };
        let result = execute_with_background_budget(
            registry,
            name.into(),
            input,
            ctx,
            Duration::from_millis(10),
        )
        .await
        .unwrap();
        assert!(
            result
                .output
                .contains(crate::tool::background_delivery_notice(expected.1))
        );
        let task_id = result
            .output
            .split("task_id: ")
            .nth(1)
            .unwrap()
            .split(')')
            .next()
            .unwrap();
        let status = crate::background::global().status(task_id).await.unwrap();
        assert_eq!((status.notify, status.wake), expected, "{name}");
        crate::background::global().cancel(task_id).await.unwrap();
    }
}

#[tokio::test]
async fn custom_sdk_shell_names_keep_default_completion_delivery() {
    let _env = crate::storage::lock_test_env();
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            crate::tool::sdk::remove_session(&self.0);
        }
    }
    for (index, (custom, called)) in [
        ("shell", "shell"),
        ("Bash", "functions.Bash"),
        ("bash", "shell"),
        ("bash", "functions.bash"),
        ("remote_exec", "remote_exec"),
    ]
    .into_iter()
    .enumerate()
    {
        let session = format!("sdk-handoff-delivery-{index}");
        let _cleanup = Cleanup(session.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        crate::tool::sdk::configure(
            &session,
            "handoff-owner",
            crate::protocol::SessionToolConfig {
                custom: vec![crate::protocol::SessionToolDefinition {
                    name: custom.into(),
                    description: "Custom SDK callback".into(),
                    parameters: serde_json::json!({"type":"object"}),
                }],
                ..Default::default()
            },
            tx,
        )
        .unwrap();
        let ctx = ToolContext {
            session_id: session,
            message_id: "message".into(),
            tool_call_id: "sdk-delivery".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };
        let output = execute_with_background_budget(
            Registry::empty(),
            called.into(),
            serde_json::json!({"notify":false,"wake":false}),
            ctx,
            Duration::from_millis(10),
        )
        .await
        .unwrap();
        assert!(rx.try_recv().is_ok(), "the custom callback must execute");
        let id = output
            .output
            .split("task_id: ")
            .nth(1)
            .unwrap()
            .split(')')
            .next()
            .unwrap();
        let status = crate::background::global().status(id).await.unwrap();
        assert!(status.notify && status.wake, "{called} overrides {custom}");
        crate::background::global().cancel(id).await.unwrap();
    }
}

#[cfg(unix)]
mod bash_completion {
    use super::*;

    #[tokio::test]
    async fn bash_outer_handoff_publishes_one_final_completion() {
        check_bash_outer_completion(false).await;
    }

    #[tokio::test]
    async fn reload_persistable_bash_outer_handoff_publishes_one_final_completion() {
        check_bash_outer_completion(true).await;
    }

    async fn check_bash_outer_completion(reload_persistable: bool) {
        for silent in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let release = directory.path().join("release");
            let session = format!(
                "bash-outer-{}-{silent}",
                directory.path().file_name().unwrap().to_string_lossy()
            );
            let expected = "one final shell result";
            let registry = Registry::new(Arc::new(NoRequestProvider)).await;
            let ctx = ToolContext {
                session_id: session.clone(),
                message_id: "message".into(),
                tool_call_id: "single-shell-completion".into(),
                working_dir: None,
                stdin_request_tx: None,
                graceful_shutdown_signal: reload_persistable.then(InterruptSignal::new),
                execution_mode: if reload_persistable {
                    ToolExecutionMode::AgentTurn
                } else {
                    ToolExecutionMode::Direct
                },
            };
            let mut input = serde_json::json!({
                "command": format!(
                    "while [ ! -f '{}' ]; do sleep 0.02; done; printf '{expected}'",
                    release.display()
                ),
                "timeout": 80,
            });
            if silent {
                input["notify"] = false.into();
                input["wake"] = false.into();
            }
            let mut bus = crate::bus::Bus::global().subscribe();
            let handoff = execute_with_background_budget(
                registry,
                "bash".into(),
                input,
                ctx,
                Duration::from_millis(20),
            )
            .await
            .unwrap();
            let task_id = handoff
                .output
                .split("task_id: ")
                .nth(1)
                .unwrap()
                .split(')')
                .next()
                .unwrap();
            // Hold the subprocess past its own timeout after the outer handoff.
            // It cannot finish early because only this test releases it.
            tokio::time::sleep(Duration::from_millis(200)).await;
            tokio::fs::write(&release, "ready").await.unwrap();
            let completed = crate::background::global()
                .wait(task_id, Duration::from_secs(10), false)
                .await
                .unwrap();
            let mut completions = Vec::new();
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let crate::bus::BusEvent::BackgroundTaskCompleted(event) =
                        bus.recv().await.unwrap()
                        && event.session_id == session
                    {
                        let final_result = event.output_preview.contains(expected);
                        completions.push(event);
                        if final_result {
                            break;
                        }
                    }
                }
            })
            .await
            .expect("the shell must publish its actual final result");
            while let Ok(event) = bus.try_recv() {
                if let crate::bus::BusEvent::BackgroundTaskCompleted(event) = event
                    && event.session_id == session
                {
                    completions.push(event);
                }
            }
            assert_eq!(
                completions.len(),
                1,
                "duplicate shell completions: {completions:?}"
            );
            let event = &completions[0];
            assert_eq!(event.task_id, task_id);
            assert_eq!((event.notify, event.wake), (!silent, !silent));
            assert_eq!(
                completed.task.status,
                crate::bus::BackgroundTaskStatus::Completed
            );
            let output = crate::background::global().output(task_id).await.unwrap();
            assert!(
                output.contains(expected),
                "the outer task lost the shell result: {output}"
            );
            assert!(
                !output.contains("continuing in background"),
                "outer completion contains a nested handoff: {output}"
            );
            let tasks = crate::background::global().list().await;
            assert_eq!(
                tasks
                    .iter()
                    .filter(|task| task.session_id == session)
                    .count(),
                1
            );
        }
    }

    struct NoRequestProvider;

    #[async_trait::async_trait]
    impl Provider for NoRequestProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            anyhow::bail!("shell handoff must not send a provider request")
        }

        fn name(&self) -> &str {
            "shell-test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }
}
