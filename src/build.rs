mod paths;
mod source_state;
mod storage_helpers;

pub use paths::{
    SELFDEV_CARGO_PROFILE, binary_name, binary_stem, client_update_candidate,
    current_binary_build_time_string, current_binary_built_at, find_dev_binary,
    find_repo_in_ancestors, get_repo_dir, is_jcode_repo, launcher_binary_path, launcher_dir,
    preferred_reload_candidate, release_binary_path, run_selfdev_build, selfdev_binary_path,
    selfdev_build_command, shared_server_update_candidate, update_launcher_symlink_to_current,
    update_launcher_symlink_to_stable,
};
pub use source_state::{
    current_build_info, current_git_diff, current_git_hash, current_git_hash_full,
    current_source_state, ensure_source_state_matches, get_commit_message, is_working_tree_dirty,
    repo_build_version, repo_scope_key, worktree_scope_key,
};
pub use storage_helpers::{
    build_log_path, build_progress_path, builds_dir, canary_binary_path, clear_build_progress,
    clear_migration_context, current_binary_path, current_version_file, load_migration_context,
    manifest_path, migration_context_path, read_build_progress, read_current_version,
    read_shared_server_version, read_stable_version, save_migration_context,
    shared_server_binary_path, shared_server_version_file, stable_binary_path, stable_version_file,
    version_binary_path, write_build_progress,
};

