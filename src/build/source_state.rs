use super::{BuildInfo, SourceState};
use anyhow::Result;
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::process::Command;

const FNV_OFFSET_BASIS_64: u64 = 0xcbf29ce484222325;
const FNV_PRIME_64: u64 = 0x100000001b3;

fn stable_hash_update(state: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(FNV_PRIME_64);
    }
}

fn stable_hash_str(state: &mut u64, value: &str) {
    stable_hash_update(state, value.as_bytes());
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut state = FNV_OFFSET_BASIS_64;
    stable_hash_update(&mut state, bytes);
    format!("{state:016x}")
}

fn canonicalize_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn hash_path_scope(path: &Path) -> String {
    stable_hash_hex(canonicalize_or_self(path).to_string_lossy().as_bytes())
}

fn git_output_bytes(repo_dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo_dir);
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd.output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed with status {:?}",
            args.join(" "),
            output.status.code()
        );
    }
    Ok(output.stdout)
}

fn git_common_dir(repo_dir: &Path) -> Result<PathBuf> {
    let output = git_output_bytes(repo_dir, &["rev-parse", "--git-common-dir"])?;
    let raw = String::from_utf8_lossy(&output).trim().to_string();
    if raw.is_empty() {
        anyhow::bail!("git rev-parse --git-common-dir returned an empty path");
    }
    let path = PathBuf::from(raw);
    let absolute = if path.is_absolute() {
        path
    } else {
        repo_dir.join(path)
    };
    Ok(canonicalize_or_self(&absolute))
}

pub fn repo_scope_key(repo_dir: &Path) -> Result<String> {
    Ok(hash_path_scope(&git_common_dir(repo_dir)?))
}

pub fn worktree_scope_key(repo_dir: &Path) -> Result<String> {
    Ok(hash_path_scope(repo_dir))
}

fn append_untracked_file_fingerprint(state: &mut u64, repo_dir: &Path, relative: &str) {
    stable_hash_str(state, relative);
    let path = repo_dir.join(relative);
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => {
            stable_hash_update(state, &meta.len().to_le_bytes());
            match std::fs::read(&path) {
                Ok(bytes) => stable_hash_update(state, &bytes),
                Err(err) => stable_hash_str(state, &format!("read-error:{err}")),
            }
        }
        Ok(meta) => {
            stable_hash_str(state, if meta.is_dir() { "dir" } else { "other" });
        }
        Err(err) => stable_hash_str(state, &format!("missing:{err}")),
    }
}

pub fn current_source_state(repo_dir: &Path) -> Result<SourceState> {
    let short_hash = current_git_hash(repo_dir)?;
    let full_hash = current_git_hash_full(repo_dir)?;
    let status = git_output_bytes(
        repo_dir,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let diff = git_output_bytes(repo_dir, &["diff", "--binary", "HEAD"])?;
    let untracked = git_output_bytes(
        repo_dir,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;

    let dirty = !status.is_empty();
    let changed_paths = status
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .count();

    let mut state = FNV_OFFSET_BASIS_64;
    stable_hash_str(&mut state, &full_hash);
    stable_hash_update(&mut state, &status);
    stable_hash_update(&mut state, &diff);
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let relative = String::from_utf8_lossy(path);
        append_untracked_file_fingerprint(&mut state, repo_dir, &relative);
    }
    let fingerprint = format!("{state:016x}");
    let version_label = if dirty {
        format!("{}-dirty-{}", short_hash, &fingerprint[..12])
    } else {
        short_hash.clone()
    };

    Ok(SourceState {
        repo_scope: repo_scope_key(repo_dir)?,
        worktree_scope: worktree_scope_key(repo_dir)?,
        short_hash,
        full_hash,
        dirty,
        fingerprint,
        version_label,
        changed_paths,
    })
}

pub fn ensure_source_state_matches(repo_dir: &Path, expected: &SourceState) -> Result<SourceState> {
    let current = current_source_state(repo_dir)?;
    if current.fingerprint != expected.fingerprint {
        anyhow::bail!(
            "Source tree drift detected while waiting/building (expected {}, now {}). Refusing to publish or attach this build to the original request.",
            expected.fingerprint,
            current.fingerprint
        );
    }
    Ok(current)
}

pub fn repo_build_version(repo_dir: &Path) -> Result<String> {
    Ok(current_source_state(repo_dir)?.version_label)
}

/// Get the current git hash
pub fn current_git_hash(repo_dir: &Path) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_dir);
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd.output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to get git hash");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the full git hash
pub fn current_git_hash_full(repo_dir: &Path) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "HEAD"]).current_dir(repo_dir);
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd.output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to get git hash");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the git diff for uncommitted changes
pub fn current_git_diff(repo_dir: &Path) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(["diff", "HEAD"]).current_dir(repo_dir);
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd.output()?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Check if working tree is dirty
pub fn is_working_tree_dirty(repo_dir: &Path) -> Result<bool> {
    let mut cmd = Command::new("git");
    cmd.args(["status", "--porcelain"]).current_dir(repo_dir);
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd.output()?;

    Ok(!output.stdout.is_empty())
}

/// Get commit message for a hash
pub fn get_commit_message(repo_dir: &Path, hash: &str) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(["log", "-1", "--format=%s", hash])
        .current_dir(repo_dir);
    crate::platform::suppress_child_console(&mut cmd);
    let output = cmd.output()?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Build info for current state
pub fn current_build_info(repo_dir: &Path) -> Result<BuildInfo> {
    let source = current_source_state(repo_dir)?;
    let commit_message = get_commit_message(repo_dir, &source.short_hash).ok();

    Ok(BuildInfo {
        hash: source.short_hash,
        full_hash: source.full_hash,
        built_at: Utc::now(),
        commit_message,
        dirty: source.dirty,
        source_fingerprint: Some(source.fingerprint),
        version_label: Some(source.version_label),
    })
}
