use super::*;
use jcode_session_types::StoredMessage;

#[tokio::test]
#[ignore = "manual local filesystem reconciliation benchmark"]
async fn benchmark_unchanged_reconciliation() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[
        ("JCODE_HOME", home.path().as_os_str()),
        (
            "JCODE_API_KEY",
            std::ffi::OsStr::new("benchmark-no-network"),
        ),
        ("JCODE_API_BASE", std::ffi::OsStr::new("http://127.0.0.1:1")),
    ]);
    DIRTY_SESSIONS.lock().unwrap().clear();
    let dir = home.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let mut state = MobileSyncState {
        enabled: true,
        ..Default::default()
    };
    for index in 0..1300 {
        let id = format!("benchmark-{index:04}");
        let path = dir.join(format!("{id}.json"));
        // An unchanged revision must never need to deserialize a transcript.
        std::fs::write(&path, b"{}").unwrap();
        state.sessions.insert(
            id,
            SessionProgress {
                uploaded: 100,
                mtime_ms: session_mtime_ms(&path),
                revision: Some(session_revision(&path)),
                fingerprint: Some(42),
            },
        );
    }
    save_state(&state).unwrap();
    // Enabling increments the fence, so seed progress in the current generation.
    state.generation = load_state().generation;
    save_state(&state).unwrap();
    let client = reqwest::Client::new();
    let mut elapsed = Vec::new();
    for _ in 0..7 {
        let started = Instant::now();
        assert_eq!(sync_once(&client).await.unwrap(), SyncReport::default());
        elapsed.push(started.elapsed().as_micros());
    }
    elapsed.sort_unstable();
    eprintln!(
        "unchanged reconciliation: candidates=1300 passes=7 median_us={} max_us={} samples_us={elapsed:?}",
        elapsed[3], elapsed[6]
    );
}

#[test]
fn sync_timing_records_only_static_fields_and_elapsed_milliseconds() {
    let mut timing = SyncTiming::new("test_stage");
    timing.started = Instant::now() - Duration::from_millis(12);
    for outcome in ["success", "error", "skipped_disabled", "incomplete"] {
        timing.outcome = outcome;
        let record = timing.record();
        let fields: Vec<_> = record.split_whitespace().collect();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0], "mobile_sync");
        assert_eq!(fields[1], "stage=test_stage");
        assert_eq!(fields[2], format!("outcome={outcome}"));
        let elapsed: u128 = fields[3]
            .strip_prefix("duration_ms=")
            .unwrap()
            .parse()
            .unwrap();
        assert!(elapsed >= 12);
    }
}

#[tokio::test]
async fn upload_failure_does_not_retain_private_server_response() {
    let secret = "private-transcript-and-auth-token";
    let (base, request) = mock_listing("500 Internal Server Error", secret).await;
    let body = UploadBody {
        title: Some(secret),
        source: "local",
        host: None,
        cwd: None,
        model: None,
        created_at: String::new(),
        updated_at: String::new(),
        messages: &[],
    };
    let error = upload(&reqwest::Client::new(), &base, secret, "test", &body)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("500"));
    assert!(!format!("{error:#}").contains(secret));
    request.await.unwrap();
}

struct TestEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

#[tokio::test]
async fn unchanged_pass_and_identical_commit_do_not_rewrite_state() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[
        ("JCODE_HOME", home.path().as_os_str()),
        ("JCODE_API_KEY", std::ffi::OsStr::new("no-network")),
        ("JCODE_API_BASE", std::ffi::OsStr::new("http://127.0.0.1:1")),
    ]);
    DIRTY_SESSIONS.lock().unwrap().clear();
    let path = home.path().join("sessions/unchanged.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"not a transcript").unwrap();
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    let generation = load_state().generation;
    let updates = BTreeMap::from([(
        "unchanged".into(),
        SessionProgress {
            revision: Some(session_revision(&path)),
            ..Default::default()
        },
    )]);
    commit_progress(generation, &updates).unwrap();
    let state = state_path().unwrap();
    std::fs::File::open(&state)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(100)))
        .unwrap();
    let before = session_revision(&state);
    commit_progress(generation, &updates).unwrap();
    assert_eq!(
        session_revision(&state),
        before,
        "identical merge must not write"
    );
    std::fs::remove_file(home.path().join("mobile_sync.lock")).unwrap();
    for reconcile in [true, false] {
        assert_eq!(
            sync_pass(&reqwest::Client::new(), reconcile).await.unwrap(),
            SyncReport::default()
        );
        assert_eq!(
            session_revision(&state),
            before,
            "no-op pass must not write"
        );
        assert!(
            !home.path().join("mobile_sync.lock").exists(),
            "no-op pass must not acquire cross-process lock"
        );
    }
}