use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfDevBuildCommand {
    pub program: String,
    pub args: Vec<String>,
    pub display: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceState {
    pub repo_scope: String,
    pub worktree_scope: String,
    pub short_hash: String,
    pub full_hash: String,
    pub dirty: bool,
    pub fingerprint: String,
    pub version_label: String,
    pub changed_paths: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishedBuild {
    pub version: String,
    pub source_fingerprint: String,
    pub versioned_path: PathBuf,
    pub current_link: PathBuf,
    pub launcher_link: PathBuf,
    pub previous_current_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingActivation {
    pub session_id: String,
    pub new_version: String,
    pub previous_current_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_shared_server_version: Option<String>,
    pub source_fingerprint: Option<String>,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevBinarySourceMetadata {
    pub version_label: String,
    pub source_fingerprint: String,
    pub short_hash: String,
    pub full_hash: String,
    pub dirty: bool,
    pub changed_paths: usize,
}

impl From<&SourceState> for DevBinarySourceMetadata {
    fn from(source: &SourceState) -> Self {
        Self {
            version_label: source.version_label.clone(),
            source_fingerprint: source.fingerprint.clone(),
            short_hash: source.short_hash.clone(),
            full_hash: source.full_hash.clone(),
            dirty: source.dirty,
            changed_paths: source.changed_paths,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct BinaryVersionReport {
    version: Option<String>,
    git_hash: Option<String>,
}

/// Status of a canary build being tested
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CanaryStatus {
    /// Build is currently being tested
    #[serde(alias = "Testing")]
    Testing,
    /// Build passed all tests and is ready for promotion
    #[serde(alias = "Passed")]
    Passed,
    /// Build failed testing
    #[serde(alias = "Failed")]
    Failed,
}

/// Information about a specific build version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Git commit hash (short)
    pub hash: String,
    /// Git commit hash (full)
    pub full_hash: String,
    /// Build timestamp
    pub built_at: DateTime<Utc>,
    /// Git commit message (first line)
    pub commit_message: Option<String>,
    /// Whether build is from dirty working tree
    pub dirty: bool,
    /// Stable fingerprint of the source state used to produce the build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    /// Immutable published version label, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_label: Option<String>,
}

/// Manifest tracking build versions and their status
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildManifest {
    /// Current stable build hash (known good)
    pub stable: Option<String>,
    /// Current canary build hash (being tested)
    pub canary: Option<String>,
    /// Session ID testing the canary build
    pub canary_session: Option<String>,
    /// Status of canary testing
    pub canary_status: Option<CanaryStatus>,
    /// History of recent builds
    #[serde(default)]
    pub history: Vec<BuildInfo>,
    /// Last crash information (if canary crashed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_crash: Option<CrashInfo>,
    /// Pending activation being validated across reload/resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_activation: Option<PendingActivation>,
}

/// Information about a crash during canary testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashInfo {
    /// Build hash that crashed
    pub build_hash: String,
    /// Exit code
    pub exit_code: i32,
    /// Stderr output (truncated)
    pub stderr: String,
    /// Timestamp of crash
    pub crashed_at: DateTime<Utc>,
    /// Git diff that was being tested
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// Context saved before migrating to a canary build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationContext {
    pub session_id: String,
    pub from_version: String,
    pub to_version: String,
    pub change_summary: Option<String>,
    pub diff: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl BuildManifest {
    /// Load manifest from disk
    pub fn load() -> Result<Self> {
        let path = manifest_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(Self::default())
        }
    }

    /// Save manifest to disk
    pub fn save(&self) -> Result<()> {
        let path = manifest_path()?;
        storage::write_json(&path, self)
    }

    /// Check if we should use stable or canary for a given session
    pub fn binary_for_session(&self, session_id: &str) -> BinaryChoice {
        // If this session is the canary tester, use canary
        if let Some(ref canary_session) = self.canary_session
            && canary_session == session_id
            && let Some(ref canary) = self.canary
        {
            return BinaryChoice::Canary(canary.clone());
        }
        // Otherwise use stable
        if let Some(ref stable) = self.stable {
            BinaryChoice::Stable(stable.clone())
        } else {
            BinaryChoice::Current
        }
    }

    /// Start canary testing for a session
    pub fn start_canary(&mut self, hash: &str, session_id: &str) -> Result<()> {
        self.canary = Some(hash.to_string());
        self.canary_session = Some(session_id.to_string());
        self.canary_status = Some(CanaryStatus::Testing);
        self.save()
    }

    /// Mark canary as passed
    pub fn mark_canary_passed(&mut self) -> Result<()> {
        self.canary_status = Some(CanaryStatus::Passed);
        self.save()
    }

    /// Mark canary as failed
    pub fn mark_canary_failed(&mut self) -> Result<()> {
        self.canary_status = Some(CanaryStatus::Failed);
        self.save()
    }

    /// Record a crash
    pub fn record_crash(
        &mut self,
        hash: &str,
        exit_code: i32,
        stderr: &str,
        diff: Option<String>,
    ) -> Result<()> {
        self.last_crash = Some(CrashInfo {
            build_hash: hash.to_string(),
            exit_code,
            stderr: stderr.chars().take(4096).collect(), // Truncate
            crashed_at: Utc::now(),
            diff,
        });
        self.canary_status = Some(CanaryStatus::Failed);
        self.save()
    }

    /// Clear crash info after it's been handled
    pub fn clear_crash(&mut self) -> Result<()> {
        self.last_crash = None;
        self.save()
    }

    pub fn set_pending_activation(&mut self, activation: PendingActivation) -> Result<()> {
        self.pending_activation = Some(activation);
        self.save()
    }

    pub fn clear_pending_activation(&mut self) -> Result<()> {
        self.pending_activation = None;
        self.save()
    }

    /// Add build to history
    pub fn add_to_history(&mut self, info: BuildInfo) -> Result<()> {
        // Keep last 20 builds
        self.history.insert(0, info);
        self.history.truncate(20);
        self.save()
    }
}

pub fn complete_pending_activation_for_session(session_id: &str) -> Result<Option<String>> {
    let mut manifest = BuildManifest::load()?;
    let Some(pending) = manifest.pending_activation.clone() else {
        return Ok(None);
    };
    if pending.session_id != session_id {
        return Ok(None);
    }

    manifest.canary = Some(pending.new_version.clone());
    manifest.canary_session = Some(session_id.to_string());
    manifest.canary_status = Some(CanaryStatus::Passed);
    manifest.pending_activation = None;
    manifest.last_crash = None;
    manifest.save()?;
    Ok(Some(pending.new_version))
}

pub fn rollback_pending_activation_for_session(session_id: &str) -> Result<Option<String>> {
    let mut manifest = BuildManifest::load()?;
    let Some(pending) = manifest.pending_activation.clone() else {
        return Ok(None);
    };
    if pending.session_id != session_id {
        return Ok(None);
    }

    if let Some(previous) = pending.previous_current_version.as_deref() {
        update_current_symlink(previous)?;
        update_launcher_symlink_to_current()?;
    }
    if let Some(previous) = pending.previous_shared_server_version.as_deref() {
        update_shared_server_symlink(previous)?;
    }
    manifest.canary_status = Some(CanaryStatus::Failed);
    manifest.pending_activation = None;
    manifest.save()?;
    Ok(Some(pending.new_version))
}

/// Which binary to use
#[derive(Debug, Clone)]
pub enum BinaryChoice {
    /// Use the stable version
    Stable(String),
    /// Use the canary version (for testing)
    Canary(String),
    /// Use current running binary (no versioned builds yet)
    Current,
}

/// Install a binary at a specific immutable version path.
pub fn install_binary_at_version(source: &std::path::Path, version: &str) -> Result<PathBuf> {
    if !source.exists() {
        anyhow::bail!("Binary not found at {:?}", source);
    }

    let dest_dir = builds_dir()?.join("versions").join(version);
    storage::ensure_dir(&dest_dir)?;

    let dest = dest_dir.join(binary_name());

    // Remove existing file first to avoid ETXTBSY when replacing a running binary.
    if dest.exists() {
        std::fs::remove_file(&dest)?;
    }

    // Prefer hard link (instant, zero I/O) over copy (71MB+ binary).
    // Falls back to copy if hard link fails (e.g. cross-filesystem).
    if std::fs::hard_link(source, &dest).is_err() {
        std::fs::copy(source, &dest)?;
    }
    crate::platform::set_permissions_executable(&dest)?;

    Ok(dest)
}

fn binary_source_metadata_path(binary: &Path) -> PathBuf {
    let file_name = binary
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| binary_stem().to_string());
    binary.with_file_name(format!("{file_name}.source.json"))
}

pub fn write_dev_binary_source_metadata(binary: &Path, source: &SourceState) -> Result<PathBuf> {
    let path = binary_source_metadata_path(binary);
    storage::write_json(&path, &DevBinarySourceMetadata::from(source))?;
    Ok(path)
}

pub fn write_current_dev_binary_source_metadata(
    repo_dir: &Path,
    source: &SourceState,
) -> Result<PathBuf> {
    let binary = find_dev_binary(repo_dir)
        .ok_or_else(|| anyhow::anyhow!("Binary not found in target/selfdev or target/release"))?;
    write_dev_binary_source_metadata(&binary, source)
}

fn read_binary_version_report(binary: &Path) -> Result<BinaryVersionReport> {
    let output = Command::new(binary)
        .args(["version", "--json"])
        .env("JCODE_NON_INTERACTIVE", "1")
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "Binary smoke test failed for {} with exit code {:?}: {}",
            binary.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout).map_err(|err| {
        anyhow::anyhow!(
            "Binary smoke test for {} returned invalid JSON: {}",
            binary.display(),
            err
        )
    })
}

pub fn smoke_test_binary(binary: &Path) -> Result<()> {
    let report = read_binary_version_report(binary)?;
    if report.version.as_deref().unwrap_or_default().is_empty() {
        anyhow::bail!(
            "Binary smoke test for {} returned JSON without a version field",
            binary.display()
        );
    }
    Ok(())
}

fn validate_binary_version_matches_source_report(
    report: &BinaryVersionReport,
    binary: &Path,
    source: &SourceState,
) -> Result<()> {
    let git_hash = report.git_hash.as_deref().unwrap_or_default();
    if git_hash.is_empty() {
        anyhow::bail!(
            "Binary {} version report did not include git_hash; rebuild before publishing {}",
            binary.display(),
            source.version_label
        );
    }
    if git_hash != source.short_hash {
        anyhow::bail!(
            "Refusing to publish {} as {}: binary was built from git hash {}, but source state is {}",
            binary.display(),
            source.version_label,
            git_hash,
            source.short_hash
        );
    }
    Ok(())
}

fn dirty_status_paths(repo_dir: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(repo_dir)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git status failed while validating dirty build freshness with status {:?}",
            output.status.code()
        );
    }

    let mut entries = output.stdout.split(|byte| *byte == 0).peekable();
    let mut paths = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.is_empty() || entry.len() < 4 {
            continue;
        }
        let x = entry[0];
        let y = entry[1];
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        let deleted = x == b'D' || y == b'D';
        paths.push((PathBuf::from(path), deleted));

        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            let _ = entries.next();
        }
    }

    Ok(paths)
}

