use anyhow::Result;
use clap::Parser;
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

    // Wire config-reload reactions without making config depend on auth/bus:
    // when the config cache reloads, invalidate the auth-status cache and
    // broadcast a models-updated event.
    crate::config::on_config_reloaded(crate::auth::AuthStatus::invalidate_cache);
    crate::config::on_config_reloaded(|| crate::bus::Bus::global().publish_models_updated());

    // Invert the legacy provider_catalog -> auth dependency: provider_catalog
    // consults registered fallback resolvers, and auth (the higher layer)
    // registers its external-CLI credential scan here.
    crate::provider_catalog::register_api_key_fallback_resolver(
        crate::auth::external::load_api_key_for_env,
    );

    // Invert the legacy safety -> notifications dependency: safety raises a
    // permission request and the notifications layer (which depends on safety
    // types) delivers it via the dispatcher registered here.
    crate::safety::register_permission_notifier(|action, description, request_id| {
        crate::notifications::NotificationDispatcher::new().dispatch_permission_request(
            action,
            description,
            request_id,
        );
    });

    // Invert the legacy memory -> skill dependency: memory collects synthetic
    // entries from registered providers, and skill (the higher layer that
    // depends on MemoryEntry) registers its registry->memory adapter here.
    crate::memory::register_synthetic_entry_provider(|| {
        crate::skill::SkillRegistry::shared_snapshot()
            .list()
            .into_iter()
            .map(|skill| skill.as_memory_entry())
            .collect()
    });

    // Invert the legacy server -> tui dependency: the TUI session picker owns
    // the session-list cache and registers its invalidator here, so the server
    // can drop the cache (e.g. after a rename) without referencing tui.
    crate::session_list_cache::register_invalidator(
        crate::tui::session_picker::invalidate_session_list_cache,
    );

    // Invert the legacy tui -> cli dependency for shared-server spawning: the
    // CLI owns the provider-bootstrap spawn logic and registers it here, so the
    // TUI reconnect loop can request a replacement server via server_spawn
    // without referencing cli.
    crate::server_spawn::register_default_server_spawner(Box::new(|| {
        Box::pin(async {
            dispatch::spawn_server(&crate::cli::provider_init::ProviderChoice::Auto, None, None)
                .await
        })
    }));

    crate::platform::raise_nofile_limit_best_effort(8_192);
    startup_profile::mark("nofile_limit");

    storage::harden_user_config_permissions();
    startup_profile::mark("perm_harden");

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

