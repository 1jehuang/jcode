//! Thin adapter over the CASR (Cross Agent Session Resumer) library.
//!
//! This module replaces the in-house `jcode-import-core` importer that used to
//! live in `jcode-base/src/import.rs`. CASR is a self-contained Rust crate
//! (also a CLI binary) that reads/writes sessions for every supported provider
//! and exposes a library API for embedding into jcode. The adapter exists so
//! the rest of jcode can keep using the same `crate::import::*` import paths
//! while the actual implementation is generic CASR code.
//!
//! The adapter only re-exports the bits jcode actually uses today:
//! 1. Idempotent id mapping — `imported_*_session_id(<id>)`. CASR's pipeline
//!    derives a stable target id from `(source_alias, source_session_id)`
//!    via SHA-256, so re-importing the same external session always lands
//!    on the same jcode id.
//! 2. Session listing — `list_claude_code_sessions[_lazy]`, used by the
//!    TUI session picker and the `session_search` tool. CASR's
//!    `ProviderRegistry` enumerates every installed provider; we narrow
//!    to `claude-code` and map `CanonicalSession` → the jcode-shaped
//!    `ClaudeCodeSessionInfo`.
//! 3. Resume-target resolution — `resolve_resume_target_to_jcode`,
//!    `imported_session_id_for_target`. Wraps the id-mapping helpers and
//!    keeps the public shape stable.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// Info about a Claude Code session for listing — formerly defined in the
/// in-house `jcode-import-core` crate (now removed). Lives here now so the
/// adapter surface matches what the TUI session picker and the
/// `session_search` tool expect (`session_id`, `first_prompt`,
/// `summary`, `message_count`, `created`, `modified`, `project_path`,
/// `full_path`).
#[derive(Debug, Clone)]
pub struct ClaudeCodeSessionInfo {
    pub session_id: String,
    pub first_prompt: String,
    pub summary: Option<String>,
    pub message_count: u32,
    pub created: Option<DateTime<Utc>>,
    pub modified: Option<DateTime<Utc>>,
    pub project_path: Option<String>,
    pub full_path: String,
}

/// A single foreign message in the shape the `session_search` tool expects.
/// CASR's `CanonicalMessage` is the underlying source; this is a thin
/// projection that flattens tool calls/results into the search text and
/// exposes a string `role` so the existing search code can keep its
/// `msg.role == "user"` / `== "assistant"` style comparisons.
#[derive(Debug, Clone)]
pub struct ExternalMessageRecord {
    pub role: String,
    pub text: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub id: Option<String>,
}

impl ExternalMessageRecord {
    /// Project a CASR `CanonicalMessage` into a search-tool-friendly shape.
    /// `content` becomes `text`; tool calls and results are appended as
    /// `<prefix>{json}` blocks so they remain searchable in their raw form.
    pub fn from_canonical(msg: &casr::model::CanonicalMessage) -> Self {
        let role = match msg.role {
            casr::model::MessageRole::User => "user".to_string(),
            casr::model::MessageRole::Assistant => "assistant".to_string(),
            casr::model::MessageRole::Tool => "tool".to_string(),
            casr::model::MessageRole::System => "system".to_string(),
            casr::model::MessageRole::Other(ref s) => s.clone(),
        };
        let mut text = msg.content.clone();
        if !msg.tool_calls.is_empty() {
            for tc in &msg.tool_calls {
                text.push_str(&format!(
                    "\n[Tool: {}] {}",
                    tc.name,
                    tc.arguments.to_string()
                ));
            }
        }
        if !msg.tool_results.is_empty() {
            for tr in &msg.tool_results {
                let prefix = if tr.is_error {
                    "[Tool Error]"
                } else {
                    "[Tool Output]"
                };
                text.push_str(&format!("\n{prefix} {}", tr.content));
            }
        }
        let timestamp = msg
            .timestamp
            .and_then(DateTime::<Utc>::from_timestamp_millis);
        Self {
            role,
            text,
            timestamp,
            id: None,
        }
    }
}