fn validate_dirty_binary_freshness_without_metadata(
    repo_dir: &Path,
    binary: &Path,
    source: &SourceState,
) -> Result<()> {
    if !source.dirty {
        return Ok(());
    }

    let binary_mtime = std::fs::metadata(binary)
        .and_then(|metadata| metadata.modified())
        .map_err(|err| {
            anyhow::anyhow!(
                "Could not read binary modification time for {}: {}",
                binary.display(),
                err
            )
        })?;
    let dirty_paths = dirty_status_paths(repo_dir)?;
    let mut unverifiable = Vec::new();
    let mut newer_than_binary = Vec::new();

    for (relative, deleted) in dirty_paths {
        if deleted {
            unverifiable.push(relative.display().to_string());
            continue;
        }
        let path = repo_dir.join(&relative);
        let modified = match std::fs::metadata(&path).and_then(|metadata| metadata.modified()) {
            Ok(modified) => modified,
            Err(_) => {
                unverifiable.push(relative.display().to_string());
                continue;
            }
        };
        if modified > binary_mtime {
            newer_than_binary.push(relative.display().to_string());
        }
    }

    if !unverifiable.is_empty() {
        anyhow::bail!(
            "Refusing to publish dirty build {} without source metadata: these changed paths cannot be checked against the binary timestamp: {}",
            source.version_label,
            unverifiable.join(", ")
        );
    }
    if !newer_than_binary.is_empty() {
        anyhow::bail!(
            "Refusing to publish stale dirty build {}: changed paths are newer than {}: {}",
            source.version_label,
            binary.display(),
            newer_than_binary.join(", ")
        );
    }

    Ok(())
}

