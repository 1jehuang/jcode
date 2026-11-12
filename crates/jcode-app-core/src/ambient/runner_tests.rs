use super::{AmbientRunnerHandle, ambient_allowed};
use crate::ambient::{AmbientStatus, Priority, ScheduleTarget, ScheduledItem};
use crate::config::Config;
use crate::message::{Message, Role, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::session::Session;
use anyhow::Result;
use async_stream::stream;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

#[path = "runner_live_delivery_tests.rs"]
mod live_delivery;

struct EnvVarGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        crate::env::remove_var(key);
        Self { key, prev }
    }

    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

struct TestProvider;

struct ResetConfigCache;

impl Drop for ResetConfigCache {
    fn drop(&mut self) {
        Config::invalidate_cache();
    }
}

#[test]
fn ambient_gate_tracks_config_toggles_and_preserves_disabled_override() {
    let _guard = crate::storage::lock_test_env();
    // Restore the process cache after JCODE_HOME is restored, including on panic.
    let _cache = ResetConfigCache;
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _enabled = EnvVarGuard::unset("JCODE_AMBIENT_ENABLED");
    let path = Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config parent");

    for enabled in [false, true, false] {
        std::fs::write(&path, format!("[ambient]\nenabled = {enabled}\n"))
            .expect("write ambient config");
        // Exercise the iteration gate against reloaded on-disk config without
        // relying on wall-clock sleeps. Config's fingerprint throttle is tested
        // separately in jcode-base.
        Config::invalidate_cache();

        assert_eq!(ambient_allowed(&AmbientStatus::Idle), enabled);
        assert_eq!(
            ambient_allowed(&AmbientStatus::Scheduled {
                next_wake: chrono::Utc::now(),
            }),
            enabled
        );
        assert!(
            !ambient_allowed(&AmbientStatus::Disabled),
            "an explicit stop must win even when config enables ambient"
        );
    }
}

#[tokio::test]
async fn running_loop_observes_enable_edit_without_cache_invalidation() {
    let _guard = crate::storage::lock_test_env();
    let _cache = ResetConfigCache;
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _enabled = EnvVarGuard::unset("JCODE_AMBIENT_ENABLED");
    let path = Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config parent");
    std::fs::write(
        &path,
        "[ambient]\nenabled = false\npause_on_active_session = true\n",
    )
    .expect("write disabled config");
    Config::invalidate_cache();

    let runner = AmbientRunnerHandle::new(Arc::new(crate::safety::SafetySystem::new()));
    // Pausing is an observable loop action that cannot invoke a model or tool.
    *runner.inner.active_user_sessions.write().await = 1;
    let task = tokio::spawn(runner.clone().run_loop(Arc::new(TestProvider)));
    // On the current-thread test runtime, let run_loop reach its first sleep
    // with the disabled startup configuration before editing the file.
    tokio::task::yield_now().await;
    let started_disabled =
        runner.is_running().await && matches!(runner.state().await.status, AmbientStatus::Idle);

    let edit = std::fs::write(
        &path,
        "[ambient]\nenabled = true\npause_on_active_session = true\n# edited\n",
    );
    // Deliberately do not invalidate Config's cache: exercise fingerprint
    // detection and the real loop's next-wake behavior, not just its helper.
    let observed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            runner.nudge();
            tokio::time::sleep(Duration::from_millis(20)).await;
            if matches!(runner.state().await.status, AmbientStatus::Paused { .. }) {
                break;
            }
        }
    })
    .await;
    task.abort();
    let _ = task.await;

    assert!(
        started_disabled,
        "loop must start idle with ambient disabled"
    );
    edit.expect("edit enabled config");
    assert!(
        observed.is_ok(),
        "a running loop must observe the enable edit and pause for the active session"
    );
}

#[derive(Clone, Default)]
struct StreamingTestProvider {
    responses: Arc<StdMutex<VecDeque<Vec<StreamEvent>>>>,
}

impl StreamingTestProvider {
    fn queue_response(&self, events: Vec<StreamEvent>) {
        self.responses.lock().unwrap().push_back(events);
    }
}

