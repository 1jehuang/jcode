//! Lifecycle and all-session event behavior which cannot be exercised by the
//! protocol-only socket-pair tests.

#![cfg(unix)]

use jcode_harness_api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame, SessionInfo, read_frame,
    write_frame,
};
use jcode_sdk::{
    ConnectOptions, GlobalEventsOptions, JcodeClient, LaunchOptions, inherit_credentials,
};
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn session(id: &str) -> SessionInfo {
    SessionInfo {
        session_id: id.to_string(),
        working_dir: None,
        title: Some(format!("Title for {id}")),
        status: "idle".to_string(),
        transcript_bytes: None,
        saved: false,
        updated_at_ms: None,
        last_active_at_ms: None,
        archived: false,
        archived_at_ms: None,
    }
}

#[test]
fn public_client_exposes_titles_from_list_and_attach() {
    let server = UnixHarness::start(0);
    let client = server.connect();

    let sessions = client.list_sessions().expect("list sessions");
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].title.as_deref(), Some("Title for persisted-1"));
    assert_eq!(sessions[1].title.as_deref(), Some("Title for persisted-2"));

    let attached = client
        .attach_session("persisted-1")
        .expect("attach session");
    assert_eq!(attached.title.as_deref(), Some("Title for persisted-1"));
}

struct UnixHarness {
    _temp: tempfile::TempDir,
    socket_path: PathBuf,
    sessions: Arc<Mutex<Vec<String>>>,
    clients: Arc<AtomicUsize>,
    include_archived: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl UnixHarness {
    fn start(events_per_attach: usize) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("api.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind mock harness");
        listener.set_nonblocking(true).expect("nonblocking");
        let sessions = Arc::new(Mutex::new(vec!["persisted-1".into(), "persisted-2".into()]));
        let clients = Arc::new(AtomicUsize::new(0));
        let include_archived = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let server_sessions = Arc::clone(&sessions);
        let server_clients = Arc::clone(&clients);
        let server_include_archived = Arc::clone(&include_archived);
        let server_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        server_clients.fetch_add(1, Ordering::AcqRel);
                        let sessions = Arc::clone(&server_sessions);
                        let clients = Arc::clone(&server_clients);
                        let include_archived = Arc::clone(&server_include_archived);
                        std::thread::spawn(move || {
                            serve_connection(socket, sessions, include_archived, events_per_attach);
                            clients.fetch_sub(1, Ordering::AcqRel);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            _temp: temp,
            socket_path,
            sessions,
            clients,
            include_archived,
            stop,
        }
    }

    fn connect(&self) -> JcodeClient {
        JcodeClient::connect(ConnectOptions {
            socket_path: Some(self.socket_path.clone()),
            ensure_runtime: false,
            request_timeout: Some(Duration::from_secs(2)),
            ..Default::default()
        })
        .expect("connect to mock harness")
    }

    fn wait_for_clients(&self, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.clients.load(Ordering::Acquire) == expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "expected {expected} clients, saw {}",
            self.clients.load(Ordering::Acquire)
        );
    }
}

impl Drop for UnixHarness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn serve_connection(
    socket: UnixStream,
    sessions: Arc<Mutex<Vec<String>>>,
    include_archived: Arc<AtomicBool>,
    events_per_attach: usize,
) {
    let mut reader = BufReader::new(socket.try_clone().expect("clone server socket"));
    let mut writer = socket;
    while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
        match frame.request {
            ApiRequest::Hello { .. } => reply(
                frame.id,
                ApiEvent::HelloOk {
                    version: API_VERSION_MAJOR,
                    server: "fake-global/1.0".into(),
                    capabilities: vec!["sessions".into()],
                },
                &mut writer,
            ),
            ApiRequest::ListSessions {
                include_archived: requested,
                ..
            } => {
                include_archived.store(requested, Ordering::Release);
                let listed = sessions
                    .lock()
                    .expect("sessions")
                    .iter()
                    .map(|id| session(id))
                    .collect();
                reply(
                    frame.id,
                    ApiEvent::Sessions { sessions: listed },
                    &mut writer,
                );
            }
            ApiRequest::AttachSession { session_id } => {
                reply(
                    frame.id,
                    ApiEvent::Attached {
                        session: session(&session_id),
                    },
                    &mut writer,
                );
                for index in 0..events_per_attach {
                    push(
                        ApiEvent::TextDelta {
                            session_id: session_id.clone(),
                            text: format!("{session_id}-{index}"),
                        },
                        &mut writer,
                    );
                }
            }
            _ => {}
        }
    }
}

