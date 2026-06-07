//! Persistent memory event log for post-session analysis.
//!
//! Writes structured JSONL (one JSON object per line) to:
//!   `~/.jcode/logs/memory-events-YYYY-MM-DD.jsonl`
//!
//! Every memory pipeline event - embedding search, sidecar verification,
//! injection, extraction, maintenance, tool actions - is captured with
//! wall-clock timestamps, session ID, and full details.
//!
//! Logs are kept for 14 days (separate from general log rotation).

use crate::memory_types::MemoryEventKind;
use chrono::Local;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static MEMORY_LOGGER: Mutex<Option<MemoryLogger>> = Mutex::new(None);

struct MemoryLogger {
    file: File,
    current_date: String,
}

impl MemoryLogger {
    fn open(date: &str) -> Option<Self> {
        let dir = log_dir()?;
        fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("memory-events-{}.jsonl", date));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        Some(Self {
            file,
            current_date: date.to_string(),
        })
    }

    fn write_entry(&mut self, entry: &LogEntry) {
        if let Ok(json) = serde_json::to_string(entry) {
            let _ = writeln!(self.file, "{}", json);
            let _ = self.file.flush();
        }
    }
}

fn log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".jcode").join("logs"))
}

fn ensure_logger(date: &str) -> bool {
    if let Ok(mut guard) = MEMORY_LOGGER.lock() {
        if let Some(ref logger) = *guard
            && logger.current_date == date
        {
            return true;
        }
        *guard = MemoryLogger::open(date);
        guard.is_some()
    } else {
        false
    }
}

