use anyhow::{Context, Result};

fn run_external_login_command_inner(
    program: &str,
    args: &[String],
    suspend_raw_mode: bool,
) -> Result<()> {
    let raw_was_enabled = suspend_raw_mode
        && crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if raw_was_enabled {
        let _ = crossterm::terminal::disable_raw_mode();
    }

    let status_result = std::process::Command::new(program).args(args).status();

    if raw_was_enabled {
        let _ = crossterm::terminal::enable_raw_mode();
    }

    let status = status_result
        .with_context(|| format!("Failed to start command: {} {}", program, args.join(" ")))?;
    if !status.success() {
        anyhow::bail!(
            "Command exited with non-zero status: {} {} ({})",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}

pub fn run_external_login_command(program: &str, args: &[&str]) -> Result<()> {
    let owned = args.iter().map(|arg| (*arg).to_string()).collect::<Vec<_>>();
    run_external_login_command_inner(program, &owned, false)
}

pub fn run_external_login_command_owned(program: &str, args: &[String]) -> Result<()> {
    run_external_login_command_inner(program, args, false)
}

pub fn run_external_login_command_with_terminal_handoff(
    program: &str,
    args: &[&str],
) -> Result<()> {
    let owned = args.iter().map(|arg| (*arg).to_string()).collect::<Vec<_>>();
    run_external_login_command_inner(program, &owned, true)
}

pub fn run_external_login_command_owned_with_terminal_handoff(
    program: &str,
    args: &[String],
) -> Result<()> {
    run_external_login_command_inner(program, args, true)
}
