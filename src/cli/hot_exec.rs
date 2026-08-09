use anyhow::Result;
use std::process::Command as ProcessCommand;

pub use crate::session_rebuild::{hot_rebuild, spawn_background_session_rebuild};

use crate::{build, tui::RunResult, update};

pub fn has_requested_action(run_result: &RunResult) -> bool {
    run_result.reload_session.is_some()
        || run_result.rebuild_session.is_some()
        || run_result.update_session.is_some()
        || run_result.restart_session.is_some()
}

pub fn execute_requested_action(run_result: &RunResult) -> Result<()> {
    if let Some(ref reload_session_id) = run_result.reload_session {
        hot_reload(reload_session_id)?;
    }

    if let Some(ref rebuild_session_id) = run_result.rebuild_session {
        hot_rebuild(rebuild_session_id)?;
    }

    if let Some(ref update_session_id) = run_result.update_session {
        hot_update(update_session_id)?;
    }

    if let Some(ref restart_session_id) = run_result.restart_session {
        hot_restart(restart_session_id)?;
    }

    Ok(())
}

pub fn hot_restart(session_id: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let exe = std::env::current_exe()?;
    let is_selfdev = crate::cli::selfdev::client_selfdev_requested();

    crate::logging::info(&format!("Restarting with current binary: {:?}", exe));

    crate::env::set_var("JCODE_RESUMING", "1");

    let mut cmd = ProcessCommand::new(&exe);
    if is_selfdev {
        cmd.arg("self-dev");
    }
    cmd.arg("--resume").arg(session_id).current_dir(&cwd);
    let err = crate::platform::replace_process(&mut cmd);

    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

pub fn hot_reload(session_id: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;

    crate::env::set_var("JCODE_RESUMING", "1");

    if let Ok(migrate_binary) = std::env::var("JCODE_MIGRATE_BINARY") {
        let binary_path = std::path::PathBuf::from(&migrate_binary);
        if binary_path.exists() {
            crate::logging::info("Migrating to stable binary...");
            let mut cmd = ProcessCommand::new(&binary_path);
            cmd.arg("--resume")
                .arg(session_id)
                .arg("--no-update")
                .env_remove("JCODE_MIGRATE_BINARY")
                .current_dir(cwd);
            let err = crate::platform::replace_process(&mut cmd);
            return Err(anyhow::anyhow!("Failed to exec {:?}: {}", binary_path, err));
        } else {
            crate::logging::warn(&format!(
                "Migration binary not found at {:?}, falling back to local binary",
                binary_path
            ));
        }
    }

    let is_selfdev = crate::cli::selfdev::client_selfdev_requested();
    let (exe, _label) = build::preferred_reload_candidate(is_selfdev)
        .ok_or_else(|| anyhow::anyhow!("No reloadable binary found"))?;

    if let Ok(metadata) = std::fs::metadata(&exe) {
        let age = metadata
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .map(|d| {
                let secs = d.as_secs();
                if secs < 60 {
                    format!("{} seconds ago", secs)
                } else if secs < 3600 {
                    format!("{} minutes ago", secs / 60)
                } else {
                    format!("{} hours ago", secs / 3600)
                }
            })
            .unwrap_or_else(|| "unknown".to_string());
        crate::logging::info(&format!("Reloading with binary built {}...", age));
    }

    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if !exe.exists() {
                continue;
            }
        }
        let mut cmd = ProcessCommand::new(&exe);
        if is_selfdev {
            cmd.arg("self-dev");
        }
        cmd.arg("--resume")
            .arg(session_id)
            // The server has already completed its handoff before the client
            // re-execs. Let the replacement client paint and accept input from
            // its startup stub immediately; the authoritative History payload
            // will repopulate the transcript after reconnect.
            .env("JCODE_RELOAD_FAST_START", "1")
            .current_dir(&cwd);
        let err = crate::platform::replace_process(&mut cmd);

        if err.kind() == std::io::ErrorKind::NotFound && attempt < 2 {
            crate::logging::warn(&format!(
                "exec attempt {} failed (ENOENT) for {:?}, retrying...",
                attempt + 1,
                exe
            ));
            continue;
        }
        return Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err));
    }
    Err(anyhow::anyhow!(
        "Failed to exec {:?}: binary not found after retries",
        exe
    ))
}

