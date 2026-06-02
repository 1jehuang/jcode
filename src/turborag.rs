//! Issue #64: TurboRAG-style precomputed retrieval cache.
//!
//! Storage primitive for TurboRAG: precompute KV-cache fragments
//! per indexed chunk so the model loads them on-demand instead of
//! re-reading the chunk into the prompt window every turn.
//!
//! This module ships the **storage layout** + **lookup API** only.
//! The actual cache fragment format is provider-specific (Anthropic
//! prompt caching vs OpenAI vs others), so the cache values here are
//! opaque blobs identified by `(model_id, chunk_id)`.
//!
//! ## Storage layout
//!
//! ```text
//! <JCODE_HOME>/turborag/
//!   manifest.toml                    # index of cached chunks
//!   blobs/
//!     <model_id>/<chunk_id>.bin      # provider-opaque cache fragment
//! ```
//!
//! `manifest.toml` schema:
//! ```toml
//! [[entry]]
//! chunk_id      = "src/lib.rs:0..200"
//! model_id      = "anthropic/claude-sonnet-4"
//! source_path   = "src/lib.rs"
//! source_hash   = "abc123..."   # xxh32 over the source bytes
//! cached_at     = "2026-05-24T..."
//! token_count   = 800
//! ```
//!
//! ## API
//!
//! ```rust,no_run
//! use jcode::turborag::{Cache, CacheEntry};
//!
//! let cache = Cache::open()?;
//! if let Some(blob) = cache.get("anthropic/claude-sonnet-4", "src/lib.rs:0..200")? {
//!     // pass blob to provider's prompt-cache API
//! }
//! cache.put(CacheEntry {
//!     chunk_id: "src/lib.rs:0..200".to_string(),
//!     model_id: "anthropic/claude-sonnet-4".to_string(),
//!     source_path: "src/lib.rs".to_string(),
//!     source_hash: 0xabc123,
//!     token_count: 800,
//! }, b"opaque cache blob")?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! ## Out of scope (#64 follow-up)
//!
//! - Producer: hooking into the agent's read tool to precompute on
//!   first-read of a file
//! - Consumer: provider integration (Anthropic prompt-cache headers,
//!   etc.). Each provider's caching API differs.
//! - Eviction policy (LRU? size cap?)
//! - Concurrency / multi-process locking on the manifest

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Stable identifier for the cached chunk. Format suggestion:
    /// `<source_path>:<start_line>..<end_line>`. Caller defines.
    pub chunk_id: String,
    /// Model id this fragment was computed for. Different models
    /// can't share fragments because tokenization differs.
    pub model_id: String,
    /// Source file path (relative to repo root or absolute,
    /// caller's choice). Diagnostic only.
    pub source_path: String,
    /// xxh32 hash of the source bytes. Used to invalidate stale
    /// fragments when the file changes.
    pub source_hash: u32,
    /// Approximate token count of the chunk. Diagnostic + budget
    /// accounting only.
    pub token_count: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default, rename = "entry")]
    entries: Vec<StoredEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredEntry {
    #[serde(flatten)]
    entry: CacheEntry,
    cached_at: DateTime<Utc>,
}

pub struct Cache {
    root: PathBuf,
    manifest: Manifest,
}

impl Cache {
    /// Open the user-level TurboRAG cache (under `JCODE_HOME`).
    /// Creates the directory layout if it doesn't exist yet.
    pub fn open() -> Result<Self> {
        let root = jcode_storage::jcode_dir()?.join("turborag");
        Self::open_at(root)
    }

