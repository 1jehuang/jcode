//! Mobile push payload, suppression, and explicit handoff regression tests.
#[test]
fn resumed_turn_cancels_pending_failure_push() {
    let token = super::queue_pending_error("push-recovered-turn");
    super::clear_cancelled("push-recovered-turn");
    assert!(!super::finish_pending_error("push-recovered-turn", &token));
}

#[test]
fn resuming_one_session_does_not_hide_another_failure() {
    let recovered = super::queue_pending_error("push-isolated-recovered");
    let stopped = super::queue_pending_error("push-isolated-stopped");
    super::clear_cancelled("push-isolated-recovered");
    assert!(!super::finish_pending_error(
        "push-isolated-recovered",
        &recovered
    ));
    assert!(super::finish_pending_error(
        "push-isolated-stopped",
        &stopped
    ));
}

#[test]
fn stopped_turn_keeps_failure_push() {
    let token = super::queue_pending_error("push-stopped-turn");
    assert!(super::finish_pending_error("push-stopped-turn", &token));
}

#[test]
fn stale_error_cannot_remove_new_failure_push() {
    let old = super::queue_pending_error("push-replaced-turn");
    let new = super::queue_pending_error("push-replaced-turn");
    assert!(!super::finish_pending_error("push-replaced-turn", &old));
    super::mark_cancelled("push-replaced-turn");
    assert!(!super::finish_pending_error("push-replaced-turn", &new));
    super::take_cancelled("push-replaced-turn");
}

use super::*;

#[test]
fn invalid_summary_regex_is_cached_without_panicking() {
    static CACHE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    assert!(cached_regex(&CACHE, "[").is_none());
    assert!(
        cached_regex(&CACHE, "valid").is_none(),
        "failed initialization is cached"
    );
}

#[test]
fn malformed_error_json_keeps_readable_context() {
    assert_eq!(
        summarize_error("request failed: {not json}: retry later"),
        "request failed: retry later"
    );
    assert_eq!(
        summarize_wake("**Background task stalled** partial"),
        "has been silent for  and may be stuck"
    );
}

#[test]
fn cancelled_marker_survives_poisoned_state() {
    let state = Mutex::new(Some(HashSet::from(["cancelled".to_string()])));
    let result = std::panic::catch_unwind(|| {
        let _guard = state.lock().unwrap();
        panic!("simulate interrupted state update");
    });
    assert!(result.is_err());
    assert!(set_remove(&state, "cancelled"));
    assert!(!set_remove(&state, "cancelled"));
}

#[tokio::test]
async fn explicit_resume_reports_delivery_and_preserves_session_target() {
    use std::io::{Read, Write};
    for (status, body, expected) in [
        (200, r#"{"sent":1,"devices":1}"#, "Mobile notification sent"),
        (
            200,
            r#"{"sent":0,"devices":0}"#,
            "No mobile devices registered",
        ),
        (
            200,
            r#"{"sent":0,"devices":1}"#,
            "No mobile notification was delivered",
        ),
        (200, "not json", "error decoding response body"),
        (401, "{}", "401 Unauthorized"),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let len: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let reason = if status == 200 { "OK" } else { "Unauthorized" };
            write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8(request).unwrap()
        });
        let payload = build_payload(
            PushKind::Wake,
            "session_resume",
            Some("My task"),
            "Tap to resume",
            None,
        );
        let result = send_mobile_resume("test-key", &base, &payload).await;
        let message = match result {
            Ok(message) => message,
            Err(error) => error.to_string(),
        };
        assert!(
            message.contains(expected),
            "{message:?} should contain {expected:?}"
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /push/send "));
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer test-key")
        );
        let body = request.split_once("\r\n\r\n").unwrap().1;
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["session_id"], "session_resume");
        assert_eq!(json["kind"], "wake");
    }
}

