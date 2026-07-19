use super::{
    build_resume_command, effort_display_label, extract_bracketed_system_message,
    format_countdown_until, gather_ambient_info_inner, inferred_reasoning_efforts,
    partition_queued_messages, pretty_model_display_name, resume_invocation_args,
};
use crate::ambient::{AmbientManager, Priority, ScheduleRequest, ScheduleTarget};
use crate::terminal_launch::{detected_resume_terminal, shell_command};
use crate::tui::session_picker::ResumeTarget;
use chrono::{Duration as ChronoDuration, Utc};

struct EnvVarGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_value(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }

    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[test]
fn extract_bracketed_system_message_strips_wrapper() {
    let parsed = extract_bracketed_system_message(
        "[SYSTEM: Your session was interrupted. Continue immediately.]",
    );
    assert_eq!(
        parsed.as_deref(),
        Some("Your session was interrupted. Continue immediately.")
    );
}

#[test]
fn partition_queued_messages_moves_system_messages_into_reminders() {
    let (user_messages, reminder, display_system_messages) = partition_queued_messages(
        vec![
            "[SYSTEM: Continue where you left off.]".to_string(),
            "normal user input".to_string(),
        ],
        vec!["hidden reminder".to_string()],
    );

    assert_eq!(user_messages, vec!["normal user input"]);
    assert_eq!(
        display_system_messages,
        vec!["Continue where you left off."]
    );
    assert_eq!(
        reminder.as_deref(),
        Some("hidden reminder\n\nContinue where you left off.")
    );
}

#[test]
fn inferred_reasoning_efforts_use_provider_specific_order_and_max_semantics() {
    assert_eq!(
        inferred_reasoning_efforts(Some("openai"), Some("gpt-5.4")),
        vec![
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "swarm",
            "swarm-deep"
        ],
        "OpenAI exposes max as a real Responses API effort level"
    );
    assert_eq!(
        inferred_reasoning_efforts(Some("openai-compatible:custom"), Some("o5-mini")),
        jcode_provider_core::OPENAI_SELECTABLE_EFFORTS,
        "direct compatible routes must preserve OpenAI max instead of aliasing it to xhigh"
    );
    assert_eq!(
        inferred_reasoning_efforts(Some("anthropic"), Some("claude-sonnet-4-6")),
        vec![
            "none",
            "low",
            "medium",
            "high",
            "max",
            "swarm",
            "swarm-deep"
        ]
    );
    assert_eq!(
        inferred_reasoning_efforts(Some("anthropic"), Some("claude-opus-4-7")),
        vec![
            "none",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "swarm",
            "swarm-deep"
        ]
    );
    assert_eq!(
        inferred_reasoning_efforts(Some("openrouter"), Some("anthropic/claude-sonnet-4.6")),
        vec![
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "swarm",
            "swarm-deep"
        ]
    );
    assert_eq!(
        inferred_reasoning_efforts(Some("openrouter"), Some("deepseek/deepseek-r1")),
        vec![
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "swarm",
            "swarm-deep"
        ],
        "OpenRouter uses unified reasoning where max is only an alias, not a cycle level"
    );
    assert_eq!(
        inferred_reasoning_efforts(Some("deepseek"), Some("deepseek-v4-pro")),
        vec![
            "none",
            "low",
            "medium",
            "high",
            "max",
            "swarm",
            "swarm-deep"
        ],
        "DeepSeek direct keeps max as a real provider level"
    );
    assert!(inferred_reasoning_efforts(Some("ollama"), Some("llama3")).is_empty());
}

#[test]
fn swarm_effort_display_labels_are_marked_beta() {
    assert_eq!(
        effort_display_label("swarm"),
        "Swarm (light fan-out) [Beta]"
    );
    assert_eq!(
        effort_display_label("swarm-deep"),
        "Swarm Deep (Max + task graph) [Beta]"
    );
    assert_eq!(effort_display_label("high"), "High");
}

#[cfg(unix)]
#[test]
fn detected_resume_terminal_recognizes_handterm_term_program() {
    let _env_lock = crate::storage::lock_test_env();
    let _guard = EnvVarGuard::set_value("TERM_PROGRAM", "handterm");
    assert_eq!(detected_resume_terminal().as_deref(), Some("handterm"));
}

#[cfg(unix)]
#[test]
fn shell_command_quotes_single_quotes_for_handterm_exec() {
    let command = shell_command(&[
        "/tmp/jcode binary".to_string(),
        "--resume".to_string(),
        "session'quote".to_string(),
    ]);
    assert_eq!(
        command,
        "'/tmp/jcode binary' '--resume' 'session'\"'\"'quote'"
    );
}

#[test]
fn resume_invocation_args_includes_socket_when_present() {
    let args = resume_invocation_args("ses_123", Some("/tmp/jcode-test.sock"));
    assert_eq!(
        args,
        vec![
            "--fresh-spawn".to_string(),
            "--resume".to_string(),
            "ses_123".to_string(),
            "--socket".to_string(),
            "/tmp/jcode-test.sock".to_string()
        ]
    );
}

