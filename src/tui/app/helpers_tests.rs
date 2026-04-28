use super::{
    build_resume_command, detected_resume_terminal, extract_bracketed_system_message,
    format_countdown_until, gather_ambient_info, partition_queued_messages, resume_invocation_args,
    shell_command, should_prefer_osc52_for_env, write_osc52_clipboard_to,
};
use crate::ambient::{AmbientManager, Priority, ScheduleRequest, ScheduleTarget};
use crate::tui::session_picker::ResumeTarget;
use chrono::{Duration as ChronoDuration, Utc};

#[cfg(all(unix, not(target_os = "macos")))]
use super::native_clipboard_commands_for_env;

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

#[cfg(unix)]
#[test]
fn detected_resume_terminal_recognizes_handterm_term_program() {
    let _guard = EnvVarGuard::set_value("TERM_PROGRAM", "handterm");
    assert_eq!(detected_resume_terminal(), Some("handterm"));
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

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn native_clipboard_commands_prefer_wayland_before_x11() {
    let commands = native_clipboard_commands_for_env(
        Some(std::ffi::OsStr::new("wayland-0")),
        Some(std::ffi::OsStr::new(":0")),
    );
    let programs = commands
        .iter()
        .map(|command| command.program)
        .collect::<Vec<_>>();

    assert_eq!(programs, vec!["wl-copy", "xclip", "xsel"]);
    assert_eq!(commands[0].args, &["--type", "text/plain;charset=utf-8"]);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn native_clipboard_commands_are_empty_without_display_env() {
    assert!(native_clipboard_commands_for_env(None, None).is_empty());
}

#[test]
fn osc52_clipboard_writer_emits_base64_bel_sequence() {
    let mut output = Vec::new();

    write_osc52_clipboard_to(b"hello", &mut output).expect("write osc52");

    assert_eq!(output, b"\x1b]52;c;aGVsbG8=\x07");
}

#[test]
fn ssh_sessions_prefer_osc52_clipboard() {
    assert!(should_prefer_osc52_for_env(
        Some(std::ffi::OsStr::new("1 2 3 4")),
        None
    ));
    assert!(should_prefer_osc52_for_env(
        None,
        Some(std::ffi::OsStr::new("/dev/pts/1"))
    ));
    assert!(!should_prefer_osc52_for_env(None, None));
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

#[test]
fn build_resume_command_uses_imported_jcode_session_for_claude_code() {
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

    let info = gather_ambient_info(false).expect("ambient info");
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