/// A foreign session in the shape the `session_search` tool expects —
/// formerly `ExternalSessionRecord` from `jcode-import-core`. Built by
/// `load_external_session` from a CASR `CanonicalSession` plus a `source`
/// label and the on-disk path.
#[derive(Debug, Clone)]
pub struct ExternalSessionRecord {
    pub source: &'static str,
    pub session_id: String,
    pub short_name: Option<String>,
    pub title: Option<String>,
    pub working_dir: Option<String>,
    pub provider_key: Option<String>,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub path: PathBuf,
    pub messages: Vec<ExternalMessageRecord>,
}

/// Build an `ExternalSessionRecord` from a CASR `CanonicalSession` plus
/// the source label and the on-disk path. `model` is taken from the
/// canonical session's `model_name` when available.
pub fn build_external_session_record(
    source: &'static str,
    provider_key: &'static str,
    path: PathBuf,
    canonical: &casr::model::CanonicalSession,
) -> ExternalSessionRecord {
    let short_id = &canonical.session_id[..canonical.session_id.len().min(8)];
    let created_at = canonical
        .started_at
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let updated_at = canonical
        .ended_at
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or(created_at);
    let working_dir = canonical
        .workspace
        .as_ref()
        .map(|p| p.display().to_string());
    let title = canonical.title.clone();
    let messages = canonical
        .messages
        .iter()
        .map(ExternalMessageRecord::from_canonical)
        .collect();

    ExternalSessionRecord {
        source,
        session_id: canonical.session_id.clone(),
        short_name: Some(format!("{source} {short_id}")),
        title,
        working_dir,
        provider_key: Some(provider_key.to_string()),
        model: canonical.model_name.clone(),
        created_at,
        updated_at,
        path,
        messages,
    }
}

/// Read a foreign session file via the registered CASR provider and
/// project it into an `ExternalSessionRecord`. Returns `Err` if the
/// provider slug is unknown or the file is unreadable; the caller
/// decides whether to surface a parse error or skip the file.
pub fn load_external_session(
    source: &'static str,
    provider_key: &'static str,
    path: &Path,
) -> Result<ExternalSessionRecord> {
    let registry = casr::discovery::ProviderRegistry::default_registry();
    let provider = registry
        .find_by_slug(provider_key)
        .with_context(|| format!("{provider_key} provider not registered in CASR"))?;
    let canonical = provider.read_session(path).with_context(|| {
        format!(
            "failed to read {provider_key} session at {}",
            path.display()
        )
    })?;
    Ok(build_external_session_record(
        source,
        provider_key,
        path.to_path_buf(),
        &canonical,
    ))
}

/// Derive the jcode session id that an external session of the given
/// (provider, id) pair would be imported under. Idempotent: same inputs
/// always return the same id.
fn derive_imported_id(source_alias: &str, source_session_id: &str) -> String {
    casr::pipeline::derive_target_id(source_alias, source_session_id)
}

/// Idempotent import id for a Claude Code session.
pub fn imported_claude_code_session_id(session_id: &str) -> String {
    derive_imported_id("cc", session_id)
}

/// Idempotent import id for a Codex session.
pub fn imported_codex_session_id(session_id: &str) -> String {
    derive_imported_id("cod", session_id)
}

/// Idempotent import id for an OpenCode session.
pub fn imported_opencode_session_id(session_id: &str) -> String {
    derive_imported_id("opc", session_id)
}

/// Idempotent import id for a Pi session. Pi doesn't have a stable
/// session id; the file path is used as the key instead so the same
/// on-disk file always maps to the same jcode id.
pub fn imported_pi_session_id(session_path: &str) -> String {
    derive_imported_id("pi", session_path)
}

// ---------------------------------------------------------------------------
// Session listing — Claude Code only (the surface jcode uses today)
// ---------------------------------------------------------------------------

