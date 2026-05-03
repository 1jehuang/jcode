use super::{
    SelfDevBuildCommand, SelfDevBuildTarget, canary_binary_path, current_binary_path,
    shared_server_binary_path, stable_binary_path,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use jcode_storage as storage;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Get the jcode repository directory
pub fn get_repo_dir() -> Option<PathBuf> {
    // First try: compile-time directory
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir);
    if is_jcode_repo(&path) {
        return Some(path);
    }

    // Fallback: check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        // Assume structure: repo/target/<profile>/<binary> (platform-specific executable name)
        if let Some(repo) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            && is_jcode_repo(repo)
        {
            return Some(repo.to_path_buf());
        }
    }

    // Final fallback: search upward from current working directory.
    // This matters for self-dev sessions launched from the repo but running
    // from an installed canary/stable binary whose current_exe() is outside
    // the source tree.
    if let Ok(cwd) = std::env::current_dir()
        && let Some(repo) = find_repo_in_ancestors(&cwd)
    {
        return Some(repo);
    }

    None
}

pub fn find_repo_in_ancestors(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if is_jcode_repo(dir) {
            return Some(dir.to_path_buf());
        }
    }
    None
}

pub fn binary_stem() -> &'static str {
    "jcode"
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "jcode.exe"
    } else {
        binary_stem()
    }
}

pub const SELFDEV_CARGO_PROFILE: &str = "selfdev";

fn profile_binary_path(repo_dir: &Path, profile: &str) -> PathBuf {
    repo_dir.join("target").join(profile).join(binary_name())
}

pub fn release_binary_path(repo_dir: &Path) -> PathBuf {
    profile_binary_path(repo_dir, "release")
}

pub fn selfdev_binary_path(repo_dir: &Path) -> PathBuf {
    profile_binary_path(repo_dir, SELFDEV_CARGO_PROFILE)
}

fn binary_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

