//! Server-initiated turns that hit a subscription usage limit wait for the
//! reset and resume instead of failing (scheduled tasks, swarm DM wakes,
//! swarm worker tasks).

use super::usage_limit_resume;
use crate::agent::Agent;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::protocol::ServerEvent;
use crate::provider::{EventStream, Provider};
use crate::server::SwarmMember;
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use jcode_provider_core::usage_limit_resume::with_usage_limit_reset;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

/// Fails the first `limited_calls` requests with the Anthropic fail-fast
/// usage-limit error (reset `reset_in` away), then streams a reply.
#[derive(Clone)]
struct UsageLimitedProvider {
    limited_calls: usize,
    reset_in: Option<Duration>,
    calls: Arc<AtomicUsize>,
    call_times: Arc<StdMutex<Vec<tokio::time::Instant>>>,
}

impl UsageLimitedProvider {
    fn new(limited_calls: usize, reset_in: Option<Duration>) -> Self {
        Self {
            limited_calls,
            reset_in,
            calls: Arc::new(AtomicUsize::new(0)),
            call_times: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn gaps(&self) -> Vec<Duration> {
        let times = self.call_times.lock().unwrap();
        times.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }
}

fn usage_limit_error(reset_in: Option<Duration>) -> anyhow::Error {
    // Same shape as the Anthropic runtime's fail-fast error: the text names
    // the reset the typed tag carries (or none when it is unknown).
    let detail = match reset_in {
        Some(reset) => format!(
            " Usage limit reached for this Claude account; resets in {}m (2026-09-30 13:30 UTC).",
            reset.as_secs().div_ceil(60).max(1)
        ),
        None => " Usage limit reached for this Claude account.".to_string(),
    };
    with_usage_limit_reset(
        anyhow::anyhow!(
            "Anthropic API error (429 Too Many Requests): {{\"type\":\"error\",\"error\":{{\"type\":\"rate_limit_error\",\"message\":\"This request would exceed your account's rate limit. Please try again later.\"}}}}{detail}"
        ),
        reset_in,
    )
}

#[async_trait]
impl Provider for UsageLimitedProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.call_times
            .lock()
            .unwrap()
            .push(tokio::time::Instant::now());
        if call < self.limited_calls {
            // Like the Anthropic runtime: the stream ends with the error.
            let error = usage_limit_error(self.reset_in);
            return Ok(Box::pin(futures::stream::iter(vec![Err(error)])));
        }
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TextDelta("resumed and finished".to_string())),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct Fixture {
    session_id: String,
    agent: Arc<Mutex<Agent>>,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Arc<RwLock<VecDeque<super::SwarmEvent>>>,
    event_counter: Arc<AtomicU64>,
    swarm_event_tx: broadcast::Sender<super::SwarmEvent>,
    events: mpsc::UnboundedReceiver<ServerEvent>,
}

impl Fixture {
    async fn new(provider: &UsageLimitedProvider) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(provider.clone());
        let registry = Registry::new(provider.clone()).await;
        let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
        let session_id = agent.lock().await.session_id().to_string();
        let (event_tx, events) = mpsc::unbounded_channel();
        let member = SwarmMember {
            session_id: session_id.clone(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            friendly_name: Some("otter".to_string()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
            task_label: None,
        };
        Self {
            sessions: Arc::new(RwLock::new(HashMap::from([(
                session_id.clone(),
                agent.clone(),
            )]))),
            members: Arc::new(RwLock::new(HashMap::from([(session_id.clone(), member)]))),
            swarms_by_id: Arc::new(RwLock::new(HashMap::new())),
            event_history: Arc::new(RwLock::new(VecDeque::new())),
            event_counter: Arc::new(AtomicU64::new(0)),
            swarm_event_tx: broadcast::channel(64).0,
            session_id,
            agent,
            events,
        }
    }

    fn ctx(&self) -> super::live_turn::LiveTurnSwarmContext {
        super::live_turn::LiveTurnSwarmContext::new(
            &self.members,
            &self.swarms_by_id,
            &self.event_history,
            &self.event_counter,
            &self.swarm_event_tx,
        )
    }

    async fn start_scheduled_turn(&self) {
        let started = super::live_turn::run_live_turn_if_idle(
            &self.session_id,
            "[scheduled task] summarize the build log",
            None,
            &self.sessions,
            self.ctx(),
        )
        .await;
        assert!(
            started,
            "idle live session should accept the scheduled turn"
        );
    }

    async fn status(&self) -> (String, Option<String>) {
        let members = self.members.read().await;
        let member = members.get(&self.session_id).unwrap();
        (member.status.clone(), member.detail.clone())
    }

    /// Next terminal event (Done or Error) for the server-initiated turn.
    /// Paused time auto-advances, so the 2 h bound costs nothing in real time
    /// but turns a missing event into a failure instead of a hang.
    async fn next_terminal(&mut self) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(2 * 3600), async {
            loop {
                match self.events.recv().await.expect("member event stream open") {
                    event @ (ServerEvent::Done { .. } | ServerEvent::Error { .. }) => {
                        return event;
                    }
                    _ => continue,
                }
            }
        })
        .await
        .expect("no further Done/Error event for the server-initiated turn")
    }

