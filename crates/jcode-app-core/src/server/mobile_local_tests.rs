//! Phone messages entering sessions held by this daemon.

use super::super::live_turn::LiveTurnSwarmContext;
use super::super::mobile_local::DaemonSessions;
use super::*;
use crate::mobile_runner::{LiveSession, LocalSessionHost};

struct Fixture {
    host: DaemonSessions,
    agent: Arc<Mutex<Agent>>,
    session_id: String,
    events: mpsc::UnboundedReceiver<ServerEvent>,
    provider: Arc<StreamingMockProvider>,
}

async fn fixture() -> Fixture {
    let provider = Arc::new(StreamingMockProvider::default());
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let agent = test_agent(provider_dyn).await;
    let session_id = agent.lock().await.session_id().to_string();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([(session_id.clone(), agent.clone())]),
    ));
    let (tx, events) = mpsc::unbounded_channel();
    let mut member = attached_swarm_member(&session_id, tx.clone());
    member.event_txs.insert("window-1".to_string(), tx);
    let members = Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)])));
    let (swarms_by_id, event_history, event_counter, swarm_event_tx) = empty_swarm_status_state();
    let host = DaemonSessions {
        sessions,
        soft_interrupt_queues: Arc::new(RwLock::new(HashMap::new())),
        swarm: LiveTurnSwarmContext::new(
            &members,
            &swarms_by_id,
            &event_history,
            &event_counter,
            &swarm_event_tx,
        ),
    };
    Fixture {
        host,
        agent,
        session_id,
        events,
        provider,
    }
}

/// Events up to and including the terminal `Done`.
async fn until_done(events: &mut mpsc::UnboundedReceiver<ServerEvent>) -> Vec<ServerEvent> {
    timeout(Duration::from_secs(5), async {
        let mut seen = Vec::new();
        loop {
            let event = unwrap_test_turn_event(events.recv().await.expect("member stream closed"));
            let done = matches!(event, ServerEvent::Done { .. });
            seen.push(event);
            if done {
                return seen;
            }
        }
    })
    .await
    .expect("turn should finish")
}

async fn transcript(agent: &Arc<Mutex<Agent>>) -> String {
    let agent = timeout(Duration::from_secs(5), agent.lock())
        .await
        .expect("turn should release the agent");
    format!("{:?}", agent.messages())
}

#[tokio::test]
async fn idle_session_is_reported_live_and_a_phone_message_starts_a_turn() {
    let mut f = fixture().await;
    f.provider.queue_response(vec![
        StreamEvent::TextDelta("on it".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    assert_eq!(
        f.host.live_sessions().await,
        vec![LiveSession {
            session_id: f.session_id.clone(),
            busy: false
        }]
    );

    f.host
        .deliver(&f.session_id, "phone: run the tests")
        .await
        .unwrap();
    let events = until_done(&mut f.events).await;
    assert!(
        events.iter().any(|event| matches!(event,
            ServerEvent::PeerUserMessage { content } if content == "phone: run the tests")),
        "the session's own window must show the prompt: {events:?}"
    );
    let transcript = transcript(&f.agent).await;
    assert!(transcript.contains("phone: run the tests"), "{transcript}");
    assert!(transcript.contains("on it"), "{transcript}");
}

#[tokio::test]
async fn message_for_a_session_mid_turn_is_queued_behind_that_turn() {
    let f = fixture().await;
    let queue = f.agent.lock().await.soft_interrupt_queue();
    f.host
        .swarm
        .members
        .write()
        .await
        .get_mut(&f.session_id)
        .unwrap()
        .status = "running".to_string();
    let running_turn = f.agent.lock().await;
    assert_eq!(
        f.host.live_sessions().await,
        vec![LiveSession {
            session_id: f.session_id.clone(),
            busy: true
        }]
    );

    // The running turn holds the agent lock. Its registered queue must still
    // accept a phone message without waiting for that turn to finish.
    f.host
        .soft_interrupt_queues
        .write()
        .await
        .insert(f.session_id.clone(), queue.clone());
    f.host
        .deliver(&f.session_id, "phone: also update the docs")
        .await
        .unwrap();
    let queued = queue.lock().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].content, "phone: also update the docs");
    assert!(!queued[0].urgent);
    drop(running_turn);
}

#[tokio::test(start_paused = true)]
async fn idle_session_that_is_briefly_locked_still_gets_a_turn() {
    let mut f = fixture().await;
    f.provider.queue_response(vec![
        StreamEvent::TextDelta("done".to_string()),
        StreamEvent::MessageEnd { stop_reason: None },
    ]);
    let agent = f.agent.clone();
    let attach = agent.lock_owned().await;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(350)).await;
        drop(attach);
    });
    f.host.deliver(&f.session_id, "phone: hello").await.unwrap();
    tokio::time::resume();
    until_done(&mut f.events).await;
    assert!(transcript(&f.agent).await.contains("phone: hello"));
}

#[tokio::test(start_paused = true)]
async fn idle_session_that_stays_locked_refuses_instead_of_stranding_the_message() {
    let f = fixture().await;
    let queue = f.agent.lock().await.soft_interrupt_queue();
    f.host
        .soft_interrupt_queues
        .write()
        .await
        .insert(f.session_id.clone(), queue.clone());
    let stuck = f.agent.lock().await;
    let error = f
        .host
        .deliver(&f.session_id, "phone: hello")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("try again"), "{error}");
    assert!(
        queue.lock().unwrap().is_empty(),
        "an idle session never drains its queue, so nothing may be queued"
    );
    drop(stuck);
}

#[tokio::test]
async fn sessions_without_a_window_are_not_live_and_refuse_messages() {
    let f = fixture().await;
    {
        let mut members = f.host.swarm.members.write().await;
        let member = members.get_mut(&f.session_id).unwrap();
        member.event_txs.clear();
        let (closed, _) = mpsc::unbounded_channel();
        member.event_tx = closed;
    }
    assert!(f.host.live_sessions().await.is_empty());
    let error = f.host.deliver(&f.session_id, "hello").await.unwrap_err();
    assert!(error.to_string().contains("no longer open"), "{error}");

    let error = f
        .host
        .deliver("session_not_here", "hello")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no longer running"), "{error}");
    assert_eq!(f.agent.lock().await.message_count(), 0);
}

#[tokio::test]
async fn headless_sessions_are_live() {
    let f = fixture().await;
    {
        let mut members = f.host.swarm.members.write().await;
        let member = members.get_mut(&f.session_id).unwrap();
        member.event_txs.clear();
        let (closed, _) = mpsc::unbounded_channel();
        member.event_tx = closed;
        member.is_headless = true;
    }
    assert_eq!(f.host.live_sessions().await.len(), 1);
}