#[tokio::test]
async fn disable_reenable_during_upload_fences_remaining_chunks_and_progress() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let _env = TestEnv::set(&[
        ("JCODE_HOME", home.path().as_os_str()),
        ("JCODE_API_KEY", std::ffi::OsStr::new("test-key")),
        ("JCODE_API_BASE", std::ffi::OsStr::new(&base)),
    ]);
    DIRTY_SESSIONS.lock().unwrap().clear();
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    let mut session = Session::create_with_id("generation-cycle".into(), None, None);
    session.messages = (0..MAX_MESSAGES_PER_UPLOAD + 1)
        .map(|index| {
            let mut message = msg(
                Role::User,
                vec![ContentBlock::Text {
                    text: "visible".into(),
                    cache_control: None,
                }],
            );
            message.id = format!("message-{index}");
            message
        })
        .collect();
    session.save().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(start) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..start]);
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= start + 4 + length {
                    break;
                }
            }
        }
        save_state(&MobileSyncState::default()).unwrap();
        save_state(&MobileSyncState {
            enabled: true,
            ..Default::default()
        })
        .unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err(),
            "old generation must not send its second chunk"
        );
    });
    let report = sync_once(&reqwest::Client::new()).await.unwrap();
    assert_eq!(report.messages_uploaded, MAX_MESSAGES_PER_UPLOAD);
    assert_eq!(report.sessions_uploaded, 0);
    server.await.unwrap();
    assert!(load_state().enabled);
    assert!(load_state().sessions.is_empty());
    DIRTY_SESSIONS.lock().unwrap().clear();
}

