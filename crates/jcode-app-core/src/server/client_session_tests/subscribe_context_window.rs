use super::*;
use anyhow::{Result, anyhow};

/// Restores `JCODE_RUNTIME_DIR` on drop so a panic cannot leak the override
/// into another test sharing this process-global.
struct RuntimeDirGuard(Option<std::ffi::OsString>);

impl Drop for RuntimeDirGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", previous);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

fn temp_runtime_dir() -> Result<RuntimeDirGuard> {
    let runtime = tempfile::TempDir::new().map_err(|error| anyhow!(error))?;
    let previous = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    Ok(RuntimeDirGuard(previous))
}

// Regression coverage for the launcher context window: 48de3e706 added an
// unconditional `ServerEvent::ModelChanged` to `handle_subscribe`
// (crates/jcode-app-core/src/server/client_session.rs:898-922).
//
// Before that commit only `handle_resume_session` sent `ModelChanged`, so a
// brand-new session started from the launcher never received one. A remote
// client cannot derive the window itself - its local provider is an inert
// placeholder with no model catalog, so it answers the generic
// DEFAULT_CONTEXT_LIMIT (200_000). The panel therefore showed 200K for a route
// that is really 1M. Measured on a pinned `stealth/space-bunny-alpha@Stealth`
// route whose catalog entry carries context_length 1000000.
//
// These tests drive the real `handle_subscribe` against a live `Agent` and read
// the events it actually emits, so they assert the wire contract rather than a
// re-statement of the fix.

/// A provider that reports a window no default could produce on its own, so a
/// passing assertion cannot be satisfied by DEFAULT_CONTEXT_LIMIT falling out.
struct MillionTokenWindowProvider;

#[async_trait]
impl Provider for MillionTokenWindowProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("subscribe bookkeeping must not invoke a provider")
    }

    fn name(&self) -> &str {
        "million-token-window-test"
    }

    fn model(&self) -> String {
        "stealth/space-bunny-alpha".to_string()
    }

    fn context_window(&self) -> usize {
        1_000_000
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MillionTokenWindowProvider)
    }
}

/// The server must report the route and its server-resolved window on every
/// subscribe, including the brand-new-session path the launcher uses.
///
/// `target_session_id` is not what a launcher client sends: it subscribes with
/// no target so the server hands it a new session. That is the exact case that
/// regressed.
#[tokio::test]
async fn subscribe_reports_server_resolved_context_window_to_new_session() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _runtime = temp_runtime_dir()?;

    let provider: Arc<dyn Provider> = Arc::new(MillionTokenWindowProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        registry.clone(),
        "session_launcher_window",
        Vec::new(),
    )));

    // Precondition: the window is genuinely non-default, so a bare 200_000
    // could never satisfy the assertions below.
    assert_eq!(
        agent.lock().await.provider_context_window(),
        1_000_000,
        "fixture provider must report a 1M window or this test proves nothing"
    );
    assert_ne!(
        agent.lock().await.provider_context_window(),
        200_000,
        "the fixture must not reproduce the 200K default it is meant to catch"
    );

    let swarm_members = Arc::new(RwLock::new(HashMap::<String, SwarmMember>::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::<String, VersionedPlan>::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::<String, String>::new()));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());
    let mut client_selfdev = false;

    handle_subscribe(
        42,
        None,
        None,
        false,
        &mut client_selfdev,
        "session_launcher_window",
        "conn_launcher",
        &None,
        &agent,
        &registry,
        false,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &swarm_coordinators,
        &client_event_tx,
        &mcp_pool,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await;

    let events = collect_events_until_done(&mut client_event_rx, 42).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 42)),
        "subscribe must still answer its request, got {events:?}"
    );

    let changed = events
        .iter()
        .find_map(|event| match event {
            ServerEvent::ModelChanged {
                model,
                provider_name,
                context_window,
                error,
                ..
            } => Some((model.clone(), provider_name.clone(), *context_window, error.clone())),
            _ => None,
        })
        .expect("subscribe must emit ModelChanged; without it a launcher session never learns the window");

    let (model, provider_name, context_window, error) = changed;
    assert!(
        error.is_none(),
        "a plain subscribe is not a model switch and must not report an error: {error:?}"
    );
    assert_eq!(
        model, "stealth/space-bunny-alpha",
        "ModelChanged must name the route the server resolved"
    );
    assert_eq!(
        provider_name.as_deref(),
        Some("million-token-window-test"),
        "ModelChanged must carry the provider name so the client can route its catalog lookups"
    );
    assert_eq!(
        context_window,
        Some(1_000_000),
        "the server-resolved window must cross the wire; the client cannot derive it because its \
         own provider is an inert placeholder that answers the 200K default"
    );

    Ok(())
}

/// The event must arrive *before* `Done`, and must not be reported as a failed
/// model switch. Ordering matters: a client that finalises subscribe state on
/// `Done` would otherwise read the window too late and flash 200K.
#[tokio::test]
async fn subscribe_model_changed_precedes_done_and_reports_no_switch_error() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _runtime = temp_runtime_dir()?;

    let provider: Arc<dyn Provider> = Arc::new(MillionTokenWindowProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider,
        registry.clone(),
        "session_launcher_order",
        Vec::new(),
    )));

    let swarm_members = Arc::new(RwLock::new(HashMap::<String, SwarmMember>::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::<String, VersionedPlan>::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::<String, String>::new()));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());
    let mut client_selfdev = false;

    handle_subscribe(
        7,
        None,
        None,
        false,
        &mut client_selfdev,
        "session_launcher_order",
        "conn_launcher",
        &None,
        &agent,
        &registry,
        false,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &swarm_coordinators,
        &client_event_tx,
        &mcp_pool,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await;

    let events = collect_events_until_done(&mut client_event_rx, 7).await;
    let changed_at = events
        .iter()
        .position(|event| matches!(event, ServerEvent::ModelChanged { .. }))
        .expect("subscribe must emit ModelChanged so the client learns the server-resolved window");
    let done_at = events
        .iter()
        .position(|event| matches!(event, ServerEvent::Done { id } if *id == 7))
        .expect("subscribe must answer its request with Done");

    assert!(
        changed_at < done_at,
        "the window must arrive before Done, or a client that finalises on Done reads it too \
         late and flashes the 200K default; order was {events:?}"
    );

    Ok(())
}