#[test]
fn mobile_push_payload_serializes_per_contract() {
    let p = build_payload(
        PushKind::TurnDone,
        "session_x",
        Some(" Fix login "),
        "Done.\n\nAll   tests pass",
        Some(42),
    );
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "kind": "turn_done",
            "session_id": "session_x",
            "session_title": "\u{2705} Done",
            "session_name": "Fix login",
            "body": "Done. All tests pass",
            "turn_id": 42
        })
    );
}

#[test]
fn mobile_push_title_leads_with_origin_repo_and_run_time() {
    let labels = PushLabels {
        origin: Some("XPS 14".to_string()),
        repo: Some("jcode-mobile".to_string()),
        elapsed: Some(Duration::from_secs(252)),
    };
    let p = build_payload_with(
        PushKind::TurnDone,
        "session_x",
        Some("Fix login"),
        "Done.",
        Some(7),
        &labels,
    );
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        serde_json::json!({
            "kind": "turn_done",
            "session_id": "session_x",
            "session_title": "XPS 14 \u{00B7} jcode-mobile \u{00B7} \u{2705} 4m 12s",
            "session_name": "Fix login",
            "body": "Done.",
            "turn_id": 7,
            "origin": "XPS 14",
            "repo": "jcode-mobile",
            "duration_secs": 252
        })
    );
    // Cloud sessions, wakes without a run time, and sessions outside a repo.
    let cloud = PushLabels {
        origin: Some("Cloud".to_string()),
        ..PushLabels::default()
    };
    let p = build_payload_with(PushKind::Wake, "s", Some("Nightly"), "x", None, &cloud);
    assert_eq!(
        p.session_title.as_deref(),
        Some("Cloud \u{00B7} \u{23F0} Update")
    );
    assert_eq!(p.session_name.as_deref(), Some("Nightly"));
    assert_eq!(p.duration_secs, None);
    // A question at the end still reports how long the turn ran.
    let p = build_payload_with(
        PushKind::TurnDone,
        "s",
        Some("T"),
        "Done.\n\nShould I deploy it now?",
        None,
        &labels,
    );
    assert_eq!(p.kind, PushKind::NeedsInput);
    assert_eq!(
        p.session_title.as_deref(),
        Some("XPS 14 \u{00B7} jcode-mobile \u{00B7} \u{2753} Needs input \u{00B7} 4m 12s")
    );
    assert_eq!(p.body, "Should I deploy it now?");
    // Labels never push the title or body past the API's limits.
    let long = "word ".repeat(60);
    let p = build_payload_with(PushKind::TurnError, "s", Some(&long), &long, None, &labels);
    assert_eq!(
        p.session_title.as_deref(),
        Some("XPS 14 \u{00B7} jcode-mobile \u{00B7} \u{274C} Failed \u{00B7} 4m 12s")
    );
    assert!(p.session_name.unwrap().chars().count() <= TITLE_MAX_CHARS);
    assert!(p.body.chars().count() <= BODY_MAX_CHARS);
    assert!(p.body.starts_with("word word"));
}

#[test]
fn mobile_push_failover_prompt_reads_as_plain_words() {
    let prompt = jcode_provider_core::failover::ProviderFailoverPrompt {
        from_provider: "claude".into(),
        from_label: "Anthropic (claude-fox)".into(),
        to_provider: "claude".into(),
        to_label: "Anthropic (claude-otter)".into(),
        target_route: None,
        reason: "503".into(),
        estimated_input_chars: 676703,
        estimated_input_tokens: 169175,
    };
    let p = build_payload(
        PushKind::TurnError,
        "s",
        Some("T"),
        &prompt.to_error_message(),
        None,
    );
    assert_eq!(
        p.body,
        "Anthropic (claude-fox) is unavailable. Open the session to continue on Anthropic (claude-otter)."
    );
}

