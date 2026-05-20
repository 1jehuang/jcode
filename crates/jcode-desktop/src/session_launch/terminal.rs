use super::launch_resume_session;
use anyhow::{Context, Result};
use std::io;
use std::process::{Command, Stdio};

pub(super) fn launch_first_available_terminal(
    candidates: Vec<Command>,
    description: &str,
) -> Result<()> {
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

pub(super) fn terminal_candidates(title: &str, jcode_args: &[&str]) -> Vec<Command> {
    let mut candidates = Vec::new();

    if let Ok(raw_terminal) = std::env::var("JCODE_DESKTOP_TERMINAL") {
        match terminal_env_command(&raw_terminal, jcode_args) {
            Ok(mut command) => {
                apply_default_working_dir(&mut command);
                candidates.push(command);
            }
            Err(error) => crate::desktop_log::warn(format_args!(
                "jcode-desktop: ignoring invalid JCODE_DESKTOP_TERMINAL={raw_terminal:?}: {error:#}"
            )),
        }
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

fn terminal_env_command(raw_terminal: &str, jcode_args: &[&str]) -> Result<Command> {
    let parts = parse_terminal_env_command(raw_terminal)?;
    let Some((program, prefix_args)) = parts.split_first() else {
        anyhow::bail!("terminal command is empty");
    };
    let mut command = Command::new(program);
    command
        .args(prefix_args)
        .arg(jcode_bin())
        .args(jcode_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

fn parse_terminal_env_command(raw_terminal: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut token_started = false;

    for ch in raw_terminal.chars() {
        if escaped {
            current.push(ch);
            token_started = true;
            escaped = false;
            continue;
        }

        if let Some(quote_ch) = quote {
            if ch == quote_ch {
                quote = None;
            } else if ch == '\\' && quote_ch == '"' {
                escaped = true;
            } else {
                current.push(ch);
                token_started = true;
            }
            continue;
        }

        match ch {
            '\\' => {
                escaped = true;
                token_started = true;
            }
            '\'' | '"' => {
                quote = Some(ch);
                token_started = true;
            }
            ch if ch.is_whitespace() => {
                if token_started {
                    parts.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            ch => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if escaped {
        anyhow::bail!("terminal command ends with an escape character");
    }
    if quote.is_some() {
        anyhow::bail!("terminal command has an unterminated quote");
    }
    if token_started {
        parts.push(current);
    }
    if parts.is_empty() {
        anyhow::bail!("terminal command is empty");
    }

    Ok(parts)
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
    apply_default_working_dir(&mut command);
    command
}

fn apply_default_working_dir(command: &mut Command) {
    if let Some(working_dir) = super::default_desktop_working_dir() {
        command.current_dir(working_dir);
    }
}

pub(super) fn jcode_bin() -> String {
    std::env::var("JCODE_BIN").unwrap_or_else(|_| "jcode".to_string())
}

pub(super) fn compact_title(title: &str) -> String {
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
    fn parses_terminal_env_command_with_quotes_and_escapes() -> Result<()> {
        assert_eq!(
            parse_terminal_env_command("kitty --title 'Jcode Desktop' --")?,
            vec!["kitty", "--title", "Jcode Desktop", "--"]
        );
        assert_eq!(
            parse_terminal_env_command(r#"footclient -T jcode\ desktop --"#)?,
            vec!["footclient", "-T", "jcode desktop", "--"]
        );
        assert_eq!(
            parse_terminal_env_command(r#"terminal --class "jcode desktop""#)?,
            vec!["terminal", "--class", "jcode desktop"]
        );
        Ok(())
    }

    #[test]
    fn rejects_malformed_terminal_env_command() {
        assert!(parse_terminal_env_command("   ").is_err());
        assert!(parse_terminal_env_command("kitty '").is_err());
        assert!(parse_terminal_env_command("kitty \\").is_err());
    }

    #[test]
    fn terminal_env_command_appends_jcode_invocation_without_shell() -> Result<()> {
        let command = terminal_env_command("kitty --title 'Jcode Desktop'", &["--resume", "abc"])?;
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program().to_string_lossy(), "kitty");
        assert_eq!(
            args,
            vec!["--title", "Jcode Desktop", "jcode", "--resume", "abc"]
        );
        Ok(())
    }
}