    async fn wait_status(&self, wanted: &str) {
        for _ in 0..10_000 {
            if self.status().await.0 == wanted {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "member never reached {wanted}, last status {:?}",
            self.status().await
        );
    }
}

fn scheduled_messages(agent: &Agent) -> usize {
    agent
        .messages()
        .iter()
        .filter(|message| {
            message.content.iter().any(|block| {
                matches!(block, crate::message::ContentBlock::Text { text, .. } if text.contains("scheduled task"))
            })
        })
        .count()
}

#[tokio::test(start_paused = true)]
async fn scheduled_turn_hitting_a_far_usage_limit_waits_for_the_reset_and_resumes() {
    let provider = UsageLimitedProvider::new(1, Some(Duration::from_secs(2)));
    let mut fx = Fixture::new(&provider).await;
    fx.start_scheduled_turn().await;

    // The client is told when the server resumes, not that the turn failed.
    let event = fx.next_terminal().await;
    let ServerEvent::Error {
        id,
        retry_after_secs,
        ..
    } = event
    else {
        panic!("expected a usage-limit notice, got {event:?}");
    };
    assert_eq!(id, 0);
    let retry_after = retry_after_secs.expect("usage-limit error must carry retry_after_secs");
    assert!(
        (2 + 5..=2 + 30).contains(&retry_after),
        "resume at the reset plus 5-30 s of jitter, got {retry_after}"
    );

    // Not failed: waiting for the reset, and the detail says when.
    fx.wait_status("rate_limited").await;
    let (_, detail) = fx.status().await;
    let detail = detail.unwrap_or_default();
    assert!(
        detail.starts_with("usage limit, resuming at ") && detail.contains("UTC"),
        "{detail}"
    );
    assert_eq!(provider.calls(), 1);

    // After the reset the same turn continues and completes.
    let event = fx.next_terminal().await;
    assert!(matches!(event, ServerEvent::Done { id: 0 }), "{event:?}");
    fx.wait_status("ready").await;
    assert_eq!(provider.calls(), 2);
    assert!(
        provider.gaps()[0] >= Duration::from_secs(2 + 5),
        "never resumed before the reported reset: {:?}",
        provider.gaps()
    );
    let agent = fx.agent.lock().await;
    assert_eq!(
        scheduled_messages(&agent),
        1,
        "the resume continues the turn instead of sending the message again"
    );
    let report = fx
        .members
        .read()
        .await
        .get(&fx.session_id)
        .and_then(|member| member.latest_completion_report.clone());
    assert!(report.is_some_and(|report| report.contains("resumed and finished")));
}

#[tokio::test(start_paused = true)]
async fn always_limited_turn_gets_one_attempt_plus_three_resumes_then_fails() {
    let reset = Duration::from_secs(120);
    let provider = UsageLimitedProvider::new(usize::MAX, Some(reset));
    let mut fx = Fixture::new(&provider).await;
    fx.start_scheduled_turn().await;

    for _ in 0..3 {
        let event = fx.next_terminal().await;
        assert!(
            matches!(
                event,
                ServerEvent::Error {
                    id: 0,
                    retry_after_secs: Some(_),
                    server_resumes: true,
                    ..
                }
            ),
            "{event:?}"
        );
    }
    let event = fx.next_terminal().await;
    let ServerEvent::Error {
        id,
        message,
        retry_after_secs,
        server_resumes,
    } = event
    else {
        panic!("expected the final failure, got {event:?}");
    };
    assert_eq!(id, 0);
    assert!(!server_resumes, "the give-up error is terminal");
    assert_eq!(retry_after_secs, None, "nothing retries after giving up");
    assert!(
        message.contains("after 3 automatic resumes"),
        "clear give-up message: {message}"
    );
    fx.wait_status("failed").await;
    assert_eq!(provider.calls(), 4, "1 attempt + 3 resumes");
    for gap in provider.gaps() {
        assert!(gap >= reset + Duration::from_secs(5), "gap {gap:?}");
    }
    assert!(!usage_limit_resume::has_pending_resume(&fx.session_id));
}

#[tokio::test(start_paused = true)]
async fn past_or_missing_reset_never_retries_without_a_delay() {
    for (reset_in, min_gap) in [
        (Some(Duration::ZERO), Duration::from_secs(60)),
        (None, Duration::from_secs(15 * 60)),
    ] {
        let provider = UsageLimitedProvider::new(usize::MAX, reset_in);
        let mut fx = Fixture::new(&provider).await;
        fx.start_scheduled_turn().await;
        for _ in 0..4 {
            fx.next_terminal().await;
        }
        fx.wait_status("failed").await;
        assert_eq!(provider.calls(), 4, "reset {reset_in:?}");
        for gap in provider.gaps() {
            assert!(
                gap >= min_gap,
                "reset {reset_in:?}: gap {gap:?} < {min_gap:?}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_user_message_cancels_the_pending_resume() {
    let provider = UsageLimitedProvider::new(1, Some(Duration::from_secs(600)));
    let mut fx = Fixture::new(&provider).await;
    fx.start_scheduled_turn().await;
    fx.next_terminal().await;
    fx.wait_status("rate_limited").await;
    assert!(usage_limit_resume::has_pending_resume(&fx.session_id));
    // The agent is free while the turn waits, so the user can talk to it.
    assert!(
        fx.agent.try_lock().is_ok(),
        "waiting must not hold the agent"
    );

    // What client_lifecycle does when the user sends the session a message.
    assert!(usage_limit_resume::cancel_pending_resume(&fx.session_id));
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(provider.calls(), 1, "the cancelled resume never runs");
    assert!(!usage_limit_resume::has_pending_resume(&fx.session_id));
}

#[tokio::test(start_paused = true)]
async fn a_second_limit_replaces_the_pending_resume() {
    let provider = UsageLimitedProvider::new(usize::MAX, Some(Duration::from_secs(600)));
    let mut fx = Fixture::new(&provider).await;
    fx.start_scheduled_turn().await;
    fx.next_terminal().await;
    fx.wait_status("rate_limited").await;

    // Another server-initiated turn for the same session hits the limit.
    let second = usage_limit_resume::register_pending_resume(&fx.session_id);
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(
        provider.calls(),
        1,
        "the replaced resume must not run: one pending resume per session"
    );
    drop(second);
}