impl TestEnv {
    fn set(values: &[(&'static str, &std::ffi::OsStr)]) -> Self {
        Self(
            values
                .iter()
                .map(|(name, value)| {
                    let previous = std::env::var_os(name);
                    crate::env::set_var(name, value);
                    (*name, previous)
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
fn disabled_dirty_hints_are_bounded_without_reading_sync_state() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    DIRTY_SESSIONS.lock().unwrap().clear();
    for index in 0..MAX_DIRTY_SESSIONS * 2 {
        notify_session_persisted(&format!("disabled-{index}"));
    }
    notify_session_persisted("../invalid");
    let mut dirty = DIRTY_SESSIONS.lock().unwrap();
    assert_eq!(dirty.len(), MAX_DIRTY_SESSIONS);
    assert!(dirty.contains(&format!("disabled-{}", MAX_DIRTY_SESSIONS * 2 - 1)));
    assert!(!home.path().join("mobile_sync.json").exists());
    dirty.clear();
}

#[test]
fn state_commits_merge_concurrent_progress_and_never_reenable() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    let generation = load_state().generation;
    let progress = |uploaded, mtime_ms| SessionProgress {
        uploaded,
        mtime_ms,
        ..Default::default()
    };
    let stale = BTreeMap::from([("first".into(), progress(1, 1))]);
    commit_progress(
        generation,
        &BTreeMap::from([("second".into(), progress(2, 2))]),
    )
    .unwrap();
    commit_progress(generation, &stale).unwrap();
    assert_eq!(load_state().sessions.len(), 2);
    commit_progress(
        generation,
        &BTreeMap::from([("first".into(), progress(3, 3))]),
    )
    .unwrap();
    commit_progress(generation, &stale).unwrap();
    assert_eq!(load_state().sessions["first"].uploaded, 3);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let writers: Vec<_> = (0..8)
        .map(|index| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                commit_progress(
                    generation,
                    &BTreeMap::from([(format!("parallel-{index}"), progress(index, 10))]),
                )
                .unwrap();
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(load_state().sessions.len(), 10);
    save_state(&MobileSyncState::default()).unwrap();
    commit_progress(generation, &stale).unwrap();
    assert!(!load_state().enabled);
    assert!(load_state().sessions.is_empty());
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    commit_progress(generation, &stale).unwrap();
    assert!(
        load_state().sessions.is_empty(),
        "old opt-in generation cannot restore wiped progress"
    );
}

#[tokio::test]
async fn persistence_notifications_upload_under_one_second_and_retain_inflight_writes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let _env = TestEnv::set(&[
        ("JCODE_HOME", home.path().as_os_str()),
        ("JCODE_API_KEY", std::ffi::OsStr::new("test-key")),
        ("JCODE_API_BASE", std::ffi::OsStr::new(&base)),
    ]);
    let (requests_tx, mut requests) = tokio::sync::mpsc::channel(8);
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let body_start = loop {
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let header = String::from_utf8_lossy(&bytes[..body_start]);
            let length: usize = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|s| s.trim().parse().unwrap())
                })
                .unwrap();
            while bytes.len() < body_start + length {
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap();
            let (release, wait) = tokio::sync::oneshot::channel::<()>();
            requests_tx.send((body, release)).await.unwrap();
            wait.await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        }
    });
    let background = tokio::spawn(background_sync());
    // Consume the initial reconciliation while disabled.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut session = Session::create_with_id("event-sync".into(), None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "first".into(),
            cache_control: None,
        }],
    );
    session.save().unwrap();
    assert!(!is_enabled());
    assert!(
        tokio::time::timeout(Duration::from_millis(220), requests.recv())
            .await
            .is_err()
    );
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    let started = std::time::Instant::now();
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "second".into(),
            cache_control: None,
        }],
    );
    session.save().unwrap();
    for title in ["coalesce one", "coalesce two", "coalesce final"] {
        session.custom_title = Some(title.into());
        session.save().unwrap();
    }
    let (first, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    eprintln!(
        "mobile persistence-to-upload notification: {}ms",
        started.elapsed().as_millis()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(first["messages"].as_array().unwrap().len(), 2);
    assert_eq!(first["title"], "coalesce final");
    // Persist while the first upload is still blocked in the network.
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "during-upload".into(),
            cache_control: None,
        }],
    );
    session.save().unwrap();
    release.send(()).unwrap();
    let (second, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second["messages"].as_array().unwrap().len(), 1);
    assert_eq!(second["messages"][0]["seq"], 2);
    assert_eq!(second["messages"][0]["text"], "during-upload");
    release.send(()).unwrap();
    // Progress persistence itself must not schedule another upload.
    assert!(
        tokio::time::timeout(Duration::from_millis(350), requests.recv())
            .await
            .is_err()
    );
    assert_eq!(load_state().sessions["event-sync"].uploaded, 3);
    // Simulate a writer in another process: no notification hook in this
    // process, so only the reconciliation path can discover the update.
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "external writer".into(),
            cache_control: None,
        }],
    );
    let path = home.path().join("sessions/event-sync.json");
    storage::write_json_fast(&path, &session).unwrap();
    session = Session::load_from_path(&path).unwrap();
    let reconcile = tokio::spawn(async { sync_once(&reqwest::Client::new()).await.unwrap() });
    let (external, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(external["messages"].as_array().unwrap().len(), 1);
    assert_eq!(external["messages"][0]["text"], "external writer");
    release.send(()).unwrap();
    assert_eq!(reconcile.await.unwrap().messages_uploaded, 1);
    // Old snapshots have no local dirty hints, as with another process's
    // backlog. Block the first old upload, then durably save a fresh session.
    for id in ["old-a", "old-b", "old-c"] {
        let mut old = Session::create_with_id(id.into(), None, None);
        old.custom_title = Some(id.into());
        old.messages = vec![msg(
            Role::User,
            vec![ContentBlock::Text {
                text: id.into(),
                cache_control: None,
            }],
        )];
        storage::write_json_fast(&home.path().join(format!("sessions/{id}.json")), &old).unwrap();
    }
    let reconcile = tokio::spawn(async { sync_once(&reqwest::Client::new()).await.unwrap() });
    let (old, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    let old_id = old["title"].as_str().unwrap().to_owned();
    assert!(old_id.starts_with("old-"));
    let mut fresh = Session::create_with_id("fresh-save".into(), None, None);
    fresh.custom_title = Some("fresh-save".into());
    fresh.messages = vec![msg(
        Role::User,
        vec![ContentBlock::Text {
            text: "fresh".into(),
            cache_control: None,
        }],
    )];
    fresh.save().unwrap();
    release.send(()).unwrap();
    let (next, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next["title"], "fresh-save",
        "fresh save must bypass old backlog"
    );
    assert_eq!(reconcile.await.unwrap().sessions_uploaded, 1);
    assert_eq!(load_state().sessions[&old_id].uploaded, 1);
    release.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(220)).await;
    // The next periodic-style pass resumes the unvisited backlog, without
    // uploading the completed old session again.
    let resume = tokio::spawn(async { sync_once(&reqwest::Client::new()).await.unwrap() });
    for _ in 0..2 {
        let (remaining, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(remaining["title"].as_str().unwrap().starts_with("old-"));
        assert_ne!(remaining["title"], old_id);
        release.send(()).unwrap();
    }
    assert_eq!(resume.await.unwrap().sessions_uploaded, 2);
    session.custom_title = Some("metadata only".into());
    session.save().unwrap();
    let (metadata, release) = tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata["title"], "metadata only");
    assert!(metadata["messages"].as_array().unwrap().is_empty());
    // A disable racing an upload must not be undone by its completion.
    save_state(&MobileSyncState::default()).unwrap();
    release.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert!(!load_state().enabled);
    assert!(load_state().sessions.is_empty());
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "disabled".into(),
            cache_control: None,
        }],
    );
    session.save().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), requests.recv())
            .await
            .is_err()
    );
    background.abort();
    server.abort();
    let _ = background.await;
    let _ = server.await;
}

