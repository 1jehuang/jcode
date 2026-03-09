use anyhow::Result;
use std::process::Command as ProcessCommand;

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
    let is_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();

    crate::logging::info(&format!("Restarting with current binary: {:?}", exe));

    std::env::set_var("JCODE_RESUMING", "1");

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

    std::env::set_var("JCODE_RESUMING", "1");

    if let Ok(migrate_binary) = std::env::var("JCODE_MIGRATE_BINARY") {
        let binary_path = std::path::PathBuf::from(&migrate_binary);
        if binary_path.exists() {
            crate::logging::info("Migrating to stable binary...");
            let err = crate::platform::replace_process(
                ProcessCommand::new(&binary_path)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("--no-update")
                    .current_dir(cwd),
            );
            return Err(anyhow::anyhow!("Failed to exec {:?}: {}", binary_path, err));
        } else {
            crate::logging::warn(&format!(
                "Migration binary not found at {:?}, falling back to local binary",
                binary_path
            ));
        }
    }

    let is_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();
    let (exe, _label) = build::client_update_candidate(is_selfdev)
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
        cmd.arg("--resume").arg(session_id).current_dir(&cwd);
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

pub fn hot_rebuild(session_id: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_dir =
        build::get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    eprintln!("Rebuilding jcode with session {}...", session_id);

    eprintln!("Pulling latest changes...");
    if let Err(e) = update::run_git_pull_ff_only(&repo_dir, true) {
        eprintln!("Warning: {}. Continuing with current version.", e);
    }

    eprintln!("Building...");
    let build_status = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build_status.success() {
        anyhow::bail!("Build failed - staying on current version");
    }

    eprintln!("Running tests...");
    let test = ProcessCommand::new("cargo")
        .args(["test", "--release", "--", "--test-threads=1"])
        .current_dir(&repo_dir)
        .status()?;

    if !test.success() {
        eprintln!("\n⚠️  Tests failed! Aborting reload to protect your session.");
        eprintln!("Fix the failing tests and try /rebuild again.");
        anyhow::bail!("Tests failed - staying on current version");
    }

    eprintln!("✓ All tests passed");

    if let Err(e) = build::install_local_release(&repo_dir) {
        eprintln!("Warning: install failed: {}", e);
    }

    let is_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();
    let exe = build::client_update_candidate(is_selfdev)
        .map(|(path, _)| path)
        .unwrap_or_else(|| build::release_binary_path(&repo_dir));
    if !exe.exists() {
        anyhow::bail!("Binary not found at {:?}", exe);
    }

    update::print_centered(&format!("Restarting with session {}...", session_id));

    std::env::set_var("JCODE_RESUMING", "1");

    let mut cmd = ProcessCommand::new(&exe);
    if is_selfdev {
        cmd.arg("self-dev");
    }
    cmd.arg("--resume").arg(session_id).current_dir(&cwd);
    let err = crate::platform::replace_process(&mut cmd);

    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

pub fn hot_update(session_id: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;

    update::print_centered("Checking for updates...");

    match update::check_for_update_blocking() {
        Ok(Some(release)) => {
            let current = env!("JCODE_VERSION");
            update::print_centered(&format!(
                "Update available: {} -> {}",
                current, release.tag_name
            ));
            update::print_centered(&format!("Downloading {}...", release.tag_name));

            match update::download_and_install_blocking(&release) {
                Ok(path) => {
                    update::print_centered(&format!("✓ Installed {}", release.tag_name));

                    let is_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();
                    let exe = build::client_update_candidate(is_selfdev)
                        .map(|(p, _)| p)
                        .unwrap_or(path);

                    update::print_centered(&format!("Restarting with session {}...", session_id));

                    std::env::set_var("JCODE_RESUMING", "1");

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
                    update::print_centered(&format!("✗ Download failed: {}", e));
                    update::print_centered("Resuming session with current version...");
                }
            }
        }
        Ok(None) => {
            update::print_centered(&format!("Already up to date ({})", env!("JCODE_VERSION")));
        }
        Err(e) => {
            update::print_centered(&format!("✗ Update check failed: {}", e));
            update::print_centered("Resuming session with current version...");
        }
    }

    std::env::set_var("JCODE_RESUMING", "1");
    let exe = std::env::current_exe()?;
    let is_selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();
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

pub fn check_for_updates() -> Option<bool> {
    let repo_dir = get_repo_dir()?;

    let fetch = ProcessCommand::new("git")
        .args(["fetch", "-q"])
        .current_dir(&repo_dir)
        .output()
        .ok()?;

    if !fetch.status.success() {
        return None;
    }

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

pub fn run_auto_update() -> Result<()> {
    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    update::run_git_pull_ff_only(&repo_dir, true)?;

    update::print_centered("Building new version...");
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
    update::print_centered(&format!("Updated to {}. Restarting...", hash.trim()));

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
                    env!("JCODE_VERSION"),
                    release.tag_name
                ));
                let _path = update::download_and_install_blocking(&release)?;
                update::print_centered(&format!("✅ Updated to {}", release.tag_name));
                update::print_centered("Restart jcode to use the new version.");
            }
            Ok(None) => {
                update::print_centered(&format!("Already up to date ({})", env!("JCODE_VERSION")));
            }
            Err(e) => {
                anyhow::bail!("Update check failed: {}", e);
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