pub fn hot_update(session_id: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;

    update::print_centered("Checking for updates...");

    match update::check_for_update_blocking() {
        Ok(Some(release)) => {
            let current = jcode_build_meta::version();
            update::print_centered(&format!(
                "Update available: {} -> {}",
                current, release.tag_name
            ));
            update::print_centered(&format!("Downloading {}...", release.tag_name));

            match update::download_and_install_blocking_with_progress(&release, |progress| {
                update::print_centered(&format!(
                    "{} {}",
                    release.tag_name,
                    update::format_download_progress_bar(progress)
                ));
            }) {
                Ok(path) => {
                    update::print_centered(&format!("✓ Installed {}", release.tag_name));
                    reload_server_after_update("installed update");

                    let is_selfdev = crate::cli::selfdev::client_selfdev_requested();
                    let exe = build::client_update_candidate(is_selfdev)
                        .map(|(p, _)| p)
                        .unwrap_or(path);

                    update::print_centered(&format!("Restarting with session {}...", session_id));

                    crate::env::set_var("JCODE_RESUMING", "1");

                    let mut cmd = ProcessCommand::new(&exe);
                    if is_selfdev {
                        cmd.arg("self-dev");
                    }
                    cmd.arg("--resume")
                        .arg(session_id)
                        .arg("--no-update")
                        .current_dir(&cwd);
                    let err = crate::platform::replace_process(&mut cmd);
                    return Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err));
                }
                Err(e) => {
                    update::print_centered(&format!(
                        "✗ Download failed: {}",
                        update::summarize_update_error(&format!("{:#}", e))
                    ));
                }
            }
        }
        Ok(None) => {
            if repair_stale_shared_server_after_update_check() {
                reload_server_after_update("repaired stale server target");
            }
            update::print_centered(&format!(
                "Already up to date ({})",
                jcode_build_meta::version()
            ));
        }
        Err(e) => {
            update::print_centered(&format!(
                "✗ Update check failed: {}",
                update::summarize_update_error(&format!("{:#}", e))
            ));
        }
    }

    crate::env::set_var("JCODE_RESUMING", "1");
    let exe = std::env::current_exe()?;
    let is_selfdev = crate::cli::selfdev::client_selfdev_requested();
    let mut cmd = ProcessCommand::new(&exe);
    if is_selfdev {
        cmd.arg("self-dev");
    }
    cmd.arg("--resume")
        .arg(session_id)
        .arg("--no-update")
        .current_dir(&cwd);
    let err = crate::platform::replace_process(&mut cmd);
    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

pub fn get_repo_dir() -> Option<std::path::PathBuf> {
    build::get_repo_dir()
}

/// Minimum interval between `git fetch` update probes across all jcode
/// processes. Every source-build client spawn used to fetch unconditionally,
/// so spawning N clients at once ran N concurrent `git fetch` + ssh sessions
/// against the remote. One probe per interval per machine is plenty; a marker
/// file's mtime coordinates it (same pattern as the session-backup pruner).
const UPDATE_FETCH_INTERVAL_SECS: u64 = 15 * 60;

fn claim_update_fetch_slot() -> bool {
    let Ok(base) = crate::storage::jcode_dir() else {
        // Cannot coordinate without a home dir; fall back to probing.
        return true;
    };
    let marker = base.join("update-fetch.stamp");
    if let Ok(metadata) = std::fs::metadata(&marker)
        && let Ok(modified) = metadata.modified()
        && let Ok(age) = std::time::SystemTime::now().duration_since(modified)
        && age.as_secs() < UPDATE_FETCH_INTERVAL_SECS
    {
        return false;
    }
    // Touch before fetching so a spawn burst collapses to ~one fetch.
    std::fs::write(&marker, b"").is_ok()
}

pub fn check_for_updates() -> Option<bool> {
    let repo_dir = get_repo_dir()?;

    if claim_update_fetch_slot() {
        let fetch = ProcessCommand::new("git")
            .args(["fetch", "-q"])
            .current_dir(&repo_dir)
            .output()
            .ok()?;

        if !fetch.status.success() {
            return None;
        }
    }
    // When the fetch slot was claimed by another recent process, still answer
    // from the (fresh enough) local refs instead of skipping the check.

    let behind = ProcessCommand::new("git")
        .args(["rev-list", "--count", "HEAD..@{u}"])
        .current_dir(&repo_dir)
        .output()
        .ok()?;

    if behind.status.success() {
        let count: u32 = String::from_utf8_lossy(&behind.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Some(count > 0)
    } else {
        None
    }
}

/// True when the source checkout has local commits that upstream lacks, so a
/// fast-forward pull (and therefore auto-update) can never succeed. Returns
/// `None` when the repo or upstream cannot be inspected.
pub fn local_commits_ahead_of_upstream() -> Option<bool> {
    let repo_dir = get_repo_dir()?;
    let ahead = ProcessCommand::new("git")
        .args(["rev-list", "--count", "@{u}..HEAD"])
        .current_dir(&repo_dir)
        .output()
        .ok()?;
    if !ahead.status.success() {
        return None;
    }
    let count: u32 = String::from_utf8_lossy(&ahead.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    Some(count > 0)
}

pub fn run_auto_update() -> Result<()> {
    use crate::bus::{Bus, BusEvent, UpdateStatus};

    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    update::run_git_pull_ff_only(&repo_dir, true)?;

    crate::logging::info("Building updated source version...");
    let build_output = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .output()?;

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        let stdout = String::from_utf8_lossy(&build_output.stdout);
        if !stderr.trim().is_empty() {
            crate::logging::error(&format!("auto-update cargo stderr:\n{}", stderr.trim()));
        }
        if !stdout.trim().is_empty() {
            crate::logging::info(&format!("auto-update cargo stdout:\n{}", stdout.trim()));
        }
        anyhow::bail!("cargo build failed");
    }

    if let Err(e) = build::install_local_release(&repo_dir) {
        crate::logging::warn(&format!("auto-update install failed: {}", e));
    }

    let hash = ProcessCommand::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&repo_dir)
        .output()?;
    let hash = String::from_utf8_lossy(&hash.stdout);
    let version = format!("main-{}", hash.trim());
    Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Installed {
        version: version.clone(),
    }));

    // With a live TUI session, let the app reload gracefully (input line
    // saved, current turn finished, session resumed) instead of exec-ing over
    // the running interface, which visibly resets the screen.
    if let Some(session_id) = crate::get_current_session() {
        use crate::bus::{ClientMaintenanceAction, SessionUpdateStatus};
        crate::logging::info(&format!(
            "Updated to {}. Requesting graceful session reload...",
            version
        ));
        Bus::global().publish(BusEvent::SessionUpdateStatus(
            SessionUpdateStatus::ReadyToReload {
                session_id,
                action: ClientMaintenanceAction::Update,
                version,
            },
        ));
        return Ok(());
    }

    crate::logging::info(&format!("Updated to {}. Restarting...", version));
    std::thread::sleep(std::time::Duration::from_millis(250));

    let exe = build::client_update_candidate(false)
        .map(|(p, _)| p)
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| anyhow::anyhow!("No executable path found after update"))?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    let err =
        crate::platform::replace_process(ProcessCommand::new(&exe).args(&args).arg("--no-update"));

    Err(anyhow::anyhow!(
        "Failed to exec new binary {:?}: {}",
        exe,
        err
    ))
}