    /// Variant for tests / non-default storage roots.
    pub fn open_at(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(root.join("blobs"))
            .with_context(|| format!("create dir {}", root.display()))?;
        let manifest_path = root.join("manifest.toml");
        let manifest: Manifest = if manifest_path.exists() {
            let raw = std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("read {}", manifest_path.display()))?;
            toml::from_str(&raw).with_context(|| format!("parse {}", manifest_path.display()))?
        } else {
            Manifest::default()
        };
        Ok(Self { root, manifest })
    }

    /// Read a cached blob for `(model_id, chunk_id)`. Returns `None`
    /// when the entry is absent or the blob file is missing on disk.
    pub fn get(&self, model_id: &str, chunk_id: &str) -> Result<Option<Vec<u8>>> {
        if !self
            .manifest
            .entries
            .iter()
            .any(|s| s.entry.model_id == model_id && s.entry.chunk_id == chunk_id)
        {
            return Ok(None);
        }
        let path = self.blob_path(model_id, chunk_id);
        if !path.exists() {
            return Ok(None);
        }
        let bytes =
            std::fs::read(&path).with_context(|| format!("read blob {}", path.display()))?;
        Ok(Some(bytes))
    }

    /// Store a blob keyed on `(model_id, chunk_id)` from the entry.
    /// Overwrites any prior entry for the same key.
    pub fn put(&mut self, entry: CacheEntry, blob: &[u8]) -> Result<()> {
        let path = self.blob_path(&entry.model_id, &entry.chunk_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        std::fs::write(&path, blob).with_context(|| format!("write blob {}", path.display()))?;
        // Replace any prior entry for the same key.
        self.manifest.entries.retain(|s| {
            !(s.entry.model_id == entry.model_id && s.entry.chunk_id == entry.chunk_id)
        });
        self.manifest.entries.push(StoredEntry {
            entry,
            cached_at: Utc::now(),
        });
        self.persist_manifest()
    }

    /// Drop all entries whose `source_hash` differs from the current
    /// hash of `source_path` (caller computes the current hash and
    /// passes it). Returns the number of entries evicted.
    pub fn invalidate_stale(&mut self, source_path: &str, current_hash: u32) -> Result<usize> {
        let before = self.manifest.entries.len();
        let mut evicted = Vec::new();
        self.manifest.entries.retain(|s| {
            let stale = s.entry.source_path == source_path && s.entry.source_hash != current_hash;
            if stale {
                evicted.push((s.entry.model_id.clone(), s.entry.chunk_id.clone()));
            }
            !stale
        });
        for (model_id, chunk_id) in &evicted {
            let _ = std::fs::remove_file(self.blob_path(model_id, chunk_id));
        }
        let count = before - self.manifest.entries.len();
        if count > 0 {
            self.persist_manifest()?;
        }
        Ok(count)
    }

    /// Total cached entries.
    pub fn len(&self) -> usize {
        self.manifest.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn blob_path(&self, model_id: &str, chunk_id: &str) -> PathBuf {
        // Sanitize segments for filesystem safety.
        let safe_model = sanitize(model_id);
        let safe_chunk = sanitize(chunk_id);
        self.root
            .join("blobs")
            .join(safe_model)
            .join(format!("{safe_chunk}.bin"))
    }

    fn persist_manifest(&self) -> Result<()> {
        let path = self.root.join("manifest.toml");
        let toml = toml::to_string_pretty(&self.manifest).context("serialize turborag manifest")?;
        std::fs::write(&path, toml).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '?' | '*' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = Cache::open_at(dir.path().to_path_buf()).unwrap();
        (dir, cache)
    }

    fn entry(chunk: &str) -> CacheEntry {
        CacheEntry {
            chunk_id: chunk.to_string(),
            model_id: "anthropic/claude-sonnet-4".to_string(),
            source_path: "src/lib.rs".to_string(),
            source_hash: 0xdead_beef,
            token_count: 800,
        }
    }

    #[test]
    fn open_creates_layout() {
        let dir = tempfile::TempDir::new().unwrap();
        Cache::open_at(dir.path().to_path_buf()).unwrap();
        assert!(dir.path().join("blobs").exists());
    }

    #[test]
    fn put_then_get_round_trip() {
        let (_dir, mut cache) = fresh_cache();
        cache.put(entry("c1"), b"blob-c1").unwrap();
        let got = cache.get("anthropic/claude-sonnet-4", "c1").unwrap();
        assert_eq!(got.as_deref(), Some(&b"blob-c1"[..]));
    }

    #[test]
    fn get_returns_none_when_missing() {
        let (_dir, cache) = fresh_cache();
        let got = cache.get("anthropic/claude-sonnet-4", "missing").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn put_overwrites_prior_entry() {
        let (_dir, mut cache) = fresh_cache();
        cache.put(entry("c1"), b"v1").unwrap();
        cache.put(entry("c1"), b"v2").unwrap();
        let got = cache.get("anthropic/claude-sonnet-4", "c1").unwrap();
        assert_eq!(got.as_deref(), Some(&b"v2"[..]));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn different_models_dont_collide() {
        let (_dir, mut cache) = fresh_cache();
        let e1 = entry("c1");
        let mut e2 = entry("c1");
        e2.model_id = "openai/gpt-5".to_string();
        cache.put(e1.clone(), b"blob-anthropic").unwrap();
        cache.put(e2.clone(), b"blob-openai").unwrap();
        assert_eq!(
            cache.get(&e1.model_id, "c1").unwrap().as_deref(),
            Some(&b"blob-anthropic"[..])
        );
        assert_eq!(
            cache.get(&e2.model_id, "c1").unwrap().as_deref(),
            Some(&b"blob-openai"[..])
        );
    }

    #[test]
    fn invalidate_stale_drops_old_hashes() {
        let (_dir, mut cache) = fresh_cache();
        cache.put(entry("c1"), b"data").unwrap();
        cache.put(entry("c2"), b"data2").unwrap();
        let mut other_file = entry("c3");
        other_file.source_path = "src/other.rs".to_string();
        cache.put(other_file, b"data3").unwrap();

        let evicted = cache.invalidate_stale("src/lib.rs", 0xfeed_face).unwrap();
        assert_eq!(evicted, 2);
        // c3 is on a different source — keeps.
        assert_eq!(cache.len(), 1);
        assert!(
            cache
                .get("anthropic/claude-sonnet-4", "c1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn invalidate_keeps_matching_hash() {
        let (_dir, mut cache) = fresh_cache();
        cache.put(entry("c1"), b"data").unwrap();
        let evicted = cache.invalidate_stale("src/lib.rs", 0xdead_beef).unwrap();
        assert_eq!(evicted, 0);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn manifest_persists_across_open() {
        let dir = tempfile::TempDir::new().unwrap();
        {
            let mut cache = Cache::open_at(dir.path().to_path_buf()).unwrap();
            cache.put(entry("c1"), b"data").unwrap();
        }
        let cache = Cache::open_at(dir.path().to_path_buf()).unwrap();
        assert_eq!(cache.len(), 1);
        assert!(
            cache
                .get("anthropic/claude-sonnet-4", "c1")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("foo/bar:baz"), "foo_bar_baz");
        assert_eq!(sanitize("a*b?c"), "a_b_c");
        assert_eq!(sanitize("plain"), "plain");
    }
}
