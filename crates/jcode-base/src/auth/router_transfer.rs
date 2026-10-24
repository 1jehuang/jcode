//! One-way transfer of the account router's non-OAuth-pool credential sources
//! for a cloud handoff.
//!
//! The OpenAI/Claude multi-account OAuth stores keep their dedicated,
//! identity-checked round trip in [`super::transfer`]. Everything else the
//! router can route to is described by an explicit allowlist here, never by a
//! directory listing:
//!
//! - Jcode home: `account-pool.json` (rotation order, no secrets),
//!   `gemini_oauth.json`, `antigravity_oauth.json` and the Muse subscription
//!   store.
//! - Jcode config dir: every provider API-key env file the router loads
//!   (OpenAI-compatible catalog and Muse profiles, the fixed first-party files,
//!   and safe `env_file` names of configured named providers).
//!
//! - Jcode home `config.toml`: only the `[provider]` routing keys in
//!   [`ROUTING_CONFIG_KEYS`], merged into the destination file with the rest
//!   of its keys, tables and comments preserved.
//!
//! External-tool logins (Codex CLI, Claude Code, Copilot, Gemini CLI, Grok,
//! OpenCode and similar) are deliberately excluded. The router only reads them
//! after a per-path trust decision bound to that machine's file, they are owned
//! and refreshed by other programs, and some live in OS keychains.
//!
//! Plain files are copied with source-wins semantics. The account pool is
//! merged so destination-only entries survive. Every destination file is
//! published atomically with owner-only permissions. The whole payload is
//! validated before anything is written.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use super::account_pool::AccountPool;

pub const MAX_BUNDLE_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const BUNDLE_VERSION: u32 = 1;

pub const ACCOUNT_POOL_FILE: &str = "account-pool.json";
pub const ROUTING_CONFIG_FILE: &str = "config.toml";

/// `[provider]` keys that decide which account/model the router picks and how
/// it fails over. Everything else in config.toml (MCP servers, paths,
/// keybinds, named providers, host-specific tuning) stays on its machine.
pub const ROUTING_CONFIG_KEYS: &[&str] = &[
    "default_model",
    "default_provider",
    "cross_provider_failover",
    "same_provider_account_failover",
    "model_picker_providers",
    "gemini_force_oauth",
    "gemini_project",
];

/// Extract only the allowlisted `[provider]` routing keys as a TOML snippet.
/// `None` when the config has none of them.
pub fn routing_config_snippet(config: &str) -> Result<Option<String>, RouterTransferError> {
    let document: toml_edit::Document =
        config.parse().map_err(|_| RouterTransferError::Malformed)?;
    let mut out = toml_edit::Document::new();
    let mut table = toml_edit::Table::new();
    if let Some(provider) = document
        .get("provider")
        .and_then(|item| item.as_table_like())
    {
        for key in ROUTING_CONFIG_KEYS {
            if let Some(value) = provider.get(key).and_then(|item| item.as_value()) {
                let mut value = value.clone();
                value.decor_mut().clear();
                table.insert(key, toml_edit::Item::Value(value));
            }
        }
    }
    if table.is_empty() {
        return Ok(None);
    }
    out.insert("provider", toml_edit::Item::Table(table));
    Ok(Some(out.to_string()))
}

