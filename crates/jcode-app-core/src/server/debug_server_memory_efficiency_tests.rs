//! No-LLM fixtures for the server memory diagnostic's session-only fast path.
use super::*;

fn diagnostic_event(id: u64, message: &str) -> SwarmEvent {
    SwarmEvent {
        id,
        session_id: format!("fixture-{id}"),
        session_name: Some("測試".into()),
        swarm_id: None,
        event: crate::server::SwarmEventType::Notification {
            notification_type: "info".into(),
            message: message.into(),
        },
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    }
}

#[test]
fn diagnostic_event_byte_count_matches_debug_bytes() {
    for text in ["", "peer joined", "\0\n\t\\\"🦀é", &"long\n".repeat(32768)] {
        let e = diagnostic_event(123, text);
        assert_eq!(estimate_swarm_event_bytes(&e), format!("{e:?}").len());
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "optimized alternating per-thread CPU benchmark"]
fn diagnostic_event_byte_count_thread_cpu() {
    use std::hint::black_box;
    fn cpu_ns() -> i128 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: ts is writable for one timespec.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) },
            0
        );
        i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec)
    }
    let old = |e: &SwarmEvent| format!("{e:?}").len();
    for (name, text, n, iters) in [
        ("common", "join".into(), 64, 400),
        ("escaped", "\0\n🦀".repeat(64), 64, 100),
        ("large", "long\n".repeat(16384), 4, 12),
    ] {
        let events: Vec<_> = (0..n).map(|i| diagnostic_event(i, &text)).collect();
        let functions: [fn(&SwarmEvent) -> usize; 2] = [old, estimate_swarm_event_bytes];
        let mut times = [Vec::new(), Vec::new()];
        for round in 0..16 {
            for index in [round % 2, (round + 1) % 2] {
                let start = cpu_ns();
                for _ in 0..iters {
                    black_box(
                        black_box(&events)
                            .iter()
                            .map(functions[index])
                            .sum::<usize>(),
                    );
                }
                times[index].push((cpu_ns() - start) / iters);
            }
        }
        for t in &mut times {
            t.sort_unstable();
        }
        eprintln!(
            "{name}: median thread CPU ns/history old={} new={}",
            (times[0][7] + times[0][8]) / 2,
            (times[1][7] + times[1][8]) / 2
        );
    }
}

struct PrivateHome {
    _dir: tempfile::TempDir,
    home: Option<std::ffi::OsString>,
    test_session: Option<std::ffi::OsString>,
}

impl PrivateHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = std::env::var_os("JCODE_HOME");
        let test_session = std::env::var_os("JCODE_TEST_SESSION");
        crate::env::set_var("JCODE_HOME", dir.path());
        crate::env::set_var("JCODE_TEST_SESSION", "1");
        Self {
            _dir: dir,
            home,
            test_session,
        }
    }
}

impl Drop for PrivateHome {
    fn drop(&mut self) {
        for (name, value) in [
            ("JCODE_HOME", self.home.take()),
            ("JCODE_TEST_SESSION", self.test_session.take()),
        ] {
            match value {
                Some(value) => crate::env::set_var(name, value),
                None => crate::env::remove_var(name),
            }
        }
    }
}

struct NoRequests;

#[async_trait::async_trait]
impl crate::provider::Provider for NoRequests {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        anyhow::bail!("memory diagnostics must never call a provider")
    }

    fn name(&self) -> &str {
        "memory-diagnostic-test"
    }

    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(Self)
    }
}

fn populated_agent(index: usize) -> Agent {
    let mut session =
        crate::session::Session::create_with_id(format!("memory-diagnostic-{index}"), None, None);
    for turn in 0..8 {
        session.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: format!("turn {turn}: {}", "escaped\n\"界".repeat(512)),
                cache_control: None,
            }],
        );
    }
    session.compaction = Some(Arc::new(crate::session::StoredCompactionState {
        summary_text: "summary\n\"界".repeat(128),
        openai_encrypted_content: Some("encrypted".repeat(1024).into()),
        covers_up_to_turn: 2,
        original_turn_count: 8,
        compacted_count: 2,
    }));
    Agent::new_with_session(
        Arc::new(NoRequests),
        crate::tool::Registry::empty(),
        session,
        None,
    )
}

