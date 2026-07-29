use super::{
    SelfDevBuildCommand, SelfDevBuildTarget, canary_binary_path, current_binary_path,
    read_current_version, read_shared_server_version, read_stable_version,
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
    if let Ok(path) = std::env::var("JCODE_REPO_DIR") {
        let path = PathBuf::from(path);
        if is_jcode_repo(&path) {
            return Some(path);
        }
    }

    // First try: compile-time directory
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir);
    if let Some(repo) = find_repo_in_ancestors(&path) {
        return Some(repo);
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

/// Resolve a channel/launcher binary path to the file that actually runs.
///
/// Release archives install a tiny `jcode` wrapper script alongside the real
/// `jcode-<platform>.bin` payload (the wrapper sets `LD_LIBRARY_PATH` and execs
/// the payload). Channel symlinks point at the wrapper and reload/exec must
/// keep using the wrapper, but the *running process* (`current_exe()`) is the
/// payload. Any "is the candidate the same/newer binary than the running one?"
/// comparison must therefore compare payloads. Comparing the wrapper against
/// the payload compares two different files with unrelated mtimes, which made
/// `server_has_newer_binary()` report a phantom update forever and locked
/// post-`/update` sessions into an infinite reload loop.
///
/// Returns the canonicalized payload path when `path` resolves to a wrapper
/// script with a unique sibling `<stem>-*.bin` payload; otherwise returns the
/// canonicalized input path.
pub fn resolve_binary_payload(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    wrapper_payload_sibling(&canonical).unwrap_or(canonical)
}

fn wrapper_payload_sibling(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    // Wrapper scripts are a few hundred bytes; real binaries are tens of MB.
    // The size gate keeps us from reading a large binary just to check "#!".
    if !meta.is_file() || meta.len() > 4096 {
        return None;
    }
    let mut prefix = [0u8; 2];
    {
        use std::io::Read;
        let mut file = std::fs::File::open(path).ok()?;
        file.read_exact(&mut prefix).ok()?;
    }
    if &prefix != b"#!" {
        return None;
    }
    let dir = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    let payload_prefix = format!("{stem}-");
    let mut payload: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_str()?;
        if name.starts_with(&payload_prefix) && name.ends_with(".bin") {
            if payload.is_some() {
                // Ambiguous: more than one payload candidate. Refuse to guess.
                return None;
            }
            payload = Some(entry.path());
        }
    }
    payload.filter(|p| p.is_file())
}

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
        SelfDevBuildTarget::Desktop2 => vec![("jcode-desktop2", "jcode-desktop2")],
        SelfDevBuildTarget::All | SelfDevBuildTarget::Auto => {
            vec![("jcode", "jcode"), ("jcode-desktop2", "jcode-desktop2")]
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
    let paths: Vec<String> = text.lines().map(porcelain_path).collect();
    build_target_for_paths(paths.iter().map(String::as_str))
}

/// The path a `git status --porcelain=v1` line refers to, following renames.
fn porcelain_path(line: &str) -> String {
    let raw = line.get(3..).unwrap_or(line).trim();
    raw.rsplit_once(" -> ")
        .map(|(_, new_path)| new_path)
        .unwrap_or(raw)
        .to_string()
}

/// Which binaries the given changed paths require rebuilding. Pure, so the
/// routing is testable: getting this wrong means `selfdev build` silently
/// builds the wrong binary and a reload appears to do nothing.
fn build_target_for_paths<'a>(paths: impl Iterator<Item = &'a str>) -> SelfDevBuildTarget {
    let mut desktop2 = false;
    let mut other = false;
    for path in paths {
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        if path == "Cargo.toml" || path == "Cargo.lock" || path.starts_with(".cargo/") {
            // Workspace-wide changes can affect every binary.
            desktop2 = true;
            other = true;
        } else if path.starts_with("crates/jcode-desktop2/") {
            desktop2 = true;
        } else {
            other = true;
        }
    }
    match (desktop2, other) {
        (true, false) => SelfDevBuildTarget::Desktop2,
        (false, _) => SelfDevBuildTarget::Tui,
        _ => SelfDevBuildTarget::All,
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
pub fn shared_server_update_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    let shared_server = existing_binary(shared_server_binary_path(), "shared-server");
    if is_selfdev_session {
        if let Some(shared_server) = shared_server {
            return Some(shared_server);
        }
    } else if let Some(shared_server) = shared_server
        && shared_server_channel_is_current_enough()
    {
        return Some(shared_server);
    }

    if let Some(stable) = existing_binary(stable_binary_path(), "stable") {
        return Some(stable);
    }

    std::env::current_exe().ok().map(|exe| (exe, "current"))
}

fn shared_server_channel_is_current_enough() -> bool {
    let shared = read_shared_server_version().ok().flatten();
    let Some(shared) = shared
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };

    let stable = read_stable_version().ok().flatten();
    if stable
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|stable| stable == shared)
    {
        return true;
    }

    let current = read_current_version().ok().flatten();
    current
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|current| current == shared)
}