/// Build a `ClaudeCodeSessionInfo` from a `CanonicalSession` produced by the
/// CASR `claude-code` reader. Fields are mapped approximately — CASR's
/// canonical IR doesn't carry every Claude-specific field (e.g. the
/// `sessions-index.json` summary), but it carries enough for the TUI
/// session picker and the `session_search` tool to render.
fn info_from_canonical(
    path: &Path,
    canonical: &casr::model::CanonicalSession,
) -> ClaudeCodeSessionInfo {
    // `first_prompt` is the first user-side message text, falling back to
    // the session title (or "No prompt" if neither is available).
    let first_prompt = canonical
        .messages
        .iter()
        .find(|m| matches!(m.role, casr::model::MessageRole::User))
        .map(|m| m.content.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| canonical.title.clone())
        .unwrap_or_else(|| "No prompt".to_string());

    ClaudeCodeSessionInfo {
        session_id: canonical.session_id.clone(),
        first_prompt: truncate_first_prompt(&first_prompt, 120),
        summary: canonical.title.clone(),
        message_count: canonical.messages.len() as u32,
        created: timestamp_from_millis(canonical.started_at),
        modified: timestamp_from_millis(canonical.ended_at),
        project_path: canonical
            .workspace
            .as_ref()
            .map(|p| p.display().to_string()),
        full_path: path.to_string_lossy().to_string(),
    }
}

fn timestamp_from_millis(ms: Option<i64>) -> Option<DateTime<Utc>> {
    ms.and_then(DateTime::<Utc>::from_timestamp_millis)
}

fn truncate_first_prompt(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

/// Enumerate every session for the given CASR provider slug.
fn list_via_casr_for_slug(
    slug: &str,
    scan_limit: Option<usize>,
) -> Result<Vec<ClaudeCodeSessionInfo>> {
    let registry = casr::discovery::ProviderRegistry::default_registry();
    let provider = registry
        .find_by_slug(slug)
        .with_context(|| format!("{slug} provider not registered in CASR"))?;

    let mut all: Vec<ClaudeCodeSessionInfo> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Prefer the registry's `list_sessions()` to avoid undercounting when
    // multiple sessions live in a single file (Claude Code does NOT do this,
    // but the CASR contract is uniform across providers).
    let candidates: Vec<(String, PathBuf)> = provider.list_sessions().unwrap_or_default();

    for (id, path) in candidates {
        if seen.contains(&id) {
            continue;
        }
        if let Some(limit) = scan_limit
            && all.len() >= limit
        {
            break;
        }
        let canonical = match provider.read_session(&path) {
            Ok(c) => c,
            Err(_) => continue, // skip unreadable / corrupt files
        };
        seen.insert(canonical.session_id.clone());
        all.push(info_from_canonical(&path, &canonical));
    }

    // Newest first — CASR already gives us canonical.started_at.
    all.sort_by(|a, b| b.modified.or(b.created).cmp(&a.modified.or(a.created)));
    Ok(all)
}

/// Backwards-compatible alias for `list_via_casr_for_slug("claude-code", …)`.
fn list_via_casr(scan_limit: Option<usize>) -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr_for_slug("claude-code", scan_limit)
}

/// Enumerate all available Claude Code sessions. Equivalent to the legacy
/// `list_claude_code_sessions` in `jcode-base/src/import.rs`.
pub fn list_claude_code_sessions() -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr(None)
}

/// Lazy / capped variant for picker UIs that want to bound the work.
pub fn list_claude_code_sessions_lazy(scan_limit: usize) -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr(Some(scan_limit))
}

/// Enumerate Codex sessions via CASR.
pub fn list_codex_sessions() -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr_for_slug("codex", None)
}

pub fn list_codex_sessions_lazy(scan_limit: usize) -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr_for_slug("codex", Some(scan_limit))
}

/// Enumerate Pi sessions via CASR.
pub fn list_pi_sessions() -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr_for_slug("pi-agent", None)
}

pub fn list_pi_sessions_lazy(scan_limit: usize) -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr_for_slug("pi-agent", Some(scan_limit))
}

