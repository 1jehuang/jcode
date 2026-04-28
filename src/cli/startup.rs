use anyhow::Result;
use clap::Parser;
use std::io::IsTerminal;
use std::process::Command as ProcessCommand;

use crate::{build, logging, perf, server, startup_profile, storage, telemetry, update};

use super::{
    args::{Args, Command},
    dispatch, hot_exec, output, terminal,
};

pub async fn run() -> Result<()> {
    startup_profile::init();

    terminal::install_panic_hook();
    startup_profile::mark("panic_hook");

    logging::init();
    startup_profile::mark("logging_init");
    logging::cleanup_old_logs();
    startup_profile::mark("log_cleanup");
    logging::info("jcode starting");
    crate::platform::raise_nofile_limit_best_effort(8_192);
    startup_profile::mark("nofile_limit");

    storage::harden_user_config_permissions();
    startup_profile::mark("perm_harden");

    #[cfg(windows)]
    clean_stale_executable_replacements();

    perf::init_background();
    startup_profile::mark("perf_init");

    telemetry::record_install_if_first_run();
    telemetry::record_upgrade_if_needed();
    startup_profile::mark("telemetry_check");

    let args = parse_and_prepare_args()?;
    spawn_background_update_check(&args);

    if let Err(e) = dispatch::run_main(args).await {
        report_main_error(&e);
        return Err(e);
    }

    Ok(())
}

/// On Windows, `replace_executable_atomic` renames running `.exe` files aside
/// rather than deleting them (Windows refuses the latter). The next launch
/// sweeps the leftover `.old` sidecars.
#[cfg(windows)]
fn clean_stale_executable_replacements() {
    let Ok(builds_root) = build::builds_dir() else {
        return;
    };
    for channel in ["stable", "current", "canary", "shared-server"] {
        crate::platform::clean_stale_executable_replacements(&builds_root.join(channel));
    }
    if let Ok(versions) = std::fs::read_dir(builds_root.join("versions")) {
        for entry in versions.flatten() {
            crate::platform::clean_stale_executable_replacements(&entry.path());
        }
    }
}

fn parse_and_prepare_args() -> Result<Args> {
    let args = Args::parse();
    startup_profile::mark("args_parse");

    output::set_quiet_enabled(args.quiet);

    if let Some(cwd) = &args.cwd {
        std::env::set_current_dir(cwd)?;
        logging::info(&format!("Changed working directory to: {}", cwd));
    }

    if args.trace {
        crate::env::set_var("JCODE_TRACE", "1");
    }

    if let Some(ref socket) = args.socket {
        server::set_socket_path(socket);
    }

    crate::process_title::set_initial_title(&args);

    Ok(args)
}

fn spawn_background_update_check(args: &Args) {
    let check_updates = should_spawn_background_update_check(args);
    let auto_update = should_auto_install_update(args, has_live_terminal_attached());

    if !check_updates {
        return;
    }

    if update::is_release_build() {
        std::thread::spawn(move || match update::check_and_maybe_update(auto_update) {
            update::UpdateCheckResult::UpdateAvailable {
                current, latest, ..
            } => {
                logging::info(&format!("Update available: {} -> {}", current, latest));
            }
            update::UpdateCheckResult::UpdateInstalled { version, path } => {
                update::print_centered(&format!("✅ Updated to {}. Restarting...", version));
                let args: Vec<String> = std::env::args().skip(1).collect();
                let exec_path = build::client_update_candidate(false)
                    .map(|(p, _)| p)
                    .unwrap_or(path);
                let err = crate::platform::replace_process(
                    ProcessCommand::new(&exec_path)
                        .args(&args)
                        .arg("--no-update"),
                );
                eprintln!("Failed to exec new binary: {}", err);
            }
            update::UpdateCheckResult::Error(e) => {
                logging::info(&format!("Update check failed: {}", e));
            }
            update::UpdateCheckResult::NoUpdate => {}
        });
    } else {
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            if let Some(update_available) = hot_exec::check_for_updates()
                && update_available
            {
                if auto_update {
                    logging::info("Update available - auto-updating...");
                    if let Err(e) = hot_exec::run_auto_update() {
                        logging::error(&format!(
                            "Auto-update failed: {}. Continuing with current version.",
                            e
                        ));
                    }
                } else {
                    logging::info("Update available! Run `jcode update` or `/reload` to update.");
                }
            }
            logging::info(&format!(
                "[TIMING] background_update_check: auto_update={}, total={}ms",
                auto_update,
                start.elapsed().as_millis()
            ));
        });
    }
}

fn should_spawn_background_update_check(args: &Args) -> bool {
    !args.quiet
        && !args.no_update
        && !matches!(args.command, Some(Command::Update) | Some(Command::Serve))
        && args.resume.is_none()
}

fn has_live_terminal_attached() -> bool {
    std::io::stdin().is_terminal()
        || std::io::stdout().is_terminal()
        || std::io::stderr().is_terminal()
}

fn should_auto_install_update(args: &Args, live_terminal_attached: bool) -> bool {
    args.auto_update && !live_terminal_attached
}

fn report_main_error(error: &anyhow::Error) {
    let error_str = format!("{:?}", error);
    logging::error(&error_str);

    if let Some(session_id) = terminal::get_current_session() {
        output::stderr_blank_line();
        output::stderr_info("\x1b[33mTo restore this session, run:\x1b[0m");
        output::stderr_info(format!("  jcode --resume {}", session_id));
        output::stderr_blank_line();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{Args, Command};
    use clap::Parser;

    fn parse_args(argv: &[&str]) -> Args {
        Args::parse_from(argv)
    }

    #[test]
    fn auto_install_allowed_without_live_terminal() {
        let args = parse_args(&["jcode", "login"]);
        assert!(should_auto_install_update(&args, false));
    }

    #[test]
    fn auto_install_deferred_when_live_terminal_is_attached() {
        let args = parse_args(&["jcode", "login"]);
        assert!(!should_auto_install_update(&args, true));
    }

    #[test]
    fn auto_install_respects_explicit_disable_even_without_terminal() {
        let mut args = parse_args(&["jcode", "login"]);
        args.auto_update = false;
        assert!(!should_auto_install_update(&args, false));
    }

    #[test]
    fn update_command_still_skips_background_check_before_auto_install_logic() {
        let args = parse_args(&["jcode", "update"]);
        assert!(matches!(args.command, Some(Command::Update)));
        assert!(!should_auto_install_update(&args, true));
        assert!(should_auto_install_update(&args, false));
    }
}
