//! Public-handler regressions using only disposable sessions and persistence.
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::message::{ContentBlock, Message, Role, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::session::Session;
use crate::tool::Registry;
use async_trait::async_trait;
use std::ffi::OsString;
use std::time::{Duration, Instant};

struct TestHome {
    previous: Option<OsString>,
    _directory: tempfile::TempDir,
}

impl TestHome {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary summary home");
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", directory.path());
        crate::config::invalidate_config_cache();
        Self {
            previous,
            _directory: directory,
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        crate::config::invalidate_config_cache();
    }
}

struct NoCallsProvider;

#[async_trait]
impl Provider for NoCallsProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        anyhow::bail!("summary must not call a provider")
    }

    fn name(&self) -> &str {
        "test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

fn session() -> Session {
    let mut session = Session::create_with_id("summary-fixture".into(), None, None);
    session.system_prompt = Some("private-system-fixture".into());
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "private-user-fixture".into(),
            cache_control: None,
        }],
    );
    session
}

fn read_completed(session: &mut Session, file: &str) {
    let id = format!("read-{}", session.messages.len());
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: id.clone(),
            name: "read".into(),
            input: serde_json::json!({"file_path": file}),
            thought_signature: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: id,
            content: "fixture file contents".into(),
            is_error: Some(false),
        }],
    );
}

async fn hosted(session: Session) -> Arc<Mutex<Agent>> {
    let provider: Arc<dyn Provider> = Arc::new(NoCallsProvider);
    let registry = Registry::new(provider.clone()).await;
    Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )))
}

fn member(id: &str) -> SwarmMember {
    let (event_tx, _) = mpsc::unbounded_channel();
    SwarmMember {
        session_id: id.into(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some("summary-swarm".into()),
        swarm_enabled: true,
        status: "ready".into(),
        detail: None,
        friendly_name: None,
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".into(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
        task_label: None,
    }
}

fn members() -> Arc<RwLock<HashMap<String, SwarmMember>>> {
    Arc::new(RwLock::new(HashMap::from([
        ("requester".into(), member("requester")),
        ("summary-fixture".into(), member("summary-fixture")),
    ])))
}

fn sessions(agent: Option<Arc<Mutex<Agent>>>) -> SessionAgents {
    Arc::new(RwLock::new(
        agent
            .into_iter()
            .map(|agent| ("summary-fixture".into(), agent))
            .collect(),
    ))
}

async fn request(
    sessions: &SessionAgents,
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    limit: Option<usize>,
) -> ServerEvent {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::time::timeout(
        Duration::from_secs(5),
        handle_comm_summary(
            71,
            "requester".into(),
            "summary-fixture".into(),
            limit,
            sessions,
            members,
            &tx,
        ),
    )
    .await
    .expect("summary must not block on an agent");
    rx.try_recv().expect("summary response")
}

fn calls(event: ServerEvent) -> Vec<crate::protocol::ToolCallSummary> {
    let ServerEvent::CommSummaryResponse {
        id,
        session_id,
        tool_calls,
    } = event
    else {
        panic!("expected summary response (event withheld)");
    };
    assert_eq!(id, 71);
    assert_eq!(session_id, "summary-fixture");
    assert!(
        tool_calls
            .iter()
            .all(|call| !call.brief_output.contains("private-"))
    );
    tool_calls
}

#[tokio::test]
async fn saved_only_read_call_and_result_are_summarized() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut saved = session();
    read_completed(&mut saved, "saved.txt");
    saved.save().expect("save fixture");
    let result = calls(request(&sessions(None), &members(), None).await);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].tool_name, "read");
    assert!(result[0].brief_output.contains("saved.txt"));
}

#[tokio::test]
async fn newer_saved_journal_supersedes_idle_host_without_mutating_it() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut saved = session();
    saved.save().expect("initial snapshot");
    let mut idle = saved.clone();
    // Keep the hosted metadata unambiguously older without timing sleeps.
    idle.updated_at = saved.updated_at - chrono::Duration::days(1);
    let agent = hosted(idle).await;
    // The next save assigns its own current updated_at and appends a journal.
    read_completed(&mut saved, "journal.txt");
    saved.save().expect("append durable read");
    let journal = crate::session::session_journal_path(&saved.id).expect("journal path");
    assert!(std::fs::metadata(journal).expect("saved journal").len() > 0);
    let before = agent.lock().await.session_for_split().messages.len();
    let result = calls(request(&sessions(Some(agent.clone())), &members(), None).await);
    assert_eq!(result.len(), 1);
    assert!(result[0].brief_output.contains("journal.txt"));
    assert_eq!(
        agent.lock().await.session_for_split().messages.len(),
        before
    );
}