fn validate_dev_binary_source_metadata(binary: &Path, source: &SourceState) -> Result<bool> {
    let path = binary_source_metadata_path(binary);
    if !path.exists() {
        return Ok(false);
    }

    let metadata: DevBinarySourceMetadata = storage::read_json(&path)?;
    if metadata.source_fingerprint != source.fingerprint
        || metadata.version_label != source.version_label
        || metadata.short_hash != source.short_hash
        || metadata.full_hash != source.full_hash
        || metadata.dirty != source.dirty
    {
        anyhow::bail!(
            "Refusing to publish {} as {}: source metadata at {} was for {} ({})",
            binary.display(),
            source.version_label,
            path.display(),
            metadata.version_label,
            metadata.source_fingerprint
        );
    }
    Ok(true)
}

fn validate_dev_binary_matches_source(
    repo_dir: &Path,
    binary: &Path,
    source: &SourceState,
) -> Result<()> {
    let report = read_binary_version_report(binary)?;
    if report.version.as_deref().unwrap_or_default().is_empty() {
        anyhow::bail!(
            "Binary smoke test for {} returned JSON without a version field",
            binary.display()
        );
    }
    validate_binary_version_matches_source_report(&report, binary, source)?;
    if !validate_dev_binary_source_metadata(binary, source)? {
        validate_dirty_binary_freshness_without_metadata(repo_dir, binary, source)?;
    }
    Ok(())
}

#[cfg(unix)]
fn smoke_test_server_request(
    stream: &mut BufReader<std::os::unix::net::UnixStream>,
    request: &serde_json::Value,
    expected_ack_id: u64,
) -> Result<()> {
    let payload = serde_json::to_string(request)? + "\n";
    stream.get_mut().write_all(payload.as_bytes())?;
    stream.get_mut().flush()?;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut line = String::new();
        let bytes = stream.read_line(&mut line)?;
        if bytes == 0 {
            anyhow::bail!("server closed the smoke-test socket before replying");
        }
        let value: serde_json::Value = serde_json::from_str(line.trim()).map_err(|err| {
            anyhow::anyhow!("server smoke test returned invalid JSON line: {}", err)
        })?;
        if value.get("type").and_then(|t| t.as_str()) == Some("ack")
            && value.get("id").and_then(|id| id.as_u64()) == Some(expected_ack_id)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for ack {} during server smoke test",
                expected_ack_id
            );
        }
    }
}

#[cfg(unix)]
fn smoke_test_server_connect(
    path: &Path,
) -> std::io::Result<BufReader<std::os::unix::net::UnixStream>> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(BufReader::new(stream))
}

