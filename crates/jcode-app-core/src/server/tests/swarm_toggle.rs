#![cfg_attr(test, allow(clippy::await_holding_lock))]
#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::*;
#[tokio::test]
async fn enabling_swarm_does_not_auto_elect_coordinator() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let (member_event_tx, _member_event_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    let session_id = "session_test_swarm_toggle";
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        session_id.to_string(),
        crate::server::SwarmMember {
            session_id: session_id.to_string(),
            event_tx: member_event_tx,
            event_txs: HashMap::new(),
            working_dir: Some(PathBuf::from("/tmp/jcode-passive-swarm")),
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            task_label: None,
            friendly_name: Some("duck".to_string()),
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
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::<String, String>::new()));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
    let mut swarm_enabled = false;

    handle_set_feature(
        42,
        FeatureToggle::Swarm,
        true,
        &agent,
        session_id,
        &Some("duck".to_string()),
        &mut swarm_enabled,
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &client_event_tx,
    )
    .await;

    assert!(swarm_enabled);
    assert!(swarm_coordinators.read().await.is_empty());
    assert_eq!(
        swarm_members
            .read()
            .await
            .get(session_id)
            .and_then(|member| member.swarm_id.clone()),
        // Root sessions own a session-scoped swarm rather than one derived
        // from the working directory (83dbc36dc).
        crate::server::util::swarm_id_for_session(session_id)
    );
    assert_eq!(
        swarm_members
            .read()
            .await
            .get(session_id)
            .map(|member| member.role.as_str()),
        Some("agent")
    );

    let events: Vec<_> = std::iter::from_fn(|| client_event_rx.try_recv().ok()).collect();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id: 42 }))
    );
    assert!(events.iter().all(|event| {
        !matches!(
            event,
            ServerEvent::Notification { message, .. }
                if message == "You are the coordinator for this swarm."
        )
    }));
}
