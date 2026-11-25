use super::*;
use crate::bus::{BackgroundTaskCompleted, Bus, BusEvent};

async fn completion(
    events: &mut tokio::sync::broadcast::Receiver<BusEvent>,
    id: &str,
) -> BackgroundTaskCompleted {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let BusEvent::BackgroundTaskCompleted(event) = events.recv().await.unwrap()
                && event.task_id == id
            {
                return event;
            }
        }
    })
    .await
    .expect("completion event")
}

async fn pause_file_reads() -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (entered, started) = tokio::sync::oneshot::channel();
    let (resume, pause) = std::sync::mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        entered.send(()).unwrap();
        pause.recv().unwrap();
    });
    started.await.unwrap();
    (resume, blocker)
}

#[test]
fn delivery_guard_returns_or_restores_completion_even_when_release_is_cancelled() {
    if std::env::var_os("JCODE_BG_DELIVERY_CHILD").is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tool::bg::delivery_tests::delivery_guard_returns_or_restores_completion_even_when_release_is_cancelled",
                "--nocapture",
            ])
            .env("JCODE_BG_DELIVERY_CHILD", "1")
            .env("HOME", home.path())
            .env("JCODE_HOME", home.path().join("state"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let manager = background::global();
        for mode in ["return", "abandon", "cancel-release"] {
            let mut events = Bus::global().subscribe();
            let (finish, wait) = tokio::sync::oneshot::channel();
            let info = manager
                .spawn_with_notify("test", None, "wait-owner", true, true, |_| async move {
                    wait.await.unwrap();
                    Ok(background::TaskResult::completed(Some(0)))
                })
                .await;
            assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
            let hold = DeliveryHold(Some(info.task_id.clone()));
            finish.send(()).unwrap();
            assert!(!completion(&mut events, &info.task_id).await.wake);
            match mode {
                "return" => {
                    let released = hold.release().await.unwrap();
                    let result = finish_wait_result(None, Some(released.clone())).unwrap();
                    assert_eq!(
                        result.reason,
                        background::BackgroundTaskWaitReason::Finished
                    );
                    assert_eq!(result.task.status, BackgroundTaskStatus::Completed);
                    let mut running = released.clone();
                    running.status = BackgroundTaskStatus::Running;
                    let timeout = background::BackgroundTaskWaitResult {
                        reason: background::BackgroundTaskWaitReason::Timeout,
                        task: running.clone(),
                        progress_event: None,
                        event_record: None,
                    };
                    assert_eq!(
                        finish_wait_result(Some(timeout), Some(released.clone()))
                            .unwrap()
                            .reason,
                        background::BackgroundTaskWaitReason::Finished
                    );
                    assert!(finish_wait_result(None, Some(running)).is_none());
                    assert_eq!(
                        finish_wait_result(Some(result), Some(released))
                            .unwrap()
                            .task
                            .task_id,
                        info.task_id
                    );
                }
                "abandon" => {
                    drop(hold);
                    assert!(completion(&mut events, &info.task_id).await.wake);
                }
                _ => {
                    // Queue the async status read behind an occupied blocking pool.
                    let (entered, started) = tokio::sync::oneshot::channel();
                    let (resume, pause) = std::sync::mpsc::channel();
                    let blocker = tokio::task::spawn_blocking(move || {
                        entered.send(()).unwrap();
                        pause.recv().unwrap();
                    });
                    started.await.unwrap();
                    let mut release = Box::pin(hold.release());
                    let pending = futures::poll!(&mut release).is_pending();
                    drop(release);
                    resume.send(()).unwrap();
                    blocker.await.unwrap();
                    assert!(pending, "release must reach the blocked status read");
                    assert!(completion(&mut events, &info.task_id).await.wake);
                }
            }
            while let Ok(event) = events.try_recv() {
                if let BusEvent::BackgroundTaskCompleted(event) = event {
                    assert_ne!(event.task_id, info.task_id, "completion delivered twice");
                }
            }
        }

        for (cancel_preview, lose_status) in [(true, false), (true, true), (false, true)] {
            let session = "wait-preview-owner";
            let mut events = Bus::global().subscribe();
            let (finish, ready) = tokio::sync::oneshot::channel();
            let info = manager
                .spawn_with_notify("test", None, session, true, true, |path| async move {
                    ready.await.unwrap();
                    tokio::fs::write(path, "completion output").await?;
                    Ok(background::TaskResult::completed(Some(0)))
                })
                .await;
            let tool = BgTool::new();
            let mut waiting = Box::pin(tool.execute(
                json!({
                    "action": "wait",
                    "intent": "wait for fixture completion",
                    "task_id": info.task_id,
                    "max_wait_seconds": 60,
                    "include_output_preview": true,
                    "return_on_progress": false,
                }),
                ToolContext {
                    session_id: session.into(),
                    message_id: "message".into(),
                    tool_call_id: "wait".into(),
                    working_dir: None,
                    stdin_request_tx: None,
                    graceful_shutdown_signal: None,
                    execution_mode: jcode_tool_core::ToolExecutionMode::AgentTurn,
                },
            ));
            // Advance initialization one filesystem operation at a time.
            for _ in 0..16 {
                assert!(futures::poll!(&mut waiting).is_pending());
                tokio::task::spawn_blocking(|| ()).await.unwrap();
            }
            assert!(crate::wait_registry::current(session).is_some());
            finish.send(()).unwrap();
            assert!(!completion(&mut events, &info.task_id).await.wake);
            if lose_status {
                std::fs::remove_file(&info.status_file).unwrap();
                std::fs::create_dir(&info.status_file).unwrap();
            }

            // Finish the wait's status read and its next read, then pause the preview/commit.
            for _ in 0..2 {
                let (resume, blocker) = pause_file_reads().await;
                let pending = futures::poll!(&mut waiting).is_pending();
                resume.send(()).unwrap();
                blocker.await.unwrap();
                tokio::task::spawn_blocking(|| ()).await.unwrap();
                assert!(pending);
            }
            let (resume, blocker) = pause_file_reads().await;
            let pending = futures::poll!(&mut waiting).is_pending();
            let left_wait = crate::wait_registry::current(session).is_none();
            resume.send(()).unwrap();
            if cancel_preview {
                drop(waiting);
                blocker.await.unwrap();
                assert!(pending && left_wait, "wait must reach preview preparation");
                assert!(completion(&mut events, &info.task_id).await.wake);
            } else {
                blocker.await.unwrap();
                assert!(pending && left_wait, "wait must reach reply preparation");
                waiting
                    .await
                    .expect("held snapshot survives failed status read");
            }
            while let Ok(event) = events.try_recv() {
                if let BusEvent::BackgroundTaskCompleted(event) = event {
                    assert_ne!(event.task_id, info.task_id, "completion delivered twice");
                }
            }
        }
    });
}
