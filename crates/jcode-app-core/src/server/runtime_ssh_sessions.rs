//! Session workflows owned by the daemon, not the SSH client's filesystem.
use crate::{agent::Agent, session::Session};
use anyhow::{Result, bail};
use serde_json::{Value, json};

pub(super) fn handles(command: &str) -> bool {
    matches!(
        command,
        "sessions" | "session" | "active" | "catchup" | "transcript"
    )
}

fn summary(session: &Session) -> Value {
    json!({"id": session.id, "title": session.display_title_or_name(),
        "parent_id": session.parent_id, "saved": session.saved,
        "status": session.status.display(), "updated_at": session.updated_at,
        "working_dir": session.working_dir, "model": session.model})
}

fn list_sessions(current: Value, command: &str, args: &[String]) -> Result<(Value, String)> {
    let saved_only = args.first().is_some_and(|arg| arg == "saved");
    if args.len() > 1
        || args
            .first()
            .is_some_and(|arg| arg != "saved" && arg != "all")
    {
        bail!("Usage: /{command} [saved|all]");
    }
    let directory = crate::storage::jcode_dir()?.join("sessions");
    let mut candidates = Vec::new();
    match std::fs::read_dir(directory) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "json")
                    && let Ok(metadata) = entry.metadata()
                    && metadata.is_file()
                {
                    candidates.push((metadata.modified().ok(), path));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    // Bound snapshot work on hosts with a large archive. The current live copy
    // is always included, even if its initial snapshot has not been written.
    let current_id = current["id"].as_str().unwrap_or_default().to_owned();
    let active_ids = crate::session::active_session_ids();
    let mut sessions = vec![current];
    for (_, path) in candidates.into_iter().take(200) {
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if id == current_id {
            continue;
        }
        if let Ok(session) = Session::load_startup_stub(id) {
            sessions.push(summary(&session));
        }
    }
    sessions.sort_by(|left, right| {
        right["updated_at"]
            .as_str()
            .cmp(&left["updated_at"].as_str())
    });
    let rows: Vec<_> = sessions
        .into_iter()
        .filter(|session| {
            (!saved_only || session["saved"] == true)
                && (command != "active"
                    || session["id"] == current_id
                    || session["id"]
                        .as_str()
                        .is_some_and(|id| active_ids.iter().any(|active| active == id)))
        })
        .collect();
    let mut message = format!(
        "Remote {command} ({} shown, latest 200 snapshots):\n",
        rows.len()
    );
    for session in &rows {
        message.push_str(&format!(
            "\n{}{} [{}] {}",
            if session["saved"] == true { "★ " } else { "" },
            session["id"].as_str().unwrap_or_default(),
            session["status"].as_str().unwrap_or_default(),
            session["title"].as_str().unwrap_or_default()
        ));
    }
    message.push_str("\n\nSwitch here with /resume <id>. /back returns to the previous session.");
    Ok((
        json!({"scope": "runtime host", "sessions": rows, "scan_limit": 200}),
        message,
    ))
}

pub(super) async fn execute(
    agent: &mut Agent,
    command: &str,
    args: &[String],
) -> Result<(Value, String)> {
    let session = agent.session_for_history();
    match command {
        "sessions" | "active" | "catchup" => {
            let current = summary(session);
            let command = command.to_owned();
            let args = args.to_vec();
            tokio::task::spawn_blocking(move || list_sessions(current, &command, &args)).await?
        }
        "session" => {
            if !args.is_empty() {
                bail!("Usage: /session or /resume <id>");
            }
            let data = summary(session);
            Ok((data.clone(), serde_json::to_string_pretty(&data)?))
        }
        "transcript" => {
            if args == ["path"] {
                let path = crate::session::session_path(&session.id)?;
                let path = path.display().to_string();
                return Ok((
                    json!({"scope": "runtime host", "path": path}),
                    format!("Remote transcript path (on the server):\n{path}"),
                ));
            }
            if !args.is_empty() {
                bail!("Usage: /transcript [path]");
            }
            let messages = crate::session::render_messages(session);
            let mut text = format!("# Remote transcript: {}\n", session.id);
            for message in messages {
                text.push_str(&format!("\n## {}\n{}\n", message.role, message.content));
            }
            Ok((json!({"session_id": session.id, "transcript": text}), text))
        }
        _ => bail!("Unknown session command: {command}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_runtime_names_are_exact() {
        for name in ["sessions", "session", "active", "catchup", "transcript"] {
            assert!(handles(name));
        }
        for name in ["resume", "sessions-other", "split", "save", "/sessions"] {
            assert!(!handles(name));
        }
    }
    #[test]
    fn current_session_summary_preserves_remote_identity() {
        let mut session = Session::create(None, Some("host title".into()));
        session.id = "remote-only-id".into();
        session.mark_saved(None);
        let data = summary(&session);
        assert_eq!(data["id"], "remote-only-id");
        assert_eq!(data["saved"], true);
        assert_eq!(data["title"], "host title");
    }
}
