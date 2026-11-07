//! Project trust: one store, one digest per scope.
//!
//! Some files inside a repository make jcode run programs without the user
//! typing anything: project MCP files start servers, project hooks run on
//! every prompt and tool call, and project agent definitions can change the
//! model, effort and tools a worker gets. This module remembers, outside the
//! repository, which exact snapshot of each of those the user approved.
//!
//! - **Store:** `<state dir>/project-trust.json`, versioned. A parse error or
//!   unknown version means nothing is trusted (fail closed) and refuses to be
//!   overwritten.
//! - **Key:** the project root. A linked git worktree uses its main
//!   checkout's root, so one approval covers every worktree, but each scope
//!   digest must still match, so a worktree with different files is untrusted.
//! - **Scopes:** `hooks`, `mcp`, `agents`, independent. Granting one never
//!   grants another.
//! - **Digest:** `sha256:<hex>` over canonical JSON (recursively sorted keys,
//!   compact), documented so other tools can recompute it.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const STORE_VERSION: u32 = 1;
const STORE_FILE: &str = "project-trust.json";

/// What a trust decision covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Hooks,
    Mcp,
    Agents,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Hooks, Scope::Mcp, Scope::Agents];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hooks => "hooks",
            Self::Mcp => "mcp",
            Self::Agents => "agents",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.as_str() == name.trim())
    }
}

/// One approved snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeTrust {
    pub digest: String,
    pub trusted_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    version: u32,
    #[serde(default)]
    projects: BTreeMap<String, BTreeMap<Scope, ScopeTrust>>,
}

/// Result of comparing a current digest with the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustState {
    Trusted,
    /// Never approved for this project and scope.
    Untrusted,
    /// Approved before, but the files changed since.
    Changed,
}

impl TrustState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Untrusted => "untrusted",
            Self::Changed => "changed",
        }
    }
}

/// What a scope would let a project run, ready to show a person.
#[derive(Debug, Clone)]
pub struct ScopeReview {
    pub scope: Scope,
    pub digest: String,
    /// Display lines, already escaped with [`safe_terminal_text`].
    pub lines: Vec<String>,
    /// Number of items (servers, commands, definitions).
    pub item_count: usize,
    /// An environment value was redacted from `lines`, so approval must be
    /// explicit (`--yes`) rather than an inline prompt.
    pub redacted: bool,
}

fn store_path() -> PathBuf {
    crate::storage::durable_state_dir().join(STORE_FILE)
}

enum Loaded {
    Ok(Store),
    /// Present but unreadable or from another version: do not trust, do not
    /// overwrite.
    Unusable(String),
}

fn load() -> Loaded {
    let path = store_path();
    if !path.exists() {
        return Loaded::Ok(Store {
            version: STORE_VERSION,
            ..Store::default()
        });
    }
    // Read the primary file only. `storage::read_json` would restore the
    // `.bak` copy over a corrupt file, which can resurrect an approval that was
    // revoked after that backup was taken. A corrupt store trusts nothing.
    match std::fs::read_to_string(&path)
        .map_err(anyhow::Error::from)
        .and_then(|text| serde_json::from_str::<Store>(&text).map_err(anyhow::Error::from))
    {
        Ok(store) if store.version == STORE_VERSION => Loaded::Ok(store),
        Ok(store) => Loaded::Unusable(format!(
            "project trust store version {} is unsupported",
            store.version
        )),
        Err(error) => Loaded::Unusable(format!(
            "failed to read project trust store {}: {error}",
            path.display()
        )),
    }
}

fn load_for_read() -> Store {
    match load() {
        Loaded::Ok(store) => store,
        Loaded::Unusable(reason) => {
            crate::logging::warn(&format!("{reason}; nothing is trusted"));
            Store {
                version: STORE_VERSION,
                ..Store::default()
            }
        }
    }
}

fn load_for_write() -> Result<Store> {
    match load() {
        Loaded::Ok(store) => Ok(store),
        Loaded::Unusable(reason) => bail!("{reason}; refusing to overwrite it"),
    }
}