#[test]
fn resume_invocation_args_omits_blank_socket() {
    let args = resume_invocation_args("ses_123", Some("   "));
    assert_eq!(
        args,
        vec![
            "--fresh-spawn".to_string(),
            "--resume".to_string(),
            "ses_123".to_string()
        ]
    );
}

/// Pin JCODE_HOME to a tempdir containing a `builds/current/jcode` binary so
/// `launch_client_executable()` resolves deterministically, independent of
/// whether the developer machine has a published local build channel and of
/// other tests mutating JCODE_HOME in parallel. Returns the guards that keep
/// the environment pinned for the duration of the test.
fn pinned_resume_test_home() -> (
    std::sync::MutexGuard<'static, ()>,
    tempfile::TempDir,
    EnvVarGuard,
) {
    let env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let current = temp.path().join("builds").join("current");
    std::fs::create_dir_all(&current).expect("create builds/current");
    std::fs::write(current.join("jcode"), b"#!/bin/sh\n").expect("write fake jcode binary");
    let home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    (env_lock, temp, home)
}

#[test]
fn build_resume_command_uses_imported_jcode_session_for_claude_code() {
    let _pinned = pinned_resume_test_home();
    let (program, args, title) = build_resume_command(
        &ResumeTarget::ClaudeCodeSession {
            session_id: "claude-session-123".to_string(),
            session_path: "/tmp/claude-session-123.jsonl".to_string(),
        },
        None,
    );

    assert_eq!(
        program.file_name().and_then(|name| name.to_str()),
        Some("jcode")
    );
    assert_eq!(
        args,
        vec![
            "--fresh-spawn".to_string(),
            "--resume".to_string(),
            crate::import::imported_claude_code_session_id("claude-session-123")
        ]
    );
    assert!(title.contains("Claude Code"));
    assert!(title.contains("claude-s"));
}

#[test]
fn build_resume_command_uses_imported_jcode_session_for_codex() {
    let _pinned = pinned_resume_test_home();
    let (program, args, title) = build_resume_command(
        &ResumeTarget::CodexSession {
            session_id: "codex-session-123".to_string(),
            session_path: "/tmp/codex-session-123.jsonl".to_string(),
        },
        None,
    );

    assert_eq!(
        program.file_name().and_then(|name| name.to_str()),
        Some("jcode")
    );
    assert_eq!(
        args,
        vec![
            "--fresh-spawn".to_string(),
            "--resume".to_string(),
            crate::import::imported_codex_session_id("codex-session-123")
        ]
    );
    assert!(title.contains("Codex"));
}

#[test]
fn format_countdown_until_handles_subminute_and_minutes() {
    let soon = Utc::now() + ChronoDuration::seconds(25);
    let medium = Utc::now() + ChronoDuration::minutes(2) + ChronoDuration::seconds(15);

    let soon_text = format_countdown_until(soon);
    let medium_text = format_countdown_until(medium);

    assert!(soon_text.starts_with("in "));
    assert!(soon_text.ends_with('s'));
    assert!(medium_text.starts_with("in 2m"));
}

#[test]
fn gather_ambient_info_filters_to_session_reminders_when_ambient_disabled() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut manager = AmbientManager::new().expect("ambient manager");
    let first_due = Utc::now() + ChronoDuration::minutes(5);
    let second_due = Utc::now() + ChronoDuration::minutes(10);

    manager
        .schedule(ScheduleRequest {
            wake_in_minutes: None,
            wake_at: Some(first_due),
            context: "ambient context".to_string(),
            priority: Priority::Normal,
            target: ScheduleTarget::Ambient,
            created_by_session: "ambient".to_string(),
            working_dir: None,
            task_description: Some("ambient work".to_string()),
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        })
        .expect("schedule ambient item");
    manager
        .schedule(ScheduleRequest {
            wake_in_minutes: None,
            wake_at: Some(first_due),
            context: "first context".to_string(),
            priority: Priority::Normal,
            target: ScheduleTarget::Session {
                session_id: "session_1".to_string(),
            },
            created_by_session: "session_1".to_string(),
            working_dir: None,
            task_description: Some("first reminder".to_string()),
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        })
        .expect("schedule first reminder");
    manager
        .schedule(ScheduleRequest {
            wake_in_minutes: None,
            wake_at: Some(second_due),
            context: "second context".to_string(),
            priority: Priority::Normal,
            target: ScheduleTarget::Session {
                session_id: "session_1".to_string(),
            },
            created_by_session: "session_1".to_string(),
            working_dir: None,
            task_description: Some("second reminder".to_string()),
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        })
        .expect("schedule second reminder");

    // This test exercises queue filtering, not the stale-while-revalidate cache.
    // Read synchronously while the temporary JCODE_HOME and its files are pinned:
    // an unrelated in-flight cache refresh can otherwise overwrite the cleared
    // process-global cache with data loaded under another test's JCODE_HOME.
    let info = gather_ambient_info_inner(false).expect("ambient info");
    assert!(info.show_widget);
    assert_eq!(info.queue_count, 3);
    assert_eq!(info.reminder_count, 2);
    assert_eq!(
        info.next_reminder_preview.as_deref(),
        Some("first reminder")
    );
    assert!(
        info.next_reminder_wake
            .as_deref()
            .is_some_and(|text| text.starts_with("in 4m") || text.starts_with("in 5m"))
    );
}

