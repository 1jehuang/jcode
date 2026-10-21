//! Deterministic cancellation at the timer/agent/provider handoffs.

use super::*;
use std::sync::atomic::AtomicBool;
use tokio::sync::Notify;
use usage_limit_resume::ResumeOutcome;

/// Run the real initial turn and resume driver, retaining its outcome so a
/// superseded turn can be awaited without relying on an absent terminal event.
async fn spawn_resume(fx: &Fixture) -> tokio::task::JoinHandle<ResumeOutcome> {
    let agent = fx.agent.clone();
    let session_id = fx.session_id.clone();
    let swarm = fx.ctx();
    let event_tx = fx.members.read().await[&session_id].event_tx.clone();
    tokio::spawn(async move {
        let mut guard = agent.clone().lock_owned().await;
        let error = guard
            .run_once_streaming_mpsc(
                "[scheduled task] summarize the build log",
                vec![],
                None,
                event_tx.clone(),
            )
            .await
            .expect_err("initial request must hit the usage limit");
        usage_limit_resume::resume_turn_after_usage_limit(
            agent,
            Some(guard),
            &session_id,
            error,
            None,
            &event_tx,
            &swarm,
            &usage_limit_resume::no_wait_hook,
        )
        .await
    })
}

async fn finish(task: tokio::task::JoinHandle<ResumeOutcome>) -> ResumeOutcome {
    tokio::time::timeout(Duration::from_secs(3600), task)
        .await
        .expect("resume driver must finish")
        .expect("resume driver must not panic")
}

#[tokio::test(start_paused = true)]
async fn cancellation_after_timer_wake_before_agent_lock_supersedes_resume() {
    let provider = UsageLimitedProvider::new(1, Some(Duration::from_secs(2)));
    let fx = Fixture::new(&provider).await;
    let (woke, release) = usage_limit_resume::gate_next_resume_wait_for_test(&fx.session_id);
    let task = spawn_resume(&fx).await;
    tokio::time::timeout(Duration::from_secs(3600), woke)
        .await
        .expect("timer must wake")
        .expect("driver must reach the timer gate");
    assert_eq!(provider.calls(), 1);
    assert!(
        fx.agent.try_lock().is_ok(),
        "the gate precedes reacquisition"
    );
    assert!(usage_limit_resume::cancel_pending_resume(&fx.session_id));
    release.send(()).unwrap();

    assert!(matches!(finish(task).await, ResumeOutcome::Superseded));
    assert_eq!(
        provider.calls(),
        1,
        "cancelled timer must not call provider"
    );
    assert!(!usage_limit_resume::has_pending_resume(&fx.session_id));
    assert!(
        fx.agent.try_lock().is_ok(),
        "new user turn can reserve agent"
    );
    assert_eq!(
        fx.status().await.0,
        "rate_limited",
        "no stale running update"
    );
}

#[tokio::test(start_paused = true)]
async fn uncancelled_timer_wake_resumes_and_completes_normally() {
    let provider = UsageLimitedProvider::new(1, Some(Duration::from_secs(2)));
    let fx = Fixture::new(&provider).await;
    let (woke, release) = usage_limit_resume::gate_next_resume_wait_for_test(&fx.session_id);
    let task = spawn_resume(&fx).await;
    tokio::time::timeout(Duration::from_secs(3600), woke)
        .await
        .expect("timer must wake")
        .expect("driver must reach the timer gate");
    assert!(usage_limit_resume::has_pending_resume(&fx.session_id));
    release.send(()).unwrap();

    let ResumeOutcome::Completed {
        guard,
        completion_report,
    } = finish(task).await
    else {
        panic!("uncancelled resume must complete");
    };
    assert_eq!(provider.calls(), 2);
    assert_eq!(
        scheduled_messages(&guard),
        1,
        "resume does not resend input"
    );
    assert!(completion_report.is_some_and(|text| text.contains("resumed and finished")));
    assert!(!usage_limit_resume::has_pending_resume(&fx.session_id));
    assert!(
        fx.agent.try_lock().is_err(),
        "completion retains reservation"
    );
    drop(guard);
    assert!(fx.agent.try_lock().is_ok());
}

#[derive(Clone)]
struct GatedResumeProvider {
    inner: UsageLimitedProvider,
    entered: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

struct MarkDropped(Arc<AtomicBool>);

impl Drop for MarkDropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl Provider for GatedResumeProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let stream = self
            .inner
            .complete(messages, tools, system, resume_session_id)
            .await?;
        if self.inner.calls() == 2 {
            let _dropped = MarkDropped(self.dropped.clone());
            self.entered.notify_one();
            // Only cancellation can end this continuation. A third request
            // (the new user turn) is not gated.
            std::future::pending::<()>().await;
        }
        Ok(stream)
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_provider_continuation_drops_future_and_releases_agent() {
    let provider = GatedResumeProvider {
        inner: UsageLimitedProvider::new(1, Some(Duration::from_secs(2))),
        entered: Arc::new(Notify::new()),
        dropped: Arc::new(AtomicBool::new(false)),
    };
    let fx = Fixture::with_provider(Arc::new(provider.clone())).await;
    let task = spawn_resume(&fx).await;
    tokio::time::timeout(Duration::from_secs(3600), provider.entered.notified())
        .await
        .expect("resume must enter provider continuation");
    assert_eq!(provider.inner.calls(), 2);
    assert!(fx.agent.try_lock().is_err(), "continuation holds the agent");
    assert!(usage_limit_resume::has_pending_resume(&fx.session_id));
    assert!(usage_limit_resume::cancel_pending_resume(&fx.session_id));

    assert!(matches!(finish(task).await, ResumeOutcome::Superseded));
    assert!(
        provider.dropped.load(Ordering::SeqCst),
        "provider future was dropped"
    );
    assert!(!usage_limit_resume::has_pending_resume(&fx.session_id));
    let mut agent = fx
        .agent
        .try_lock()
        .expect("new message must not be delayed");
    assert!(!agent.messages().iter().any(|message| message.content.iter().any(|block| {
        matches!(block, crate::message::ContentBlock::Text { text, .. } if text.contains("resumed and finished"))
    })), "cancelled provider must not finish the old turn");
    let (events, _rx) = mpsc::unbounded_channel();
    agent
        .run_once_streaming_mpsc("new user message", vec![], None, events)
        .await
        .expect("new user message should run after cancellation");
    assert_eq!(provider.inner.calls(), 3);
}