#[test]
fn reconciliation_does_not_starve_old_candidates_or_miss_same_millisecond_writes() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    for index in 0..205 {
        std::fs::write(sessions.join(format!("session-{index}.json")), b"{}").unwrap();
    }
    assert_eq!(candidate_sessions(0).unwrap().len(), 205);
    let path = sessions.join("session-0.json");
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let original = file.metadata().unwrap().modified().unwrap();
    let before = session_revision(&path);
    let before_ms = session_mtime_ms(&path);
    std::fs::write(&path, b"{\"changed\":true}").unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(original))
        .unwrap();
    assert_eq!(session_mtime_ms(&path), before_ms);
    assert_ne!(session_revision(&path), before);
    let journal = crate::session::session_journal_path_from_snapshot(&path);
    std::fs::write(&journal, b"journal update\n").unwrap();
    let with_journal = session_revision(&path);
    std::fs::remove_file(journal).unwrap();
    assert_ne!(session_revision(&path), with_journal);
    assert_eq!(SYNC_INTERVAL, Duration::from_secs(20));
}

async fn mock_listing(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
        }
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (base, task)
}

#[tokio::test]
async fn cloud_session_listing_filters_and_preserves_metadata() {
    let body = serde_json::json!({"sessions": [
        {"session_id": "cloud-1", "source": "cloud", "title": "Title",
         "host": "worker", "cwd": "/repo", "updated_at": "updated", "created_at": "created"},
        {"session_id": "cloud-2", "source": "cloud"},
        {"session_id": "local-1", "source": "local"},
        {"session_id": "../unsafe", "source": "cloud"},
        {"session_id": "", "source": "cloud"},
        {"session_id": "x".repeat(129), "source": "cloud"}
    ]})
    .to_string();
    let (base, request) = mock_listing("200 OK", &body).await;
    let sessions = list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "test-key")
        .await
        .unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].session_id, "cloud-1");
    assert_eq!(sessions[0].title.as_deref(), Some("Title"));
    assert_eq!(sessions[0].host.as_deref(), Some("worker"));
    assert_eq!(sessions[0].cwd.as_deref(), Some("/repo"));
    assert_eq!(sessions[0].updated_at.as_deref(), Some("updated"));
    assert_eq!(sessions[0].created_at.as_deref(), Some("created"));
    assert_eq!(sessions[1].title, None);
    assert_eq!(sessions[1].created_at, None);
    let request = request.await.unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /mobile/sessions?limit=500 http/1.1\r\n"));
    assert!(request.contains("\r\nauthorization: bearer test-key\r\n"));
}

