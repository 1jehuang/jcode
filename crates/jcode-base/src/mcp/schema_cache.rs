//! Disk cache of MCP tool schemas (`~/.jcode/mcp-schema-cache.json`).
//!
//! MCP servers connect on a background task and only expose their tool
//! schemas after a JSON-RPC `initialize` + `tools/list` round-trip. That means
//! a freshly spawned session cannot advertise any `mcp__*` tools until those
//! handshakes finish, forcing either a blocking startup or a late tool-snapshot
//! rebuild (an intentional provider prompt-cache miss; see #206).
//!
//! This cache lets us *advertise* a server's tools immediately at spawn using
//! the schemas observed the last time the server connected, so the very first
//! locked tool snapshot already contains them — zero cache miss in the common
//! case. The live connection still happens lazily/in the background, and the
//! actual `tools/call` waits for it (connect-on-first-call). After a real
//! connection completes we reconcile the live schemas back into this cache.
//!
//! Correctness guards:
//! - Each server's cached entry is keyed by a fingerprint of its *config*
//!   (command + args + env + transport + URL + headers + shared). If the config
//!   changes, the fingerprint changes and the stale entry is ignored, so we
//!   never advertise tools for a server that has been reconfigured.
//! - The cache is a hint, not truth. On a real connection we replace the entry
//!   with the live schemas; if they drift, the tool registry reconciles and the
//!   one-shot late-registration rebuild (#206) surfaces the corrected set.

use super::protocol::{McpServerConfig, McpToolDef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

const CACHE_FILE: &str = "mcp-schema-cache.json";
const CACHE_VERSION: u32 = 1;

/// One server's cached tool schemas plus the config fingerprint they were
/// observed under.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedServerSchemas {
    /// Fingerprint of the `McpServerConfig` these schemas were captured under.
    pub fingerprint: String,
    /// Tool definitions exactly as returned by the server's `tools/list`.
    pub tools: Vec<McpToolDef>,
}

/// On-disk representation of the whole cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSchemaCache {
    version: u32,
    /// server name -> cached schemas
    servers: BTreeMap<String, CachedServerSchemas>,
}

impl Default for McpSchemaCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            servers: BTreeMap::new(),
        }
    }
}