#[test]
fn session_only_profile_matches_full_agent_profile_and_removes_repeated_process_samples() {
    // The private home and test-session flag isolate this fixture on either
    // the local or hosted test runner. No model requests or daemon are needed.
    let _env = crate::storage::lock_test_env();
    let _home = PrivateHome::new();
    let agents: Vec<_> = (0..8).map(populated_agent).collect();
    let expected: Vec<_> = agents
        .iter()
        .map(|agent| agent.debug_memory_profile()["session"].clone())
        .collect();
    let old_start = Instant::now();
    for _ in 0..8 {
        for agent in &agents {
            std::hint::black_box(agent.debug_memory_profile()["session"].clone());
        }
    }
    let old_elapsed = old_start.elapsed();
    let history_before = crate::process_memory::history(128);
    assert_eq!(
        history_before
            .iter()
            .take(64)
            .filter(|entry| entry.source == "agent:memory")
            .count(),
        64,
        "the legacy 8-session, 8-pass loop records one process sample per profile"
    );

    let new_start = Instant::now();
    for _ in 0..8 {
        for agent in &agents {
            std::hint::black_box(agent.session_for_split().debug_memory_profile());
        }
    }
    let new_elapsed = new_start.elapsed();
    for (agent, expected) in agents.iter().zip(&expected) {
        let actual = agent.session_for_split().debug_memory_profile();
        assert_eq!(&actual, expected);
        // In particular, compact snapshots cannot substitute for these
        // capacity and encrypted-payload fields in server:memory.
        assert!(
            actual["compaction"]["encrypted_content_bytes"]
                .as_u64()
                .unwrap()
                > 0
        );
        let mut borrowed = CompactionMemoryTotals::default();
        borrowed.observe("fixture", &actual);
        let mut legacy = CompactionMemoryTotals::default();
        legacy.observe("fixture", expected);
        assert_eq!(borrowed.into_json(), legacy.into_json());
    }
    let history_after = crate::process_memory::history(128);
    assert_eq!(
        serde_json::to_value(history_before).unwrap(),
        serde_json::to_value(history_after).unwrap(),
        "session-only profiling must not sample or append process history"
    );
    eprintln!(
        "memory diagnostic: 8 populated sessions x 8 passes legacy={old_elapsed:?}; session-only={new_elapsed:?}; process samples legacy=64, new=0"
    );
}

async fn memory_payload(sessions: &SessionAgents) -> serde_json::Value {
    build_server_memory_payload(
        sessions,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(ClientDebugState::default())),
        &ServerIdentity {
            id: "fixture".into(),
            name: "fixture".into(),
            icon: "test".into(),
            git_hash: "test".into(),
            version: "test".into(),
        },
        Instant::now(),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &FileTouchService::new(),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await
}

#[tokio::test]
async fn server_memory_preserves_session_output_and_skips_busy_agents() {
    let _env = crate::storage::lock_test_env();
    let _home = PrivateHome::new();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::new());
    let agent = Arc::new(Mutex::new(populated_agent(0)));
    let legacy = agent.lock().await.debug_memory_profile()["session"].clone();
    sessions
        .register_new("memory-diagnostic-0".into(), Arc::clone(&agent))
        .await
        .unwrap();
    let payload = memory_payload(&sessions).await;
    let output = &payload["sessions"];
    assert_eq!(output["live_count"], 1);
    assert_eq!(output["locked_count"], 1);
    assert_eq!(output["contended_count"], 0);
    assert_eq!(output["total_json_bytes"], legacy["totals"]["json_bytes"]);
    assert_eq!(output["total_message_count"], legacy["messages"]["count"]);
    assert_eq!(
        output["total_payload_text_bytes"],
        legacy["totals"]["payload_text_bytes"]
    );
    let mut compaction = CompactionMemoryTotals::default();
    compaction.observe("memory-diagnostic-0", &legacy);
    assert_eq!(output["compaction"], compaction.into_json());
    assert_eq!(payload["history"][0]["source"], "server:memory");

    let _busy = agent.lock().await;
    let payload =
        tokio::time::timeout(std::time::Duration::from_secs(2), memory_payload(&sessions))
            .await
            .expect("busy agent must never block server:memory");
    assert_eq!(payload["sessions"]["live_count"], 1);
    assert_eq!(payload["sessions"]["locked_count"], 0);
    assert_eq!(payload["sessions"]["contended_count"], 1);
    assert_eq!(payload["sessions"]["total_json_bytes"], 0);
    assert_eq!(
        payload["sessions"]["top_by_json_bytes"],
        serde_json::json!([])
    );
}
