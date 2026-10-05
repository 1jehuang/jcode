#![cfg_attr(test, allow(clippy::await_holding_lock))]
#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::*;
#[tokio::test]
async fn resume_all_continues_interrupted_idle_live_session() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let provider = Arc::new(StreamingMockProvider::default());
    provider.queue_response(vec![
        StreamEvent::TextDelta("Continuing where I left off.".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let registry = Registry::new(provider_dyn.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider_dyn, registry)));
    let session_id = {
        let mut guard = agent.lock().await;
        // Leave the session with a pending user turn the assistant never answered
        // (simulating a turn that errored / was interrupted mid-generation).
        guard.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "please keep going on the refactor".to_string(),
                cache_control: None,
            }],
        );
        guard.session_id().to_string()
    };

    let sessions = Arc::new(RwLock::new(HashMap::<String, Arc<Mutex<Agent>>>::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let (member, mut attach_rx) = live_member(&session_id, None);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    handle_resume_all_sessions(
        91,
        &sessions,
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_event_tx,
        // No working directory on either side, so the session is in scope.
        None,
    )
    .await;

    // The session should resume and stream the continuation.
    let streamed = timeout(Duration::from_secs(2), async {
        loop {
            match attach_rx.recv().await {
                Some(ServerEvent::TextDelta { text })
                    if text.contains("Continuing where I left off.") =>
                {
                    return text;
                }
                Some(_) => continue,
                None => panic!("live attachment closed before continuation streamed"),
            }
        }
    })
    .await
    .expect("interrupted session should resume promptly");
    assert!(streamed.contains("Continuing where I left off."));

    // The requesting client receives a summary describing one resumed session.
    let result = timeout(Duration::from_secs(2), async {
        loop {
            match client_event_rx.recv().await {
                Some(event @ ServerEvent::ResumeAllResult { .. }) => return event,
                Some(_) => continue,
                None => panic!("client channel closed before resume-all result"),
            }
        }
    })
    .await
    .expect("resume-all result should be emitted");
    match result {
        ServerEvent::ResumeAllResult {
            id,
            resumed,
            skipped,
            ..
        } => {
            assert_eq!(id, 91);
            assert_eq!(resumed, 1);
            assert_eq!(skipped, 0);
        }
        other => panic!("expected ResumeAllResult, got {other:?}"),
    }

    if let Some(home) = prev_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn resume_all_skips_session_with_completed_turn() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let session_id = {
        let mut guard = agent.lock().await;
        // A completed turn: last visible message is from the assistant.
        guard.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "do the thing".to_string(),
                cache_control: None,
            }],
        );
        guard.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "done".to_string(),
                cache_control: None,
            }],
        );
        guard.session_id().to_string()
    };

    let sessions = Arc::new(RwLock::new(HashMap::<String, Arc<Mutex<Agent>>>::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let (member, _attach_rx) = live_member(&session_id, None);
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    handle_resume_all_sessions(
        92,
        &sessions,
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_event_tx,
        // No working directory on either side, so the session is in scope.
        None,
    )
    .await;

    let result = timeout(Duration::from_secs(2), async {
        loop {
            match client_event_rx.recv().await {
                Some(event @ ServerEvent::ResumeAllResult { .. }) => return event,
                Some(_) => continue,
                None => panic!("client channel closed before resume-all result"),
            }
        }
    })
    .await
    .expect("resume-all result should be emitted");
    match result {
        ServerEvent::ResumeAllResult {
            id,
            resumed,
            skipped,
            ..
        } => {
            assert_eq!(id, 92);
            assert_eq!(resumed, 0);
            assert_eq!(skipped, 1);
        }
        other => panic!("expected ResumeAllResult, got {other:?}"),
    }

    if let Some(home) = prev_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

/// P0.2: a `/continue` typed in project A must not deliver a continuation into
/// a live session belonging to project B.
///
/// Before the fix the sweep walked every live session in the daemon regardless
/// of directory, so one keystroke in one project restarted work in every other
/// project the daemon happened to be serving. Both sessions here are
/// identically resumable, so the only thing that can distinguish them is the
/// project scope.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn resume_all_skips_sessions_from_other_projects() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project_a = temp.path().join("project-a");
    let project_b = temp.path().join("project-b");
    std::fs::create_dir_all(&project_a).expect("create project-a");
    std::fs::create_dir_all(&project_b).expect("create project-b");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    // Build an idle session that owes the model a continuation, rooted at
    // `working_dir`.
    let make_interrupted = |working_dir: String| async move {
        let provider = Arc::new(StreamingMockProvider::default());
        provider.queue_response(vec![
            StreamEvent::TextDelta("Continuing where I left off.".to_string()),
            StreamEvent::MessageEnd { stop_reason: None },
        ]);
        let provider_dyn: Arc<dyn Provider> = provider.clone();
        let registry = Registry::new(provider_dyn.clone()).await;
        let mut session = crate::session::Session::create(None, None);
        session.working_dir = Some(working_dir);
        let agent = Arc::new(Mutex::new(Agent::new_with_session(
            provider_dyn,
            registry,
            session,
            None,
        )));
        (agent, provider)
    };

    let (agent_a, _provider_a) = make_interrupted(project_a.to_string_lossy().to_string()).await;
    let (agent_b, _provider_b) = make_interrupted(project_b.to_string_lossy().to_string()).await;

    let (id_a, id_b) = {
        let mut guard_a = agent_a.lock().await;
        let mut guard_b = agent_b.lock().await;
        for guard in [&mut guard_a, &mut guard_b] {
            guard.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: "keep going".to_string(),
                    cache_control: None,
                }],
            );
        }
        (
            guard_a.session_id().to_string(),
            guard_b.session_id().to_string(),
        )
    };

    let dir_a = project_a.to_string_lossy().to_string();
    let dir_b = project_b.to_string_lossy().to_string();
    let sessions = Arc::new(RwLock::new(HashMap::<String, Arc<Mutex<Agent>>>::from([
        (id_a.clone(), agent_a.clone()),
        (id_b.clone(), agent_b.clone()),
    ])));
    let (member_a, mut attach_a) = live_member(&id_a, Some(&dir_a));
    let (member_b, mut attach_b) = live_member(&id_b, Some(&dir_b));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (id_a.clone(), member_a),
        (id_b.clone(), member_b),
    ])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    // The caller is attached to project A.
    handle_resume_all_sessions(
        93,
        &sessions,
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_event_tx,
        Some(&dir_a),
    )
    .await;

    let result = timeout(Duration::from_secs(2), async {
        loop {
            match client_event_rx.recv().await {
                Some(event @ ServerEvent::ResumeAllResult { .. }) => return event,
                Some(_) => continue,
                None => panic!("client channel closed before resume-all result"),
            }
        }
    })
    .await
    .expect("resume-all result should be emitted");
    match result {
        ServerEvent::ResumeAllResult {
            id,
            resumed,
            resumed_sessions,
            ..
        } => {
            assert_eq!(id, 93);
            assert_eq!(
                resumed, 1,
                "only project A's session should resume, got {resumed_sessions:?}"
            );
            assert_eq!(resumed_sessions.len(), 1);
        }
        other => panic!("expected ResumeAllResult, got {other:?}"),
    }

    // Project B must see no continuation at all. This is the assertion that
    // matters: the count above could still be 1 while B was the one resumed.
    // `try_recv` rather than `recv`: B's attachment stays open for the whole
    // daemon lifetime, so `recv` would wait forever instead of reporting
    // "nothing arrived".
    let leaked = attach_b.try_recv();
    assert!(
        leaked.is_err(),
        "project B received {leaked:?}; a /continue in project A must not touch it"
    );

    // Sanity check that A really was resumed, so the test cannot pass by
    // skipping both sessions.
    let resumed_a = timeout(Duration::from_secs(2), async {
        loop {
            match attach_a.recv().await {
                Some(ServerEvent::TextDelta { text })
                    if text.contains("Continuing where I left off.") =>
                {
                    return text;
                }
                Some(_) => continue,
                None => panic!("project A's attachment closed before continuation"),
            }
        }
    })
    .await
    .expect("project A's session should resume");
    assert!(resumed_a.contains("Continuing where I left off."));

    if let Some(home) = prev_home {
        crate::env::set_var("JCODE_HOME", home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