fn newest_existing_binary(
    candidates: Vec<(PathBuf, &'static str)>,
) -> Option<(PathBuf, &'static str)> {
    candidates
        .into_iter()
        .filter(|(path, _)| path.exists())
        .max_by_key(|(path, _)| binary_mtime(path))
}

fn existing_binary(path: Result<PathBuf>, label: &'static str) -> Option<(PathBuf, &'static str)> {
    path.ok()
        .filter(|path| path.exists())
        .map(|path| (path, label))
}

pub fn selfdev_build_command(repo_dir: &Path) -> SelfDevBuildCommand {
    selfdev_build_command_for_target(repo_dir, SelfDevBuildTarget::Auto)
}

pub fn selfdev_build_command_for_target(
    repo_dir: &Path,
    target: SelfDevBuildTarget,
) -> SelfDevBuildCommand {
    let target = match target {
        SelfDevBuildTarget::Auto => infer_selfdev_build_target(repo_dir),
        explicit => explicit,
    };
    let specs = match target {
        SelfDevBuildTarget::Tui => vec![("jcode", "jcode")],
        SelfDevBuildTarget::Desktop => vec![("jcode-desktop", "jcode-desktop")],
        SelfDevBuildTarget::All | SelfDevBuildTarget::Auto => {
            vec![("jcode", "jcode"), ("jcode-desktop", "jcode-desktop")]
        }
    };
    let wrapper = repo_dir.join("scripts").join("dev_cargo.sh");
    if wrapper.is_file() {
        let script = wrapper.to_string_lossy();
        let command = specs
            .iter()
            .map(|(package, binary)| {
                format!(
                    "{} build --profile {} -p {} --bin {}",
                    shell_escape(&script),
                    SELFDEV_CARGO_PROFILE,
                    package,
                    binary
                )
            })
            .collect::<Vec<_>>()
            .join(" && ");
        return SelfDevBuildCommand {
            program: "bash".to_string(),
            args: vec!["-lc".to_string(), command],
            display: display_build_command("scripts/dev_cargo.sh", &specs),
        };
    }

    let command = display_build_command("cargo", &specs);
    SelfDevBuildCommand {
        program: "bash".to_string(),
        args: vec!["-lc".to_string(), command.clone()],
        display: command,
    }
}

fn display_build_command(program: &str, specs: &[(&str, &str)]) -> String {
    specs
        .iter()
        .map(|(package, binary)| {
            format!(
                "{} build --profile {} -p {} --bin {}",
                program, SELFDEV_CARGO_PROFILE, package, binary
            )
        })
        .collect::<Vec<_>>()
        .join(" && ")
}

fn infer_selfdev_build_target(repo_dir: &Path) -> SelfDevBuildTarget {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(repo_dir)
        .output();
    let Ok(output) = output else {
        return SelfDevBuildTarget::Tui;
    };
    if !output.status.success() {
        return SelfDevBuildTarget::Tui;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut desktop = false;
    let mut other = false;
    for line in text.lines() {
        let path = line
            .get(3..)
            .unwrap_or(line)
            .trim()
            .rsplit_once(" -> ")
            .map(|(_, new_path)| new_path)
            .unwrap_or_else(|| line.get(3..).unwrap_or(line).trim());
        if path == "Cargo.toml" || path == "Cargo.lock" || path.starts_with(".cargo/") {
            desktop = true;
            other = true;
        } else if path.starts_with("crates/jcode-desktop/") {
            desktop = true;
        } else if !path.is_empty() {
            other = true;
        }
    }
    match (desktop, other) {
        (true, false) => SelfDevBuildTarget::Desktop,
        (false, true) => SelfDevBuildTarget::Tui,
        (true, true) => SelfDevBuildTarget::All,
        (false, false) => SelfDevBuildTarget::Tui,
    }
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn run_selfdev_build(repo_dir: &Path) -> Result<SelfDevBuildCommand> {
    let source = super::current_source_state(repo_dir)?;
    let build = selfdev_build_command(repo_dir);
    let status = Command::new(&build.program)
        .args(&build.args)
        .current_dir(repo_dir)
        .status()?;

    if !status.success() {
        anyhow::bail!("Build failed: {}", build.display);
    }

    let source_after_build = super::ensure_source_state_matches(repo_dir, &source)?;
    super::write_current_dev_binary_source_metadata(repo_dir, &source_after_build)?;

    Ok(build)
}

pub fn current_binary_built_at() -> Option<DateTime<Utc>> {
    let modified: SystemTime = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())?;
    Some(DateTime::<Utc>::from(modified))
}

pub fn current_binary_build_time_string() -> Option<String> {
    current_binary_built_at().map(|dt| dt.format("%Y-%m-%d %H:%M:%S %z").to_string())
}

/// Find the best development binary in the repo.
/// Prefers the newest local self-dev or release binary.
pub fn find_dev_binary(repo_dir: &Path) -> Option<PathBuf> {
    newest_existing_binary(vec![
        (selfdev_binary_path(repo_dir), "repo-selfdev"),
        (release_binary_path(repo_dir), "repo-release"),
    ])
    .map(|(path, _)| path)
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| anyhow::anyhow!("HOME/USERPROFILE not set"))
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Directory for the single launcher path users execute from PATH.
///
/// Defaults to `~/.local/bin` on Unix, `%LOCALAPPDATA%\jcode\bin` on Windows.
/// Overridable with `JCODE_INSTALL_DIR`.
pub fn launcher_dir() -> Result<PathBuf> {
    if let Some(custom) = non_empty_env_path("JCODE_INSTALL_DIR") {
        return Ok(custom);
    }

    if let Some(sandbox_home) = non_empty_env_path("JCODE_HOME") {
        return Ok(sandbox_home.join("bin"));
    }

    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return Ok(PathBuf::from(local).join("jcode").join("bin"));
        }
        Ok(home_dir()?
            .join("AppData")
            .join("Local")
            .join("jcode")
            .join("bin"))
    }
    #[cfg(not(windows))]
    {
        Ok(home_dir()?.join(".local").join("bin"))
    }
}

/// Path to the launcher binary (`~/.local/bin/jcode` by default).
pub fn launcher_binary_path() -> Result<PathBuf> {
    Ok(launcher_dir()?.join(binary_name()))
}

fn update_launcher_symlink(target: &Path) -> Result<PathBuf> {
    let launcher = launcher_binary_path()?;

    if let Some(parent) = launcher.parent() {
        storage::ensure_dir(parent)?;
    }

    let temp = launcher
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{}-launcher-{}",
            binary_stem(),
            std::process::id()
        ));

    crate::platform_support::atomic_symlink_swap(target, &launcher, &temp)?;
    Ok(launcher)
}