fn reply(id: u64, event: ApiEvent, writer: &mut UnixStream) {
    write_frame(
        writer,
        &ServerFrame {
            v: API_VERSION_MAJOR,
            reply_to: Some(id),
            event,
        },
    )
    .expect("reply");
}

fn push(event: ApiEvent, writer: &mut UnixStream) {
    write_frame(
        writer,
        &ServerFrame {
            v: API_VERSION_MAJOR,
            reply_to: None,
            event,
        },
    )
    .expect("event");
}

#[test]
fn global_events_discovers_existing_and_new_sessions_then_closes_children() {
    let server = UnixHarness::start(1);
    let client = server.connect();
    let stream = client
        .global_events(GlobalEventsOptions {
            discovery_interval: Duration::from_millis(10),
            ..Default::default()
        })
        .expect("global stream");

    let first = stream.next().expect("stream").expect("first event");
    let second = stream.next().expect("stream").expect("second event");
    let ids = [first, second]
        .into_iter()
        .filter_map(|event| match event {
            ApiEvent::TextDelta { session_id, .. } => Some(session_id),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        ids,
        ["persisted-1".to_string(), "persisted-2".to_string()].into()
    );
    assert!(server.include_archived.load(Ordering::Acquire));

    server
        .sessions
        .lock()
        .expect("sessions")
        .push("new-3".into());
    let third = stream.next().expect("stream").expect("third event");
    assert!(matches!(
        third,
        ApiEvent::TextDelta { ref session_id, .. } if session_id == "new-3"
    ));
    server.wait_for_clients(4);

    drop(stream);
    server.wait_for_clients(1);
    drop(client);
    server.wait_for_clients(0);
}

#[test]
fn global_events_reports_bounded_queue_overflow() {
    let server = UnixHarness::start(2);
    server.sessions.lock().expect("sessions").truncate(1);
    let client = server.connect();
    let stream = client
        .global_events(GlobalEventsOptions {
            discovery_interval: Duration::ZERO,
            max_buffered_events: 1,
        })
        .expect("global stream");
    std::thread::sleep(Duration::from_millis(100));
    let error = stream.next().expect_err("overflow must fail loudly");
    assert_eq!(error.code(), "event_buffer_overflow");
}

#[test]
fn missing_launch_binary_is_typed_and_cleans_up_its_temporary_home() {
    let before = std::fs::read_dir(std::env::temp_dir())
        .expect("temp root")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("jcode-sdk-instance-")
        })
        .count();
    let error = match JcodeClient::launch(LaunchOptions {
        binary: Some(PathBuf::from("jcode-definitely-not-installed-sdk-test")),
        inherit_logins: false,
        startup_timeout: Duration::from_millis(100),
        cleanup_timeout: Duration::ZERO,
        ..Default::default()
    }) {
        Ok(_) => panic!("missing binary unexpectedly launched"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "jcode_not_found");
    let after = std::fs::read_dir(std::env::temp_dir())
        .expect("temp root")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("jcode-sdk-instance-")
        })
        .count();
    assert_eq!(after, before, "failed launch leaked a temporary home");
}

#[test]
fn rotating_credentials_are_shared_but_mutable_config_is_copied() {
    let sandbox = tempfile::tempdir().expect("sandbox");
    let user = sandbox.path().join("user");
    let instance = sandbox.path().join("instance");
    std::fs::create_dir(&user).expect("user home");
    std::fs::write(user.join("auth.json"), r#"{"refresh":"v1"}"#).expect("auth");
    std::fs::write(user.join("config.toml"), "[auth]\n").expect("config");
    std::fs::write(user.join("auth-refresh-state.json"), "stale").expect("derived state");

    let inherited = inherit_credentials(&user, &instance).expect("inherit credentials");
    assert!(inherited.contains(&PathBuf::from("auth.json")));
    assert!(
        std::fs::symlink_metadata(instance.join("auth.json"))
            .expect("auth metadata")
            .file_type()
            .is_symlink()
    );
    assert!(
        !std::fs::symlink_metadata(instance.join("config.toml"))
            .expect("config metadata")
            .file_type()
            .is_symlink()
    );
    assert!(!instance.join("auth-refresh-state.json").exists());

    std::fs::write(user.join("auth.json"), r#"{"refresh":"v2"}"#).expect("rotate");
    assert_eq!(
        std::fs::read_to_string(instance.join("auth.json")).expect("shared auth"),
        r#"{"refresh":"v2"}"#
    );
}

#[test]
fn credential_inheritance_rejects_the_users_own_home() {
    let home = tempfile::tempdir().expect("home");
    let error = inherit_credentials(home.path(), home.path()).expect_err("same home must fail");
    assert_eq!(error.code(), "invalid_instance_home");
}
