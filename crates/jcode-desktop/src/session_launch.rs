use anyhow::{Context, Result};
use std::io;
use std::process::{Command, Stdio};

pub fn launch_resume_session(session_id: &str, title: &str) -> Result<()> {
    let title = format!("jcode · {}", compact_title(title));
    let candidates = terminal_candidates(&title, &["--resume", session_id]);
    launch_first_available_terminal(candidates, &format!("jcode --resume {session_id}"))
}

pub fn launch_new_session() -> Result<()> {
    let candidates = terminal_candidates("jcode · new session", &["--fresh-spawn"]);
    launch_first_available_terminal(candidates, "jcode")
}

fn launch_first_available_terminal(candidates: Vec<Command>, description: &str) -> Result<()> {
    let mut failures = Vec::new();

    for mut candidate in candidates {
        match candidate.spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                failures.push(format!(
                    "{} not found",
                    candidate.get_program().to_string_lossy()
                ));
            }
            Err(error) => {
                failures.push(format!(
                    "{}: {error}",
                    candidate.get_program().to_string_lossy()
                ));
            }
        }
    }

    anyhow::bail!(
        "failed to launch a terminal for {description}: {}",
        failures.join("; ")
    )
}

fn terminal_candidates(title: &str, jcode_args: &[&str]) -> Vec<Command> {
    let mut candidates = Vec::new();

    if let Ok(program) = std::env::var("JCODE_DESKTOP_TERMINAL") {
        candidates.push(terminal_command(program, &[], jcode_args));
    }

    candidates.push(terminal_command(
        "footclient",
        &["-T", title, "--"],
        jcode_args,
    ));
    candidates.push(terminal_command("foot", &["-T", title, "--"], jcode_args));
    candidates.push(terminal_command("kitty", &["--title", title], jcode_args));
    candidates.push(terminal_command(
        "alacritty",
        &["-t", title, "-e"],
        jcode_args,
    ));
    candidates.push(terminal_command("wezterm", &["start", "--"], jcode_args));
    candidates.push(terminal_command(
        "x-terminal-emulator",
        &["-T", title, "-e"],
        jcode_args,
    ));

    candidates
}

fn terminal_command(
    program: impl AsRef<str>,
    prefix_args: &[&str],
    jcode_args: &[&str],
) -> Command {
    let mut command = Command::new(program.as_ref());
    command
        .args(prefix_args)
        .arg(jcode_bin())
        .args(jcode_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn jcode_bin() -> String {
    std::env::var("JCODE_BIN").unwrap_or_else(|_| "jcode".to_string())
}

fn compact_title(title: &str) -> String {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "session".to_string();
    }

    let mut chars = normalized.chars();
    let compact = chars.by_ref().take(48).collect::<String>();
    if chars.next().is_some() {
        format!("{compact}…")
    } else {
        compact
    }
}

pub fn validate_resume_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty() {
        anyhow::bail!("empty session id");
    }
    if !session_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        anyhow::bail!("session id contains unsupported characters");
    }
    Ok(())
}

pub fn launch_validated_resume_session(session_id: &str, title: &str) -> Result<()> {
    validate_resume_session_id(session_id).context("refusing to launch invalid session id")?;
    launch_resume_session(session_id, title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_safe_session_ids() {
        validate_resume_session_id("session_cow_123-abc.def").unwrap();
        assert!(validate_resume_session_id("bad/id").is_err());
        assert!(validate_resume_session_id("bad id").is_err());
    }

    #[test]
    fn compact_title_shortens_long_titles() {
        let title =
            compact_title("this is a very long title that should become shorter for terminals");
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= 49);
    }
}