#[tokio::test]
async fn metadata_only_stale_save_keeps_extended_journal_summary_on_timestamp_tie() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    // Construct first: attaching an Agent records and saves an environment
    // snapshot, so constructing it after the tie would advance the timestamp.
    let agent = hosted(session()).await;
    let resident_id = agent.lock().await.session_for_split().id.clone();
    let resident_len = agent.lock().await.session_for_split().messages.len();
    let snapshot_path = crate::session::session_path(&resident_id).expect("snapshot path");
    let snapshot = std::fs::read(&snapshot_path).expect("initial snapshot bytes");

    // A standalone resume appends a real call/result to the shared journal.
    let mut standalone = Session::load(&resident_id).expect("standalone resume");
    read_completed(&mut standalone, "standalone-journal.txt");
    standalone.save().expect("standalone read journal save");
    // The resident still has no read. Its metadata-only save stamps a newer
    // timestamp without discarding the standalone append already on disk.
    let resident = {
        let mut hosted = agent.lock().await;
        // Already unbookmarked: this public setter performs an actual save
        // without changing snapshot-requiring metadata or adding messages.
        hosted
            .set_session_saved(false, None)
            .expect("stale resident metadata-only save");
        hosted.session_for_split().clone()
    };
    assert_eq!(resident.messages.len(), resident_len);
    assert!(std::fs::read(&snapshot_path).expect("unchanged snapshot") == snapshot);
    let journal_path = crate::session::session_journal_path(&resident.id).expect("journal path");
    let journal = std::fs::read_to_string(journal_path).expect("journal entries");
    let last: serde_json::Value = serde_json::from_str(journal.lines().last().expect("last entry"))
        .expect("metadata entry JSON");
    assert!(last.get("append_messages").is_none());
    let durable = Session::load(&resident.id).expect("replay read followed by stale metadata");
    assert_eq!(durable.updated_at, resident.updated_at);
    assert!(durable.messages.len() > resident.messages.len());
    assert!(
        resident
            .messages
            .iter()
            .zip(&durable.messages)
            .all(|(live, saved)| live.id == saved.id)
    );

    assert_eq!(
        agent.lock().await.session_for_split().updated_at,
        durable.updated_at
    );
    let result = calls(request(&sessions(Some(agent.clone())), &members(), None).await);
    assert_eq!(result.len(), 1);
    assert!(result[0].brief_output.contains("standalone-journal.txt"));
    assert_eq!(
        agent.lock().await.session_for_split().messages.len(),
        resident_len
    );
}

#[tokio::test]
async fn equal_timestamp_divergent_longer_durable_transcript_keeps_live_summary() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let base = session();
    let mut live = base.clone();
    read_completed(&mut live, "live-branch.txt");
    let agent = hosted(live).await;
    let mut saved = base;
    read_completed(&mut saved, "durable-branch.txt");
    read_completed(&mut saved, "durable-extra.txt");
    saved.save().expect("save longer divergent transcript");
    let live = {
        let mut hosted = agent.lock().await;
        hosted
            .set_session_saved(false, None)
            .expect("tie resident metadata save");
        hosted.session_for_split().clone()
    };
    let saved = Session::load(&saved.id).expect("replay divergent transcript and tied metadata");
    assert_eq!(saved.updated_at, live.updated_at);
    assert!(saved.messages.len() > live.messages.len());
    assert!(live.messages[0].id == saved.messages[0].id);
    assert!(live.messages[1].id != saved.messages[1].id);
    let result = calls(request(&sessions(Some(agent)), &members(), None).await);
    assert_eq!(result.len(), 1);
    assert!(result[0].brief_output.contains("live-branch.txt"));
}

