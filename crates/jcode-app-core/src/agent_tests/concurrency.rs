use super::*;

// These test the real Agent lifecycle wiring. Telemetry-core separately tests
// live OS leases and process crashes. Never send synthetic Agent events to the
// production endpoint while exercising construction/clear/resume here.
struct IsolatedEnv {
    _home: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_opt_out: Option<std::ffi::OsString>,
}

impl IsolatedEnv {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        let previous_opt_out = std::env::var_os("JCODE_NO_TELEMETRY");
        crate::env::set_var("JCODE_HOME", home.path());
        crate::env::set_var("JCODE_NO_TELEMETRY", "1");
        Self {
            _home: home,
            previous_home,
            previous_opt_out,
        }
    }
}

impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        for (key, value) in [
            ("JCODE_HOME", self.previous_home.take()),
            ("JCODE_NO_TELEMETRY", self.previous_opt_out.take()),
        ] {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

fn assert_owns_current_session(agent: &Agent) {
    let guard = agent
        .concurrency_session
        .as_ref()
        .expect("Agent owns a guard");
    assert_eq!(guard.session_id(), agent.session_id());
    assert!(!guard.is_active(), "test explicitly opted out of telemetry");
}

#[tokio::test]
async fn concurrency_guard_follows_clear_restore_and_close() {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    assert_owns_current_session(&agent);
    let original_id = agent.session_id().to_owned();

    // Failed restores must not end the currently owned session.
    assert!(
        agent
            .restore_session("nonexistent-concurrency-session")
            .is_err()
    );
    assert_eq!(agent.session_id(), original_id);
    assert_owns_current_session(&agent);

    agent.clear();
    assert_ne!(agent.session_id(), original_id);
    assert_owns_current_session(&agent);

    let mut restored = Session::create(
        Some("parent-concurrency-test".to_owned()),
        Some("Concurrency restore fixture".to_owned()),
    );
    restored.save().unwrap();
    agent.restore_session(&restored.id).unwrap();
    assert_owns_current_session(&agent);

    agent.mark_closed();
    assert!(
        agent.concurrency_session.is_none(),
        "retained closed agents are not live"
    );
    agent.mark_closed();
    assert!(agent.concurrency_session.is_none(), "closing is idempotent");

    // A retained Agent can resume after it has already been closed.
    agent.restore_session(&restored.id).unwrap();
    assert_owns_current_session(&agent);
    agent.mark_crashed(Some("test".to_owned()));
    assert!(agent.concurrency_session.is_none());
}

#[tokio::test]
async fn concurrency_guards_belong_to_each_agent_not_the_global_telemetry_slot() {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut first = Agent::new(provider.clone(), registry);
    let child_session = Session::create(Some(first.session_id().to_owned()), None);
    let registry = Registry::new(provider.clone()).await;
    let second = Agent::new_with_session(provider, registry, child_session, None);
    assert_owns_current_session(&first);
    assert_owns_current_session(&second);
    assert_ne!(first.session_id(), second.session_id());

    first.mark_closed();
    assert!(first.concurrency_session.is_none());
    assert_owns_current_session(&second);
}