#[cfg(unix)]
fn smoke_test_server_protocol(path: &Path, working_dir: &str) -> Result<()> {
    // The server handles an initial Ping on a dedicated lightweight-control
    // connection and closes it after replying, so the subscribed-client probe
    // must use a fresh socket.
    {
        let mut stream = smoke_test_server_connect(path)?;
        smoke_test_server_request(
            &mut stream,
            &serde_json::json!({
                "type": "ping",
                "id": 1
            }),
            1,
        )?;
    }

    let mut stream = smoke_test_server_connect(path)?;
    smoke_test_server_request(
        &mut stream,
        &serde_json::json!({
            "type": "subscribe",
            "id": 2,
            "working_dir": working_dir
        }),
        2,
    )?;
    Ok(())
}

#[cfg(unix)]
pub fn smoke_test_server_binary(binary: &Path) -> Result<()> {
    use std::fs::File;
    use std::process::Stdio;
    use std::thread;

    smoke_test_binary(binary)?;

    let temp = tempfile::tempdir()?;
    let runtime_dir = temp.path().join("runtime");
    storage::ensure_dir(&runtime_dir)?;
    let socket_path = temp.path().join("jcode-smoke.sock");
    let stderr_path = temp.path().join("jcode-smoke.stderr.log");
    let stderr = File::create(&stderr_path)?;

    let mut child = Command::new(binary)
        .arg("serve")
        .arg("--socket")
        .arg(&socket_path)
        .env("JCODE_NON_INTERACTIVE", "1")
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_GATEWAY_ENABLED", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()?;

    let result = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait()? {
                let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                anyhow::bail!(
                    "server smoke test process exited early with status {:?}: {}",
                    status.code(),
                    stderr.trim()
                );
            }

            match smoke_test_server_connect(&socket_path) {
                Ok(_) => {
                    smoke_test_server_protocol(&socket_path, env!("CARGO_MANIFEST_DIR"))?;
                    return Ok(());
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::NotFound
                            | std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    if Instant::now() >= deadline {
                        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                        anyhow::bail!(
                            "timed out waiting for server smoke test socket {}: {}",
                            socket_path.display(),
                            stderr.trim()
                        );
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(err) => return Err(err.into()),
            }
        }
    })();

    let _ = child.kill();
    let shutdown_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= shutdown_deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    result
}

#[cfg(not(unix))]
pub fn smoke_test_server_binary(binary: &Path) -> Result<()> {
    smoke_test_binary(binary)
}

fn update_channel_symlink(channel: &str, version: &str) -> Result<PathBuf> {
    let channel_dir = builds_dir()?.join(channel);
    storage::ensure_dir(&channel_dir)?;

    let link_path = channel_dir.join(binary_name());
    let target = version_binary_path(version)?;
    if !target.exists() {
        anyhow::bail!("Version binary not found at {:?}", target);
    }

    let temp = channel_dir.join(format!(
        ".{}-{}-{}",
        binary_stem(),
        channel,
        std::process::id()
    ));
    crate::platform::atomic_symlink_swap(&target, &link_path, &temp)?;

    Ok(link_path)
}

/// Update stable symlink to point to a version and publish stable-version marker.
pub fn update_stable_symlink(version: &str) -> Result<PathBuf> {
    let stable_link = update_channel_symlink("stable", version)?;
    std::fs::write(stable_version_file()?, version)?;
    Ok(stable_link)
}

/// Update current symlink to point to a version and publish current-version marker.
pub fn update_current_symlink(version: &str) -> Result<PathBuf> {
    let current_link = update_channel_symlink("current", version)?;
    std::fs::write(current_version_file()?, version)?;
    Ok(current_link)
}

/// Update the shared server symlink to point to a version and publish the
/// shared-server-version marker.
pub fn update_shared_server_symlink(version: &str) -> Result<PathBuf> {
    let shared_link = update_channel_symlink("shared-server", version)?;
    std::fs::write(shared_server_version_file()?, version)?;
    Ok(shared_link)
}

pub fn publish_local_current_build_for_source(
    repo_dir: &Path,
    source: &SourceState,
) -> Result<PublishedBuild> {
    let binary = find_dev_binary(repo_dir)
        .ok_or_else(|| anyhow::anyhow!("Binary not found in target/selfdev or target/release"))?;
    if !binary.exists() {
        anyhow::bail!("Binary not found at {:?}", binary);
    }

    validate_dev_binary_matches_source(repo_dir, &binary, source)?;
    let previous_current_version = read_current_version()?;
    let versioned_path = install_binary_at_version(&binary, &source.version_label)?;
    let installed_report = read_binary_version_report(&versioned_path)?;
    if installed_report
        .version
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        anyhow::bail!(
            "Binary smoke test for {} returned JSON without a version field",
            versioned_path.display()
        );
    }
    validate_binary_version_matches_source_report(&installed_report, &versioned_path, source)?;
    let current_link = update_current_symlink(&source.version_label)?;
    let launcher_link = update_launcher_symlink_to_current()?;

    Ok(PublishedBuild {
        version: source.version_label.clone(),
        source_fingerprint: source.fingerprint.clone(),
        versioned_path,
        current_link,
        launcher_link,
        previous_current_version,
    })
}