/// Update launcher path to point at the current channel binary.
pub fn update_launcher_symlink_to_current() -> Result<PathBuf> {
    let current = current_binary_path()?;
    update_launcher_symlink(&current)
}

/// Update launcher path to point at the stable channel binary.
pub fn update_launcher_symlink_to_stable() -> Result<PathBuf> {
    let stable = stable_binary_path()?;
    update_launcher_symlink(&stable)
}

/// Resolve which client binary should be considered for launches, updates, and reloads.
///
/// Order matters:
/// - Prefer the published `current` channel first (active local build)
/// - Self-dev sessions can fall back to an unpublished repo build from `target/selfdev` or `target/release`
/// - Then the self-dev canary channel
/// - Then launcher path
/// - Then stable channel path
/// - Finally currently running executable
pub fn client_update_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    if let Some(current) = existing_binary(current_binary_path(), "current") {
        return Some(current);
    }

    if is_selfdev_session {
        if let Some(repo_dir) = get_repo_dir()
            && let Some(dev) = find_dev_binary(&repo_dir)
            && dev.exists()
        {
            return Some((dev, "dev"));
        }
        if let Some(canary) = existing_binary(canary_binary_path(), "canary") {
            return Some(canary);
        }
    }

    if let Some(launcher) = existing_binary(launcher_binary_path(), "launcher") {
        return Some(launcher);
    }

    if let Some(stable) = existing_binary(stable_binary_path(), "stable") {
        return Some(stable);
    }

    std::env::current_exe().ok().map(|exe| (exe, "current"))
}

/// Resolve the binary that the shared daemon should spawn or reload into.
///
/// This intentionally does not follow the fast-moving `current` channel. The
/// shared server should only run binaries that were explicitly promoted onto the
/// shared-server channel (or stable as fallback), so local dirty self-dev builds
/// stop taking out every client by accident.
pub fn shared_server_update_candidate(
    _is_selfdev_session: bool,
) -> Option<(PathBuf, &'static str)> {
    if let Some(shared_server) = existing_binary(shared_server_binary_path(), "shared-server") {
        return Some(shared_server);
    }

    if let Some(stable) = existing_binary(stable_binary_path(), "stable") {
        return Some(stable);
    }

    std::env::current_exe().ok().map(|exe| (exe, "current"))
}

/// Resolve the best binary to use for `/reload`.
///
/// This mostly follows `client_update_candidate`, but if a freshly built repo
/// release binary exists and is newer than the selected channel binary, prefer
/// that so local rebuilds can reload correctly even if publishing the build
/// failed.
pub fn preferred_reload_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    let candidate = client_update_candidate(is_selfdev_session);

    let repo_binary = get_repo_dir().and_then(|repo_dir| {
        if is_selfdev_session {
            newest_existing_binary(vec![
                (selfdev_binary_path(&repo_dir), "repo-selfdev"),
                (release_binary_path(&repo_dir), "repo-release"),
            ])
        } else {
            newest_existing_binary(vec![(release_binary_path(&repo_dir), "repo-release")])
        }
    });

    let repo_is_newer =
        |repo: &Path, current: &Path| match (binary_mtime(repo), binary_mtime(current)) {
            (Some(repo), Some(current)) => repo > current,
            (Some(_), None) => true,
            _ => false,
        };

    match (repo_binary, candidate) {
        (Some((repo, label)), Some((current, _))) if repo_is_newer(&repo, &current) => {
            Some((repo, label))
        }
        (Some((repo, label)), None) => Some((repo, label)),
        (_, Some(candidate)) => Some(candidate),
        (None, None) => None,
    }
}

/// Check if a directory is the jcode repository
pub fn is_jcode_repo(dir: &Path) -> bool {
    // Check for Cargo.toml with name = "jcode"
    let cargo_toml = dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return false;
    }

    // Check for .git directory
    if !dir.join(".git").exists() {
        return false;
    }

    // Read Cargo.toml and check package name
    if let Ok(content) = std::fs::read_to_string(&cargo_toml)
        && content.contains("name = \"jcode\"")
    {
        return true;
    }

    false
}
