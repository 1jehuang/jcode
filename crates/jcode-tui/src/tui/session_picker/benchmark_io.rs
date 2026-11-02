//! Offline disk-backed resume benchmarks. Run ignored tests serially with --nocapture.
//! Newly written fixtures are OS-page-cache warm, never claimed as cold disk reads.
//! The 16 ms polling below models observation cadence, not the live terminal loop.
//! No SSH/network case: remote transport, authentication and remote cache need a live host.
use super::*;
use serde_json::{Value, json};
use std::time::Instant;

const POLL: Duration = Duration::from_millis(16);
const LIMIT: Duration = Duration::from_secs(10);
const NEEDLE: &str = "middleonlyplatypus";

struct HomeGuard {
    home: Option<std::ffi::OsString>,
    runtime: Option<std::ffi::OsString>,
}
impl HomeGuard {
    fn install(path: &Path) -> Self {
        let home = std::env::var_os("JCODE_HOME");
        let runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        let runtime_path = path.join("runtime");
        std::fs::create_dir_all(&runtime_path).unwrap();
        crate::env::set_var("JCODE_HOME", path);
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime_path);
        invalidate_session_list_cache();
        crate::config::Config::invalidate_cache();
        Self { home, runtime }
    }
}
impl Drop for HomeGuard {
    fn drop(&mut self) {
        invalidate_session_list_cache();
        crate::config::Config::invalidate_cache();
        if let Some(old) = self.home.take() {
            crate::env::set_var("JCODE_HOME", old);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        if let Some(old) = self.runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", old);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

fn fixtures(home: &Path, count: usize) -> Vec<SessionInfo> {
    let id = "session_benchmark_1793661873000";
    let mut local = Session::create_with_id(id.into(), None, Some("Benchmark session".into()));
    local.is_debug = false;
    local.working_dir = Some(
        home.join("synthetic-workspace")
            .to_string_lossy()
            .into_owned(),
    );
    for i in 0..count {
        let text = format!(
            "{} turn {i} {}",
            "ordinary transcript prose ".repeat(40),
            if i == count / 2 { NEEDLE } else { "padding" }
        );
        local.append_stored_message(crate::session::StoredMessage {
            id: format!("message-{i}"),
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text,
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
    }
    local.save().expect("save real local snapshot");
    let dir = home.join("external/.codex/sessions/2026/11/02");
    std::fs::create_dir_all(&dir).unwrap();
    let mut codex = json!({"type":"session_meta","payload":{"id":"benchmark-codex","timestamp":"2026-11-02T23:24:33Z","source":"cli"}}).to_string();
    codex.push('\n');
    for i in 0..count {
        let text = format!(
            "{} turn {i} {}",
            "ordinary transcript prose ".repeat(40),
            if i == count / 2 { NEEDLE } else { "padding" }
        );
        codex.push_str(&json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}).to_string());
        codex.push('\n');
    }
    std::fs::write(dir.join("rollout-benchmark.jsonl"), codex).unwrap();
    invalidate_session_list_cache();
    // Bypass external-session user preferences without changing configuration or scanning HOME.
    let mut sessions: Vec<_> = load_sessions()
        .unwrap()
        .into_iter()
        .filter(|s| s.id == id)
        .collect();
    sessions.extend(
        loading::load_external_cli_sessions_grouped(
            crate::tui::app::onboarding_flow::ExternalCli::Codex,
        )
        .1
        .into_iter()
        .filter(|s| s.id == "codex:benchmark-codex"),
    );
    sessions
}

#[derive(Default)]
struct Measurements {
    wall: Vec<Duration>,
    active: Vec<Duration>,
    wait: Vec<Duration>,
    failures: usize,
    timeouts: usize,
}
impl Measurements {
    fn record(
        &mut self,
        wall: Duration,
        active: Duration,
        wait: Duration,
        ok: bool,
        timeout: bool,
    ) {
        if timeout {
            self.timeouts += 1;
        } else if !ok {
            self.failures += 1;
        } else {
            self.wall.push(wall);
            self.active.push(active);
            self.wait.push(wait);
        }
    }
    fn row(&self, scenario: &str, source: &str, attempts: usize) -> Value {
        json!({"resume_bench":1,"scenario":scenario,"cache":"new_picker_os_page_cache_warm",
            "source":source,"terminal":null,"terminal_scope":"no_render",
            "samples":self.wall.len(),"attempts":attempts,"failures":self.failures,"timeouts":self.timeouts,
            "sample_quality":if self.wall.len() < 100 { "small_sample_descriptive_not_tail_estimate" } else { "empirical_nearest_rank_percentiles" },
            "poll_model_ms":16,"wall_ms":distribution(&self.wall),
            "active_caller_wall_ms_not_cpu":distribution(&self.active),"poll_sleep_wall_ms":distribution(&self.wait),
            "cpu_time":"not_measured","typing_model":"zero_delay_burst_observed_after_final_key","remote_ssh":"not_measured_offline"})
    }
}
fn distribution(values: &[Duration]) -> Value {
    if values.is_empty() {
        return json!({"p50":null,"p95":null,"p99":null,"max":null});
    }
    let mut v: Vec<_> = values
        .iter()
        .map(Duration::as_secs_f64)
        .map(|s| s * 1000.0)
        .collect();
    v.sort_by(f64::total_cmp);
    let q = |p: usize| v[(v.len() * p).div_ceil(100).saturating_sub(1)];
    json!({"p50":q(50),"p95":q(95),"p99":q(99),"max":v.last()})
}

fn poll_until(mut done: impl FnMut() -> bool) -> (Duration, Duration, bool) {
    let start = Instant::now();
    let mut wait = Duration::ZERO;
    loop {
        if done() {
            return (start.elapsed(), wait, false);
        }
        if start.elapsed() >= LIMIT {
            return (start.elapsed(), wait, true);
        }
        let sleeping = Instant::now();
        std::thread::sleep(POLL);
        wait += sleeping.elapsed();
    }
}
fn type_query(picker: &mut SessionPicker, query: &str) {
    picker.activate_search();
    for ch in query.chars() {
        picker
            .handle_search_key(KeyCode::Char(ch), KeyModifiers::NONE)
            .unwrap();
    }
}

#[test]
#[ignore = "manual disk-backed resume latency benchmark, serial --nocapture"]
fn resume_bench_disk_preview_and_search() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = HomeGuard::install(temp.path());
    let sessions = fixtures(temp.path(), 2048);
    assert_eq!(
        sessions.len(),
        2,
        "both actual source loaders must discover fixtures"
    );
    let samples = benchmark_matrix::sample_count();
    for session in sessions {
        let source = if session.source == SessionSource::Jcode {
            "jcode_snapshot"
        } else {
            "codex_jsonl"
        };
        let mut preview = Measurements::default();
        let mut metadata = Measurements::default();
        let mut search = Measurements::default();
        let mut cancellation = Measurements::default();
        for _ in 0..samples {
            let mut picker = SessionPicker::new(vec![session.clone()]);
            let start = Instant::now();
            picker.ensure_selected_preview_loading();
            let (_, wait, timeout) = poll_until(|| {
                picker.poll_preview_load();
                picker.pending_preview_load.is_none()
            });
            let elapsed = start.elapsed();
            let ok = picker
                .selected_session()
                .is_some_and(|s| !s.messages_preview.is_empty());
            preview.record(elapsed, elapsed.saturating_sub(wait), wait, ok, timeout);
            // Keep sandbox ownership until the optional worker has finished too.
            // This drain is outside transcript-data-readiness measurement.
            if let Some(workspace) = picker.pending_workspace_load.take() {
                // Fixture work is strictly local and finite. Drain to disconnect
                // even after a measured timeout so no worker outlives JCODE_HOME.
                let _ = workspace.receiver.recv();
            }

            // A new picker keeps preview text from satisfying full-text queries in memory.
            let mut picker = SessionPicker::new(vec![session.clone()]);
            let start = Instant::now();
            type_query(&mut picker, "session");
            let elapsed = start.elapsed();
            metadata.record(
                elapsed,
                elapsed,
                Duration::ZERO,
                !picker.visible_sessions.is_empty(),
                false,
            );
            let mut picker = SessionPicker::new(vec![session.clone()]);
            let start = Instant::now();
            type_query(&mut picker, NEEDLE);
            let (_, wait, timeout) = poll_until(|| {
                picker.poll_transcript_search();
                !picker.transcript_search.pending()
            });
            let elapsed = start.elapsed();
            search.record(
                elapsed,
                elapsed.saturating_sub(wait),
                wait,
                picker.transcript_search.contains_literal(&session.id),
                timeout,
            );

            // Real query replacement while the old scan is still debounced.
            // This measures UI cancellation state, not worker termination latency.
            let start = Instant::now();
            picker.search_query = "absentoldquery".into();
            picker.rebuild_items();
            picker.search_query.clear();
            picker.rebuild_items();
            let elapsed = start.elapsed();
            cancellation.record(
                elapsed,
                elapsed,
                Duration::ZERO,
                !picker.transcript_search.pending() && picker.visible_sessions.len() == 1,
                false,
            );
        }
        for (name, stats) in [
            ("disk_to_transcript_data_ready", preview),
            ("typing_to_first_metadata_results", metadata),
            (
                "typing_to_complete_transcript_matches_including_debounce",
                search,
            ),
            (
                "query_replacement_to_cancelled_debounce_state",
                cancellation,
            ),
        ] {
            println!("{}", stats.row(name, source, samples));
            assert_eq!(
                stats.failures + stats.timeouts,
                0,
                "{name}: failed samples are not successes"
            );
        }
    }
}

#[test]
fn benchmark_io_distribution_and_failures_are_explicit() {
    assert!(distribution(&[])["p50"].is_null());
    let d = distribution(&[Duration::from_millis(3), Duration::from_millis(1)]);
    assert_eq!(d["p50"], 1.0);
    assert_eq!(d["p99"], 3.0);
    let mut stats = Measurements::default();
    stats.record(Duration::ZERO, Duration::ZERO, Duration::ZERO, false, false);
    stats.record(LIMIT, Duration::ZERO, LIMIT, false, true);
    let row = stats.row("test", "test", 2);
    assert_eq!(row["samples"], 0);
    assert_eq!(row["failures"], 1);
    assert_eq!(row["timeouts"], 1);
    assert!(row["wall_ms"]["max"].is_null());
}

#[test]
fn benchmark_io_real_fixture_load_and_search() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = HomeGuard::install(temp.path());
    let sessions = fixtures(temp.path(), 256);
    assert_eq!(sessions.len(), 2);
    for session in sessions {
        assert!(
            !session.search_index.contains(NEEDLE),
            "needle must require disk scan"
        );
        let preview = SessionPicker::load_preview_for_target(
            session.resume_target.clone(),
            session.external_path.clone(),
        )
        .unwrap();
        assert!(!preview.messages.is_empty());
        let id = session.id.clone();
        let mut picker = SessionPicker::new(vec![session]);
        type_query(&mut picker, NEEDLE);
        let (_, _, timeout) = poll_until(|| {
            picker.poll_transcript_search();
            !picker.transcript_search.pending()
        });
        assert!(!timeout);
        assert_eq!(picker.visible_sessions.len(), 1);
        assert!(picker.transcript_search.contains_literal(&id));
    }
}
