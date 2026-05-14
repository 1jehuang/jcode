#![cfg_attr(test, allow(clippy::await_holding_lock))]

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::{build, logging, session, startup_profile};

use super::output;
use super::provider_init::ProviderChoice;

pub const CLIENT_SELFDEV_ENV: &str = "JCODE_CLIENT_SELFDEV_MODE";

pub fn client_selfdev_requested() -> bool {
    std::env::var(CLIENT_SELFDEV_ENV).is_ok()
}

fn repo_from_session_working_dir(session_id: &str) -> Option<PathBuf> {
    let session = session::Session::load(session_id).ok()?;
    let working_dir = session.working_dir.as_deref()?;
    build::find_repo_in_ancestors(Path::new(working_dir))
}

fn resolve_selfdev_cli_repo_dir_from(
    primary: Option<PathBuf>,
    resume_session: Option<&str>,
) -> Option<PathBuf> {
    resume_session
        .and_then(repo_from_session_working_dir)
        .or(primary)
}

fn resolve_selfdev_cli_repo_dir(resume_session: Option<&str>) -> Option<PathBuf> {
    resolve_selfdev_cli_repo_dir_from(build::get_repo_dir(), resume_session)
}

fn selfdev_repo_discovery_error(resume_session: Option<&str>) -> anyhow::Error {
    let session_working_dir = resume_session
        .and_then(|id| session::Session::load(id).ok())
        .and_then(|session| session.working_dir);
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("<unavailable: {}>", error));

    anyhow::anyhow!(
        "Could not find jcode repository for self-dev. Tried resumed session working_dir={}, current_dir={}, and build/executable fallbacks. Run from the jcode repo or use `selfdev enter` to create a self-dev session with a valid repo working directory.",
        session_working_dir
            .as_deref()
            .unwrap_or("<none>"),
        cwd
    )
}

async fn wait_for_reloading_server() -> bool {
    match crate::server::await_reload_handoff(
        &crate::server::socket_path(),
        std::time::Duration::from_secs(30),
    )
    .await
    {
        crate::server::ReloadWaitStatus::Ready => true,
        crate::server::ReloadWaitStatus::Failed(detail) => {
            logging::warn(&format!(
                "Reload handoff failed while resuming self-dev session on {}: {}; recent_state={}",
                crate::server::socket_path().display(),
                detail.unwrap_or_else(|| "unknown reload failure".to_string()),
                crate::server::reload_state_summary(std::time::Duration::from_secs(60))
            ));
            false
        }
        crate::server::ReloadWaitStatus::Idle => false,
        crate::server::ReloadWaitStatus::Waiting { .. } => false,
    }
}

pub async fn run_self_dev(should_build: bool, resume_session: Option<String>) -> Result<()> {
    startup_profile::mark("run_self_dev_enter");
    crate::env::set_var(CLIENT_SELFDEV_ENV, "1");

    let repo_dir = resolve_selfdev_cli_repo_dir(resume_session.as_deref())
        .ok_or_else(|| selfdev_repo_discovery_error(resume_session.as_deref()))?;

    startup_profile::mark("selfdev_session_create");
    let is_resume = resume_session.is_some();
    let session_id = if let Some(id) = resume_session {
        if let Ok(mut session) = session::Session::load(&id)
            && !session.is_canary
        {
            session.set_canary("self-dev");
            let _ = session.save();
        }
        id
    } else {
        let mut session =
            session::Session::create(None, Some("Self-development session".to_string()));
        session.set_canary("self-dev");
        session.id.clone()
    };

    crate::process_title::set_client_session_title(&session_id, true);

    if should_build {
        let source = build::current_source_state(&repo_dir)?;
        let build = build::selfdev_build_command(&repo_dir);
        output::stderr_info(format!("Building with {}...", build.display));

        build::run_selfdev_build(&repo_dir)?;
        build::ensure_source_state_matches(&repo_dir, &source)?;

        build::publish_local_current_build_for_source(&repo_dir, &source)?;

        output::stderr_info("✓ Build complete; updated current launcher");
    }

    let target_binary = build::client_update_candidate(true)
        .map(|(path, _)| path)
        .or_else(|| build::find_dev_binary(&repo_dir))
        .unwrap_or_else(|| build::release_binary_path(&repo_dir));

    if !target_binary.exists() {
        anyhow::bail!(
            "No binary found at {:?}\n\
             Run 'jcode self-dev --build' first, or build with '{}' and then publish current.",
            target_binary,
            build::selfdev_build_command(&repo_dir).display,
        );
    }

    let hash = build::current_git_hash(&repo_dir)?;
    startup_profile::mark("selfdev_git_hash");

    if !is_resume {
        output::stderr_info(format!("Starting self-dev session with {}...", hash));
    } else {
        logging::info(&format!("Resuming self-dev session with {}...", hash));
    }

    if is_resume {
        crate::env::set_var("JCODE_RESUMING", "1");
    }

    let mut server_running = super::dispatch::server_is_running().await;
    if !server_running && std::env::var("JCODE_RESUMING").is_ok() {
        if let Some(state) = crate::server::recent_reload_state(std::time::Duration::from_secs(30))
        {
            match state.phase {
                crate::server::ReloadPhase::Starting => {
                    logging::info(
                        "Reload state=starting while resuming self-dev session; waiting for existing server to come back",
                    );
                    server_running = wait_for_reloading_server().await;
                }
                crate::server::ReloadPhase::Failed => {
                    if let Ok(Some(version)) =
                        build::rollback_pending_activation_for_session(&session_id)
                    {
                        logging::warn(&format!(
                            "Rolled back failed pending activation for build {} while resuming self-dev session",
                            version
                        ));
                    }
                    logging::warn(&format!(
                        "Reload state=failed while resuming self-dev session on {}: {}; recent_state={}",
                        crate::server::socket_path().display(),
                        state
                            .detail
                            .unwrap_or_else(|| "unknown reload failure".to_string()),
                        crate::server::reload_state_summary(std::time::Duration::from_secs(60))
                    ));
                }
                crate::server::ReloadPhase::SocketReady => {}
            }
        }

        if !server_running {
            server_running = super::dispatch::wait_for_resuming_server(
                "self-dev resume without reload marker",
                std::time::Duration::from_secs(5),
            )
            .await;
        }
    }

    if server_running
        && let Ok(Some(version)) = build::complete_pending_activation_for_session(&session_id)
    {
        logging::info(&format!(
            "Marked pending self-dev activation as successful for build {}",
            version
        ));
    }

    if !server_running {
        super::dispatch::maybe_prompt_server_bootstrap_login(&ProviderChoice::Auto).await?;
        super::dispatch::spawn_server(&ProviderChoice::Auto, None, None).await?;
    }

    if std::env::var("JCODE_RESUMING").is_err() && server_running {
        output::stderr_info("Connecting to shared server...");
    }

    output::stderr_info("Starting self-dev TUI...");

    super::tui_launch::run_tui_client(Some(session_id), None, !server_running, false).await
}
#[cfg(test)]
#[path = "selfdev_tests.rs"]
mod selfdev_tests;