/// Source wins for every allowlisted key: set when present in the snippet,
/// removed when absent, so routing is identical. Every other key, table and
/// comment in the destination is preserved.
pub fn merge_routing_config(
    snippet: &str,
    destination: Option<&str>,
) -> Result<String, RouterTransferError> {
    let snippet: toml_edit::Document = snippet
        .parse()
        .map_err(|_| RouterTransferError::Malformed)?;
    if snippet.as_table().iter().any(|(key, _)| key != "provider") {
        return Err(RouterTransferError::NotAllowed);
    }
    let source = snippet
        .get("provider")
        .and_then(|item| item.as_table())
        .ok_or(RouterTransferError::Malformed)?;
    if source
        .iter()
        .any(|(key, item)| !ROUTING_CONFIG_KEYS.contains(&key) || item.as_value().is_none())
    {
        return Err(RouterTransferError::NotAllowed);
    }
    let mut document: toml_edit::Document = destination
        .unwrap_or("")
        .parse()
        .map_err(|_| RouterTransferError::Malformed)?;
    if document.get("provider").is_none() {
        document.insert("provider", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let provider = document
        .get_mut("provider")
        .and_then(|item| item.as_table_like_mut())
        .ok_or(RouterTransferError::Malformed)?;
    for key in ROUTING_CONFIG_KEYS {
        match source.get(key).and_then(|item| item.as_value()) {
            Some(value) => {
                let mut value = value.clone();
                if let Some(existing) = provider.get(key).and_then(|item| item.as_value()) {
                    *value.decor_mut() = existing.decor().clone();
                }
                // Overwrite only the value. Re-inserting would drop the key's
                // own decor, which holds comments written above the key.
                match provider.get_mut(key) {
                    Some(item) if item.is_value() => *item = toml_edit::Item::Value(value),
                    _ => {
                        provider.insert(key, toml_edit::Item::Value(value));
                    }
                }
            }
            None => {
                provider.remove(key);
            }
        }
    }
    Ok(document.to_string())
}

/// Credential stores under the Jcode home directory.
pub fn jcode_home_files() -> &'static [&'static str] {
    &[
        ACCOUNT_POOL_FILE,
        "gemini_oauth.json",
        "antigravity_oauth.json",
        "muse_subscription.json",
    ]
}

/// Fixed first-party API-key env files that are not catalog profiles.
const FIXED_ENV_FILES: &[&str] = &[
    "anthropic.env",
    "openai.env",
    "openrouter.env",
    "gemini.env",
    "azure-openai.env",
    "bedrock.env",
    "cursor.env",
    "jcode-subscription.env",
];

/// Every env-file name the router may load, plus `extra` named-provider files.
pub fn env_file_names<'a>(extra: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = FIXED_ENV_FILES.iter().map(|s| s.to_string()).collect();
    for profile in jcode_provider_metadata::openai_compatible_profiles() {
        names.insert(profile.env_file.to_string());
    }
    names.insert(
        jcode_provider_metadata::META_MUSE_PROFILE
            .env_file
            .to_string(),
    );
    names.insert(
        jcode_provider_metadata::MUSE_SUBSCRIPTION_PROFILE
            .env_file
            .to_string(),
    );
    names.extend(extra.into_iter().map(str::trim).map(str::to_string));
    names.retain(|name| is_transferable_env_name(name));
    names
}

fn is_transferable_env_name(name: &str) -> bool {
    jcode_provider_metadata::is_safe_env_file_name(name)
        && name.ends_with(".env")
        && name.len() > ".env".len()
        && !name.starts_with('.')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Root {
    Jcode,
    Config,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    root: Root,
    name: String,
    /// Hex keeps arbitrary text intact and avoids JSON escaping surprises.
    data: String,
}

#[derive(Serialize, Deserialize)]
struct Bundle {
    version: u32,
    files: Vec<Entry>,
}

/// Serialized bundle. Debug never prints contents.
pub struct RouterBundle {
    bytes: Vec<u8>,
    names: Vec<String>,
}

impl RouterBundle {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
    /// File names only, safe to show.
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

impl std::fmt::Debug for RouterBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterBundle")
            .field("files", &self.names)
            .finish_non_exhaustive()
    }
}

/// Errors carry file names only, never contents.
#[derive(Debug)]
pub enum RouterTransferError {
    NotRegular(String),
    TooLarge(String),
    NotText(String),
    Read(String),
    Malformed,
    NotAllowed,
    Write(String),
}

impl std::fmt::Display for RouterTransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRegular(n) => write!(f, "router credential file {n} is not a regular file"),
            Self::TooLarge(n) => write!(f, "router credential file {n} is too large"),
            Self::NotText(n) => write!(f, "router credential file {n} is not valid UTF-8 text"),
            Self::Read(n) => write!(f, "could not read router credential file {n}"),
            Self::Malformed => f.write_str("router credential bundle is malformed"),
            Self::NotAllowed => {
                f.write_str("router credential bundle names a file outside the allowlist")
            }
            Self::Write(n) => write!(f, "could not write router credential file {n}"),
        }
    }
}

impl std::error::Error for RouterTransferError {}

