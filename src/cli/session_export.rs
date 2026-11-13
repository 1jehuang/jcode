use anyhow::Result;
use std::path::{Path, PathBuf};

use super::args::SessionCommand;
use crate::message::{ContentBlock, Role};
use crate::session::{Session, StoredMessage};

pub(crate) fn run_session_command(command: SessionCommand) -> Result<()> {
    match command {
        SessionCommand::Rename {
            session,
            name,
            clear,
            json,
        } => super::commands::run_session_rename_command(&session, name.as_deref(), clear, json),
        SessionCommand::Export { session, output } => {
            run_session_export_command(&session, output.as_deref())
        }
    }
}

pub(crate) fn run_session_export_command(session_ref: &str, output: Option<&Path>) -> Result<()> {
    let path = export_session(session_ref, output)?;
    println!("{}", path.display());
    Ok(())
}

/// Format user and assistant turns as Markdown. Hidden reasoning, tool I/O,
/// images, and non-visible roles are omitted.
pub(crate) fn session_markdown(session: &Session) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(session.display_title_or_name());
    out.push('\n');

    for message in &session.messages {
        if !exportable_message(message) {
            continue;
        }
        let texts = exportable_texts(&message.content);
        if texts.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(match &message.role {
            Role::User => "## User\n\n",
            Role::Assistant => "## Assistant\n\n",
        });
        out.push_str(&texts.join("\n\n"));
        out.push('\n');
    }
    out
}

fn exportable_message(message: &StoredMessage) -> bool {
    matches!(message.role, Role::User | Role::Assistant)
        && message.display_role.is_none()
        && !crate::session::is_scheduled_task_message(message)
        && !is_system_reminder(message)
}

fn is_system_reminder(message: &StoredMessage) -> bool {
    message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.trim_start()),
            _ => None,
        })
        .is_some_and(|text| text.starts_with("<system-reminder>"))
}