/// Stable fingerprint of a server's launch configuration. Two configs that
/// would spawn/connect the same server (and therefore expose the same tools)
/// produce the same fingerprint; any meaningful change invalidates the cache.
pub fn fingerprint_config(config: &McpServerConfig) -> String {
    use std::collections::BTreeMap as SortedMap;
    use std::hash::{Hash, Hasher};

    // Use a deterministic hash over normalized fields. env is sorted so map
    // ordering does not affect the fingerprint.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config.command.hash(&mut hasher);
    for arg in &config.args {
        arg.hash(&mut hasher);
        // Separator so ["a","b"] and ["ab"] differ.
        0u8.hash(&mut hasher);
    }
    let sorted_env: SortedMap<&String, &String> = config.env.iter().collect();
    for (k, v) in sorted_env {
        k.hash(&mut hasher);
        v.hash(&mut hasher);
        0u8.hash(&mut hasher);
    }
    config.transport.hash(&mut hasher);
    config.url.hash(&mut hasher);
    let sorted_headers: SortedMap<&String, &String> = config.headers.iter().collect();
    for (k, v) in sorted_headers {
        k.hash(&mut hasher);
        v.hash(&mut hasher);
        0u8.hash(&mut hasher);
    }
    config.shared.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn cache_path() -> Option<PathBuf> {
    crate::storage::jcode_dir().ok().map(|d| d.join(CACHE_FILE))
}

impl McpSchemaCache {
    /// Load the cache from disk, or return an empty cache on any error or if it
    /// does not exist yet (cold start).
    pub fn load() -> Self {
        let Some(path) = cache_path() else {
            return Self::default();
        };
        // Every session parses this (~1MB) file at spawn and again after each
        // of its MCP connections, so a reload storm parsed it hundreds of times.
        // Reuse the parsed copy while the file is unchanged.
        type Memo = Option<(PathBuf, std::time::SystemTime, u64, McpSchemaCache)>;
        static MEMO: std::sync::LazyLock<std::sync::Mutex<Memo>> =
            std::sync::LazyLock::new(Default::default);
        let stamp = std::fs::metadata(&path)
            .ok()
            .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
        if let Some((mtime, len)) = stamp
            && let Ok(memo) = MEMO.lock()
            && let Some((memo_path, memo_mtime, memo_len, cache)) = memo.as_ref()
            && *memo_path == path
            && *memo_mtime == mtime
            && *memo_len == len
        {
            return cache.clone();
        }
        let loaded = Self::load_uncached(&path);
        if let Some((mtime, len)) = stamp
            && let Ok(mut memo) = MEMO.lock()
        {
            *memo = Some((path, mtime, len, loaded.clone()));
        }
        loaded
    }

    fn load_uncached(path: &std::path::Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&content) {
            Ok(cache) if cache.version == CACHE_VERSION => cache,
            // Version mismatch or parse error: ignore and start fresh.
            _ => Self::default(),
        }
    }

    /// Return cached tools for `server` only if the cached fingerprint matches
    /// the current config. A mismatch means the server was reconfigured, so the
    /// stale schemas must not be advertised.
    pub fn tools_for(&self, server: &str, config: &McpServerConfig) -> Option<&[McpToolDef]> {
        let entry = self.servers.get(server)?;
        if entry.fingerprint == fingerprint_config(config) {
            Some(&entry.tools)
        } else {
            None
        }
    }

    /// Persist `tools` for `server` unless this process already recorded the
    /// identical catalog under the same config. Each session reports every
    /// shared server's connection, so without this a reload storm cloned,
    /// compared, and rewrote the whole cache hundreds of times (~30ms CPU each).
    /// Returns true when the file was written.
    pub fn sync_server(server: &str, config: &McpServerConfig, tools: &[McpToolDef]) -> bool {
        use std::hash::{Hash, Hasher};
        static SEEN: std::sync::LazyLock<std::sync::Mutex<BTreeMap<String, u64>>> =
            std::sync::LazyLock::new(Default::default);
        let mut catalog = std::collections::hash_map::DefaultHasher::new();
        fingerprint_config(config).hash(&mut catalog);
        for tool in tools {
            tool.name.hash(&mut catalog);
            tool.description.hash(&mut catalog);
            tool.input_schema.to_string().hash(&mut catalog);
        }
        let catalog = catalog.finish();
        // Include the file's identity so an external edit (another process,
        // a manual reset) is never masked by the in-process memo.
        let digest = || {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            catalog.hash(&mut hasher);
            if let Some(path) = cache_path()
                && let Ok(meta) = std::fs::metadata(&path)
            {
                meta.len().hash(&mut hasher);
                meta.modified().ok().hash(&mut hasher);
            }
            hasher.finish()
        };
        if let Ok(seen) = SEEN.lock()
            && seen.get(server) == Some(&digest())
        {
            return false;
        }
        let mut cache = Self::load();
        let written = cache.update(server, config, tools.to_vec());
        if written {
            cache.save();
        }
        if let Ok(mut seen) = SEEN.lock() {
            seen.insert(server.to_string(), digest());
        }
        written
    }

    /// Insert/replace the cached schemas for a server. Returns true if the entry
    /// actually changed (new server, or different fingerprint/tools), so callers
    /// can avoid rewriting the file when nothing changed.
    pub fn update(
        &mut self,
        server: &str,
        config: &McpServerConfig,
        tools: Vec<McpToolDef>,
    ) -> bool {
        let fingerprint = fingerprint_config(config);
        let changed = match self.servers.get(server) {
            Some(existing) => {
                existing.fingerprint != fingerprint || !tool_defs_equal(&existing.tools, &tools)
            }
            None => true,
        };
        if changed {
            self.servers.insert(
                server.to_string(),
                CachedServerSchemas { fingerprint, tools },
            );
        }
        changed
    }

    /// Remove cached entries for servers no longer present in `current_servers`.
    /// Returns true if anything was pruned.
    pub fn retain_servers(&mut self, current_servers: &[String]) -> bool {
        let before = self.servers.len();
        self.servers
            .retain(|name, _| current_servers.iter().any(|n| n == name));
        self.servers.len() != before
    }

    /// Persist the cache to disk (best effort). Errors are logged, not returned,
    /// because a failed cache write must never break MCP functionality.
    pub fn save(&self) {
        let Some(path) = cache_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&path, json) {
                    crate::logging::error(&format!(
                        "MCP schema cache: failed to write {}: {}",
                        path.display(),
                        err
                    ));
                }
            }
            Err(err) => {
                crate::logging::error(&format!("MCP schema cache: failed to serialize: {}", err));
            }
        }
    }

    /// Number of servers currently cached (for tests/diagnostics).
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }
}

/// Structural equality of two tool-def lists (order-sensitive on name, but we
/// compare the full set so reordering by the server doesn't churn the cache).
fn tool_defs_equal(a: &[McpToolDef], b: &[McpToolDef]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let key = |t: &McpToolDef| {
        (
            t.name.clone(),
            t.description.clone(),
            t.input_schema.to_string(),
        )
    };
    let mut a_keys: Vec<_> = a.iter().map(key).collect();
    let mut b_keys: Vec<_> = b.iter().map(key).collect();
    a_keys.sort();
    b_keys.sort();
    a_keys == b_keys
}

#[cfg(test)]
#[path = "schema_cache_tests.rs"]
mod schema_cache_tests;