/// Install the local release binary into immutable versions and make it the active `current`
/// build + launcher, while keeping `stable` untouched.
pub fn publish_local_current_build(repo_dir: &std::path::Path) -> Result<PathBuf> {
    let source = current_source_state(repo_dir)?;
    Ok(publish_local_current_build_for_source(repo_dir, &source)?.versioned_path)
}

/// Promote an already installed immutable version onto the shared server channel.
pub fn promote_version_to_shared_server(version: &str) -> Result<Option<String>> {
    let previous = read_shared_server_version()?;
    update_shared_server_symlink(version)?;
    Ok(previous)
}

/// Install release binary into immutable versions, promote it to stable, and also make it the
/// active current/launcher build.
pub fn install_local_release(repo_dir: &std::path::Path) -> Result<PathBuf> {
    let source = release_binary_path(repo_dir);
    if !source.exists() {
        anyhow::bail!("Binary not found at {:?}", source);
    }

    let version = repo_build_version(repo_dir)?;

    let versioned = install_binary_at_version(&source, &version)?;
    update_stable_symlink(&version)?;
    update_current_symlink(&version)?;
    update_shared_server_symlink(&version)?;
    update_launcher_symlink_to_current()?;

    Ok(versioned)
}

/// Copy binary to versioned location
pub fn install_version(repo_dir: &std::path::Path, hash: &str) -> Result<PathBuf> {
    let source = release_binary_path(repo_dir);
    install_binary_at_version(&source, hash)
}