#[async_trait]
impl Provider for TestProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Err(anyhow::anyhow!(
            "TestProvider should not be used for streaming completions in ambient runner tests"
        ))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(TestProvider)
    }
}

#[async_trait]
impl Provider for StreamingTestProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let events = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        let stream = stream! {
            for event in events {
                yield Ok(event);
            }
        };
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test]
async fn runner_stays_alive_to_service_schedules_when_ambient_disabled() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let runner = AmbientRunnerHandle::new(Arc::new(crate::safety::SafetySystem::new()));
    let task = tokio::spawn(runner.clone().run_loop(provider));

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        runner.is_running().await,
        "runner should remain active for scheduled tasks even with ambient disabled"
    );

    task.abort();
    let _ = task.await;
}

async fn assert_visible_launch_error_falls_back(error_kind: std::io::ErrorKind) {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let provider: Arc<dyn Provider> = Arc::new(StreamingTestProvider::default());
    let runner = AmbientRunnerHandle::new(Arc::new(crate::safety::SafetySystem::new()));
    let launch_attempted = Arc::new(AtomicBool::new(false));
    let launch_attempted_in_callback = launch_attempted.clone();

    let result = runner
        .run_cycle_with_visible_launcher(&provider, true, move || {
            launch_attempted_in_callback.store(true, Ordering::SeqCst);
            Err(std::io::Error::from(error_kind))
        })
        .await
        .expect("failed visible launch should continue as a headless cycle");

    assert!(launch_attempted.load(Ordering::SeqCst));
    assert!(
        result.conversation.is_some(),
        "headless fallback should capture an agent conversation"
    );
    assert!(
        result.summary.contains("forced end after 2 attempts"),
        "headless fallback should return the headless agent result"
    );
}

#[tokio::test]
async fn unsupported_visible_launch_falls_back_to_headless() {
    assert_visible_launch_error_falls_back(std::io::ErrorKind::Unsupported).await;
}

#[tokio::test]
async fn missing_visible_launcher_falls_back_to_headless() {
    assert_visible_launch_error_falls_back(std::io::ErrorKind::NotFound).await;
}

#[tokio::test]
async fn spawn_target_creates_one_child_session_and_runs_task() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let provider = StreamingTestProvider::default();
    provider.queue_response(vec![
        StreamEvent::TextDelta("Spawned session handled task.".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    let provider: Arc<dyn Provider> = Arc::new(provider);

    let mut parent = Session::create_with_id(
        "session_parent_spawn_test".to_string(),
        None,
        Some("Parent".to_string()),
    );
    parent.working_dir = Some(temp.path().display().to_string());
    parent.save().expect("save parent session");

    let item = ScheduledItem {
        id: "sched_spawn_test".to_string(),
        scheduled_for: chrono::Utc::now(),
        context: "Follow up later".to_string(),
        priority: Priority::Normal,
        target: ScheduleTarget::Spawn {
            parent_session_id: parent.id.clone(),
        },
        created_by_session: parent.id.clone(),
        created_at: chrono::Utc::now(),
        working_dir: parent.working_dir.clone(),
        task_description: Some("Follow up later".to_string()),
        relevant_files: vec!["src/lib.rs".to_string()],
        git_branch: None,
        additional_context: Some("Background: spawned schedule test".to_string()),
    };

    let runner = AmbientRunnerHandle::new(Arc::new(crate::safety::SafetySystem::new()));
    let child_session_id = runner
        .spawn_session_for_scheduled_item(&provider, &item, &parent.id)
        .await
        .expect("spawned scheduled task should succeed");

    assert_ne!(child_session_id, parent.id);

    let child = Session::load(&child_session_id).expect("load spawned child session");
    assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(child.working_dir, parent.working_dir);
    assert!(child.messages.iter().any(|message| {
        message.role == Role::User
            && message.content_preview().contains("[Scheduled task]")
            && message.content_preview().contains("Follow up later")
    }));
    assert!(child.messages.iter().any(|message| {
        message.role == Role::Assistant
            && message
                .content_preview()
                .contains("Spawned session handled task.")
    }));
}