#[test]
fn mobile_push_repeat_guard_quiets_identical_errors_only() {
    let mut guard = RepeatGuard::default();
    let t0 = Instant::now();
    assert!(!guard.is_repeat("a", "503", t0));
    assert!(guard.is_repeat("a", "503", t0 + Duration::from_secs(60)));
    // Another session, a different error, or any other push in between
    // is news.
    assert!(!guard.is_repeat("b", "503", t0));
    assert!(!guard.is_repeat("a", "401", t0 + Duration::from_secs(61)));
    assert!(guard.is_repeat("a", "401", t0 + Duration::from_secs(62)));
    guard.clear("a");
    assert!(!guard.is_repeat("a", "401", t0 + Duration::from_secs(63)));
    // The quiet window ends on its own.
    assert!(!guard.is_repeat(
        "a",
        "401",
        t0 + Duration::from_secs(64) + REPEAT_ERROR_QUIET
    ));
}

#[test]
fn mobile_push_machine_and_repo_labels() {
    assert_eq!(
        machine_label(Some("XPS 14 DA14260\n"), Some("xps14")),
        "XPS 14"
    );
    assert_eq!(machine_label(Some("MacBook Pro"), None), "MacBook Pro");
    // A product name that is only a SKU, or a board placeholder, says
    // nothing: use the hostname.
    assert_eq!(
        machine_label(Some("20XW00FPUS"), Some("carbon.lan\n")),
        "carbon"
    );
    assert_eq!(
        machine_label(Some("System Product Name"), Some("tower")),
        "tower"
    );
    assert_eq!(machine_label(None, Some("localhost")), "Local");
    assert_eq!(machine_label(None, None), "Local");

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("my-repo");
    let nested = repo.join("crates").join("core");
    std::fs::create_dir_all(&nested).unwrap();
    // No repository above it: the directory names itself.
    assert_eq!(repo_label(&nested).as_deref(), Some("core"));
    std::fs::create_dir(repo.join(".git")).unwrap();
    assert_eq!(repo_label(&nested).as_deref(), Some("my-repo"));
    assert_eq!(repo_label(&repo).as_deref(), Some("my-repo"));
    assert_eq!(repo_label(std::path::Path::new("/")), None);
    if let Some(home) = dirs::home_dir() {
        assert_eq!(repo_label(&home), None);
    }
}

#[test]
fn mobile_push_payload_defaults_and_truncation() {
    let p = build_payload(PushKind::NeedsInput, "s", Some("  "), "", None);
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["kind"], "needs_input");
    assert_eq!(v["session_title"], "\u{2753} Needs input");
    assert!(v.get("session_name").is_none());
    assert!(v.get("turn_id").is_none());
    assert_eq!(v["body"], "Waiting for your input");
    let long = "a".repeat(500);
    let p = build_payload(PushKind::TurnError, "s", None, &long, None);
    assert_eq!(p.body.chars().count(), BODY_MAX_CHARS);
    assert_eq!(
        serde_json::to_value(PushKind::Wake).unwrap(),
        serde_json::json!("wake")
    );
}

#[test]
fn mobile_push_turn_id_is_js_safe() {
    let p = build_payload(
        PushKind::TurnDone,
        "s",
        None,
        "ok",
        Some(8338987288606337570),
    );
    assert!(p.turn_id.unwrap() <= (1 << 53) - 1);
    assert_eq!(
        build_payload(PushKind::TurnDone, "s", None, "ok", Some(42)).turn_id,
        Some(42)
    );
}

#[test]
fn mobile_push_answer_uses_lead_paragraph_as_plain_text() {
    let answer = "## Summary\n\nFixed the **login** bug in `auth.rs`; all 12 tests pass.\n\n- detail one\n- detail two\n\n```rust\nfn x() {}\n```";
    let p = build_payload(PushKind::TurnDone, "s", Some("Fix `login`"), answer, None);
    assert_eq!(p.body, "Fixed the login bug in auth.rs; all 12 tests pass.");
    assert_eq!(p.session_title.as_deref(), Some("\u{2705} Done"));
    assert_eq!(p.session_name.as_deref(), Some("Fix login"));
    for (kind, body, marker) in [
        (PushKind::TurnDone, "done", "\u{2705} Done"),
        (PushKind::TurnDone, "Should I push?", "\u{2753} Needs input"),
        (PushKind::TurnError, "boom", "\u{274C} Failed"),
        (PushKind::Wake, "bash completed", "\u{23F0} Update"),
    ] {
        let t = build_payload(kind, "s", Some("T"), body, None)
            .session_title
            .unwrap();
        assert_eq!(t, marker);
    }
    let bullets = "- Updated parser\n- Added tests\n- Shipped v2 to production servers today";
    assert_eq!(
        build_payload(PushKind::TurnDone, "s", None, bullets, None).body,
        "Updated parser. Added tests. Shipped v2 to production servers today"
    );
}