fn normalize_version_marker(value: &str) -> String {
    let value = value.trim();
    let value = value.strip_prefix('v').unwrap_or(value);
    value
        .split([' ', '(', ')'])
        .next()
        .unwrap_or(value)
        .trim()
        .to_string()
}

pub fn version_matches_installed_channel(version: &str, git_hash: &str) -> bool {
    let version = normalize_version_marker(version);
    let git_hash = git_hash.trim();
    let mut saw_marker = false;
    for marker in [read_stable_version(), read_current_version()] {
        let Some(marker) = marker.ok().flatten() else {
            continue;
        };
        let marker_trimmed = marker.trim();
        if marker_trimmed.is_empty() {
            continue;
        }
        saw_marker = true;
        if normalize_version_marker(marker_trimmed) == version {
            return true;
        }
        if !git_hash.is_empty()
            && git_hash != "unknown"
            && (marker_trimmed == git_hash || marker_trimmed.starts_with(git_hash))
        {
            return true;
        }
    }
    !saw_marker
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

    let repo_is_newer = |repo: &Path, current: &Path| {
        // `current` may be a channel symlink to a release wrapper script;
        // compare the payload that actually runs, not the wrapper.
        match (
            binary_mtime(repo),
            binary_mtime(&resolve_binary_payload(current)),
        ) {
            (Some(repo), Some(current)) => repo > current,
            (Some(_), None) => true,
            _ => false,
        }
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

    // Check for a .git directory or gitdir file (worktrees use a file).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_fixture(git_file: bool) -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().expect("temp repo");
        if git_file {
            std::fs::write(temp.path().join(".git"), "gitdir: /tmp/jcode-test-git\n")
                .expect("git file");
        } else {
            std::fs::create_dir_all(temp.path().join(".git")).expect("git dir");
        }
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"jcode\"\nversion = \"0.1.0\"\n",
        )
        .expect("Cargo.toml");
        temp
    }

    #[test]
    fn find_repo_in_ancestors_finds_workspace_from_crate_dir() {
        let repo = repo_fixture(false);
        let crate_dir = repo.path().join("crates").join("jcode-build-support");
        std::fs::create_dir_all(&crate_dir).expect("crate dir");

        assert_eq!(
            find_repo_in_ancestors(&crate_dir).as_deref(),
            Some(repo.path())
        );
    }

    /// Every build target must map to the package it claims to build, or
    /// `selfdev build target=X` silently builds something else.
    #[test]
    fn every_build_target_builds_its_own_package() {
        let repo = repo_fixture(false);
        let cases = [
            (SelfDevBuildTarget::Tui, vec!["-p jcode "]),
            (SelfDevBuildTarget::Desktop2, vec!["-p jcode-desktop2 "]),
            (
                SelfDevBuildTarget::All,
                vec!["-p jcode ", "-p jcode-desktop2 "],
            ),
        ];
        for (target, expected) in cases {
            let command = selfdev_build_command_for_target(repo.path(), target);
            let text = format!("{} ", command.display);
            for needle in &expected {
                assert!(
                    text.contains(needle),
                    "{target:?} did not build `{needle}`: {}",
                    command.display
                );
            }
        }
    }

    /// A single-target build must not drag in the other binaries: building the
    /// desktop app when only the TUI changed wastes minutes.
    #[test]
    fn single_targets_do_not_build_other_binaries() {
        let repo = repo_fixture(false);
        let tui = selfdev_build_command_for_target(repo.path(), SelfDevBuildTarget::Tui);
        assert!(!tui.display.contains("jcode-desktop"));
        let desktop2 = selfdev_build_command_for_target(repo.path(), SelfDevBuildTarget::Desktop2);
        assert!(!desktop2.display.contains("-p jcode "));
    }

    /// `auto` must route a change to the binary that contains it. Before
    /// desktop2 was added here, editing it built the TUI instead.
    #[test]
    fn auto_routes_changed_paths_to_the_right_binary() {
        let cases: Vec<(Vec<&str>, SelfDevBuildTarget)> = vec![
            (vec![], SelfDevBuildTarget::Tui),
            (vec!["src/main.rs"], SelfDevBuildTarget::Tui),
            (vec!["crates/jcode-tui/src/lib.rs"], SelfDevBuildTarget::Tui),
            (
                vec!["crates/jcode-desktop2/src/main.rs"],
                SelfDevBuildTarget::Desktop2,
            ),
            (
                vec!["crates/jcode-desktop2/src/editor.rs", "src/main.rs"],
                SelfDevBuildTarget::All,
            ),
            // Workspace manifests can affect everything.
            (vec!["Cargo.toml"], SelfDevBuildTarget::All),
            (vec!["Cargo.lock"], SelfDevBuildTarget::All),
        ];
        for (paths, expected) in cases {
            assert_eq!(
                build_target_for_paths(paths.iter().copied()),
                expected,
                "paths {paths:?} routed to the wrong target"
            );
        }
    }

    #[test]
    fn porcelain_lines_are_parsed_including_renames() {
        assert_eq!(porcelain_path(" M src/main.rs"), "src/main.rs");
        assert_eq!(
            porcelain_path("?? crates/jcode-desktop2/src/new.rs"),
            "crates/jcode-desktop2/src/new.rs"
        );
        assert_eq!(
            porcelain_path("R  old/path.rs -> crates/jcode-desktop2/src/moved.rs"),
            "crates/jcode-desktop2/src/moved.rs"
        );
    }

    #[test]
    fn build_targets_parse_from_their_names() {
        for (name, expected) in [
            ("tui", SelfDevBuildTarget::Tui),
            ("desktop", SelfDevBuildTarget::Desktop2),
            ("desktop2", SelfDevBuildTarget::Desktop2),
            ("jcode-desktop2", SelfDevBuildTarget::Desktop2),
            ("all", SelfDevBuildTarget::All),
            ("auto", SelfDevBuildTarget::Auto),
        ] {
            assert_eq!(
                SelfDevBuildTarget::parse(Some(name)).expect("parse"),
                expected,
                "target name `{name}` parsed wrong"
            );
        }
        assert!(SelfDevBuildTarget::parse(Some("nonsense")).is_err());
    }

    #[test]
    fn is_jcode_repo_accepts_git_file_for_worktree() {
        let repo = repo_fixture(true);
        assert!(is_jcode_repo(repo.path()));
    }

    /// Build a release-style install dir: `jcode` wrapper script + payload.
    fn release_install_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().expect("temp install");
        let wrapper = temp.path().join("jcode");
        let payload = temp.path().join("jcode-linux-x86_64.bin");
        std::fs::write(
            &wrapper,
            "#!/usr/bin/env sh\nexec ./jcode-linux-x86_64.bin \"$@\"\n",
        )
        .expect("wrapper");
        std::fs::write(&payload, vec![0x7fu8; 64]).expect("payload");
        (temp, wrapper, payload)
    }

    #[test]
    fn resolve_binary_payload_maps_wrapper_to_payload() {
        let (_temp, wrapper, payload) = release_install_fixture();
        assert_eq!(
            resolve_binary_payload(&wrapper),
            std::fs::canonicalize(&payload).expect("canonical payload")
        );
    }

    #[test]
    fn resolve_binary_payload_follows_channel_symlink_to_wrapper() {
        // The real layout: builds/<channel>/jcode -> versions/<v>/jcode (wrapper).
        let (temp, wrapper, payload) = release_install_fixture();
        let channel_dir = temp.path().join("channel");
        std::fs::create_dir_all(&channel_dir).expect("channel dir");
        let link = channel_dir.join("jcode");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&wrapper, &link).expect("symlink");
        #[cfg(not(unix))]
        std::fs::copy(&wrapper, &link).map(|_| ()).expect("copy");
        assert_eq!(
            resolve_binary_payload(&link),
            std::fs::canonicalize(&payload).expect("canonical payload")
        );
    }

    #[test]
    fn resolve_binary_payload_keeps_real_binary() {
        // A normal (non-wrapper) binary resolves to itself even with a sibling
        // `.bin` file present: it is too large / not a script.
        let temp = tempfile::TempDir::new().expect("temp install");
        let binary = temp.path().join("jcode");
        std::fs::write(&binary, vec![0x7fu8; 8192]).expect("binary");
        std::fs::write(temp.path().join("jcode-linux-x86_64.bin"), [0u8; 8]).expect("bin");
        assert_eq!(
            resolve_binary_payload(&binary),
            std::fs::canonicalize(&binary).expect("canonical binary")
        );
    }

    #[test]
    fn resolve_binary_payload_keeps_small_script_without_payload() {
        let temp = tempfile::TempDir::new().expect("temp install");
        let script = temp.path().join("jcode");
        std::fs::write(&script, "#!/usr/bin/env sh\nexec true\n").expect("script");
        assert_eq!(
            resolve_binary_payload(&script),
            std::fs::canonicalize(&script).expect("canonical script")
        );
    }

    #[test]
    fn resolve_binary_payload_refuses_ambiguous_payloads() {
        let (temp, wrapper, _payload) = release_install_fixture();
        std::fs::write(temp.path().join("jcode-macos-aarch64.bin"), [0u8; 8]).expect("second bin");
        assert_eq!(
            resolve_binary_payload(&wrapper),
            std::fs::canonicalize(&wrapper).expect("canonical wrapper")
        );
    }
}
