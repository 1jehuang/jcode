#![cfg_attr(test, allow(clippy::await_holding_lock))]
#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::*;
#[test]
fn transfer_preserves_system_prompt_including_empty_override() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    for prompt in [None, Some(""), Some("custom system prompt")] {
        let mut parent = crate::session::Session::create(None, None);
        parent.system_prompt = prompt.map(str::to_string);
        let (child_id, _) = create_transfer_child_session(&parent.id, &parent, None)
            .expect("create transfer session");
        let child = crate::session::Session::load(&child_id).expect("load transfer session");
        assert_eq!(child.system_prompt, parent.system_prompt);
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn rename_session_event_uses_agent_session_id_even_when_client_id_is_stale() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let agent_session_id = agent.lock().await.session_id().to_string();
    let stale_client_session_id = "session_stale_client_id";
    let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        stale_client_session_id.to_string(),
        SwarmMember {
            session_id: stale_client_session_id.to_string(),
            event_tx: member_event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            task_label: None,
            friendly_name: Some("stale".to_string()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".to_string(),
            joined_at: now,
            last_status_change: now,
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        },
    )])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_rename_session(
        99,
        Some("Release planning".to_string()),
        &agent,
        stale_client_session_id,
        &swarm_members,
        &client_event_tx,
    )
    .await;

    let rename_event = timeout(Duration::from_secs(2), member_event_rx.recv())
        .await
        .expect("rename event should arrive")
        .expect("member event channel should stay open");
    match rename_event {
        ServerEvent::SessionRenamed {
            session_id,
            title,
            display_title,
        } => {
            assert_eq!(session_id, agent_session_id);
            assert_eq!(title.as_deref(), Some("Release planning"));
            assert_eq!(display_title, "Release planning");
        }
        other => panic!("expected SessionRenamed, got {other:?}"),
    }

    let client_events: Vec<_> = std::iter::from_fn(|| client_event_rx.try_recv().ok()).collect();
    assert!(
        client_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 99))
    );
    let loaded = crate::session::Session::load(&agent_session_id).expect("renamed session saved");
    assert_eq!(loaded.custom_title.as_deref(), Some("Release planning"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn daemon_saved_flag_survives_later_session_writes() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let session_id = agent.lock().await.session_id().to_string();

    let label = agent
        .lock()
        .await
        .set_session_saved(true, Some("investor catch up".to_string()))
        .expect("save session");
    assert_eq!(label.as_deref(), Some("investor catch up"));
    // A later daemon-owned write, such as the next turn, must keep the bookmark.
    agent
        .lock()
        .await
        .set_autoreview_enabled(true)
        .expect("later write");
    let loaded = crate::session::Session::load(&session_id).expect("load saved session");
    assert!(loaded.saved);
    assert_eq!(loaded.save_label.as_deref(), Some("investor catch up"));

    agent
        .lock()
        .await
        .set_session_saved(false, None)
        .expect("unsave session");
    let loaded = crate::session::Session::load(&session_id).expect("load unsaved session");
    assert!(!loaded.saved);
    assert!(loaded.save_label.is_none());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
