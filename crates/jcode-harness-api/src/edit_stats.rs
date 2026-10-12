//! Per-session edit accounting. Never consults the shared worktree or git.
use crate::SessionInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

/// Cumulative changed lines, not the net diff. A replacement adds and removes
/// a line. Covers built-in write/edit/multiedit/patch/apply_patch only, not shell,
/// MCP or external changes. Repeated edits count again. Forks do not inherit
/// exact counters. Legacy transcript estimates may be incomplete or truncated.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionEditStats {
    pub added: u64,
    pub removed: u64,
    #[serde(default)]
    pub approximate: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("JCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".jcode"))
        })?;
    Some(home.join("sessions"))
}

/// Persist a successful mutation immediately, before tool output truncation or
/// transcript compaction. Caller supplies counts from complete before/after
/// content. This does not make file mutation and accounting crash-atomic.
/// The small counter is locked across threads/processes and atomically replaced.
#[doc(hidden)]
pub fn record_session_edit(dir: &Path, id: &str, delta: SessionEditStats) -> std::io::Result<()> {
    if !valid_id(id) {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let counter_dir = dir.join("edit-stats");
    std::fs::create_dir_all(&counter_dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(counter_dir.join(format!("{id}.lock")))?;
    lock.lock()?;
    let path = counter_dir.join(format!("{id}.json"));
    let mut stats = if path.exists() {
        serde_json::from_reader(std::fs::File::open(&path)?)?
    } else if !dir.join(format!("{id}.json")).exists()
        && !dir.join(format!("{id}.journal.jsonl")).exists()
    {
        SessionEditStats::default()
    } else {
        legacy_stats(dir, id).unwrap_or(SessionEditStats {
            approximate: true,
            ..Default::default()
        })
    };
    stats.added = stats.added.saturating_add(delta.added);
    stats.removed = stats.removed.saturating_add(delta.removed);
    stats.approximate |= delta.approximate;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec(&stats)?)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

const MAX_LEGACY_BYTES: u64 = 32 * 1024 * 1024;
fn read_bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

fn legacy_stats(dir: &Path, id: &str) -> Option<SessionEditStats> {
    let snapshot = dir.join(format!("{id}.json"));
    let journal = dir.join(format!("{id}.journal.jsonl"));
    if !snapshot.exists() && !journal.exists() {
        return None;
    }
    let bytes = read_bounded(&snapshot, MAX_LEGACY_BYTES)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let mut messages = value.get("messages")?.as_array()?.clone();
    // Fork snapshots contain their parent's transcript, which cannot be
    // attributed to this session. Unknown is better than a misleading total.
    if value.get("parent_id").is_some_and(|v| !v.is_null()) {
        return None;
    }
    if journal.exists() {
        let journal_bytes = read_bounded(
            &journal,
            MAX_LEGACY_BYTES.saturating_sub(bytes.len() as u64),
        )?;
        for line in journal_bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
        {
            let entry: Value = serde_json::from_slice(line).ok()?;
            if let Some(appended) = entry.get("append_messages").and_then(Value::as_array) {
                messages.extend(appended.iter().cloned());
            }
        }
    }
    let mut stats = count_legacy_messages(&messages);
    stats.approximate |= value.get("compaction").is_some_and(|v| !v.is_null());
    Some(stats)
}

fn count_legacy_messages(messages: &[Value]) -> SessionEditStats {
    let mut calls = HashMap::new();
    let mut counted = HashSet::new();
    let mut stats = SessionEditStats::default();
    for message in messages {
        for block in message
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match block["type"].as_str() {
                Some("tool_use") => {
                    let name = block["name"]
                        .as_str()
                        .unwrap_or("")
                        .trim_start_matches("functions.");
                    if matches!(
                        name,
                        "write"
                            | "Write"
                            | "file_write"
                            | "write_file"
                            | "edit"
                            | "Edit"
                            | "file_edit"
                            | "edit_file"
                            | "multiedit"
                            | "patch"
                            | "apply_patch"
                            | "batch"
                    ) && let Some(id) = block["id"].as_str()
                    {
                        calls.insert(id, (name, &block["input"]));
                    }
                }
                Some("tool_result") if block["is_error"] != true => {
                    let Some(id) = block["tool_use_id"].as_str() else {
                        continue;
                    };
                    let Some((name, input)) = calls.get(id) else {
                        continue;
                    };
                    if !counted.insert(id) {
                        continue;
                    }
                    let text = block["content"].as_str().unwrap_or("");
                    if *name == "batch" {
                        if let Some(subcalls) = input["tool_calls"].as_array() {
                            for (index, call) in subcalls.iter().enumerate() {
                                let tool = call["tool"]
                                    .as_str()
                                    .unwrap_or("")
                                    .trim_start_matches("functions.");
                                // Nested batches are unsupported by the tool itself.
                                if tool == "batch" {
                                    continue;
                                }
                                let header = format!("--- [{}] {} ---\n", index + 1, tool);
                                let Some(start) = text.find(&header).map(|at| at + header.len())
                                else {
                                    continue;
                                };
                                let body = &text[start..];
                                let body = body.split("\n--- [").next().unwrap_or(body);
                                let rows = vec![serde_json::json!({"content":[
                                    {"type":"tool_use","id":"sub","name":tool,"input":{}},
                                    {"type":"tool_result","tool_use_id":"sub","content":body,"is_error":body.starts_with("Error:")}
                                ]})];
                                let part = count_legacy_messages(&rows);
                                stats.added = stats.added.saturating_add(part.added);
                                stats.removed = stats.removed.saturating_add(part.removed);
                                stats.approximate |= part.approximate;
                            }
                        }
                        continue;
                    }
                    // Only count known success output, never proposed patches,
                    // failed tool responses, context windows or error excerpts.
                    let success = match *name {
                        "write" | "Write" | "file_write" | "write_file" => {
                            text.starts_with("Created ") || text.starts_with("Updated ")
                        }
                        "edit" | "Edit" | "file_edit" | "edit_file" => text.starts_with("Edited "),
                        "multiedit" => text.starts_with("Edited "),
                        _ => text.lines().any(|line| line.starts_with("✓ ")),
                    };
                    if !success {
                        continue;
                    }
                    stats.approximate = true;
                    let mut section_ok = !matches!(*name, "patch" | "apply_patch");
                    for line in text.lines() {
                        if line.starts_with("✓ ") {
                            section_ok = true;
                            continue;
                        }
                        if line.starts_with("✗ ") {
                            section_ok = false;
                            continue;
                        }
                        if !section_ok {
                            continue;
                        }
                        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
                        if digits == 0 {
                            continue;
                        }
                        match line.as_bytes().get(digits..) {
                            Some([b'+', b' ', ..]) => stats.added += 1,
                            Some([b'-', b' ', ..]) => stats.removed += 1,
                            _ => (),
                        }
                    }
                }
                _ => (),
            }
        }
    }
    stats
}

type Stamp = Vec<Option<(u64, SystemTime)>>;
fn stamp(dir: &Path, id: &str) -> Stamp {
    ["json", "journal.jsonl"]
        .iter()
        .map(|ext| {
            let meta = std::fs::metadata(dir.join(format!("{id}.{ext}"))).ok()?;
            Some((meta.len(), meta.modified().ok()?))
        })
        .collect()
}
#[derive(Default)]
struct Cache {
    entries: HashMap<PathBuf, (Stamp, Option<SessionEditStats>)>,
    pending: HashSet<PathBuf>,
    queue: VecDeque<(PathBuf, String, PathBuf, Stamp)>,
    running: bool,
}
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

/// Opt-in enrichment for a LOCAL daemon, including older running daemons.
/// Never call this for remote sessions. Uses JCODE_HOME or ~/.jcode. Exact
/// sidecars are cheap and read immediately. Legacy estimates load in background
/// (one background worker, 32 MiB per session, 512 queued/cached entries), so
/// callers should invoke this on periodic refresh. Missing, malformed, oversized
/// and forked legacy records remain unknown. Cache invalidates on size/mtime.
/// Existing API values win. No worktree, git, or transcript writes are performed.
pub fn enrich_sessions_from_local_edit_stats(sessions: &mut [SessionInfo]) {
    if let Some(dir) = sessions_dir() {
        enrich_sessions_from_edit_stats(sessions, &dir);
    }
}

/// Explicit session-directory variant of the local enrichment helper.
pub fn enrich_sessions_from_edit_stats(sessions: &mut [SessionInfo], dir: &Path) {
    for session in sessions {
        if session.edit_stats.is_some() || !valid_id(&session.session_id) {
            continue;
        }
        let id = &session.session_id;
        let sidecar = dir.join("edit-stats").join(format!("{id}.json"));
        if sidecar.exists() {
            session.edit_stats =
                read_bounded(&sidecar, 4096).and_then(|v| serde_json::from_slice(&v).ok());
            continue;
        }
        let key = dir.join(format!("{id}.json"));
        let current = stamp(dir, id);
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((previous, stats)) = cache.entries.get(&key) {
            session.edit_stats = *stats;
            if previous == &current {
                continue;
            }
        }
        if cache.pending.len() >= 512 || !cache.pending.insert(key.clone()) {
            continue;
        }
        cache
            .queue
            .push_back((dir.to_owned(), id.clone(), key, current));
        if !cache.running {
            cache.running = true;
            std::thread::spawn(scan_queue);
        }
    }
}

fn scan_queue() {
    loop {
        let work = {
            let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
            match cache.queue.pop_front() {
                Some(work) => work,
                None => {
                    cache.running = false;
                    return;
                }
            }
        };
        let (dir, id, key, current) = work;
        let stats = legacy_stats(&dir, &id).map(|mut stats| {
            stats.approximate = true;
            stats
        });
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        cache.pending.remove(&key);
        if cache.entries.len() >= 512
            && let Some(key) = cache.entries.keys().next().cloned()
        {
            cache.entries.remove(&key);
        }
        cache.entries.insert(key, (current, stats));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "jcode-edit-stats-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, name: &str, value: Value) {
            std::fs::write(self.0.join(name), value.to_string()).unwrap();
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn pair(id: &str, tool: &str, output: &str, error: bool) -> Vec<Value> {
        vec![
            json!({"content":[{"type":"tool_use","id":id,"name":tool,"input":{"patch_text":"+proposed"}}]}),
            json!({"content":[{"type":"tool_result","tool_use_id":id,"content":output,"is_error":error}]}),
        ]
    }
    fn session(id: &str) -> SessionInfo {
        serde_json::from_value(json!({"session_id":id,"status":"idle"})).unwrap()
    }
    #[test]
    fn schema_is_additive_and_roundtrips() {
        let mut session = session("a");
        assert!(session.edit_stats.is_none());
        assert!(
            serde_json::to_value(&session)
                .unwrap()
                .get("edit_stats")
                .is_none()
        );
        session.edit_stats = Some(SessionEditStats {
            added: 7,
            removed: 2,
            approximate: true,
        });
        let decoded: SessionInfo =
            serde_json::from_value(serde_json::to_value(&session).unwrap()).unwrap();
        assert_eq!(decoded.edit_stats, session.edit_stats);
    }
    #[test]
    fn legacy_counts_success_only_deduplicates_and_marks_estimates() {
        let mut messages = pair(
            "a",
            "functions.edit",
            "Edited f: replaced 20 occurrence(s)\n1- old\n1+ new\n... (diff truncated)",
            false,
        );
        messages.extend(pair("b", "write", "Created f\n1+ failed", true));
        messages.extend(pair("c", "bash", "Edited f\n1+ shell output", false));
        messages.extend(pair(
            "d",
            "apply_patch",
            "✓ a: modified\n1- a\n1+ b\n✗ b: failed\n1+ error excerpt",
            false,
        ));
        messages.extend(pair(
            "e",
            "multiedit",
            "Edited f\nApplied:\n  ✓ one\nDiff:\n1+ ok",
            false,
        ));
        messages.push(messages[1].clone());
        messages.push(json!({"content":[{"type":"tool_use","name":"apply_patch","id":"proposed","input":{"patch_text":"1+ proposed"}}]}));
        assert_eq!(
            count_legacy_messages(&messages),
            SessionEditStats {
                added: 3,
                removed: 2,
                approximate: true
            }
        );
    }
    #[test]
    fn legacy_batch_matches_only_completed_subcalls() {
        let messages = vec![serde_json::json!({"content":[
            {"type":"tool_use","id":"batch1","name":"batch","input":{"tool_calls":[{"tool":"edit"},{"tool":"write"},{"tool":"bash"}]}},
            {"type":"tool_result","tool_use_id":"batch1","is_error":false,"content":"--- [1] edit ---\nEdited a\n1- old\n1+ new\n\n--- [2] write ---\nError: denied\n1+ never\n\n--- [3] bash ---\nCreated f\n1+ not a file tool\n\nCompleted: 2 succeeded, 1 failed"}
        ]})];
        assert_eq!(
            count_legacy_messages(&messages),
            SessionEditStats {
                added: 1,
                removed: 1,
                approximate: true
            }
        );
    }

    #[test]
    fn snapshot_and_journal_are_combined_without_duplicate_results() {
        let temp = Temp::new();
        let messages = pair("a", "write", "Created f\n1+ a", false);
        temp.write("a.json", json!({"messages":[messages[0]],"parent_id":null}));
        temp.write(
            "a.journal.jsonl",
            json!({"append_messages":[messages[1],messages[1]]}),
        );
        assert_eq!(legacy_stats(&temp.0, "a").unwrap().added, 1);
        temp.write("fork.json", json!({"parent_id":"a","messages":messages}));
        assert!(legacy_stats(&temp.0, "fork").is_none());
        std::fs::write(temp.0.join("a.journal.jsonl"), b"{incomplete").unwrap();
        assert!(legacy_stats(&temp.0, "a").is_none());
    }
    #[test]
    fn durable_counters_isolate_sessions_and_survive_transcript_changes() {
        let temp = Temp::new();
        temp.write("a.json", json!({"messages":[]}));
        let delta = SessionEditStats {
            added: 100,
            removed: 20,
            approximate: false,
        };
        record_session_edit(&temp.0, "a", delta).unwrap();
        record_session_edit(&temp.0, "a", delta).unwrap();
        record_session_edit(&temp.0, "b", SessionEditStats::default()).unwrap();
        temp.write(
            "a.json",
            json!({"messages":[],"compaction":{"summary":"compacted"}}),
        );
        let mut sessions = [session("a"), session("b")];
        enrich_sessions_from_edit_stats(&mut sessions, &temp.0);
        assert_eq!(
            sessions[0].edit_stats,
            Some(SessionEditStats {
                added: 200,
                removed: 40,
                approximate: false
            })
        );
        assert_eq!(sessions[1].edit_stats, Some(SessionEditStats::default()));
        assert!(!temp.0.join("a.edits.json").exists());
        assert!(record_session_edit(&temp.0, "../bad", delta).is_err());
    }
    #[test]
    fn migration_baseline_is_counted_once_and_remains_approximate() {
        let temp = Temp::new();
        temp.write(
            "a.json",
            json!({"messages":pair("old", "write", "Created f\n1+ a", false)}),
        );
        let delta = SessionEditStats {
            added: 10,
            ..Default::default()
        };
        record_session_edit(&temp.0, "a", delta).unwrap();
        record_session_edit(&temp.0, "a", delta).unwrap();
        let mut sessions = [session("a")];
        enrich_sessions_from_edit_stats(&mut sessions, &temp.0);
        assert_eq!(
            sessions[0].edit_stats,
            Some(SessionEditStats {
                added: 21,
                removed: 0,
                approximate: true
            })
        );
    }
    #[test]
    fn concurrent_recorders_do_not_lose_updates() {
        let temp = Temp::new();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..10 {
                        record_session_edit(
                            &temp.0,
                            "a",
                            SessionEditStats {
                                added: 1,
                                ..Default::default()
                            },
                        )
                        .unwrap();
                    }
                });
            }
        });
        let mut sessions = [session("a")];
        enrich_sessions_from_edit_stats(&mut sessions, &temp.0);
        assert_eq!(sessions[0].edit_stats.unwrap().added, 80);
    }
    #[test]
    fn background_cache_invalidates_and_api_values_win() {
        let temp = Temp::new();
        temp.write(
            "a.json",
            json!({"messages":pair("a", "write", "Created f\n1+ a", false)}),
        );
        fn wait(temp: &Temp, added: u64) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let mut sessions = [session("a")];
                enrich_sessions_from_edit_stats(&mut sessions, &temp.0);
                if sessions[0]
                    .edit_stats
                    .is_some_and(|stats| stats.added == added)
                {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "cache did not refresh"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        wait(&temp, 1);
        temp.write(
            "a.journal.jsonl",
            json!({"append_messages":pair("b", "write", "Created g\n1+ b\n2+ c", false)}),
        );
        wait(&temp, 3);
        let mut sessions = [session("a")];
        sessions[0].edit_stats = Some(SessionEditStats::default());
        enrich_sessions_from_edit_stats(&mut sessions, &temp.0);
        assert_eq!(sessions[0].edit_stats, Some(SessionEditStats::default()));
    }
    #[test]
    fn malformed_and_oversized_records_stay_unknown() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("bad.json"), b"bad json").unwrap();
        assert!(legacy_stats(&temp.0, "bad").is_none());
        let file = std::fs::File::create(temp.0.join("large.json")).unwrap();
        file.set_len(MAX_LEGACY_BYTES + 1).unwrap();
        assert!(legacy_stats(&temp.0, "large").is_none());
    }
}