pub fn run_update() -> Result<()> {
    if update::is_release_build() {
        update::print_centered("Checking GitHub for latest release...");
        match update::check_for_update_blocking() {
            Ok(Some(release)) => {
                update::print_centered(&format!(
                    "Downloading {} \u{2192} {}...",
                    jcode_build_meta::version(),
                    release.tag_name
                ));
                let _path =
                    update::download_and_install_blocking_with_progress(&release, |progress| {
                        update::print_centered(&format!(
                            "{} {}",
                            release.tag_name,
                            update::format_download_progress_bar(progress)
                        ));
                    })?;
                update::print_centered(&format!("✅ Updated to {}", release.tag_name));
                reload_server_after_update("installed update");
                update::print_centered("Restart jcode to use the new version.");
            }
            Ok(None) => {
                if repair_stale_shared_server_after_update_check() {
                    reload_server_after_update("repaired stale server target");
                }
                update::print_centered(&format!(
                    "Already up to date ({})",
                    jcode_build_meta::version()
                ));
            }
            Err(e) => {
                anyhow::bail!(
                    "Update check failed: {}",
                    update::summarize_update_error(&format!("{:#}", e))
                );
            }
        }
        return Ok(());
    }

    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    update::print_centered(&format!("Updating jcode from {}...", repo_dir.display()));

    update::print_centered("Pulling latest changes (fast-forward only)...");
    update::run_git_pull_ff_only(&repo_dir, true)?;

    update::print_centered("Building...");
    let build_status = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build_status.success() {
        anyhow::bail!("cargo build failed");
    }

    if let Err(e) = build::install_local_release(&repo_dir) {
        update::print_centered(&format!("Warning: install failed: {}", e));
    }

    let hash = ProcessCommand::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&repo_dir)
        .output()?;

    let hash = String::from_utf8_lossy(&hash.stdout);
    update::print_centered(&format!("Successfully updated to {}", hash.trim()));

    Ok(())
}

fn repair_stale_shared_server_after_update_check() -> bool {
    match build::repair_stale_shared_server_channel() {
        Ok(build::SharedServerRepair::Repaired {
            previous,
            repaired_to,
        }) => {
            crate::logging::info(&format!(
                "update: repaired stale shared-server channel {:?} -> {}",
                previous, repaired_to
            ));
            update::print_centered(&format!(
                "Repaired stale server reload target: {}",
                repaired_to
            ));
            true
        }
        Ok(build::SharedServerRepair::AlreadyCurrent) => false,
        Err(error) => {
            crate::logging::warn(&format!(
                "update: failed to repair stale shared-server channel: {}",
                error
            ));
            false
        }
    }
}

fn reload_server_after_update(reason: &str) {
    let exe = build::client_update_candidate(false)
        .map(|(path, _)| path)
        .or_else(|| std::env::current_exe().ok());
    let Some(exe) = exe else {
        crate::logging::warn("update: could not find jcode binary to reload stale server");
        return;
    };

    let output = ProcessCommand::new(&exe)
        .args(["--no-update", "server", "reload", "--force"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            crate::logging::info(&format!(
                "update: requested server reload after {} via {:?}",
                reason, exe
            ));
        }
        Ok(output) => {
            crate::logging::warn(&format!(
                "update: server reload after {} failed with status {:?}: {}",
                reason,
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "update: failed to request server reload after {} via {:?}: {}",
                reason, exe, error
            ));
        }
    }
}