fn read_regular(path: &Path, name: &str) -> Result<Option<String>, RouterTransferError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(RouterTransferError::Read(name.into())),
    };
    if !metadata.file_type().is_file() {
        return Err(RouterTransferError::NotRegular(name.into()));
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(RouterTransferError::TooLarge(name.into()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take((MAX_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| RouterTransferError::Read(name.into()))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(RouterTransferError::TooLarge(name.into()));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| RouterTransferError::NotText(name.into()))
}

/// Collect every allowlisted source that exists. Never writes.
pub fn export_at(
    jcode_home: &Path,
    config_dir: &Path,
    env_names: &BTreeSet<String>,
) -> Result<RouterBundle, RouterTransferError> {
    let mut files = Vec::new();
    let mut names = Vec::new();
    let sources = jcode_home_files()
        .iter()
        .map(|name| (Root::Jcode, jcode_home.join(name), name.to_string()))
        .chain(
            env_names
                .iter()
                .map(|name| (Root::Config, config_dir.join(name), name.clone())),
        );
    if let Some(config) = read_regular(&jcode_home.join(ROUTING_CONFIG_FILE), ROUTING_CONFIG_FILE)?
        && let Some(snippet) = routing_config_snippet(&config)?
    {
        files.push(Entry {
            root: Root::Jcode,
            name: ROUTING_CONFIG_FILE.into(),
            data: hex::encode(snippet.as_bytes()),
        });
        names.push(ROUTING_CONFIG_FILE.into());
    }
    for (root, path, name) in sources {
        if let Some(text) = read_regular(&path, &name)? {
            if root == Root::Jcode && name != ACCOUNT_POOL_FILE && text.trim().is_empty() {
                continue;
            }
            files.push(Entry {
                root,
                name: name.clone(),
                data: hex::encode(text.as_bytes()),
            });
            names.push(name);
        }
    }
    let bytes = serde_json::to_vec(&Bundle {
        version: BUNDLE_VERSION,
        files,
    })
    .map_err(|_| RouterTransferError::Malformed)?;
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(RouterTransferError::TooLarge("bundle".into()));
    }
    Ok(RouterBundle { bytes, names })
}

/// Merge pools: source order first, then destination-only members; an entry
/// is excluded when either side excluded it and the source did not place it.
fn merge_pool(source: &str, destination: Option<&str>) -> Result<String, RouterTransferError> {
    let source: AccountPool =
        serde_json::from_str(source).map_err(|_| RouterTransferError::Malformed)?;
    let destination: AccountPool = destination
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default();
    let mut merged = source.clone();
    for member in destination.members {
        if !merged.members.contains(&member) && !source.excluded.contains(&member) {
            merged.members.push(member);
        }
    }
    for excluded in destination.excluded {
        if !merged.excluded.contains(&excluded) && !merged.members.contains(&excluded) {
            merged.excluded.push(excluded);
        }
    }
    serde_json::to_string_pretty(&merged).map_err(|_| RouterTransferError::Malformed)
}

/// Validate the whole bundle, then publish each file atomically (0600, 0700 dirs).
/// Returns the published file names.
pub fn import_at(
    jcode_home: &Path,
    config_dir: &Path,
    bytes: &[u8],
) -> Result<Vec<String>, RouterTransferError> {
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(RouterTransferError::TooLarge("bundle".into()));
    }
    let bundle: Bundle =
        serde_json::from_slice(bytes).map_err(|_| RouterTransferError::Malformed)?;
    if bundle.version != BUNDLE_VERSION {
        return Err(RouterTransferError::Malformed);
    }
    let mut seen = BTreeSet::new();
    let mut planned = Vec::new();
    for entry in bundle.files {
        let allowed = match entry.root {
            Root::Jcode => {
                jcode_home_files().contains(&entry.name.as_str())
                    || entry.name == ROUTING_CONFIG_FILE
            }
            Root::Config => is_transferable_env_name(&entry.name),
        };
        if !allowed || !seen.insert((entry.root, entry.name.clone())) {
            return Err(RouterTransferError::NotAllowed);
        }
        let raw = hex::decode(&entry.data).map_err(|_| RouterTransferError::Malformed)?;
        if raw.len() > MAX_FILE_BYTES {
            return Err(RouterTransferError::TooLarge(entry.name));
        }
        let text =
            String::from_utf8(raw).map_err(|_| RouterTransferError::NotText(entry.name.clone()))?;
        let dir = match entry.root {
            Root::Jcode => jcode_home,
            Root::Config => config_dir,
        };
        let path = dir.join(&entry.name);
        let text = if entry.root == Root::Jcode && entry.name == ACCOUNT_POOL_FILE {
            let existing = read_regular(&path, &entry.name)?;
            merge_pool(&text, existing.as_deref())?
        } else if entry.root == Root::Jcode && entry.name == ROUTING_CONFIG_FILE {
            let existing = read_regular(&path, &entry.name)?;
            merge_routing_config(&text, existing.as_deref())?
        } else {
            if entry.root == Root::Jcode {
                serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|_| RouterTransferError::Malformed)?;
            }
            // Refuse to follow a destination symlink or replace a directory.
            read_regular(&path, &entry.name)?;
            text
        };
        planned.push((path, entry.name, text));
    }
    let mut written = Vec::new();
    for (path, name, text) in planned {
        crate::storage::write_text_secret(&path, &text)
            .map_err(|_| RouterTransferError::Write(name.clone()))?;
        written.push(name);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn assert_mode(path: &std::path::Path, want: u32) {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            want
        );
    }

    #[cfg(not(unix))]
    fn assert_mode(_path: &std::path::Path, _want: u32) {}

    const SECRET: &str = "synthetic-router-key-not-real";

    fn dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let config = tmp.path().join("config");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        (tmp, home, config)
    }

    #[test]
    fn allowlist_covers_router_sources_and_rejects_unsafe_names() {
        let names = env_file_names(["custom-named.env", "../evil.env", "config.toml", ".env"]);
        for expected in [
            "openai.env",
            "anthropic.env",
            "bedrock.env",
            "cursor.env",
            "azure-openai.env",
            "jcode-subscription.env",
            "muse-subscription.env",
            "meta-muse.env",
            "openrouter.env",
            "custom-named.env",
        ] {
            assert!(names.contains(expected), "{expected}");
        }
        assert!(
            !names
                .iter()
                .any(|n| n.contains('/') || n == "config.toml" || n == ".env")
        );
    }

    #[test]
    fn roundtrip_publishes_private_files_and_merges_pool() {
        let (_src_tmp, src_home, src_config) = dirs();
        let (_dst_tmp, dst_home, dst_config) = dirs();
        std::fs::write(
            src_config.join("openai.env"),
            format!("OPENAI_API_KEY={SECRET}\n"),
        )
        .unwrap();
        std::fs::write(src_config.join("unlisted.env"), "X=1\n").unwrap();
        std::fs::write(src_config.join("config.toml"), "x=1").unwrap();
        std::fs::write(
            src_home.join("gemini_oauth.json"),
            r#"{"refresh_token":"r"}"#,
        )
        .unwrap();
        std::fs::write(src_home.join("google_oauth.json"), "{}").unwrap();
        std::fs::write(
            src_home.join(ACCOUNT_POOL_FILE),
            r#"{"members":["openai:a","gemini"],"excluded":["openrouter"]}"#,
        )
        .unwrap();
        std::fs::write(dst_config.join("openai.env"), "OPENAI_API_KEY=old\n").unwrap();
        std::fs::write(dst_config.join("groq.env"), "GROQ_API_KEY=keep\n").unwrap();
        std::fs::write(
            dst_home.join(ACCOUNT_POOL_FILE),
            r#"{"members":["cloud-only","openrouter"],"excluded":["xai"]}"#,
        )
        .unwrap();

        let bundle = export_at(&src_home, &src_config, &env_file_names([])).unwrap();
        assert!(!format!("{bundle:?}").contains(SECRET));
        let mut names = bundle.names().to_vec();
        names.sort();
        assert_eq!(
            names,
            ["account-pool.json", "gemini_oauth.json", "openai.env"]
        );

        import_at(&dst_home, &dst_config, bundle.as_bytes()).unwrap();
        let env = dst_config.join("openai.env");
        assert_eq!(
            std::fs::read_to_string(&env).unwrap(),
            format!("OPENAI_API_KEY={SECRET}\n")
        );
        assert_mode(&env, 0o600);
        assert_mode(&dst_config, 0o700);
        assert!(dst_config.join("groq.env").exists());
        assert!(!dst_config.join("unlisted.env").exists());
        assert!(!dst_config.join("config.toml").exists());
        assert!(!dst_home.join("google_oauth.json").exists());
        let pool: AccountPool = serde_json::from_str(
            &std::fs::read_to_string(dst_home.join(ACCOUNT_POOL_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(pool.members, ["openai:a", "gemini", "cloud-only"]);
        assert_eq!(pool.excluded, ["openrouter", "xai"]);
    }

    #[test]
    fn routing_config_extracts_allowlist_and_merges_preserving_destination() {
        let local = r#"
mcp_secret_path = "/local/only"
[provider]
default_model = "openai-oauth:gpt-test"
default_provider = "openai-oauth"
cross_provider_failover = "countdown"
openai_service_tier = "priority"
[keybindings]
quit = "ctrl-q"
"#;
        let snippet = routing_config_snippet(local).unwrap().unwrap();
        assert!(snippet.contains("cross_provider_failover = \"countdown\""));
        assert!(!snippet.contains("priority") && !snippet.contains("keybindings"));
        assert!(!snippet.contains("/local/only"));
        let cloud = r#"# cloud comment
[provider]
# pick claude
default_model = "claude-oauth:claude-test"
same_provider_account_failover = false
openai_transport = "https"

[mcp.servers]
x = 1
"#;
        let merged = merge_routing_config(&snippet, Some(cloud)).unwrap();
        let parsed: toml::Value = toml::from_str(&merged).unwrap();
        let provider = &parsed["provider"];
        assert_eq!(
            provider["default_model"].as_str(),
            Some("openai-oauth:gpt-test")
        );
        assert_eq!(provider["default_provider"].as_str(), Some("openai-oauth"));
        assert_eq!(
            provider["cross_provider_failover"].as_str(),
            Some("countdown")
        );
        assert!(provider.get("same_provider_account_failover").is_none());
        assert_eq!(provider["openai_transport"].as_str(), Some("https"));
        assert!(provider.get("openai_service_tier").is_none());
        assert_eq!(parsed["mcp"]["servers"]["x"].as_integer(), Some(1));
        assert!(merged.contains("# cloud comment") && merged.contains("# pick claude"));
        assert!(
            merge_routing_config(&snippet, None)
                .unwrap()
                .contains("countdown")
        );
        assert!(routing_config_snippet("[ui]\nx=1").unwrap().is_none());
        for evil in [
            "[mcp]\nx=1\n[provider]\ndefault_model=\"a\"",
            "[provider]\nopenai_service_tier=\"x\"",
        ] {
            assert!(merge_routing_config(evil, Some(cloud)).is_err());
        }
    }

    #[test]
    fn routing_config_travels_in_bundle() {
        let (_s, src_home, src_config) = dirs();
        let (_d, dst_home, dst_config) = dirs();
        std::fs::write(
            src_home.join(ROUTING_CONFIG_FILE),
            "[provider]\ndefault_provider = \"openai-oauth\"\n[mcp]\nleak = \"no\"\n",
        )
        .unwrap();
        std::fs::write(
            dst_home.join(ROUTING_CONFIG_FILE),
            "[display]\ntheme = \"dark\"\n",
        )
        .unwrap();
        let bundle = export_at(&src_home, &src_config, &env_file_names([])).unwrap();
        assert!(!String::from_utf8_lossy(bundle.as_bytes()).contains(&hex::encode("leak")));
        import_at(&dst_home, &dst_config, bundle.as_bytes()).unwrap();
        let path = dst_home.join(ROUTING_CONFIG_FILE);
        let merged = std::fs::read_to_string(&path).unwrap();
        assert!(
            merged.contains("openai-oauth") && merged.contains("theme") && !merged.contains("leak")
        );
        assert_mode(&path, 0o600);
    }

    #[test]
    fn rejects_traversal_and_symlinks_without_partial_writes() {
        let (_tmp, home, config) = dirs();
        let evil = serde_json::json!({"version":1,"files":[
            {"root":"config","name":"openai.env","data":hex::encode("A=1")},
            {"root":"config","name":"../x.env","data":hex::encode("A=1")}]});
        assert!(import_at(&home, &config, evil.to_string().as_bytes()).is_err());
        assert!(!config.join("openai.env").exists());
        let other = serde_json::json!({"version":1,"files":[
            {"root":"jcode","name":"config.toml","data":hex::encode("x")}]});
        assert!(import_at(&home, &config, other.to_string().as_bytes()).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/dev/null", config.join("openai.env")).unwrap();
            let ok = serde_json::json!({"version":1,"files":[
                {"root":"config","name":"openai.env","data":hex::encode("A=1")}]});
            assert!(import_at(&home, &config, ok.to_string().as_bytes()).is_err());
            assert!(export_at(&home, &config, &env_file_names([])).is_err());
        }
    }
}
