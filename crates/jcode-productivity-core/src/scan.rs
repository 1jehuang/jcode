//! Transcript scanning with an on-disk incremental cache.
//!
//! Scanning is the expensive part: there can be ~100k JSON transcripts totaling
//! several GB. We keep a sidecar cache (`~/.jcode/cache/productivity/summaries.json`)
//! keyed by `(file_len, mtime_ns)` so a re-run only re-parses changed files.
//! Parsing of the changed set is parallelized with rayon.

use crate::model::SessionSummary;
use anyhow::Result;
use chrono::{DateTime, Datelike, Local, Timelike};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// One cached entry: the file fingerprint plus the computed summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    len: u64,
    mtime_ns: i128,
    summary: SessionSummary,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    /// Cache format version; bump to invalidate when summary semantics change.
    #[serde(default)]
    version: u32,
    entries: HashMap<String, CacheEntry>,
}

const CACHE_VERSION: u32 = 1;

fn cache_path() -> Result<PathBuf> {
    let dir = jcode_storage::jcode_dir()?
        .join("cache")
        .join("productivity");
    std::fs::create_dir_all(&dir).ok();
    Ok(dir.join("summaries.json"))
}

fn sessions_dir() -> Result<PathBuf> {
    Ok(jcode_storage::jcode_dir()?.join("sessions"))
}

/// Result of a full scan: every session summary plus scan diagnostics.
pub struct ScanResult {
    pub summaries: Vec<SessionSummary>,
    pub scanned_files: u64,
    pub parse_errors: u64,
    pub cache_hits: u64,
    pub scan_secs: f64,
}

/// Scan all session transcripts, using and refreshing the incremental cache.
pub fn scan_all() -> Result<ScanResult> {
    let started = Instant::now();
    let dir = sessions_dir()?;

    // Load prior cache (best-effort; ignore corruption).
    let mut cache: Cache = std::fs::read(cache_path()?)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if cache.version != CACHE_VERSION {
        cache = Cache {
            version: CACHE_VERSION,
            entries: HashMap::new(),
        };
    }

    // Enumerate candidate transcript files.
    let mut files: Vec<(String, u64, i128)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            let is_json = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("json"))
                .unwrap_or(false);
            if !is_json {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let len = meta.len();
            let mtime_ns = mtime_ns(&meta);
            files.push((name, len, mtime_ns));
        }
    }

    let cache_hits = Mutex::new(0u64);
    let parse_errors = Mutex::new(0u64);

    // Parse (or reuse cache) in parallel. Returns (filename, entry).
    let results: Vec<(String, CacheEntry)> = files
        .par_iter()
        .filter_map(|(name, len, mtime_ns)| {
            if let Some(prev) = cache.entries.get(name)
                && prev.len == *len
                && prev.mtime_ns == *mtime_ns
            {
                *cache_hits.lock().unwrap() += 1;
                return Some((name.clone(), prev.clone()));
            }
            let path = dir.join(name);
            match parse_session_file(&path) {
                Ok(summary) => Some((
                    name.clone(),
                    CacheEntry {
                        len: *len,
                        mtime_ns: *mtime_ns,
                        summary,
                    },
                )),
                Err(_) => {
                    *parse_errors.lock().unwrap() += 1;
                    None
                }
            }
        })
        .collect();

    // Rebuild cache from this scan (drops entries for deleted files).
    let mut new_entries: HashMap<String, CacheEntry> = HashMap::with_capacity(results.len());
    let mut summaries: Vec<SessionSummary> = Vec::with_capacity(results.len());
    for (name, entry) in results {
        summaries.push(entry.summary.clone());
        new_entries.insert(name, entry);
    }
    let new_cache = Cache {
        version: CACHE_VERSION,
        entries: new_entries,
    };
    if let Ok(bytes) = serde_json::to_vec(&new_cache)
        && let Ok(path) = cache_path()
    {
        let _ = std::fs::write(path, bytes);
    }

    Ok(ScanResult {
        scanned_files: summaries.len() as u64,
        parse_errors: parse_errors.into_inner().unwrap_or(0),
        cache_hits: cache_hits.into_inner().unwrap_or(0),
        scan_secs: started.elapsed().as_secs_f64(),
        summaries,
    })
}

#[cfg(unix)]
fn mtime_ns(meta: &std::fs::Metadata) -> i128 {
    use std::os::unix::fs::MetadataExt;
    (meta.mtime() as i128) * 1_000_000_000 + (meta.mtime_nsec() as i128)
}

#[cfg(not(unix))]
fn mtime_ns(meta: &std::fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Minimal transcript parsing
//
// We deliberately use a tolerant, partial deserialization instead of the full
// `Session`/`StoredMessage` types so this crate stays dependency-light and keeps
// working even if the canonical schema drifts.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawSession {
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    provider_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    messages: Vec<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Vec<RawBlock>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    token_usage: Option<RawTokenUsage>,
}

