use super::*;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use tokio::sync::mpsc;

struct WakeProvider;

#[async_trait::async_trait]
impl Provider for WakeProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TextDelta("done".into())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

/// The producer's `notify` preference controls receipt cards, not whether an
/// actual model injection gets its own chronological transcript entry.
#[tokio::test]
async fn silent_stall_and_await_wakes_publish_idle_and_queued_context_entries() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let mut agent = Agent::new(Arc::new(WakeProvider), Registry::empty());
    agent.set_memory_enabled(false);
    let sid = agent.session_id().to_string();
    let queue = agent.soft_interrupt_queue();
    let agent = Arc::new(Mutex::new(agent));
    let sessions = Arc::new(super::super::session_manager::SessionManager::new());
    sessions
        .register_new(sid.clone(), agent.clone())
        .await
        .unwrap();
    let queues = Arc::new(RwLock::new(HashMap::from([(sid.clone(), queue)])));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let member = SwarmMember {
        session_id: sid.clone(),
        event_tx: tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: None,
        swarm_enabled: true,
        status: "ready".into(),
        detail: None,
        task_label: None,
        friendly_name: None,
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".into(),
        joined_at: std::time::Instant::now(),
        last_status_change: std::time::Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: vec![],
        runtime: Default::default(),
    };
    let members = Arc::new(RwLock::new(HashMap::from([(sid.clone(), member)])));
    let swarms = Arc::new(RwLock::new(HashMap::new()));
    let history = Arc::new(RwLock::new(VecDeque::new()));
    let counter = Arc::new(AtomicU64::new(0));
    let (swarm_tx, _) = broadcast::channel(16);

    for queued in [false, true] {
        for await_result in [false, true] {
            let busy = if queued {
                Some(agent.lock().await)
            } else {
                None
            };
            let expected = if await_result {
                let event = crate::bus::SwarmAwaitCompleted {
                    session_id: sid.clone(),
                    completed: true,
                    summary: "workers done".into(),
                    notification: "Await resolved: workers done".into(),
                    notify: false,
                    wake: true,
                };
                super::super::background_tasks::dispatch_swarm_await_completion(
                    &event, &sessions, &queues, &members, &swarms, &history, &counter, &swarm_tx,
                )
                .await;
                event.notification
            } else {
                let event = crate::bus::BackgroundTaskStalled {
                    task_id: "quiet-task".into(),
                    tool_name: "bash".into(),
                    display_name: None,
                    session_id: sid.clone(),
                    stall_wake_seconds: 30,
                    running_secs: 60.0,
                    output_tail: "still working".into(),
                    output_file: "quiet.output".into(),
                    notify: false,
                    wake: true,
                };
                let expected = crate::message::format_background_task_stalled_markdown(&event);
                super::super::background_tasks::dispatch_background_task_stalled(
                    &event, &sessions, &queues, &members, &swarms, &history, &counter, &swarm_tx,
                )
                .await;
                expected
            };
            drop(busy);
            if queued {
                assert_eq!(agent.lock().await.soft_interrupt_count(), 1);
                let (turn_tx, mut turn_rx) = mpsc::unbounded_channel();
                agent
                    .lock()
                    .await
                    .run_once_streaming_mpsc("continue", vec![], None, turn_tx)
                    .await
                    .unwrap();
                let mut delivered = 0;
                while let Ok(event) = turn_rx.try_recv() {
                    if let ServerEvent::SoftInterruptInjected { content, .. } = event {
                        assert_eq!(content, expected);
                        delivered += 1;
                    }
                }
                assert_eq!(delivered, 1, "queued delivery must create one entry");
            } else {
                let mut delivered = 0;
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        let event = rx.recv().await.unwrap();
                        let event = match event {
                            ServerEvent::TurnEvent { event, .. } => *event,
                            event => event,
                        };
                        match event {
                            ServerEvent::SoftInterruptInjected { content, .. }
                                if content == expected =>
                            {
                                delivered += 1
                            }
                            ServerEvent::Notification { .. } => {
                                panic!("notify=false must not add a receipt card")
                            }
                            ServerEvent::Error { message, .. } => panic!("wake failed: {message}"),
                            ServerEvent::Done { .. } => break,
                            _ => {}
                        }
                    }
                })
                .await
                .unwrap();
                assert_eq!(delivered, 1, "idle wake input must create one entry");
                // Join the reservation before the next dispatch.
                drop(agent.lock().await);
            }
        }
    }

    // The watchdog deliberately only queues its worker nudge. The owner's
    // separate warning card is not a substitute for the worker's delivery.
    let nudge = super::super::swarm_watchdog::nudge_message(600);
    assert!(
        super::super::queue_soft_interrupt_for_session(
            &sid,
            nudge.clone(),
            false,
            jcode_agent_runtime::SoftInterruptSource::System,
            &queues,
            &sessions,
        )
        .await
    );
    let (tx, mut events) = mpsc::unbounded_channel();
    agent
        .lock()
        .await
        .run_once_streaming_mpsc("continue", vec![], None, tx)
        .await
        .unwrap();
    let delivered = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| {
            matches!(event, ServerEvent::SoftInterruptInjected {
            content, display_role, ..
        } if content == &nudge && display_role.as_deref() == Some("system"))
        })
        .count();
    assert_eq!(delivered, 1);
    let stored = crate::session::Session::load(&sid).unwrap();
    assert!(stored.messages.iter().any(|message| {
        message.display_role == Some(crate::session::StoredDisplayRole::System)
            && message.content.iter().any(|block| {
                matches!(block,
                crate::message::ContentBlock::Text { text, .. } if text == &nudge)
            })
    }));
}