#[test]
fn pretty_model_display_name_formats_common_models() {
    assert_eq!(pretty_model_display_name("gpt-5.5"), "GPT-5.5");
    assert_eq!(pretty_model_display_name("gpt-5.1-codex"), "GPT-5.1-codex");
    assert_eq!(
        pretty_model_display_name("claude-opus-4-8"),
        "Claude Opus 4.8"
    );
    assert_eq!(
        pretty_model_display_name("claude-sonnet-4-5"),
        "Claude Sonnet 4.5"
    );
    assert_eq!(
        pretty_model_display_name("claude-opus-4-8[1m]"),
        "Claude Opus 4.8 (1M)"
    );
    assert_eq!(
        pretty_model_display_name("gemini-2.5-pro"),
        "Gemini 2.5 Pro"
    );
}

#[test]
fn pretty_model_display_name_handles_empty_and_unknown() {
    assert_eq!(pretty_model_display_name(""), "your default model");
    assert_eq!(pretty_model_display_name("   "), "your default model");
    // Unknown shapes fall back to a title-cased dashed rendering.
    assert_eq!(
        pretty_model_display_name("some-new-model"),
        "Some New Model"
    );
}

#[test]
fn invalidate_todos_cache_backdates_entry_so_next_gather_refetches() {
    use super::{
        clear_todos_cache_for_tests, gather_todos_and_goals_for_session, invalidate_todos_cache,
        todos_cache_entry_age_for_tests,
    };

    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    clear_todos_cache_for_tests();

    let session_id = "freshness-test-session";

    // No entry yet.
    assert_eq!(todos_cache_entry_age_for_tests(session_id), None);

    // First gather seeds the cache entry (and spawns the initial fetch). The
    // entry exists immediately, marked as actively refreshing / freshly stamped.
    let _ = gather_todos_and_goals_for_session(Some(session_id));
    let before = todos_cache_entry_age_for_tests(session_id);
    assert!(before.is_some(), "first gather must seed a cache entry");

    // Let the background fetch settle so we have a non-refreshing, fresh entry.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let _ = gather_todos_and_goals_for_session(Some(session_id));
    std::thread::sleep(std::time::Duration::from_millis(50));
    let settled = todos_cache_entry_age_for_tests(session_id)
        .expect("entry should exist after gather settles");
    assert!(
        settled.0 < 5,
        "a freshly fetched entry should be recent, got age={}s",
        settled.0
    );

    // Invalidation backdates the timestamp far past the TTL and clears the
    // refreshing flag, so the next gather treats it as expired and refetches.
    invalidate_todos_cache(session_id);
    let after = todos_cache_entry_age_for_tests(session_id)
        .expect("entry should still exist after invalidation");
    assert!(
        after.0 >= 1000,
        "invalidation must backdate the entry well past the 1s TTL, got age={}s",
        after.0
    );
    assert!(
        !after.1,
        "invalidation must clear the refreshing flag so the next gather refetches"
    );
}

#[test]
fn fresh_session_command_includes_fresh_spawn_and_socket() {
    let command = super::build_fresh_session_command(Some("/tmp/test.sock"));
    assert!(command.fresh_spawn, "must hand off as a fresh spawn");
    assert_eq!(command.kind.as_deref(), Some("new-terminal"));
    assert_eq!(command.title.as_deref(), Some("jcode · new session"));
    assert_eq!(
        command.args,
        vec![
            "--fresh-spawn".to_string(),
            "--socket".to_string(),
            "/tmp/test.sock".to_string(),
        ]
    );
}

#[test]
fn fresh_session_command_omits_blank_socket() {
    let command = super::build_fresh_session_command(Some("   "));
    assert_eq!(command.args, vec!["--fresh-spawn".to_string()]);
    let command = super::build_fresh_session_command(None);
    assert_eq!(command.args, vec!["--fresh-spawn".to_string()]);
}

/// Regression for issue #424: `Instant::now() - Duration` panics with
/// "overflow when subtracting duration from instant" when the monotonic clock
/// epoch (boot time) is more recent than the backdate amount. `backdated_now`
/// must saturate instead of panicking, and still return a value in the past
/// when possible so TTL checks treat the entry as expired.
#[test]
fn backdated_now_never_panics_and_prefers_past_instants() {
    use std::time::{Duration, Instant};

    let now = Instant::now();

    // Typical case: small backdate should land in the past.
    let recent = super::backdated_now(Duration::from_millis(10));
    assert!(recent <= now, "backdated instant must not be in the future");

    // Huge backdate (longer than any plausible uptime) must not panic and
    // must still return something no later than now.
    let ancient = super::backdated_now(Duration::from_secs(60 * 60 * 24 * 365 * 100));
    assert!(
        ancient <= now,
        "saturated backdate must not be in the future"
    );

    // Zero backdate is a no-op.
    let zero = super::backdated_now(Duration::ZERO);
    assert!(zero <= Instant::now());
}