#[derive(Deserialize)]
struct RawTokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    Image {},
    #[serde(other)]
    Other,
}

fn parse_session_file(path: &Path) -> Result<SessionSummary> {
    let bytes = std::fs::read(path)?;
    let raw: RawSession = serde_json::from_slice(&bytes)?;
    Ok(summarize(raw))
}

fn summarize(raw: RawSession) -> SessionSummary {
    let mut s = SessionSummary {
        created_at: raw.created_at.clone(),
        updated_at: raw.updated_at.clone(),
        working_dir: raw.working_dir.clone(),
        project: raw.working_dir.as_deref().map(project_name),
        provider_key: raw.provider_key,
        model: raw.model,
        ..Default::default()
    };

    let mut active_dates = std::collections::BTreeSet::new();
    let record_time =
        |ts: &str, s: &mut SessionSummary, dates: &mut std::collections::BTreeSet<String>| {
            if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
                let local = dt.with_timezone(&Local);
                s.hour_hist[local.hour() as usize] += 1;
                s.weekday_hist[local.weekday().num_days_from_monday() as usize] += 1;
                dates.insert(local.format("%Y-%m-%d").to_string());
            }
        };

    for msg in &raw.messages {
        let role = msg.role.as_deref().unwrap_or("");
        match role {
            "user" => s.user_msgs += 1,
            "assistant" => s.assistant_msgs += 1,
            _ => {}
        }

        if let Some(ts) = &msg.timestamp {
            if s.first_ts.is_none() {
                s.first_ts = Some(ts.clone());
            }
            s.last_ts = Some(ts.clone());
            record_time(ts, &mut s, &mut active_dates);
        }

        if let Some(tu) = &msg.token_usage {
            s.input_tokens += tu.input_tokens;
            s.output_tokens += tu.output_tokens;
            s.cache_read_tokens += tu.cache_read_input_tokens.unwrap_or(0);
            s.cache_creation_tokens += tu.cache_creation_input_tokens.unwrap_or(0);
        }

        for block in &msg.content {
            match block {
                RawBlock::Text { text } => {
                    let len = text.chars().count() as u64;
                    if role == "user" {
                        // Skip giant tool-result-ish or reminder blobs from the
                        // human "typed" proxy; keep it to real prompts.
                        if !text.trim_start().starts_with("<system-reminder>") {
                            s.user_chars += len;
                        }
                    } else if role == "assistant" {
                        s.assistant_chars += len;
                    }
                }
                RawBlock::ToolUse { name, input } => {
                    count_tool(&mut s.tools, name, input);
                }
                RawBlock::Image {} => s.images += 1,
                RawBlock::Other => {}
            }
        }
    }

    // Fall back to header timestamps for the activity calendar when individual
    // messages lacked timestamps (common for imported transcripts).
    if active_dates.is_empty() {
        if let Some(ts) = raw.updated_at.as_deref().or(raw.created_at.as_deref())
            && let Ok(dt) = DateTime::parse_from_rfc3339(ts)
        {
            let local = dt.with_timezone(&Local);
            s.hour_hist[local.hour() as usize] += 1;
            s.weekday_hist[local.weekday().num_days_from_monday() as usize] += 1;
            active_dates.insert(local.format("%Y-%m-%d").to_string());
        }
    }

    s.active_dates = active_dates.into_iter().collect();
    s
}

/// Expand a single tool invocation into the histogram. `batch` is special-cased
/// so the inner tool calls get individual credit.
fn count_tool(tools: &mut BTreeMap<String, u32>, name: &str, input: &serde_json::Value) {
    let canonical = canonical_tool_name(name);
    *tools.entry(canonical.to_string()).or_insert(0) += 1;

    if canonical == "batch"
        && let Some(calls) = input.get("tool_calls").and_then(|v| v.as_array())
    {
        for call in calls {
            if let Some(inner) = call.get("tool").and_then(|v| v.as_str()) {
                let inner_canon = canonical_tool_name(inner);
                *tools.entry(inner_canon.to_string()).or_insert(0) += 1;
            }
        }
    }
}

/// Normalize legacy/alias tool names to their canonical identity.
fn canonical_tool_name(name: &str) -> &str {
    match name {
        "file_read" => "read",
        "file_write" => "write",
        "file_edit" => "edit",
        "file_grep" => "grep",
        "todowrite" => "todo",
        other => other,
    }
}

fn project_name(working_dir: &str) -> String {
    let trimmed = working_dir.trim_end_matches('/');
    Path::new(trimmed)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}
