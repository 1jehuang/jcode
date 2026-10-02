use super::*;
use crate::memory::{MemoryCategory, MemoryEntry};

#[test]
fn extraction_transcript_omits_internal_system_reminders() {
    let messages = vec![
        crate::message::Message::user(
            "<system-reminder>\n# Session Context\nHardware: private\n</system-reminder>",
        ),
        crate::message::Message::user("Remember that tests use a temporary database."),
        crate::message::Message::assistant_text("Understood."),
    ];
    let transcript = build_transcript_for_extraction(&messages);
    assert!(!transcript.contains("Hardware: private"));
    assert!(transcript.contains("tests use a temporary database"));
    assert!(transcript.contains("Understood"));
}

struct TestEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl TestEnv {
    fn set(values: &[(&'static str, &str)]) -> Self {
        let old = values
            .iter()
            .map(|(key, value)| {
                let old = std::env::var_os(key);
                crate::env::set_var(key, value);
                (*key, old)
            })
            .collect();
        Self(old)
    }
}
impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[tokio::test]
async fn missing_jev_access_clears_pending_without_fallback() {
    let _lock = crate::storage::lock_test_env();
    let _env = TestEnv::set(&[("JCODE_MEMORY_JEV_PROVIDER", "disabled-for-test")]);
    let sid = "jev-agent-no-fallback";
    memory::set_pending_memory(sid, "stale result".into(), 1);
    let (_, rx) = mpsc::channel(1);
    let mut agent = MemoryAgent::new(rx);
    agent
        .process_context(sid, &[crate::message::Message::user("current query")])
        .await
        .unwrap();
    assert!(!memory::has_pending_memory(sid));
}

#[test]
fn manager_without_working_dir_does_not_infer_process_project() {
    let manager = manager_for_working_dir(None);
    assert!(manager.load_project_graph().unwrap().memories.is_empty());
}

/// Exercises the real credential resolver, subscription capability check, HTTP
/// Decisions adapter, local stores, selection, and pending-injection boundary.
/// No provider calls or credentials leave this loopback fixture.
#[tokio::test]
async fn automatic_recall_uses_jev_http_without_embeddings_or_sidecar() {
    use std::io::{Read, Write};
    let _lock = crate::storage::lock_test_env();
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let _env = TestEnv::set(&[
        ("JCODE_HOME", dir.path().to_str().unwrap()),
        ("JCODE_MEMORY_JEV_PROVIDER", "jcode"),
        ("JCODE_API_KEY", "jcode_test_only_never_a_real_key"),
        ("JCODE_API_BASE", &base),
    ]);
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut decisions_seen = false;
        while !decisions_seen && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let (header_end, length) = loop {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(offset) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..offset]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    break (offset + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let response = if headers.starts_with("GET /v1/me ") {
                serde_json::json!({"capabilities":{"memory_jev":true}})
            } else {
                assert!(headers.starts_with("POST /v1/decisions "));
                let body: serde_json::Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                assert_eq!(body["model"], "typesafe/jev-1.13");
                let state: serde_json::Value =
                    serde_json::from_str(body["state"].as_str().unwrap()).unwrap();
                let mut answers = serde_json::Map::new();
                for (id, question) in body["questions"].as_object().unwrap() {
                    assert_eq!(question["type"], "noul");
                    assert!(state["candidates"][id].get("embedding").is_none());
                    let relevant = state["candidates"][id]["content"]
                        .as_str()
                        .unwrap()
                        .contains("cargo test");
                    answers.insert(
                        id.clone(),
                        serde_json::json!({"type":"noul","noul":if relevant {0.98} else {0.01}}),
                    );
                }
                decisions_seen = true;
                serde_json::json!({"answers":answers})
            };
            let body = serde_json::to_vec(&response).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
        }
        assert!(decisions_seen, "no Decisions request reached fixture");
    });
    let manager = MemoryManager::new().with_project_dir("/jev-project");
    let id = manager
        .remember_project(MemoryEntry::new(
            MemoryCategory::Fact,
            "Run cargo test to test this Rust project.",
        ))
        .unwrap();
    manager
        .remember_project(MemoryEntry::new(
            MemoryCategory::Fact,
            "The garden has red flowers.",
        ))
        .unwrap();
    assert!(
        manager
            .list_all()
            .unwrap()
            .iter()
            .all(|m| m.embedding.is_none())
    );
    let (_, rx) = mpsc::channel(1);
    let mut agent = MemoryAgent::new(rx);
    let sid = "jev-agent-http-acceptance";
    agent.sessions.insert(
        sid.into(),
        SessionState {
            working_dir: Some("/jev-project".into()),
            ..Default::default()
        },
    );
    let result = agent
        .process_context(
            sid,
            &[crate::message::Message::user(
                "How do I test this Rust project?",
            )],
        )
        .await;
    server.join().unwrap();
    result.unwrap();
    let pending = memory::take_pending_memory_for_project(sid, Some("/jev-project"))
        .expect("judged memory ready for next turn");
    assert_eq!(pending.memory_ids, vec![id]);
    assert!(pending.prompt.contains("cargo test"));
    assert!(!pending.prompt.contains("red flowers"));
    assert!(!dir.path().join("embeddings").exists());
    memory::clear_pending_memory(sid);
    memory::clear_injected_memories(sid);
}
