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
    });
}