fn parse_and_prepare_args() -> Result<Args> {
    let mut args = Args::parse();
    startup_profile::mark("args_parse");

    output::set_quiet_enabled(args.quiet);

    if let Some(cwd) = &args.cwd {
        std::env::set_current_dir(cwd)?;
        logging::info(&format!("Changed working directory to: {}", cwd));
    }

    if args.trace {
        crate::env::set_var("JCODE_TRACE", "1");
    }

    // Translate --offline to the in-process JCODE_OFFLINE flag so deep code
    // paths can read it without threading an extra argument. See issue #24.
    // Honor a pre-existing env value too (the env var is the documented way
    // to enable offline mode for wrapper scripts).
    if args.offline || std::env::var("JCODE_OFFLINE").is_ok() {
        crate::env::set_var("JCODE_OFFLINE", "1");
        if !args.quiet {
            output::stderr_info(
                "Offline mode: startup network operations disabled (JCODE_OFFLINE=1).",
            );
        }
    }

    // --safe-eval: layered first-run sandbox. Sets an isolated JCODE_HOME and
    // turns off network/ambient/telemetry/selfdev so users can poke at jcode
    // without touching their main credentials/sessions/memory. Issue #60.
    if args.safe_eval || std::env::var("JCODE_SAFE_EVAL").is_ok() {
        crate::env::set_var("JCODE_SAFE_EVAL", "1");
        if std::env::var_os("JCODE_HOME").is_none()
            && let Some(home) = dirs::home_dir()
        {
            let isolated = home.join(".jcode-safe-eval");
            crate::env::set_var("JCODE_HOME", &isolated);
        }
        crate::env::set_var("JCODE_OFFLINE", "1");
        crate::env::set_var("JCODE_NO_TELEMETRY", "1");
        crate::env::set_var("JCODE_AMBIENT_DISABLED", "1");
        crate::env::set_var("JCODE_NO_SELFDEV", "1");
        // Issue #62: project-local MCP configs in safe-eval mode require
        // explicit trust via `jcode mcp trust <path>`.
        crate::env::set_var("JCODE_REQUIRE_MCP_TRUST", "1");
        if !args.quiet {
            output::stderr_info(
                "Safe-eval profile: isolated JCODE_HOME, telemetry off, offline, ambient/selfdev gated.",
            );
            if let Some(home) = std::env::var_os("JCODE_HOME") {
                output::stderr_info(format!("  JCODE_HOME = {}", home.to_string_lossy()));
            }
        }
    }

    // --system-prompt / --append-system-prompt: translate to env vars so the
    // build_system_prompt helpers (which run on demand from many code paths)
    // can pick them up without threading args through every layer. Issue #22.
    if let Some(ref text) = args.system_prompt {
        crate::env::set_var("JCODE_SYSTEM_PROMPT", text);
    }
    if let Some(ref text) = args.append_system_prompt {
        crate::env::set_var("JCODE_APPEND_SYSTEM_PROMPT", text);
    }

    // --models <patterns>: translate to JCODE_SCOPED_MODELS env so cycle_model
    // and the `/scoped-models` slash command can see it. Issue #26.
    if !args.scoped_models.is_empty() {
        let joined = args
            .scoped_models
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        if !joined.is_empty() {
            crate::env::set_var("JCODE_SCOPED_MODELS", &joined);
        }
    }

    // --name <title>: translate to env so the next freshly-created top-level
    // session picks it up as `Session::title`. Issue #99.
    if let Some(ref text) = args.session_name {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            crate::env::set_var("JCODE_SESSION_NAME", trimmed);
        }
    }

    // --no-context-files: translate to JCODE_NO_CONTEXT_FILES so the prompt
    // loading helpers can skip AGENTS.md without threading args.
    if args.no_context_files {
        crate::env::set_var("JCODE_NO_CONTEXT_FILES", "1");
    }

    // --no-builtin-tools: translate to JCODE_NO_BUILTIN_TOOLS so the tool
    // registry can skip the built-in entries without threading args.
    if args.no_builtin_tools {
        crate::env::set_var("JCODE_NO_BUILTIN_TOOLS", "1");
    }

    // Issue #14: --extension-policy → JCODE_EXTENSION_POLICY.
    // Other subsystems (MCP loader, future extension runtime) read
    // this env var via crate::extension_policy::current().
    if let Some(ref policy) = args.extension_policy {
        crate::env::set_var("JCODE_EXTENSION_POLICY", policy);
    }

    // Issue #110: --sandbox shortcut for hardened defaults.
    // Lighter than --safe-eval; doesn't reroute JCODE_HOME or
    // disable network — just locks down extension loading to
    // explicitly trusted entries.
    if args.sandbox {
        crate::env::set_var("JCODE_REQUIRE_MCP_TRUST", "1");
        // Only set if user hasn't explicitly chosen a different
        // policy via --extension-policy; their choice wins.
        if std::env::var_os("JCODE_EXTENSION_POLICY").is_none() {
            crate::env::set_var("JCODE_EXTENSION_POLICY", "trusted");
        }
    }

    // Issue #110: --sandbox-root <DIR> → JCODE_SANDBOX_ROOT.
    // Canonicalize the path so downstream tool-context comparisons
    // are stable (matters when user passes a relative directory).
    if let Some(ref dir) = args.sandbox_root {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        crate::env::set_var("JCODE_SANDBOX_ROOT", &canonical);
    }

    // --permission-mode → dcg_bridge global mode. When unspecified we leave
    // the default (`Mode::Default`, set by the bridge's LazyLock) untouched
    // so behavior matches the legacy AUTO_ALLOWED-based classify.
    //
    // `--dangerously-skip-permissions` is the Claude Code compatibility alias
    // for `--permission-mode bypass-permissions`. Explicit `--permission-mode`
    // wins when both are set.
    if let Some(mode) = args.permission_mode {
        crate::dcg_bridge::set_mode(mode.into_dcg_mode());
    } else if args.dangerously_skip_permissions {
        crate::dcg_bridge::set_mode(dcg_core::Mode::BypassPermissions);
    } else if let Ok(env_mode) = std::env::var("JCODE_PERMISSION_MODE")
        && let Some(mode) = dcg_core::Mode::parse(env_mode.trim())
    {
        crate::dcg_bridge::set_mode(mode);
    }

    // JCODE_MODEL fallback: when --model is not passed on the CLI,
    // read JCODE_MODEL from the env so users can `export JCODE_MODEL=...`
    // in their shell profile and have it apply to every jcode invocation.
    // CLI flag still wins when both are set.
    if args.model.is_none()
        && let Ok(env_model) = std::env::var("JCODE_MODEL")
    {
        let trimmed = env_model.trim();
        if !trimmed.is_empty() {
            args.model = Some(trimmed.to_string());
        }
    }

    if let Some(ref socket) = args.socket {
        server::set_socket_path(socket);
    }

    crate::cli::proctitle::set_initial_title(&args);

    Ok(args)
}