fn exportable_texts(content: &[ContentBlock]) -> Vec<&str> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => (!text.trim().is_empty()).then_some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn sanitize_export_filename(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn default_export_path(session: &Session) -> PathBuf {
    let stem = sanitize_export_filename(session.display_title_or_name());
    let stem = if stem.is_empty() {
        "session".to_string()
    } else {
        stem
    };
    PathBuf::from(format!("{stem}.md"))
}

fn resolve_export_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub(crate) fn export_session(session_ref: &str, output: Option<&Path>) -> Result<PathBuf> {
    let resolved_id = crate::session::find_session_by_name_or_id(session_ref)?;
    let session = Session::load(&resolved_id)?;
    let path = resolve_export_path(match output {
        Some(path) => path.to_path_buf(),
        None => default_export_path(&session),
    })?;
    crate::storage::write_bytes(&path, session_markdown(&session).as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ContentBlock;
    use crate::session::{Session, StoredDisplayRole};
    use serde_json::json;
    use std::path::PathBuf;

    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", path);
            Self(previous)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => crate::env::set_var("JCODE_HOME", previous),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    struct CwdGuard {
        previous: PathBuf,
    }

    impl CwdGuard {
        fn enter(path: &std::path::Path) -> std::io::Result<Self> {
            let previous = std::env::current_dir()?;
            std::env::set_current_dir(path)?;
            Ok(Self { previous })
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.previous);
        }
    }

    fn text(value: &str) -> ContentBlock {
        ContentBlock::Text {
            text: value.to_string(),
            cache_control: None,
        }
    }

    fn fixture_session(title: &str) -> Session {
        let mut session = Session::create_with_id(
            "session_exportfix_1".to_string(),
            None,
            Some(title.to_string()),
        );
        session.short_name = Some("exportfix".to_string());
        session.rename_title(Some(title.to_string()));
        session
    }

    #[test]
    fn session_markdown_includes_heading_and_user_assistant_text() {
        let mut session = fixture_session("Release Planning");
        session.add_message(Role::User, vec![text("Ship the export command")]);
        session.add_message(Role::Assistant, vec![text("Will do, here is the plan.")]);

        let markdown = session_markdown(&session);
        assert!(
            markdown.contains("# Release Planning"),
            "heading missing: {markdown}"
        );
        assert!(
            markdown.contains("Ship the export command"),
            "user text missing: {markdown}"
        );
        assert!(
            markdown.contains("Will do, here is the plan."),
            "assistant text missing: {markdown}"
        );
        assert!(
            markdown.contains("## User"),
            "user heading missing: {markdown}"
        );
        assert!(
            markdown.contains("## Assistant"),
            "assistant heading missing: {markdown}"
        );
    }

    #[test]
    fn session_markdown_omits_reasoning_and_tool_content() {
        let mut session = fixture_session("Export Filter");
        session.add_message(Role::User, vec![text("What files changed?")]);
        session.add_message(
            Role::Assistant,
            vec![
                ContentBlock::Reasoning {
                    text: "secret chain of thought".to_string(),
                },
                ContentBlock::AnthropicThinking {
                    thinking: "hidden thinking".to_string(),
                    signature: "sig".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: json!({"cmd": "cat secrets"}),
                    thought_signature: None,
                },
                text("src/cli/session_export.rs"),
                ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "tool output secret".to_string(),
                    is_error: None,
                },
                ContentBlock::ReasoningTrace {
                    text: "trace secret".to_string(),
                },
                ContentBlock::OpenAIReasoning {
                    id: "r1".to_string(),
                    summary: vec!["summary secret".to_string()],
                    encrypted_content: Some("enc".to_string()),
                    status: None,
                },
            ],
        );
        session.add_message_with_display_role(
            Role::User,
            vec![text("system reminder should stay out")],
            Some(StoredDisplayRole::System),
        );

        let markdown = session_markdown(&session);
        assert!(markdown.contains("What files changed?"));
        assert!(markdown.contains("src/cli/session_export.rs"));
        for forbidden in [
            "secret chain of thought",
            "hidden thinking",
            "cat secrets",
            "tool output secret",
            "trace secret",
            "summary secret",
            "system reminder should stay out",
        ] {
            assert!(
                !markdown.contains(forbidden),
                "forbidden content {forbidden:?} leaked: {markdown}"
            );
        }
    }

    #[test]
    fn omitted_output_uses_sanitized_display_name_and_explicit_path_is_honored() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("home");
        let _home = HomeGuard::set(home.path());
        let cwd = tempfile::tempdir().expect("cwd");
        let _cwd = CwdGuard::enter(cwd.path()).expect("chdir");

        let mut session = fixture_session("Release Planning!");
        session.add_message(Role::User, vec![text("hello")]);
        session.add_message(Role::Assistant, vec![text("world")]);
        session.save().expect("save fixture");

        let default_path = export_session(&session.id, None).expect("default export");
        assert_eq!(
            default_path.file_name().and_then(|name| name.to_str()),
            Some("Release_Planning_.md")
        );
        let default_abs = cwd.path().join("Release_Planning_.md");
        assert!(
            default_abs.is_file(),
            "default file missing at {default_abs:?}"
        );
        let default_body = std::fs::read_to_string(&default_abs).expect("read default");
        assert!(default_body.contains("hello"));
        assert!(default_body.contains("world"));
        assert!(default_body.contains("Release Planning!"));

        let explicit = cwd.path().join("custom-export.md");
        let written = export_session(&session.id, Some(&explicit)).expect("explicit export");
        assert_eq!(written, explicit);
        let body = std::fs::read_to_string(&explicit).expect("read explicit");
        assert!(body.contains("hello"));
        assert!(body.contains("world"));
    }

    #[test]
    fn missing_session_returns_error_and_does_not_write_file() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("home");
        let _home = HomeGuard::set(home.path());
        let out = home.path().join("must-not-exist.md");

        let err =
            export_session("no-such-session", Some(&out)).expect_err("missing session must fail");
        let message = format!("{err:#}");
        assert!(
            message.contains("no-such-session") || message.to_lowercase().contains("no session"),
            "unexpected error: {message}"
        );
        assert!(!out.exists(), "export must not create {out:?}");
    }

    #[test]
    fn unreadable_session_returns_error_and_does_not_write_file() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("home");
        let _home = HomeGuard::set(home.path());
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(&sessions).expect("sessions dir");
        std::fs::write(sessions.join("broken-export.json"), "{not-json").expect("corrupt snapshot");
        let out = home.path().join("truncated.md");

        let err =
            export_session("broken-export", Some(&out)).expect_err("unreadable session must fail");
        let _ = err;
        assert!(!out.exists(), "export must not create {out:?}");
    }

    #[test]
    fn default_export_path_sanitizes_display_name() {
        let session = fixture_session("Release Planning!");
        assert_eq!(
            default_export_path(&session),
            PathBuf::from("Release_Planning_.md")
        );
    }
}
