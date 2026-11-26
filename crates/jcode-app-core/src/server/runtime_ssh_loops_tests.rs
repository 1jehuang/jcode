#![allow(clippy::await_holding_lock)]
use super::*;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn open_snapshot() -> todo::followup::TodoSnapshot {
    todo::followup::TodoSnapshot {
        todos: vec![todo::TodoItem {
            id: "remote-task".into(),
            content: "finish remote work".into(),
            status: "pending".into(),
            priority: "high".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn enabled_state() -> LoopState {
    LoopState {
        poke_enabled: true,
        policy: todo::followup::FollowUpPolicy::new(true),
        ..Default::default()
    }
}

#[test]
fn persisted_policy_keeps_unchanged_todo_retry_budget_across_reload() {
    let mut state = enabled_state();
    let snapshot = open_snapshot();
    for _ in 0..todo::followup::INCOMPLETE_TODO_MAX_ATTEMPTS {
        assert!(next_followup(&mut state, &snapshot).follow_up.is_some());
        state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    }
    for _ in 0..5 {
        assert!(next_followup(&mut state, &snapshot).follow_up.is_none());
    }
    let mut progress = snapshot;
    progress.todos[0].content.push_str(" with new progress");
    assert!(next_followup(&mut state, &progress).follow_up.is_some());
}

#[test]
fn paused_plan_and_unregistered_sessions_never_continue() {
    let snapshot = open_snapshot();
    assert!(
        next_followup(&mut LoopState::default(), &snapshot)
            .follow_up
            .is_none()
    );
    for (active, plan_only) in [(false, false), (true, true)] {
        let mut state = enabled_state();
        state.workflow = Some(Workflow {
            kind: "improve".into(),
            active,
            plan_only,
            focus: None,
        });
        assert!(next_followup(&mut state, &snapshot).follow_up.is_none());
    }
}

#[test]
fn delegated_work_waits_without_nagging_the_root_or_hiding_independent_work() {
    let mut snapshot = open_snapshot();
    let mut state = enabled_state();
    snapshot.todos[0].assigned_to = Some("worker".into());
    snapshot.running_workers.push("worker".into());
    for _ in 0..5 {
        assert!(next_followup(&mut state, &snapshot).follow_up.is_none());
    }
    let mut independent = open_snapshot().todos.remove(0);
    independent.content = "root work".into();
    snapshot.todos.push(independent);
    let message = next_followup(&mut state, &snapshot)
        .follow_up
        .unwrap()
        .message;
    assert!(message.contains("root work"));
    assert!(!message.contains("finish remote work"));
    snapshot.running_workers.clear();
    assert!(
        next_followup(&mut state, &snapshot)
            .follow_up
            .unwrap()
            .message
            .contains("finish remote work")
    );
}

#[test]
fn blockers_and_running_tasks_pause_without_consuming_retry_budget() {
    let mut snapshot = open_snapshot();
    let mut state = enabled_state();
    snapshot.todos[0].waiting_on = vec!["task-remote".into()];
    snapshot.running_background_tasks = vec!["task-remote".into()];
    for _ in 0..10 {
        assert!(next_followup(&mut state, &snapshot).follow_up.is_none());
    }
    snapshot.running_background_tasks.clear();
    assert!(next_followup(&mut state, &snapshot).follow_up.is_some());
    snapshot.todos[0].requires_user_auth = Some(todo::TodoUserAuth {
        reason: "Owner must approve access".into(),
    });
    assert!(next_followup(&mut state, &snapshot).follow_up.is_none());
    snapshot.todos[0].requires_user_auth = None;
    assert!(next_followup(&mut state, &snapshot).follow_up.is_some());
    snapshot.goals = vec![todo::TodoGoal {
        cannot_complete: Some("Target host was decommissioned".into()),
        ..Default::default()
    }];
    assert!(next_followup(&mut state, &snapshot).follow_up.is_none());
    snapshot.goals.clear();
    assert!(next_followup(&mut state, &snapshot).follow_up.is_some());
}

#[test]
fn state_files_are_session_scoped_and_invalid_ids_cannot_escape() {
    let root = tempfile::tempdir().unwrap();
    let first = state_path_at(root.path(), "session_remote-a").unwrap();
    let second = state_path_at(root.path(), "session_remote-b").unwrap();
    crate::storage::write_json(&first, &enabled_state()).unwrap();
    assert!(read_state(&first).unwrap().poke_enabled);
    assert!(!read_state(&second).unwrap().poke_enabled);
    for bad in [
        "",
        "..",
        "../session",
        "/absolute",
        "session/name",
        "session\\name",
    ] {
        assert!(state_path_at(root.path(), bad).is_err(), "{bad}");
    }
    let corrupt = root.path().join("corrupt.json");
    std::fs::write(&corrupt, "not json").unwrap();
    assert!(read_state(&corrupt).is_err());
    let unsupported = root.path().join("future.json");
    let mut future = enabled_state();
    future.version = 99;
    crate::storage::write_json(&unsupported, &future).unwrap();
    assert!(read_state(&unsupported).is_err());
}

#[test]
fn launch_prompts_keep_plan_and_refactor_review_semantics() {
    for kind in ["improve", "refactor"] {
        let mut workflow = Workflow {
            kind: kind.into(),
            plan_only: true,
            focus: Some("parser".into()),
            active: true,
        };
        let prompt = launch_prompt(&workflow, false);
        assert!(prompt.contains("Do not edit files"));
        assert!(prompt.contains("parser"));
        workflow.plan_only = false;
        let prompt = launch_prompt(&workflow, true);
        assert!(prompt.contains("Resume the persisted"));
        assert!(prompt.contains("validate meaningful changes"));
        if kind == "refactor" {
            assert!(prompt.contains("independent read-only review"));
        }
    }
    assert!(!handles("improvement"));
    assert!(!handles("refactoring"));
    for command in ["poke", "improve", "refactor", "fix"] {
        assert!(handles(command));
    }
}

#[derive(Clone, Default)]
struct RecordingProvider {
    calls: Arc<AtomicUsize>,
    progress_session: Arc<std::sync::Mutex<Option<String>>>,
    progress_turns: usize,
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= self.progress_turns {
            if let Some(session_id) = self.progress_session.lock().unwrap().as_ref() {
                let mut snapshot = open_snapshot();
                snapshot.todos[0].content =
                    format!("Complete remaining campaign work after step {call}");
                todo::save_todos(session_id, &snapshot.todos)?;
            }
        }
        Ok(Box::pin(futures::stream::iter([
            Ok(StreamEvent::TextDelta(
                "A real remote model turn completed.".into(),
            )),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }
    fn name(&self) -> &str {
        "remote-loop-test"
    }
    fn supports_compaction(&self) -> bool {
        false
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct RestoreHome(Option<std::ffi::OsString>);
impl Drop for RestoreHome {
    fn drop(&mut self) {
        if let Some(home) = &self.0 {
            crate::env::set_var("JCODE_HOME", home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        crate::config::Config::invalidate_cache();
    }
}

#[tokio::test]
async fn productive_remote_campaign_exceeds_old_turn_cap_then_stops_on_no_progress() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let calls = Arc::new(AtomicUsize::new(0));
    let progress_session = Arc::new(std::sync::Mutex::new(None));
    // Allocate a session first so the provider can publish durable progress
    // exactly as a model's todo tool would during each real continuation.
    let mut agent = Agent::new_with_initial_working_dir(
        Arc::new(RecordingProvider {
            calls: Arc::clone(&calls),
            progress_session: Arc::clone(&progress_session),
            progress_turns: 40,
        }),
        crate::tool::Registry::empty(),
        home.path().to_str(),
    );
    let session_id = agent.session_id().to_string();
    *progress_session.lock().unwrap() = Some(session_id.clone());
    todo::save_todos(&session_id, &open_snapshot().todos).unwrap();
    initialize_remote_session(&session_id, true).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    drop(rx);
    run_followups(&mut agent, tx.clone()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 43);
    let state = read_state(&state_path(&session_id).unwrap()).unwrap();
    assert!(state.poke_enabled, "productive work must not disable poke");
    assert_eq!(state.turns, 43);
    initialize_remote_session(&session_id, true).unwrap();
    run_followups(&mut agent, tx).await.unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        43,
        "no-progress budget survives reattach"
    );
}

#[tokio::test]
async fn delegated_work_durable_await_survives_process_task_refresh() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let provider = RecordingProvider::default();
    let mut agent = Agent::new_with_initial_working_dir(
        Arc::new(provider.clone()),
        crate::tool::Registry::empty(),
        home.path().to_str(),
    );
    let session = agent.session_id().to_owned();
    let key = format!("loop-{session}");
    let wait = todo::await_dependency::dependency_id(&key, 1);
    let path = crate::storage::runtime_dir()
        .join(todo::await_dependency::STATE_DIR)
        .join(format!("{key}.json"));
    crate::storage::write_json_fast(&path, &json!({"key":key,"session_id":session,"created_at_unix_ms":1,"deadline_unix_ms":u64::MAX,"background":true})).unwrap();
    let mut todos = open_snapshot().todos;
    todos[0].waiting_on = vec![wait];
    todo::save_todos(&session, &todos).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    run_followups(&mut agent, tx.clone()).await.unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    run_followups(&mut agent, tx).await.unwrap();
    assert!(provider.calls.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn ordinary_session_enables_real_followups_without_client_opt_in() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let provider = RecordingProvider::default();
    let mut agent = Agent::new_with_initial_working_dir(
        Arc::new(provider.clone()),
        crate::tool::Registry::empty(),
        home.path().to_str(),
    );
    let session_id = agent.session_id().to_string();
    todo::save_todos(&session_id, &open_snapshot().todos).unwrap();
    assert!(!state_path(&session_id).unwrap().exists());
    let (tx, rx) = mpsc::unbounded_channel();
    // The daemon must continue with no observer attached and without a
    // client-specific subscription or an explicit /poke command.
    drop(rx);
    run_followups(&mut agent, tx.clone()).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    let persisted = std::fs::read(state_path(&session_id).unwrap()).unwrap();
    initialize_remote_session(&session_id, true).unwrap();
    assert_eq!(
        std::fs::read(state_path(&session_id).unwrap()).unwrap(),
        persisted
    );
    run_followups(&mut agent, tx.clone()).await.unwrap();
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        3,
        "reattaching must not reset retry budgets"
    );

    control(&session_id, "poke", &["off".into()])
        .unwrap()
        .unwrap();
    initialize_remote_session(&session_id, true).unwrap();
    assert!(
        !read_state(&state_path(&session_id).unwrap())
            .unwrap()
            .poke_enabled
    );
    run_followups(&mut agent, tx.clone()).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);

    // Arming needs no Agent lock and does not launch an overlapping turn.
    let (data, _) = control(&session_id, "poke", &["on".into()])
        .unwrap()
        .unwrap();
    assert_eq!(data["poke_enabled"], true);
    assert!(data.get("next_prompt").is_none());
    run_followups(&mut agent, tx).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 6);

    initialize_remote_session("remote-disabled", false).unwrap();
    initialize_remote_session("remote-disabled", true).unwrap();
    assert!(
        !read_state(&state_path("remote-disabled").unwrap())
            .unwrap()
            .poke_enabled
    );
}

#[tokio::test]
async fn ordinary_session_respects_host_default_off_and_explicit_on() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    std::fs::write(
        home.path().join("config.toml"),
        "[features]\nauto_poke = false\n",
    )
    .unwrap();
    crate::config::Config::invalidate_cache();
    let provider = RecordingProvider::default();
    let mut agent = Agent::new_with_initial_working_dir(
        Arc::new(provider.clone()),
        crate::tool::Registry::empty(),
        home.path().to_str(),
    );
    let session_id = agent.session_id().to_string();
    todo::save_todos(&session_id, &open_snapshot().todos).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    run_followups(&mut agent, tx.clone()).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(
        !read_state(&state_path(&session_id).unwrap())
            .unwrap()
            .poke_enabled
    );
    assert!(rx.try_recv().is_err(), "disabled continuation is silent");

    control(&session_id, "poke", &["on".into()])
        .unwrap()
        .unwrap();
    run_followups(&mut agent, tx).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    let mut notices = 0;
    while let Ok(event) = rx.try_recv() {
        match event {
            ServerEvent::Notification { from_name, .. }
                if from_name.as_deref() == Some("daemon continuation") =>
            {
                notices += 1
            }
            ServerEvent::Done { .. } => panic!("internal continuation must not publish Done"),
            _ => {}
        }
    }
    assert_eq!(notices, 3, "observers see every daemon-owned continuation");
}

#[tokio::test]
async fn cancelled_session_does_not_restart_for_unfinished_todos() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let provider = RecordingProvider::default();
    let mut agent = Agent::new_with_initial_working_dir(
        Arc::new(provider.clone()),
        crate::tool::Registry::empty(),
        home.path().to_str(),
    );
    let session_id = agent.session_id().to_string();
    todo::save_todos(&session_id, &open_snapshot().todos).unwrap();
    agent.request_graceful_shutdown();
    let (tx, mut rx) = mpsc::unbounded_channel();
    run_followups(&mut agent, tx).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        read_state(&state_path(&session_id).unwrap()).unwrap().turns,
        0
    );
    assert!(
        rx.try_recv().is_err(),
        "cancel must not announce another poke"
    );
}

#[tokio::test]
async fn remote_loop_runtime_executes_turns_persists_controls_and_recovers_actual_context() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let provider = RecordingProvider::default();
    let mut agent = Agent::new_with_initial_working_dir(
        Arc::new(provider.clone()),
        crate::tool::Registry::empty(),
        home.path().to_str(),
    );
    let session_id = agent.session_id().to_string();
    let (tx, _rx) = mpsc::unbounded_channel();
    // Ordinary sessions now delegate to the daemon without a special subscribe.
    // Disable explicitly here so the workflow assertions below start fresh.
    control(&session_id, "poke", &["off".into()])
        .unwrap()
        .unwrap();
    todo::save_todos(&session_id, &open_snapshot().todos).unwrap();
    run_followups(&mut agent, tx.clone()).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(state_path(&session_id).unwrap().exists());

    let (start, _) = execute(&mut agent, "improve", &["parser reliability".into()])
        .await
        .unwrap();
    assert!(
        start["next_prompt"]
            .as_str()
            .unwrap()
            .contains("parser reliability")
    );
    super::super::turn_failover::process_locked_message_with_server_failover(
        &mut agent,
        start["next_prompt"].as_str().unwrap(),
        vec![],
        None,
        tx.clone(),
    )
    .await
    .unwrap();
    // One launch plus three bounded real follow-up turns.
    assert_eq!(provider.calls.load(Ordering::SeqCst), 4);
    run_followups(&mut agent, tx.clone()).await.unwrap();
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        4,
        "persisted budget cannot reset at each turn"
    );

    // Busy-safe controls take only a session ID and work while Agent is locked.
    let agent_lock = tokio::sync::Mutex::new(agent);
    let mut guard = agent_lock.lock().await;
    let (status, _) = control(&session_id, "improve", &["status".into()])
        .unwrap()
        .unwrap();
    assert_eq!(status["workflow"]["focus"], "parser reliability");
    assert_eq!(status["todos"][0]["id"], "remote-task");
    control(&session_id, "improve", &["stop".into()])
        .unwrap()
        .unwrap();
    assert!(
        !read_state(&state_path(&session_id).unwrap())
            .unwrap()
            .workflow
            .unwrap()
            .active
    );
    run_followups(&mut guard, tx.clone()).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 4);
    let (resumed, _) = execute(&mut guard, "improve", &["resume".into()])
        .await
        .unwrap();
    assert!(
        resumed["next_prompt"]
            .as_str()
            .unwrap()
            .contains("parser reliability")
    );
    let (plan, _) = execute(
        &mut guard,
        "refactor",
        &["plan".into(), "boundaries".into()],
    )
    .await
    .unwrap();
    assert!(
        plan["next_prompt"]
            .as_str()
            .unwrap()
            .contains("Do not edit files")
    );
    run_followups(&mut guard, tx.clone()).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 4);
    // Stopping an unrelated workflow must not clobber the active refactor.
    control(&session_id, "improve", &["stop".into()])
        .unwrap()
        .unwrap();
    assert!(
        read_state(&state_path(&session_id).unwrap())
            .unwrap()
            .workflow
            .unwrap()
            .active
    );
    assert!(
        execute(
            &mut guard,
            "refactor",
            &["resume".into(), "unexpected".into()]
        )
        .await
        .is_err()
    );
    assert!(execute(&mut guard, "poke", &["typo".into()]).await.is_err());
    assert!(execute(&mut guard, "fix", &["wipe".into()]).await.is_err());

    // /fix performs real tool-pair repair, preserves session and history, and
    // does not ask the model to pretend to recover runtime state.
    guard.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "interrupted-call".into(),
            name: "bash".into(),
            input: json!({"command": "true"}),
            thought_signature: None,
        }],
    );
    let before = guard.message_count();
    let calls_before_fix = provider.calls.load(Ordering::SeqCst);
    let (fixed, _) = execute(&mut guard, "fix", &[]).await.unwrap();
    assert_eq!(fixed["session_id"], session_id);
    assert_eq!(fixed["repaired_tool_outputs"], 1);
    assert_eq!(fixed["history_preserved"], true);
    assert!(guard.message_count() >= before);
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls_before_fix);
    let (fixed_again, _) = execute(&mut guard, "fix", &[]).await.unwrap();
    assert_eq!(fixed_again["repaired_tool_outputs"], 0);

    let other = "session_other_remote";
    let (other_status, _) = control(other, "poke", &["status".into()]).unwrap().unwrap();
    assert_eq!(other_status["poke_enabled"], false);
    assert!(!state_path(other).unwrap().exists(), "status is read-only");
}
