use super::*;
use std::ffi::{OsStr, OsString};

struct TestEnv(Vec<(&'static str, Option<OsString>)>);
impl TestEnv {
    fn set(values: &[(&'static str, &OsStr)]) -> Self {
        Self(
            values
                .iter()
                .map(|(name, value)| {
                    let old = std::env::var_os(name);
                    crate::env::set_var(name, value);
                    (*name, old)
                })
                .collect(),
        )
    }
}
impl Drop for TestEnv {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            if let Some(value) = value {
                crate::env::set_var(name, value);
            } else {
                crate::env::remove_var(name);
            }
        }
    }
}

#[test]
fn retry_deadlines_are_per_session_exponential_bounded_and_change_aware() {
    let mut cache = FailureBackoff::default();
    let start = Instant::now();
    let revision = [1, 2, 3, 4];
    let mut now = start;
    for seconds in [20, 40, 80, 160, 300, 300] {
        cache.failed("bad", revision, now);
        assert!(cache.deferred(
            "bad",
            revision,
            false,
            now + Duration::from_secs(seconds - 1)
        ));
        now += Duration::from_secs(seconds);
        assert!(!cache.deferred("bad", revision, false, now));
        assert!(!cache.deferred("good", revision, false, now));
    }
    cache.failed("bad", revision, now);
    assert!(!cache.deferred("bad", [2, 2, 3, 4], false, now));
    assert!(!cache.sessions.contains_key("bad"));
    cache.failed("bad", revision, now);
    assert!(!cache.deferred("bad", revision, true, now));
    cache.failed("bad", revision, now);
    assert_eq!(cache.sessions["bad"].attempts, 1);
    for i in 0..MAX_DIRTY_SESSIONS + 10 {
        cache.failed(&format!("session-{i}"), revision, now);
    }
    assert_eq!(cache.sessions.len(), MAX_DIRTY_SESSIONS);
}

#[tokio::test]
async fn failed_uploads_skip_unchanged_reloads_but_retry_changes_deadlines_and_generations() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    // A closed local port fails immediately without contacting any real service.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let _env = TestEnv::set(&[
        ("JCODE_HOME", home.path().as_os_str()),
        ("JCODE_API_KEY", OsStr::new("backoff-test")),
        ("JCODE_API_BASE", OsStr::new(&base)),
    ]);
    DIRTY_SESSIONS.lock().unwrap().clear();
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    let mut session = Session::create_with_id("failing".into(), None, None);
    session.messages.push(jcode_session_types::StoredMessage {
        id: "backoff-message".into(),
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".into(),
            cache_control: None,
        }]
        .into(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().unwrap();
    let client = reqwest::Client::new();
    assert_eq!(sync_once(&client).await.unwrap().errors, 1);
    assert!(load_state().sessions.is_empty());
    assert!(
        DIRTY_SESSIONS.lock().unwrap().is_empty(),
        "failure is not a fresh save"
    );
    let deadline = FAILURE_BACKOFF.lock().unwrap().as_ref().unwrap().sessions["failing"].retry_at;
    for reconcile in [true, false, true] {
        assert_eq!(
            sync_pass(&client, reconcile).await.unwrap(),
            SyncReport::default()
        );
        let cache = FAILURE_BACKOFF.lock().unwrap();
        let failed = &cache.as_ref().unwrap().sessions["failing"];
        assert_eq!(
            failed.attempts, 1,
            "a reload/upload error would increase attempts"
        );
        assert_eq!(failed.retry_at, deadline);
    }
    // Same revision, but a durable-save hint must bypass the failure deadline.
    notify_session_persisted("failing");
    assert_eq!(sync_pass(&client, false).await.unwrap().errors, 1);
    // Cross-process changes have no dirty hint and must also bypass backoff.
    session.custom_title = Some("changed revision".into());
    session.save().unwrap();
    DIRTY_SESSIONS.lock().unwrap().clear();
    assert_eq!(sync_once(&client).await.unwrap().errors, 1);
    FAILURE_BACKOFF
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .sessions
        .get_mut("failing")
        .unwrap()
        .retry_at = Instant::now();
    assert_eq!(sync_once(&client).await.unwrap().errors, 1);
    assert_eq!(
        FAILURE_BACKOFF.lock().unwrap().as_ref().unwrap().sessions["failing"].attempts,
        2
    );
    // Successful/empty sessions must still advance independently of failures.
    let empty = Session::create_with_id("empty".into(), None, None);
    // Persistence intentionally skips empty sessions, so seed a valid snapshot
    // directly to exercise sync's successful empty/debug fast path.
    std::fs::write(
        home.path().join("sessions/empty.json"),
        serde_json::to_vec(&empty).unwrap(),
    )
    .unwrap();
    notify_session_persisted("empty");
    sync_once(&client).await.unwrap();
    assert!(load_state().sessions.contains_key("empty"));
    save_state(&MobileSyncState::default()).unwrap();
    assert_eq!(sync_once(&client).await.unwrap(), SyncReport::default());
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(sync_once(&client).await.unwrap().errors, 1);
    assert_eq!(
        FAILURE_BACKOFF.lock().unwrap().as_ref().unwrap().sessions["failing"].attempts,
        1
    );
    DIRTY_SESSIONS.lock().unwrap().clear();
}