/// Update canary symlink to point to a version
pub fn update_canary_symlink(hash: &str) -> Result<()> {
    let _ = update_channel_symlink("canary", hash)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_jcode_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::storage::lock_test_env();
        let temp_home = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());
        let result = f();
        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        result
    }

    fn create_git_repo_fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".git")).expect("create .git dir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"jcode\"\nversion = \"0.0.0\"\n",
        )
        .expect("write Cargo.toml");
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(temp.path())
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(temp.path())
            .output()
            .expect("git config email");
        std::process::Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(temp.path())
            .output()
            .expect("git config name");
        std::process::Command::new("git")
            .args(["add", "Cargo.toml"])
            .current_dir(temp.path())
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(temp.path())
            .output()
            .expect("git commit");
        temp
    }

    fn source_state_fixture(short_hash: &str, fingerprint: &str) -> SourceState {
        SourceState {
            repo_scope: "repo-scope".to_string(),
            worktree_scope: "worktree-scope".to_string(),
            short_hash: short_hash.to_string(),
            full_hash: format!("{short_hash}-full"),
            dirty: true,
            fingerprint: fingerprint.to_string(),
            version_label: format!("{short_hash}-dirty-{}", &fingerprint[..12]),
            changed_paths: 1,
        }
    }

    #[test]
    fn test_build_manifest_default() {
        let manifest = BuildManifest::default();
        assert!(manifest.stable.is_none());
        assert!(manifest.canary.is_none());
        assert!(manifest.history.is_empty());
    }

    #[test]
    fn test_binary_version_hash_mismatch_rejects_publish_candidate() {
        let source = source_state_fixture("newhash", "123456789abcffff");
        let report = BinaryVersionReport {
            version: Some("v0.0.0-dev (oldhash, dirty)".to_string()),
            git_hash: Some("oldhash".to_string()),
        };

        let error =
            validate_binary_version_matches_source_report(&report, Path::new("jcode"), &source)
                .expect_err("mismatched git hash should be rejected");

        assert!(
            error
                .to_string()
                .contains("binary was built from git hash oldhash")
        );
    }

    #[test]
    fn test_dev_binary_source_metadata_mismatch_rejects_publish_candidate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join(binary_name());
        std::fs::write(&binary, b"fake").expect("write fake binary");
        let source = source_state_fixture("abc1234", "1111111111112222");
        let stale_source = source_state_fixture("abc1234", "999999999999aaaa");
        write_dev_binary_source_metadata(&binary, &stale_source).expect("write metadata");

        let error = validate_dev_binary_source_metadata(&binary, &source)
            .expect_err("mismatched source metadata should be rejected");

        assert!(error.to_string().contains("source metadata"));
        assert!(error.to_string().contains("999999999999aaaa"));
    }

    #[cfg(unix)]
    #[test]
    fn test_smoke_test_server_protocol_uses_fresh_connection_after_ping() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("smoke.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind unix listener");

        let server = std::thread::spawn(move || {
            let (first, _) = listener.accept().expect("accept ping client");
            let mut first = BufReader::new(first);
            let mut line = String::new();
            first.read_line(&mut line).expect("read ping request");
            assert!(line.contains("\"type\":\"ping\""));
            first
                .get_mut()
                .write_all(b"{\"type\":\"pong\",\"id\":1}\n")
                .expect("write pong");

            let (second, _) = listener.accept().expect("accept subscribe client");
            let mut second = BufReader::new(second);
            line.clear();
            second.read_line(&mut line).expect("read subscribe request");
            assert!(line.contains("\"type\":\"subscribe\""));
            second
                .get_mut()
                .write_all(b"{\"type\":\"ack\",\"id\":2}\n")
                .expect("write subscribe ack");
        });

        smoke_test_server_protocol(&socket_path, "/tmp").expect("smoke test protocol succeeds");
        server.join().expect("server thread join");
    }

    #[test]
    fn test_binary_choice_for_canary_session() {
        let manifest = BuildManifest {
            canary: Some("abc123".to_string()),
            canary_session: Some("session_test".to_string()),
            ..Default::default()
        };

        // Canary session should get canary binary
        match manifest.binary_for_session("session_test") {
            BinaryChoice::Canary(hash) => assert_eq!(hash, "abc123"),
            _ => panic!("Expected canary binary"),
        }

        // Other sessions should get stable (or current if no stable)
        match manifest.binary_for_session("other_session") {
            BinaryChoice::Current => {}
            _ => panic!("Expected current binary"),
        }
    }

    #[test]
    fn test_find_repo_in_ancestors_walks_upward() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("jcode-repo");
        let nested = repo.join("a").join("b").join("c");

        std::fs::create_dir_all(repo.join(".git")).expect("create .git");
        std::fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"jcode\"\nversion = \"0.0.0\"\n",
        )
        .expect("write Cargo.toml");
        std::fs::create_dir_all(&nested).expect("create nested dirs");

        let found = find_repo_in_ancestors(&nested).expect("repo should be found");
        assert_eq!(found, repo);
    }

    #[test]
    fn test_client_update_candidate_prefers_dev_binary_for_selfdev() {
        let _guard = crate::storage::lock_test_env();
        let temp_home = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());

        let version = "test-current";
        let version_binary =
            install_binary_at_version(std::env::current_exe().as_ref().unwrap(), version)
                .expect("install test version");
        update_current_symlink(version).expect("update current symlink");

        let candidate = client_update_candidate(true).expect("expected selfdev candidate");
        assert_eq!(candidate.1, "current");
        assert_eq!(
            std::fs::canonicalize(candidate.0).expect("canonical candidate"),
            std::fs::canonicalize(version_binary).expect("canonical version binary")
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn launcher_dir_uses_sandbox_bin_when_jcode_home_is_set() {
        with_temp_jcode_home(|| {
            let launcher_dir = launcher_dir().expect("launcher dir");
            let expected = storage::jcode_dir().expect("jcode dir").join("bin");
            assert_eq!(launcher_dir, expected);
        });
    }

    #[test]
    fn update_launcher_symlink_stays_inside_sandbox_home() {
        with_temp_jcode_home(|| {
            let version = "sandbox-current";
            let version_binary =
                install_binary_at_version(std::env::current_exe().as_ref().unwrap(), version)
                    .expect("install test version");
            update_current_symlink(version).expect("update current symlink");

            let launcher = update_launcher_symlink_to_current().expect("update launcher");
            let expected_launcher = storage::jcode_dir()
                .expect("jcode dir")
                .join("bin")
                .join(binary_name());
            assert_eq!(launcher, expected_launcher);
            assert_eq!(
                std::fs::canonicalize(&launcher).expect("canonical launcher"),
                std::fs::canonicalize(version_binary).expect("canonical version binary")
            );
        });
    }

    #[test]
    fn test_canary_status_serialization() {
        assert_eq!(
            serde_json::to_string(&CanaryStatus::Testing).unwrap(),
            "\"testing\""
        );
        assert_eq!(
            serde_json::to_string(&CanaryStatus::Passed).unwrap(),
            "\"passed\""
        );
    }

    #[test]
    fn dirty_source_state_uses_fingerprint_in_version_label() {
        let repo = create_git_repo_fixture();
        std::fs::write(repo.path().join("notes.txt"), "dirty change\n").expect("write dirty file");

        let state = current_source_state(repo.path()).expect("source state");
        assert!(state.dirty);
        assert!(
            state
                .version_label
                .starts_with(&format!("{}-dirty-", state.short_hash))
        );
        assert!(state.version_label.len() > state.short_hash.len() + 7);
    }

    #[test]
    fn pending_activation_can_complete_and_roll_back() {
        with_temp_jcode_home(|| {
            let current_version = "stable-prev";
            let shared_version = "shared-prev";
            install_binary_at_version(std::env::current_exe().as_ref().unwrap(), current_version)
                .expect("install previous version");
            install_binary_at_version(std::env::current_exe().as_ref().unwrap(), shared_version)
                .expect("install previous shared version");
            update_current_symlink(current_version).expect("publish previous current");
            update_shared_server_symlink(shared_version).expect("publish previous shared");

            let mut manifest = BuildManifest::default();
            manifest
                .set_pending_activation(PendingActivation {
                    session_id: "session-a".to_string(),
                    new_version: "canary-next".to_string(),
                    previous_current_version: Some(current_version.to_string()),
                    previous_shared_server_version: Some(shared_version.to_string()),
                    source_fingerprint: Some("fingerprint-a".to_string()),
                    requested_at: Utc::now(),
                })
                .expect("set pending activation");

            let completed = complete_pending_activation_for_session("session-a")
                .expect("complete activation")
                .expect("completed version");
            assert_eq!(completed, "canary-next");
            let manifest = BuildManifest::load().expect("load manifest");
            assert!(manifest.pending_activation.is_none());
            assert_eq!(manifest.canary.as_deref(), Some("canary-next"));
            assert_eq!(manifest.canary_status, Some(CanaryStatus::Passed));

            let mut manifest = BuildManifest::load().expect("reload manifest");
            manifest
                .set_pending_activation(PendingActivation {
                    session_id: "session-b".to_string(),
                    new_version: "canary-bad".to_string(),
                    previous_current_version: Some(current_version.to_string()),
                    previous_shared_server_version: Some(shared_version.to_string()),
                    source_fingerprint: Some("fingerprint-b".to_string()),
                    requested_at: Utc::now(),
                })
                .expect("set second pending activation");

            let rolled_back = rollback_pending_activation_for_session("session-b")
                .expect("rollback activation")
                .expect("rolled back version");
            assert_eq!(rolled_back, "canary-bad");
            let restored = read_current_version()
                .expect("read current version")
                .expect("restored current version");
            assert_eq!(restored, current_version);
            let restored_shared = read_shared_server_version()
                .expect("read shared server version")
                .expect("restored shared server version");
            assert_eq!(restored_shared, shared_version);
        });
    }

    #[test]
    fn shared_server_candidate_prefers_approved_channel_over_current() {
        with_temp_jcode_home(|| {
            let approved_version = "shared-ok";
            let current_version = "current-dev";
            install_binary_at_version(std::env::current_exe().as_ref().unwrap(), approved_version)
                .expect("install approved version");
            install_binary_at_version(std::env::current_exe().as_ref().unwrap(), current_version)
                .expect("install current version");
            update_shared_server_symlink(approved_version).expect("update shared server");
            update_current_symlink(current_version).expect("update current");

            let candidate =
                shared_server_update_candidate(true).expect("expected shared-server candidate");
            assert_eq!(candidate.1, "shared-server");
            let selected = std::fs::canonicalize(candidate.0).expect("canonical selected");
            let approved = std::fs::canonicalize(version_binary_path(approved_version).unwrap())
                .expect("canonical approved");
            assert_eq!(selected, approved);
        });
    }
}