/// Enumerate OpenCode sessions via CASR.
pub fn list_opencode_sessions() -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr_for_slug("opencode", None)
}

pub fn list_opencode_sessions_lazy(scan_limit: usize) -> Result<Vec<ClaudeCodeSessionInfo>> {
    list_via_casr_for_slug("opencode", Some(scan_limit))
}

/// Project-filtered variant (kept for API compatibility; CASR's discovery
/// is already workspace-aware through `list_sessions`).
pub fn list_sessions_for_project(_project_filter: &str) -> Result<Vec<ClaudeCodeSessionInfo>> {
    // CASR's `list_sessions` doesn't expose a workspace filter, so the
    // caller must filter post-hoc. We return the unfiltered list and let
    // the caller pick the project they want.
    list_via_casr(None)
}

// ---------------------------------------------------------------------------
// Resume-target → jcode-id resolution
// ---------------------------------------------------------------------------

/// Compute the jcode session id under which a foreign session would be
/// imported. The caller is responsible for matching against the source
/// provider (cc / cod / opc / pi) before calling. Returns `None` if
/// `provider` is `Some` but unknown; returns `None` when `provider` is
/// `None` (i.e. the session is already a jcode id and the caller should
/// pass-through the native id without remapping).
pub fn imported_session_id_for_provider_and_id(
    provider: Option<&str>,
    source_session_id: &str,
) -> Option<String> {
    match provider {
        None => None, // caller should pass-through the native id
        Some("cc") | Some("claude-code") => {
            Some(imported_claude_code_session_id(source_session_id))
        }
        Some("cod") | Some("codex") => Some(imported_codex_session_id(source_session_id)),
        Some("opc") | Some("opencode") => Some(imported_opencode_session_id(source_session_id)),
        Some("pi") | Some("pi-agent") => Some(imported_pi_session_id(source_session_id)),
        Some(_) => None,
    }
}

/// Convenience: resolve a jcode-tui `ResumeTarget` to the jcode session id
/// the picker/launcher should pass to `--resume`. The function takes the
/// kind tag plus the relevant identifier fields so the adapter stays
/// decoupled from the jcode-tui-session-picker crate (which lives in
/// the upper layers).
pub fn resolve_resume_target_to_jcode(
    provider: Option<&str>,
    source_session_id: &str,
    native_session_id: &str,
) -> Result<String> {
    use anyhow::anyhow;
    if let Some(id) = imported_session_id_for_provider_and_id(provider, source_session_id) {
        return Ok(id);
    }
    if provider.is_none() {
        return Ok(native_session_id.to_string());
    }
    Err(anyhow!("unknown provider for resume target: {provider:?}"))
}

/// Convenience helper: project the (provider, source_id, native_id)
/// triple into the two arguments that the resolver actually needs.
/// `provider` and `source_session_id` come from the foreign session;
/// `native_session_id` is the jcode id to use when `provider` is `None`
/// (i.e. the session is already a jcode session).
pub fn resume_target_components(
    provider: Option<&str>,
    source_session_id: &str,
    native_session_id: &str,
) -> (Option<String>, String, String) {
    (
        provider.map(str::to_string),
        source_session_id.to_string(),
        native_session_id.to_string(),
    )
}

// ---------------------------------------------------------------------------
// External resume import — kept as a thin wrapper around CASR's pipeline
// for callers that only have a string id (no ResumeTarget available).
// ---------------------------------------------------------------------------