#[tokio::test]
async fn cloud_session_listing_rejects_http_and_schema_errors() {
    for (status, body) in [
        ("401 Unauthorized", "{\"sessions\":[]}"),
        ("200 OK", "not json"),
        ("200 OK", "{}"),
        ("200 OK", "{\"sessions\":[{\"source\":\"cloud\"}]}"),
        ("200 OK", "{\"sessions\":[{\"session_id\":\"s1\"}]}"),
    ] {
        let (base, request) = mock_listing(status, body).await;
        assert!(
            list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "key")
                .await
                .is_err()
        );
        request.await.unwrap();
    }
    let (base, request) = mock_listing("200 OK", "{\"sessions\":[]}").await;
    assert!(
        list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "key")
            .await
            .unwrap()
            .is_empty()
    );
    request.await.unwrap();
}

fn msg(role: Role, content: Vec<ContentBlock>) -> StoredMessage {
    StoredMessage {
        id: "m".into(),
        role,
        content,
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }
}

#[test]
fn wire_messages_keep_visible_text_and_skip_hidden() {
    let mut session = Session::create_with_id("s1".into(), None, None);
    session.messages = vec![
        msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "hi".into(),
                cache_control: None,
            }],
        ),
        msg(
            Role::Assistant,
            vec![ContentBlock::Reasoning {
                text: "secret".into(),
            }],
        ),
        msg(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "running".into(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                    thought_signature: None,
                },
            ],
        ),
        msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: "x".repeat(MAX_TOOL_RESULT_CHARS + 10),
                is_error: None,
            }],
        ),
    ];
    let wire = wire_messages(&session, 0);
    assert_eq!(
        wire.iter().map(|m| m.seq).collect::<Vec<_>>(),
        vec![0, 2, 3]
    );
    assert_eq!(wire[0].role, "user");
    assert_eq!(wire[1].text, "running\n\n🔧 bash: ls");
    assert_eq!(wire[2].role, "tool");
    assert!(wire[2].text.ends_with("(truncated)"));
    assert!(!wire.iter().any(|m| m.text.contains("secret")));
    assert_eq!(wire_messages(&session, 3).len(), 1);
}

#[test]
fn session_ids_are_validated() {
    assert!(valid_session_id("session_abc-123.x"));
    assert!(!valid_session_id("bad id"));
    assert!(!valid_session_id(""));
}

#[test]
fn incremental_updates_replay_rewritten_and_shrunk_prefixes() {
    let mut session = Session::create_with_id("rewrite".into(), None, None);
    for text in ["first", "second"] {
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        );
    }
    let (initial, fingerprint) = incremental_messages(&session, &SessionProgress::default());
    assert_eq!(initial.len(), 2);
    let progress = SessionProgress {
        uploaded: 2,
        fingerprint: Some(fingerprint),
        ..Default::default()
    };
    assert!(incremental_messages(&session, &progress).0.is_empty());
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "appended".into(),
            cache_control: None,
        }],
    );
    let appended = incremental_messages(&session, &progress).0;
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].seq, 2);
    session.messages[0].content = vec![ContentBlock::Text {
        text: "rewritten".into(),
        cache_control: None,
    }];
    let rewritten = incremental_messages(&session, &progress).0;
    assert_eq!(rewritten.len(), 3);
    assert_eq!(rewritten[0].text, "rewritten");
    session.truncate_messages(1);
    assert_eq!(incremental_messages(&session, &progress).0.len(), 1);
}