/// Serialize a whole load-modify-save of the store across threads and
/// processes, so a stale writer cannot restore an approval another process
/// just revoked.
fn lock_store_file() -> Result<crate::auth::account_store::StoreLockGuard> {
    let dir = crate::storage::durable_state_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        // The lock refuses a directory others can write to, which a loose
        // umask would otherwise produce. The directory is ours, so tighten it.
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = std::fs::symlink_metadata(&dir)?;
        if metadata.is_dir()
            && metadata.mode() & 0o022 != 0
            && metadata.uid() == unsafe { libc::geteuid() }
        {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    crate::auth::account_store::lock_store(&dir, STORE_FILE)
}

fn save(store: &Store) -> Result<()> {
    crate::storage::write_json(&store_path(), store)
}

/// Where a project's own configuration lives: the git top level containing
/// `path` (a linked worktree's own top level, not the main checkout), else
/// `path` itself. Always canonical.
pub fn config_dir(path: &Path) -> Result<PathBuf> {
    let start = std::fs::canonicalize(path)
        .with_context(|| format!("failed to resolve project directory {}", path.display()))?;
    if !start.is_dir() {
        bail!("project path is not a directory: {}", start.display());
    }
    Ok(start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map_or_else(|| start.clone(), Path::to_path_buf))
}

/// The directory that identifies the project containing `path`: the git top
/// level, resolved to the main checkout for a linked worktree, else `path`
/// itself. Always canonical.
pub fn project_root(path: &Path) -> Result<PathBuf> {
    let top = config_dir(path)?;
    let dot_git = top.join(".git");
    if dot_git.is_file() {
        return Ok(main_checkout_of(&top, &dot_git).unwrap_or(top));
    }
    Ok(top)
}

/// For a linked worktree whose `.git` file points at
/// `<main>/.git/worktrees/<name>`, the main checkout root.
fn main_checkout_of(worktree: &Path, dot_git_file: &Path) -> Option<PathBuf> {
    let Ok(text) = std::fs::read_to_string(dot_git_file) else {
        return None;
    };
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    let gitdir = {
        let raw = Path::new(target);
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            worktree.join(raw)
        }
    };
    let Ok(gitdir) = std::fs::canonicalize(gitdir) else {
        return None;
    };
    // The worktree must be registered by the main checkout: its admin dir has
    // to point back at this very `.git` file. Without this check any directory
    // could forge a `.git` file aiming at an approved checkout and inherit its
    // approvals.
    let Ok(text) = std::fs::read_to_string(gitdir.join("gitdir")) else {
        return None;
    };
    let (Ok(registered), Ok(this)) = (
        std::fs::canonicalize(text.trim()),
        std::fs::canonicalize(dot_git_file),
    ) else {
        return None;
    };
    if registered != this {
        return None;
    }
    let common = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(text) => {
            let rel = text.trim();
            let raw = Path::new(rel);
            let joined = if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                gitdir.join(raw)
            };
            let Ok(common) = std::fs::canonicalize(joined) else {
                return None;
            };
            common
        }
        Err(_) => return None,
    };
    if common.file_name().is_some_and(|name| name == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

fn key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// Recursively sorted, compact JSON. `serde_json` already orders object keys
/// without the `preserve_order` feature, but the digest must not depend on a
/// feature flag, so order explicitly.
pub fn canonical_json(value: &serde_json::Value) -> String {
    fn canon(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), canon(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(canon).collect())
            }
            other => other.clone(),
        }
    }
    canon(value).to_string()
}

/// `sha256:<hex>` of the canonical JSON of `value`.
pub fn digest_json(value: &serde_json::Value) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(canonical_json(value).as_bytes()))
    )
}

/// Trust state of `digest` for `scope` in the project containing `path`.
pub fn trust_state(path: &Path, scope: Scope, digest: &str) -> Result<TrustState> {
    let root = project_root(path)?;
    let store = load_for_read();
    Ok(
        match store
            .projects
            .get(&key(&root))
            .and_then(|scopes| scopes.get(&scope))
        {
            Some(entry) if entry.digest == digest => TrustState::Trusted,
            Some(_) => TrustState::Changed,
            None => TrustState::Untrusted,
        },
    )
}

/// Whether `digest` is approved for `scope` in the project containing `path`.
/// Any error resolving the project counts as untrusted.
pub fn is_trusted(path: &Path, scope: Scope, digest: &str) -> bool {
    trust_state(path, scope, digest).is_ok_and(|state| state == TrustState::Trusted)
}

/// Approve `digest` for `scope`. Other scopes are untouched.
pub fn grant(path: &Path, scope: Scope, digest: &str) -> Result<PathBuf> {
    let root = project_root(path)?;
    let _lock = lock_store_file()?;
    let mut store = load_for_write()?;
    store.projects.entry(key(&root)).or_default().insert(
        scope,
        ScopeTrust {
            digest: digest.to_string(),
            trusted_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        },
    );
    save(&store)?;
    Ok(root)
}

/// Remove approval for one scope, or every scope when `scope` is `None`.
/// Returns whether anything was removed.
pub fn revoke(path: &Path, scope: Option<Scope>) -> Result<bool> {
    let root = project_root(path)?;
    let _lock = lock_store_file()?;
    let mut store = load_for_write()?;
    let project = key(&root);
    let Some(scopes) = store.projects.get_mut(&project) else {
        return Ok(false);
    };
    let removed = match scope {
        Some(scope) => scopes.remove(&scope).is_some(),
        None => {
            let had = !scopes.is_empty();
            scopes.clear();
            had
        }
    };
    if scopes.is_empty() {
        store.projects.remove(&project);
    }
    if removed {
        save(&store)?;
    }
    Ok(removed)
}

/// Every approved project and scope, sorted.
pub fn list() -> Vec<(String, Scope, ScopeTrust)> {
    load_for_read()
        .projects
        .into_iter()
        .flat_map(|(project, scopes)| {
            scopes
                .into_iter()
                .map(move |(scope, trust)| (project.clone(), scope, trust))
        })
        .collect()
}

fn is_invisible_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
            | '\u{e0100}'..='\u{e01ef}'
    )
}

/// Escape control and invisible formatting characters so text from a
/// repository cannot disguise itself in a review.
pub fn safe_terminal_text(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        if !character.is_control() && !is_invisible_format_character(character) {
            sanitized.push(character);
        } else {
            sanitized.extend(character.escape_default());
        }
    }
    sanitized
}

#[cfg(test)]
#[path = "project_trust_tests.rs"]
mod tests;