#[tokio::test]
async fn equal_timestamp_equal_or_live_ahead_transcript_keeps_live_summary() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    for live_ahead in [false, true] {
        let mut saved = session();
        read_completed(&mut saved, "saved.txt");
        let mut live = saved.clone();
        if live_ahead {
            read_completed(&mut live, "live-ahead.txt");
        } else {
            // Even an equal-length transcript with the same IDs must retain
            // current live content rather than replacing it on a timestamp tie.
            let ContentBlock::ToolUse { input, .. } = &mut live.messages[1].content[0] else {
                panic!("expected fixture tool use");
            };
            *input = serde_json::json!({"file_path": "live-equal.txt"});
        }
        let agent = hosted(live).await;
        saved
            .save()
            .expect("save shorter or equal durable snapshot");
        let live = {
            let mut hosted = agent.lock().await;
            hosted
                .set_session_saved(false, None)
                .expect("tie resident metadata save");
            hosted.session_for_split().clone()
        };
        let durable = Session::load(&saved.id).expect("replay tied metadata over durable snapshot");
        assert_eq!(live.updated_at, durable.updated_at);
        assert_eq!(
            live.messages.len(),
            durable.messages.len() + if live_ahead { 2 } else { 0 }
        );
        let result = calls(request(&sessions(Some(agent)), &members(), Some(1)).await);
        assert_eq!(result.len(), 1);
        let expected = if live_ahead {
            "live-ahead.txt"
        } else {
            "live-equal.txt"
        };
        assert!(result[0].brief_output.contains(expected));
    }
}

#[tokio::test]
async fn current_host_is_not_replaced_by_older_saved_transcript() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut saved = session();
    read_completed(&mut saved, "old.txt");
    saved.save().expect("save old state");
    let mut live = saved.clone();
    read_completed(&mut live, "live.txt");
    live.updated_at = saved.updated_at + chrono::Duration::days(1);
    let result = calls(request(&sessions(Some(hosted(live).await)), &members(), Some(1)).await);
    assert_eq!(result.len(), 1);
    assert!(result[0].brief_output.contains("live.txt"));
}

#[tokio::test]
async fn busy_host_errors_instead_of_serving_saved_summary() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut saved = session();
    read_completed(&mut saved, "saved.txt");
    saved.save().expect("save fixture");
    let agent = hosted(saved).await;
    let _busy = agent.lock().await;
    let event = request(&sessions(Some(agent.clone())), &members(), None).await;
    assert!(
        matches!(event, ServerEvent::Error { id: 71, retry_after_secs: Some(1), message, .. } if message.contains("busy"))
    );
}

#[tokio::test]
async fn saved_summary_requires_same_swarm_access() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut saved = session();
    read_completed(&mut saved, "saved.txt");
    saved.save().expect("save fixture");
    let members = members();
    members.write().await.get_mut("requester").unwrap().swarm_id = Some("other-swarm".into());
    let event = request(&sessions(None), &members, None).await;
    assert!(
        matches!(event, ServerEvent::Error { message, retry_after_secs: None, .. } if message.contains("not in the same swarm"))
    );
}

#[tokio::test]
async fn durable_summary_respects_limit_including_zero() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut saved = session();
    for file in ["first.txt", "second.txt", "third.txt"] {
        read_completed(&mut saved, file);
    }
    saved.save().expect("save fixture");
    let sessions = sessions(None);
    let members = members();
    let result = calls(request(&sessions, &members, Some(1)).await);
    assert_eq!(result.len(), 1);
    assert!(result[0].brief_output.contains("third.txt"));
    assert!(calls(request(&sessions, &members, Some(0)).await).is_empty());
}

#[tokio::test]
async fn missing_or_corrupt_saved_only_summary_errors_without_context() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let sessions = sessions(None);
    let members = members();
    let path = crate::session::session_path("summary-fixture").expect("fixture path");
    for corrupt in [false, true] {
        if corrupt {
            std::fs::create_dir_all(path.parent().unwrap()).expect("session directory");
            std::fs::write(&path, "private-corrupt-fixture").expect("corrupt disposable snapshot");
        }
        let event = request(&sessions, &members, None).await;
        assert!(
            matches!(event, ServerEvent::Error { id: 71, message, retry_after_secs: None, .. }
            if message == "Session 'summary-fixture' summary is unavailable")
        );
    }
}

#[tokio::test]
async fn idle_live_summary_survives_missing_or_corrupt_persistence() {
    let _lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let mut live = session();
    read_completed(&mut live, "live-only.txt");
    let sessions = sessions(Some(hosted(live).await));
    let members = members();
    let path = crate::session::session_path("summary-fixture").expect("fixture path");
    for corrupt in [false, true] {
        if corrupt {
            std::fs::create_dir_all(path.parent().unwrap()).expect("session directory");
            std::fs::write(&path, "private-corrupt-fixture").expect("corrupt disposable snapshot");
        }
        let result = calls(request(&sessions, &members, None).await);
        assert_eq!(result.len(), 1);
        assert!(result[0].brief_output.contains("live-only.txt"));
    }
}