#[test]
fn mobile_push_trailing_question_becomes_needs_input() {
    let p = build_payload(
        PushKind::TurnDone,
        "s",
        None,
        "I found two options.\n\nShould I use OAuth or a password?",
        None,
    );
    assert_eq!(p.kind, PushKind::NeedsInput);
    assert_eq!(p.body, "Should I use OAuth or a password?");
    let p = build_payload(
        PushKind::TurnDone,
        "s",
        None,
        "Done. Want me to deploy it?",
        None,
    );
    assert_eq!(
        (p.kind, p.body.as_str()),
        (PushKind::NeedsInput, "Want me to deploy it?")
    );
}

#[test]
fn mobile_push_errors_and_wakes_are_readable() {
    let e = r#"Provider request failed: HTTP 529: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    assert_eq!(
        build_payload(PushKind::TurnError, "s", None, e, None).body,
        "Overloaded (Provider request failed: HTTP 529)"
    );
    let bg = "**Background task** `abc123` · `bash` · ✓ completed · 754.2s · exit 0\n\n```text\nCompiling...\ntest result: ok. 42 passed\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"abc123\"`";
    assert_eq!(
        build_payload(PushKind::Wake, "s", None, bg, None).body,
        "bash completed in 12m 34s: test result: ok. 42 passed"
    );
    let long = "word ".repeat(60);
    let p = build_payload(PushKind::Wake, "s", None, &long, None);
    assert!(p.body.ends_with("word…"));
}

#[test]
fn mobile_push_suppression_window() {
    let now = Instant::now();
    let mut s = Suppression::default();
    assert!(!s.is_suppressed(now));
    s.record_reply(Some(0), now);
    assert!(s.is_suppressed(now + Duration::from_secs(60)));
    assert!(!s.is_suppressed(now + NO_DEVICES_SUPPRESS));
    s.record_reply(Some(0), now);
    s.record_reply(Some(2), now);
    assert!(!s.is_suppressed(now));
    s.record_reply(None, now);
    assert!(!s.is_suppressed(now));
}

#[test]
fn mobile_push_opt_out_and_threshold() {
    assert!(env_opted_out(Some("0")));
    assert!(env_opted_out(Some("false")));
    assert!(!env_opted_out(Some("1")));
    assert!(!env_opted_out(None));
    let sid = "session_mobile_push_test_threshold";
    assert!(!should_push_turn_done(sid, Duration::ZERO));
    assert!(!should_push_turn_done(
        sid,
        MIN_TURN_DURATION_FOR_PUSH - Duration::from_nanos(1)
    ));
    assert!(should_push_turn_done(sid, MIN_TURN_DURATION_FOR_PUSH));
    assert_eq!(threshold_for(true), Duration::ZERO);
}

#[test]
fn mobile_push_ios_sessions_and_cancel_markers() {
    let sid = "session_mobile_push_test_ios";
    assert!(!should_push_turn_done(sid, Duration::from_secs(1)));
    note_client_attached(sid, "jcode-desktop/1.0");
    assert!(!should_push_turn_done(sid, Duration::from_secs(1)));
    note_client_attached(sid, "jcode-ios/0.3.0");
    assert!(should_push_turn_done(sid, Duration::ZERO));

    let c = "session_mobile_push_test_cancel";
    assert!(!take_cancelled(c));
    mark_cancelled(c);
    assert!(take_cancelled(c));
    assert!(!take_cancelled(c));
    mark_cancelled(c);
    clear_cancelled(c);
    assert!(!take_cancelled(c));
}