#[derive(Serialize)]
struct LogEntry {
    timestamp: String,
    session_id: Option<String>,
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

fn current_session_id() -> Option<String> {
    crate::logging::current_session()
}

fn write_log(event: &str, detail: Option<serde_json::Value>) {
    let now = Local::now();
    let date = now.format("%Y-%m-%d").to_string();

    if !ensure_logger(&date) {
        return;
    }

    let entry = LogEntry {
        timestamp: now.format("%Y-%m-%dT%H:%M:%S%.3f%z").to_string(),
        session_id: current_session_id(),
        event: event.to_string(),
        detail,
    };

    if let Ok(mut guard) = MEMORY_LOGGER.lock()
        && let Some(logger) = guard.as_mut()
    {
        logger.write_entry(&entry);
    }
}

/// Log a memory event from the in-memory event system.
pub fn log_event(kind: &MemoryEventKind) {
    let (event, detail) = match kind {
        MemoryEventKind::EmbeddingStarted => ("embedding_started", None),

        MemoryEventKind::EmbeddingComplete { latency_ms, hits } => (
            "embedding_complete",
            Some(serde_json::json!({
                "latency_ms": latency_ms,
                "hits": hits,
            })),
        ),

        MemoryEventKind::SidecarStarted => ("sidecar_started", None),

        MemoryEventKind::SidecarRelevant { memory_preview } => (
            "sidecar_relevant",
            Some(serde_json::json!({
                "memory_preview": memory_preview,
            })),
        ),

        MemoryEventKind::SidecarNotRelevant => ("sidecar_not_relevant", None),

        MemoryEventKind::SidecarComplete { latency_ms } => (
            "sidecar_complete",
            Some(serde_json::json!({
                "latency_ms": latency_ms,
            })),
        ),

        MemoryEventKind::MemorySurfaced { memory_preview } => (
            "memory_surfaced",
            Some(serde_json::json!({
                "memory_preview": memory_preview,
            })),
        ),

        MemoryEventKind::MemoryInjected {
            count,
            prompt_chars,
            age_ms,
            preview,
            items,
        } => (
            "memory_injected",
            Some(serde_json::json!({
                "count": count,
                "prompt_chars": prompt_chars,
                "age_ms": age_ms,
                "preview": preview,
                "items": items.iter().map(|i| serde_json::json!({
                    "section": i.section,
                    "content": i.content,
                })).collect::<Vec<_>>(),
            })),
        ),

        MemoryEventKind::MaintenanceStarted { verified, rejected } => (
            "maintenance_started",
            Some(serde_json::json!({
                "verified": verified,
                "rejected": rejected,
            })),
        ),

        MemoryEventKind::MaintenanceLinked { links } => (
            "maintenance_linked",
            Some(serde_json::json!({ "links": links })),
        ),

        MemoryEventKind::MaintenanceConfidence { boosted, decayed } => (
            "maintenance_confidence",
            Some(serde_json::json!({
                "boosted": boosted,
                "decayed": decayed,
            })),
        ),

        MemoryEventKind::MaintenanceCluster { clusters, members } => (
            "maintenance_cluster",
            Some(serde_json::json!({
                "clusters": clusters,
                "members": members,
            })),
        ),

        MemoryEventKind::MaintenanceTagInferred { tag, applied } => (
            "maintenance_tag_inferred",
            Some(serde_json::json!({
                "tag": tag,
                "applied": applied,
            })),
        ),

        MemoryEventKind::MaintenanceGap { candidates } => (
            "maintenance_gap",
            Some(serde_json::json!({ "candidates": candidates })),
        ),

        MemoryEventKind::MaintenanceComplete { latency_ms } => (
            "maintenance_complete",
            Some(serde_json::json!({ "latency_ms": latency_ms })),
        ),

        MemoryEventKind::ExtractionStarted { reason } => (
            "extraction_started",
            Some(serde_json::json!({ "reason": reason })),
        ),

        MemoryEventKind::ExtractionComplete { count } => (
            "extraction_complete",
            Some(serde_json::json!({ "count": count })),
        ),

        MemoryEventKind::Error { message } => {
            ("error", Some(serde_json::json!({ "message": message })))
        }

        MemoryEventKind::ToolRemembered {
            content,
            scope,
            category,
        } => (
            "tool_remembered",
            Some(serde_json::json!({
                "content": content,
                "scope": scope,
                "category": category,
            })),
        ),

        MemoryEventKind::ToolRecalled { query, count } => (
            "tool_recalled",
            Some(serde_json::json!({
                "query": query,
                "count": count,
            })),
        ),

        MemoryEventKind::ToolForgot { id } => {
            ("tool_forgot", Some(serde_json::json!({ "id": id })))
        }

        MemoryEventKind::ToolTagged { id, tags } => (
            "tool_tagged",
            Some(serde_json::json!({
                "id": id,
                "tags": tags,
            })),
        ),

        MemoryEventKind::ToolLinked { from, to } => (
            "tool_linked",
            Some(serde_json::json!({
                "from": from,
                "to": to,
            })),
        ),

        MemoryEventKind::ToolListed { count } => {
            ("tool_listed", Some(serde_json::json!({ "count": count })))
        }
    };

    write_log(event, detail);
}

/// Log when a pending memory is prepared (before it's actually injected).
pub fn log_pending_prepared(session_id: &str, prompt: &str, count: usize, memory_ids: &[String]) {
    write_log(
        "pending_prepared",
        Some(serde_json::json!({
            "target_session": session_id,
            "count": count,
            "prompt_chars": prompt.chars().count(),
            "prompt_preview": &prompt[..prompt.len().min(500)],
            "memory_ids": memory_ids,
        })),
    );
}

/// Log when memories are marked as injected (dedup tracking).
pub fn log_marked_injected(session_id: &str, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    write_log(
        "marked_injected",
        Some(serde_json::json!({
            "target_session": session_id,
            "memory_ids": ids,
        })),
    );
}

/// Log when a pending memory is consumed (actually injected into context).
pub fn log_pending_consumed(session_id: &str, count: usize, age_ms: u64, prompt_chars: usize) {
    write_log(
        "pending_consumed",
        Some(serde_json::json!({
            "target_session": session_id,
            "count": count,
            "age_ms": age_ms,
            "prompt_chars": prompt_chars,
        })),
    );
}

/// Log when a pending memory is discarded (stale, duplicate, etc.)
pub fn log_pending_discarded(session_id: &str, reason: &str) {
    write_log(
        "pending_discarded",
        Some(serde_json::json!({
            "target_session": session_id,
            "reason": reason,
        })),
    );
}

/// Log topic change detection (which triggers extraction).
pub fn log_topic_change(session_id: &str, old_topic: &str, new_topic: &str) {
    write_log(
        "topic_change",
        Some(serde_json::json!({
            "target_session": session_id,
            "old_topic": old_topic,
            "new_topic": new_topic,
        })),
    );
}

/// Log final extraction trigger (session end).
pub fn log_final_extraction(session_id: &str, transcript_chars: usize) {
    write_log(
        "final_extraction_started",
        Some(serde_json::json!({
            "target_session": session_id,
            "transcript_chars": transcript_chars,
        })),
    );
}

/// Log embedding candidate filtering results.
pub fn log_candidate_filter(
    session_id: &str,
    total_candidates: usize,
    after_dedup: usize,
    context_preview: &str,
) {
    write_log(
        "candidate_filter",
        Some(serde_json::json!({
            "target_session": session_id,
            "total_candidates": total_candidates,
            "after_dedup": after_dedup,
            "context_preview": &context_preview[..context_preview.len().min(200)],
        })),
    );
}

/// Remove `memory-events-*.jsonl` files older than the documented 14-day
/// retention window. The general log rotation deliberately leaves these files
/// alone (they are analysis data, not debug logs), so this is the only place
/// that prunes them.
pub fn cleanup_old_memory_logs() {
    if let Some(dir) = log_dir() {
        cleanup_old_memory_logs_in(&dir, Local::now());
    }
}

/// Core of [`cleanup_old_memory_logs`], parameterized on the directory and
/// "now" for unit testing without touching the real log directory.
fn cleanup_old_memory_logs_in(dir: &std::path::Path, now: chrono::DateTime<Local>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = now - chrono::Duration::days(14);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with("memory-events-") && name.ends_with(".jsonl")) {
            continue;
        }
        if let Ok(metadata) = entry.metadata()
            && metadata.is_file()
            && let Ok(modified) = metadata.modified()
        {
            let modified: chrono::DateTime<Local> = modified.into();
            if modified < cutoff {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::time::{Duration, SystemTime};

    #[test]
    fn memory_log_cleanup_respects_14_day_window() {
        let dir = std::env::temp_dir().join(format!(
            "jcode-mem-cleanup-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create temp dir");

        let write = |name: &str, age_days: u64| {
            let path = dir.join(name);
            let mut f = File::create(&path).expect("create");
            f.write_all(b"{}\n").ok();
            if age_days > 0 {
                let mtime = SystemTime::now() - Duration::from_secs(age_days * 24 * 60 * 60);
                f.set_modified(mtime).expect("set mtime");
            }
            path
        };

        // 20 days old: past the 14-day window -> deleted.
        let old = write("memory-events-2000-01-01.jsonl", 20);
        // 10 days old: within the window -> kept.
        let recent = write("memory-events-2000-02-01.jsonl", 10);
        // Old, but not a memory-events file -> must be left alone here.
        let other = write("jcode-2000-01-01.log", 20);

        cleanup_old_memory_logs_in(&dir, Local::now());

        assert!(!old.exists(), "20-day-old memory log should be deleted");
        assert!(recent.exists(), "10-day-old memory log must survive");
        assert!(
            other.exists(),
            "non-memory files are out of scope for this cleanup"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
