use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus};

use crate::bus::{Bus, BusEvent, ClientMaintenanceAction, SessionUpdateStatus};
use crate::{build, update};

pub fn hot_rebuild(session_id: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_dir =
        build::get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    eprintln!("Rebuilding jcode with session {}...", session_id);
    pull_latest_changes_for_rebuild(&repo_dir);
    run_release_build(&repo_dir)?;
    run_release_tests(&repo_dir)?;
    install_local_release_with_warning(&repo_dir);

    let is_selfdev = crate::cli::selfdev::client_selfdev_requested();
    let exe = rebuild_reload_candidate(&repo_dir, is_selfdev);
    if !exe.exists() {
        anyhow::bail!("Binary not found at {:?}", exe);
    }

    update::print_centered(&format!("Restarting with session {}...", session_id));
    exec_rebuilt_session(&exe, session_id, &cwd, is_selfdev)
}

pub fn spawn_background_session_rebuild(session_id: String) {
    std::thread::spawn(move || run_background_session_rebuild(session_id));
}

fn pull_latest_changes_for_rebuild(repo_dir: &Path) {
    eprintln!("Pulling latest changes...");
    if let Err(e) = update::run_git_pull_ff_only(repo_dir, true) {
        eprintln!("Warning: {}. Continuing with current version.", e);
    }
}

fn run_release_build(repo_dir: &Path) -> Result<()> {
    eprintln!("Building...");
    let status = run_cargo_release_step(repo_dir, &["build", "--release"])?;
    if !status.success() {
        anyhow::bail!("Build failed - staying on current version");
    }
    Ok(())
}

fn run_release_tests(repo_dir: &Path) -> Result<()> {
    eprintln!("Running tests...");
    let status =
        run_cargo_release_step(repo_dir, &["test", "--release", "--", "--test-threads=1"])?;
    if !status.success() {
        eprintln!("\n⚠️  Tests failed! Aborting reload to protect your session.");
        eprintln!("Fix the failing tests and try /rebuild again.");
        anyhow::bail!("Tests failed - staying on current version");
    }
    eprintln!("✓ All tests passed");
    Ok(())
}

fn run_cargo_release_step(repo_dir: &Path, args: &[&str]) -> Result<ExitStatus> {
    Ok(ProcessCommand::new("cargo")
        .args(args)
        .current_dir(repo_dir)
        .status()?)
}

fn install_local_release_with_warning(repo_dir: &Path) {
    if let Err(e) = build::install_local_release(repo_dir) {
        eprintln!("Warning: install failed: {}", e);
    }
}

fn rebuild_reload_candidate(repo_dir: &Path, is_selfdev: bool) -> PathBuf {
    build::client_update_candidate(is_selfdev)
        .map(|(path, _)| path)
        .unwrap_or_else(|| build::release_binary_path(repo_dir))
}

fn exec_rebuilt_session(exe: &Path, session_id: &str, cwd: &Path, is_selfdev: bool) -> Result<()> {
    crate::env::set_var("JCODE_RESUMING", "1");

    let mut cmd = ProcessCommand::new(exe);
    if is_selfdev {
        cmd.arg("self-dev");
    }
    cmd.arg("--resume").arg(session_id).current_dir(cwd);
    let err = crate::platform::replace_process(&mut cmd);

    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

fn run_background_session_rebuild(session_id: String) {
    let publisher = BackgroundRebuildPublisher::new(session_id);
    let Some(repo_dir) = build::get_repo_dir() else {
        publisher.error("Rebuild failed: could not find the jcode repository.");
        return;
    };

    background_pull_latest_changes(&publisher, &repo_dir);
    if !background_release_build(&publisher, &repo_dir) {
        return;
    }
    if !background_release_tests(&publisher, &repo_dir) {
        return;
    }
    background_install_local_release(&publisher, &repo_dir);
    publish_rebuild_ready_or_error(publisher, &repo_dir);
}

#[derive(Clone)]
struct BackgroundRebuildPublisher {
    session_id: String,
    action: ClientMaintenanceAction,
}

impl BackgroundRebuildPublisher {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            action: ClientMaintenanceAction::Rebuild,
        }
    }

    fn status(&self, message: impl Into<String>) {
        self.publish(SessionUpdateStatus::Status {
            session_id: self.session_id.clone(),
            action: self.action,
            message: message.into(),
        });
    }

    fn error(&self, message: impl Into<String>) {
        self.publish(SessionUpdateStatus::Error {
            session_id: self.session_id.clone(),
            action: self.action,
            message: message.into(),
        });
    }

    fn ready(self, repo_dir: &Path) {
        Bus::global().publish(BusEvent::SessionUpdateStatus(
            SessionUpdateStatus::ReadyToReload {
                session_id: self.session_id,
                action: self.action,
                version: rebuild_version_label(repo_dir),
            },
        ));
    }

    fn publish(&self, status: SessionUpdateStatus) {
        Bus::global().publish(BusEvent::SessionUpdateStatus(status));
    }
}

fn background_pull_latest_changes(publisher: &BackgroundRebuildPublisher, repo_dir: &Path) {
    publisher.status("Pulling latest changes in the background...");
    if let Err(error) = update::run_git_pull_ff_only(repo_dir, true) {
        publisher.status(format!(
            "Git pull skipped: {}. Continuing with the current checkout.",
            error
        ));
    }
}

fn background_release_build(publisher: &BackgroundRebuildPublisher, repo_dir: &Path) -> bool {
    publisher.status("Building release binary in the background...");
    let status = match run_cargo_release_step(repo_dir, &["build", "--release"]) {
        Ok(status) => status,
        Err(error) => {
            publisher.error(format!(
                "Rebuild failed while starting cargo build: {}",
                error
            ));
            return false;
        }
    };

    if !status.success() {
        publisher.error("Build failed — staying on the current binary.");
        return false;
    }
    true
}

fn background_release_tests(publisher: &BackgroundRebuildPublisher, repo_dir: &Path) -> bool {
    publisher.status("Running release tests in the background...");
    let status =
        match run_cargo_release_step(repo_dir, &["test", "--release", "--", "--test-threads=1"]) {
            Ok(status) => status,
            Err(error) => {
                publisher.error(format!("Rebuild failed while starting tests: {}", error));
                return false;
            }
        };

    if !status.success() {
        publisher.error(
            "Tests failed — staying on the current binary. Fix the failing tests and try /rebuild again.",
        );
        return false;
    }
    true
}

fn background_install_local_release(publisher: &BackgroundRebuildPublisher, repo_dir: &Path) {
    if let Err(error) = build::install_local_release(repo_dir) {
        publisher.status(format!(
            "Install warning: {}. Will reload from the repo build if needed.",
            error
        ));
    }
}

fn publish_rebuild_ready_or_error(publisher: BackgroundRebuildPublisher, repo_dir: &Path) {
    let is_selfdev = crate::cli::selfdev::client_selfdev_requested();
    let exe = build::preferred_reload_candidate(is_selfdev)
        .map(|(path, _)| path)
        .unwrap_or_else(|| build::release_binary_path(repo_dir));
    if !exe.exists() {
        publisher.error(format!(
            "Rebuild finished but no reloadable binary was found at {:?}.",
            exe
        ));
        return;
    }

    publisher.ready(repo_dir);
}

fn rebuild_version_label(repo_dir: &Path) -> String {
    build::current_build_info(repo_dir)
        .map(|info| {
            if info.dirty {
                format!("{}-dirty", info.hash)
            } else {
                info.hash
            }
        })
        .unwrap_or_else(|_| "local source build".to_string())
}