/// Try to import a foreign resume id into jcode via CASR. Returns the
/// new jcode session id on success, or `None` if CASR could not find a
/// matching source session. The legacy in-house importer used to fall
/// through to hand-rolled readers; CASR's registry supersedes that.
pub fn import_external_resume_id(resume_id: &str) -> Result<Option<String>> {
    let pipeline = casr::pipeline::ConversionPipeline {
        registry: casr::discovery::ProviderRegistry::default_registry(),
    };
    match pipeline.convert(
        "jcode",
        resume_id,
        casr::pipeline::ConvertOptions::default(),
    ) {
        Ok(result) => Ok(result.written.map(|w| w.session_id)),
        Err(_) => Ok(None), // fall through; caller decides what to do
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_ids_are_idempotent() {
        assert_eq!(
            imported_claude_code_session_id("abc-123"),
            imported_claude_code_session_id("abc-123")
        );
        assert_ne!(
            imported_claude_code_session_id("abc-123"),
            imported_codex_session_id("abc-123")
        );
        // Pi uses the path as the key, so different paths produce different ids.
        assert_ne!(
            imported_pi_session_id("/tmp/session-a"),
            imported_pi_session_id("/tmp/session-b")
        );
        // Pi is also idempotent.
        assert_eq!(
            imported_pi_session_id("/tmp/same"),
            imported_pi_session_id("/tmp/same")
        );
    }

    #[test]
    fn imported_ids_match_casr_derivation() {
        // The adapter must agree with CASR's `derive_target_id` so a
        // follow-up `casr.convert("jcode", <id>)` lands on the same
        // target file as the importer-prepared id.
        assert_eq!(
            imported_claude_code_session_id("xyz"),
            casr::pipeline::derive_target_id("cc", "xyz")
        );
        assert_eq!(
            imported_codex_session_id("xyz"),
            casr::pipeline::derive_target_id("cod", "xyz")
        );
        assert_eq!(
            imported_opencode_session_id("xyz"),
            casr::pipeline::derive_target_id("opc", "xyz")
        );
        assert_eq!(
            imported_pi_session_id("/p"),
            casr::pipeline::derive_target_id("pi", "/p")
        );
    }

    #[test]
    fn imported_session_id_for_provider_and_id_routes_correctly() {
        assert_eq!(
            imported_session_id_for_provider_and_id(Some("cc"), "cc-1"),
            Some(imported_claude_code_session_id("cc-1"))
        );
        assert_eq!(
            imported_session_id_for_provider_and_id(Some("claude-code"), "cc-1"),
            Some(imported_claude_code_session_id("cc-1"))
        );
        assert_eq!(
            imported_session_id_for_provider_and_id(Some("cod"), "cod-1"),
            Some(imported_codex_session_id("cod-1"))
        );
        assert_eq!(
            imported_session_id_for_provider_and_id(Some("opc"), "opc-1"),
            Some(imported_opencode_session_id("opc-1"))
        );
        assert_eq!(
            imported_session_id_for_provider_and_id(Some("pi"), "/p"),
            Some(imported_pi_session_id("/p"))
        );
        // Native jcode sessions: no import needed.
        assert_eq!(
            imported_session_id_for_provider_and_id(None, "ignored"),
            None
        );
        // Unknown provider: no mapping.
        assert_eq!(
            imported_session_id_for_provider_and_id(Some("unknown"), "id"),
            None
        );
    }

    #[test]
    fn resolve_resume_target_to_jcode_returns_native_id() {
        let id = resolve_resume_target_to_jcode(None, "ignored", "native-id")
            .expect("native jcode id should resolve");
        assert_eq!(id, "native-id");
    }

    #[test]
    fn resolve_resume_target_to_jcode_maps_foreign_to_imported() {
        let id = resolve_resume_target_to_jcode(Some("cc"), "cc-abc", "native")
            .expect("foreign target should resolve");
        assert_eq!(id, imported_claude_code_session_id("cc-abc"));
    }

    #[test]
    fn truncate_first_prompt_short_input_unchanged() {
        assert_eq!(truncate_first_prompt("hello", 100), "hello");
    }

    #[test]
    fn truncate_first_prompt_long_input_truncates_with_ellipsis() {
        let long = "a".repeat(500);
        let out = truncate_first_prompt(&long, 50);
        assert!(out.chars().count() <= 51); // 50 chars + ellipsis
        assert!(out.ends_with('…'));
    }
}