fn spawn_background_update_check(args: &Args) {
    let check_updates = should_spawn_background_update_check(args);
    let auto_update = should_auto_install_update(args);

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
                logging::info(&format!("Updated to {}. Restarting...", version));
                std::thread::sleep(std::time::Duration::from_millis(250));
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
            use crate::bus::{Bus, BusEvent, UpdateStatus};

            let start = std::time::Instant::now();
            Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Checking));
            if let Some(update_available) = hot_exec::check_for_updates()
                && update_available
            {
                Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Available {
                    current: jcode_build_meta::VERSION.to_string(),
                    latest: "latest source".to_string(),
                }));
                if auto_update {
                    logging::info("Update available - auto-updating...");
                    Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Installing {
                        version: "latest source".to_string(),
                    }));
                    if let Err(e) = hot_exec::run_auto_update() {
                        Bus::global()
                            .publish(BusEvent::UpdateStatus(UpdateStatus::Error(e.to_string())));
                        logging::error(&format!(
                            "Auto-update failed: {}. Continuing with current version.",
                            e
                        ));
                    }
                } else {
                    logging::info("Update available! Run `jcode update` or `/reload` to update.");
                }
            } else {
                Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::UpToDate));
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
    if std::env::var("JCODE_OFFLINE").is_ok() {
        return false;
    }
    !args.quiet
        && !args.no_update
        && !matches!(
            args.command,
            Some(Command::Update) | Some(Command::Serve { .. }) | Some(Command::Acp)
        )
        && args.resume.is_none()
}

fn should_auto_install_update(args: &Args) -> bool {
    args.auto_update
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
        assert!(should_auto_install_update(&args));
    }

    #[test]
    fn auto_install_allowed_with_live_terminal_attached() {
        let args = parse_args(&["jcode", "login"]);
        assert!(should_auto_install_update(&args));
    }

    #[test]
    fn auto_install_respects_explicit_disable_even_without_terminal() {
        let mut args = parse_args(&["jcode", "login"]);
        args.auto_update = false;
        assert!(!should_auto_install_update(&args));
    }

    #[test]
    fn update_command_still_skips_background_check_before_auto_install_logic() {
        let args = parse_args(&["jcode", "update"]);
        assert!(matches!(args.command, Some(Command::Update)));
        assert!(!should_spawn_background_update_check(&args));
        assert!(should_auto_install_update(&args));
    }

    // ---- JCODE_MODEL fallback ----

    fn apply_model_env_fallback(args: &mut Args) {
        if args.model.is_none()
            && let Ok(env_model) = std::env::var("JCODE_MODEL")
        {
            let trimmed = env_model.trim();
            if !trimmed.is_empty() {
                args.model = Some(trimmed.to_string());
            }
        }
    }

    #[test]
    fn jcode_model_env_used_when_cli_flag_absent() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_MODEL");
        crate::env::set_var("JCODE_MODEL", "claude-haiku-4");

        let mut args = parse_args(&["jcode"]);
        assert!(args.model.is_none());
        apply_model_env_fallback(&mut args);
        assert_eq!(args.model.as_deref(), Some("claude-haiku-4"));

        if let Some(p) = prev {
            crate::env::set_var("JCODE_MODEL", p);
        } else {
            crate::env::remove_var("JCODE_MODEL");
        }
    }

    #[test]
    fn cli_flag_wins_over_jcode_model_env() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_MODEL");
        crate::env::set_var("JCODE_MODEL", "from-env");

        let mut args = parse_args(&["jcode", "--model", "from-cli"]);
        apply_model_env_fallback(&mut args);
        assert_eq!(args.model.as_deref(), Some("from-cli"));

        if let Some(p) = prev {
            crate::env::set_var("JCODE_MODEL", p);
        } else {
            crate::env::remove_var("JCODE_MODEL");
        }
    }

    #[test]
    fn empty_jcode_model_env_treated_as_unset() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_MODEL");
        crate::env::set_var("JCODE_MODEL", "   ");

        let mut args = parse_args(&["jcode"]);
        apply_model_env_fallback(&mut args);
        assert!(args.model.is_none(), "blank env should not override");

        if let Some(p) = prev {
            crate::env::set_var("JCODE_MODEL", p);
        } else {
            crate::env::remove_var("JCODE_MODEL");
        }
    }
}
