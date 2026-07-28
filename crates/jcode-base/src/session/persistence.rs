use anyhow::Result;
use chrono::Utc;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Instant;

use super::journal::{PersistVectorMode, SessionJournalEntry, metadata_requires_snapshot};
use super::storage_paths::{file_len_or_zero, session_journal_path_from_snapshot, session_path};
use super::{MAX_SESSION_JOURNAL_BYTES, RemoteStartupSessionSnapshot, Session, SessionStartupStub};
use crate::storage;

/// Outcome of replaying one session journal file.
#[derive(Debug, Default)]
struct JournalReplayStats {
    entries: usize,
    skipped_lines: usize,
    salvaged_entries: usize,
}

impl JournalReplayStats {
    fn is_corrupt(&self) -> bool {
        self.skipped_lines > 0
    }
}

/// Attempt to recover complete entries from a journal line that failed the
/// strict one-entry-per-line parse.
///
/// If a writer died mid-append (torn line without a trailing newline), the
/// next successful append starts writing on the same line, producing
/// `<torn json><complete entry json>\n` or `<entry json><entry json>\n`.
/// Serialized entries always begin with `{"meta":` (struct field order), so
/// scan for candidate starts and stream-parse consecutive complete entries
/// from the first position that yields any.
fn salvage_glued_journal_entries(line: &str, mut apply: impl FnMut(SessionJournalEntry)) -> usize {
    const ENTRY_START: &str = "{\"meta\":";
    let mut salvaged = 0usize;
    let mut search_from = 0usize;
    while let Some(rel) = line
        .get(search_from..)
        .and_then(|rest| rest.find(ENTRY_START))
    {
        let candidate_start = search_from + rel;
        let mut stream = serde_json::Deserializer::from_str(&line[candidate_start..])
            .into_iter::<SessionJournalEntry>();
        let mut parsed = Vec::new();
        for item in &mut stream {
            match item {
                Ok(entry) => parsed.push(entry),
                Err(_) => break,
            }
        }
        if !parsed.is_empty() {
            salvaged += parsed.len();
            for entry in parsed {
                apply(entry);
            }
            break;
        }
        search_from = candidate_start + ENTRY_START.len();
    }
    salvaged
}

/// Replay every parseable entry from a session journal, tolerating corrupt
/// lines instead of truncating the transcript at the first bad byte.
///
/// Journals are append-only JSONL written by `append_json_line_fast`. A crash,
/// full disk, or (historically) interleaved multi-write appends can leave a
/// torn or glued line behind. The old replay loop stopped at the first parse
/// failure, silently dropping every later entry, which surfaced as "my last
/// prompt is missing" after resuming a long session. Skipping only the bad
/// line preserves the rest of the transcript; per-entry `meta` snapshots make
/// later entries self-sufficient for metadata, and appended messages after a
/// gap are far better than losing the whole tail.
fn replay_journal_lines(
    journal_path: &Path,
    mut apply: impl FnMut(SessionJournalEntry),
) -> Result<JournalReplayStats> {
    let mut stats = JournalReplayStats::default();
    if !journal_path.exists() {
        return Ok(stats);
    }

    let file = std::fs::File::open(journal_path)?;
    let reader = BufReader::new(file);
    for (line_idx, line) in reader.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionJournalEntry>(trimmed) {
            Ok(entry) => {
                stats.entries += 1;
                apply(entry);
            }
            Err(err) => {
                stats.skipped_lines += 1;
                let salvaged = salvage_glued_journal_entries(trimmed, &mut apply);
                stats.entries += salvaged;
                stats.salvaged_entries += salvaged;
                crate::logging::warn(&format!(
                    "Session journal parse failed at {} line {} ({}); salvaged {} glued entr{} and continuing replay",
                    journal_path.display(),
                    line_idx + 1,
                    err,
                    salvaged,
                    if salvaged == 1 { "y" } else { "ies" }
                ));
            }
        }
    }

    if stats.is_corrupt() {
        crate::logging::event_warn(
            "SESSION_PERSISTENCE",
            vec![
                ("phase", "journal_replay_corruption".to_string()),
                ("path", journal_path.display().to_string()),
                ("entries_replayed", stats.entries.to_string()),
                ("lines_skipped", stats.skipped_lines.to_string()),
                ("entries_salvaged", stats.salvaged_entries.to_string()),
            ],
        );
    }

    Ok(stats)
}

impl Session {
    fn apply_journal_entry(&mut self, entry: SessionJournalEntry) {
        self.apply_journal_meta(entry.meta);
        self.messages.extend(entry.append_messages);
        self.env_snapshots.extend(entry.append_env_snapshots);
        self.memory_injections
            .extend(entry.append_memory_injections);
        self.replay_events.extend(entry.append_replay_events);
        self.mark_memory_profile_dirty();
    }

    fn checkpoint_snapshot(&mut self, snapshot_path: &Path, journal_path: &Path) -> Result<()> {
        storage::write_json_fast(snapshot_path, self)?;
        if journal_path.exists() {
            let _ = std::fs::remove_file(journal_path);
        }
        self.reset_persist_state(true);
        Ok(())
    }

    /// After replaying a journal that contained unparseable lines, force the
    /// next `save()` to checkpoint a full snapshot (which deletes the corrupt
    /// journal) so the salvaged in-memory state becomes durable and the bad
    /// lines can never be replayed again. A best-effort copy of the corrupt
    /// journal is kept next to it for forensics.
    fn schedule_checkpoint_after_corrupt_journal(&mut self, journal_path: &Path) {
        self.mark_messages_full_dirty();
        let backup_path = journal_path.with_extension("corrupt.jsonl");
        if let Err(err) = std::fs::copy(journal_path, &backup_path) {
            crate::logging::warn(&format!(
                "Failed to back up corrupt session journal {} to {}: {}",
                journal_path.display(),
                backup_path.display(),
                err
            ));
        }
        crate::logging::warn(&format!(
            "Session {} journal {} contained corrupt lines; next save will checkpoint a full snapshot (backup at {})",
            self.id,
            journal_path.display(),
            backup_path.display()
        ));
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let load_start = Instant::now();
        let snapshot_bytes = file_len_or_zero(path);
        let snapshot_start = Instant::now();
        let mut session: Session = storage::read_json(path)?;
        let snapshot_ms = snapshot_start.elapsed().as_millis();
        let journal_path = session_journal_path_from_snapshot(path);
        let journal_bytes = file_len_or_zero(&journal_path);
        let journal_start = Instant::now();
        let replay_stats = replay_journal_lines(&journal_path, |entry| {
            session.apply_journal_entry(entry);
        })?;
        let journal_entries = replay_stats.entries;
        let journal_ms = journal_start.elapsed().as_millis();
        let finalize_start = Instant::now();
        session.reset_persist_state(path.exists());
        session.reset_provider_messages_cache();
        session.mark_memory_profile_dirty();
        if replay_stats.is_corrupt() {
            session.schedule_checkpoint_after_corrupt_journal(&journal_path);
        }
        let finalize_ms = finalize_start.elapsed().as_millis();
        // Bulk scans of a large sessions directory can drive tens of thousands
        // of loads through here. Logging each one floods the log and hides the
        // scan that caused it, so hand the sample to the burst detector and let
        // it decide between per-load detail and a single attributed summary.
        let load_elapsed = load_start.elapsed();
        if !super::load_telemetry::note_load(load_elapsed, snapshot_bytes + journal_bytes)
            .should_log()
        {
            return Ok(session);
        }
        crate::logging::info(&format!(
            "[TIMING] session_load: session={}, snapshot={}ms, journal={}ms, finalize={}ms, snapshot_bytes={}, journal_bytes={}, journal_entries={}, messages={}, env_snapshots={}, replay_events={}, total={}ms",
            session.id,
            snapshot_ms,
            journal_ms,
            finalize_ms,
            snapshot_bytes,
            journal_bytes,
            journal_entries,
            session.messages.len(),
            session.env_snapshots.len(),
            session.replay_events.len(),
            load_elapsed.as_millis(),
        ));
        crate::logging::event_info(
            "SESSION_PERSISTENCE",
            vec![
                ("phase", "load_done".to_string()),
                ("session_id", session.id.clone()),
                ("path", path.display().to_string()),
                ("status", format!("{:?}", session.status)),
                ("messages", session.messages.len().to_string()),
                ("env_snapshots", session.env_snapshots.len().to_string()),
                ("replay_events", session.replay_events.len().to_string()),
                ("snapshot_bytes", snapshot_bytes.to_string()),
                ("journal_bytes", journal_bytes.to_string()),
                ("journal_entries", journal_entries.to_string()),
                ("snapshot_ms", snapshot_ms.to_string()),
                ("journal_ms", journal_ms.to_string()),
                ("finalize_ms", finalize_ms.to_string()),
                ("elapsed_ms", load_elapsed.as_millis().to_string()),
            ],
        );
        Ok(session)
    }

    pub fn load(session_id: &str) -> Result<Self> {
        let path = session_path(session_id)?;
        Self::load_from_path(&path)
    }

    /// Load only the metadata needed for remote-client startup.
    ///
    /// This intentionally skips heavyweight transcript vectors so the remote
    /// client can paint quickly while the server performs the authoritative
    /// session restore + history bootstrap.
    pub fn load_startup_stub(session_id: &str) -> Result<Self> {
        let path = session_path(session_id)?;
        let reader = BufReader::new(std::fs::File::open(&path)?);
        let stub: SessionStartupStub = serde_json::from_reader(reader)?;
        Ok(Self::session_from_startup_stub(stub))
    }

    pub fn load_for_remote_startup(session_id: &str) -> Result<Self> {
        let path = session_path(session_id)?;
        let load_start = Instant::now();
        let snapshot_bytes = file_len_or_zero(&path);
        let snapshot_start = Instant::now();
        let reader = BufReader::new(std::fs::File::open(&path)?);
        let snapshot: RemoteStartupSessionSnapshot = serde_json::from_reader(reader)?;
        let snapshot_ms = snapshot_start.elapsed().as_millis();
        let mut session = Self::session_from_remote_startup_snapshot(snapshot);
        let journal_path = session_journal_path_from_snapshot(&path);
        let journal_bytes = file_len_or_zero(&journal_path);
        let journal_start = Instant::now();
        let mut journal_entries = 0usize;
        replay_journal_lines(&journal_path, |entry| {
            journal_entries += 1;
            session.apply_journal_meta(entry.meta);
            session.messages.extend(entry.append_messages);
            session.replay_events.extend(entry.append_replay_events);
        })?;
        let journal_ms = journal_start.elapsed().as_millis();
        let finalize_start = Instant::now();
        session.reset_persist_state(path.exists());
        session.reset_provider_messages_cache();
        session.mark_memory_profile_dirty();
        let finalize_ms = finalize_start.elapsed().as_millis();
        crate::logging::info(&format!(
            "[TIMING] remote_startup_load: session={}, snapshot={}ms, journal={}ms, finalize={}ms, snapshot_bytes={}, journal_bytes={}, journal_entries={}, messages={}, total={}ms",
            session.id,
            snapshot_ms,
            journal_ms,
            finalize_ms,
            snapshot_bytes,
            journal_bytes,
            journal_entries,
            session.messages.len(),
            load_start.elapsed().as_millis(),
        ));
        crate::logging::event_info(
            "SESSION_PERSISTENCE",
            vec![
                ("phase", "remote_startup_load_done".to_string()),
                ("session_id", session.id.clone()),
                ("path", path.display().to_string()),
                ("status", format!("{:?}", session.status)),
                ("messages", session.messages.len().to_string()),
                ("snapshot_bytes", snapshot_bytes.to_string()),
                ("journal_bytes", journal_bytes.to_string()),
                ("journal_entries", journal_entries.to_string()),
                ("snapshot_ms", snapshot_ms.to_string()),
                ("journal_ms", journal_ms.to_string()),
                ("finalize_ms", finalize_ms.to_string()),
                ("elapsed_ms", load_start.elapsed().as_millis().to_string()),
            ],
        );
        Ok(session)
    }

    pub fn save(&mut self) -> Result<()> {
        self.updated_at = Utc::now();
        let path = session_path(&self.id)?;
        let journal_path = session_journal_path_from_snapshot(&path);
        let start = std::time::Instant::now();
        let snapshot_bytes_before = file_len_or_zero(&path);
        let journal_bytes_before = file_len_or_zero(&journal_path);
        let current_meta = self.journal_meta();
        let metadata_needs_snapshot = self
            .persist_state
            .last_meta
            .as_ref()
            .is_some_and(|prev| metadata_requires_snapshot(prev, &current_meta));
        let vectors_need_snapshot = !self.persist_state.snapshot_exists
            || self.persist_state.messages_mode == PersistVectorMode::Full
            || self.persist_state.env_snapshots_mode == PersistVectorMode::Full
            || self.persist_state.memory_injections_mode == PersistVectorMode::Full
            || self.persist_state.replay_events_mode == PersistVectorMode::Full
            || self.messages.len() < self.persist_state.messages_len
            || self.env_snapshots.len() < self.persist_state.env_snapshots_len
            || self.memory_injections.len() < self.persist_state.memory_injections_len
            || self.replay_events.len() < self.persist_state.replay_events_len;

        let delta_messages = self
            .messages
            .len()
            .saturating_sub(self.persist_state.messages_len);
        let delta_env_snapshots = self
            .env_snapshots
            .len()
            .saturating_sub(self.persist_state.env_snapshots_len);
        let delta_memory_injections = self
            .memory_injections
            .len()
            .saturating_sub(self.persist_state.memory_injections_len);
        let delta_replay_events = self
            .replay_events
            .len()
            .saturating_sub(self.persist_state.replay_events_len);
        let (
            result,
            save_mode,
            entry_build_ms,
            append_ms,
            journal_stat_ms,
            checkpoint_ms,
            journal_bytes_after,
        ) = if metadata_needs_snapshot || vectors_need_snapshot {
            let checkpoint_start = Instant::now();
            let result = self.checkpoint_snapshot(&path, &journal_path);
            let checkpoint_ms = checkpoint_start.elapsed().as_millis();
            let journal_bytes_after = file_len_or_zero(&journal_path);
            (
                result,
                "snapshot",
                0,
                0,
                0,
                checkpoint_ms,
                journal_bytes_after,
            )
        } else {
            let entry_build_start = Instant::now();
            let entry = SessionJournalEntry {
                meta: current_meta.clone(),
                append_messages: self.messages[self.persist_state.messages_len..].to_vec(),
                append_env_snapshots: self.env_snapshots[self.persist_state.env_snapshots_len..]
                    .to_vec(),
                append_memory_injections: self.memory_injections
                    [self.persist_state.memory_injections_len..]
                    .to_vec(),
                append_replay_events: self.replay_events[self.persist_state.replay_events_len..]
                    .to_vec(),
            };
            let entry_build_ms = entry_build_start.elapsed().as_millis();
            let append_start = Instant::now();
            let append_result = storage::append_json_line_fast(&journal_path, &entry);
            let append_ms = append_start.elapsed().as_millis();
            match append_result {
                Ok(()) => {
                    self.reset_persist_state(true);
                    let journal_stat_start = Instant::now();
                    let journal_bytes_after = file_len_or_zero(&journal_path);
                    let journal_stat_ms = journal_stat_start.elapsed().as_millis();
                    if journal_bytes_after > MAX_SESSION_JOURNAL_BYTES {
                        let checkpoint_start = Instant::now();
                        let result = self.checkpoint_snapshot(&path, &journal_path);
                        let checkpoint_ms = checkpoint_start.elapsed().as_millis();
                        let journal_bytes_after = file_len_or_zero(&journal_path);
                        (
                            result,
                            "append+checkpoint",
                            entry_build_ms,
                            append_ms,
                            journal_stat_ms,
                            checkpoint_ms,
                            journal_bytes_after,
                        )
                    } else {
                        (
                            Ok(()),
                            "append",
                            entry_build_ms,
                            append_ms,
                            journal_stat_ms,
                            0,
                            journal_bytes_after,
                        )
                    }
                }
                Err(err) => {
                    crate::logging::warn(&format!(
                        "Session journal append failed for {} ({}); checkpointing full snapshot",
                        self.id, err
                    ));
                    let checkpoint_start = Instant::now();
                    let result = self.checkpoint_snapshot(&path, &journal_path);
                    let checkpoint_ms = checkpoint_start.elapsed().as_millis();
                    let journal_bytes_after = file_len_or_zero(&journal_path);
                    (
                        result,
                        "append_failed_fallback_snapshot",
                        entry_build_ms,
                        append_ms,
                        0,
                        checkpoint_ms,
                        journal_bytes_after,
                    )
                }
            }
        };
        let elapsed = start.elapsed();
        let snapshot_bytes_after = file_len_or_zero(&path);
        let result_ok = result.is_ok();
        if elapsed.as_millis() > 50 {
            crate::logging::info(&format!(
                "Session save slow: total={:.0}ms mode={} metadata_snapshot={} vectors_snapshot={} entry_build={}ms append={}ms journal_stat={}ms checkpoint={}ms messages={} delta_messages={} delta_env_snapshots={} delta_memory_injections={} delta_replay_events={} snapshot_bytes_before={} journal_bytes_before={} journal_bytes_after={}",
                elapsed.as_secs_f64() * 1000.0,
                save_mode,
                metadata_needs_snapshot,
                vectors_need_snapshot,
                entry_build_ms,
                append_ms,
                journal_stat_ms,
                checkpoint_ms,
                self.messages.len(),
                delta_messages,
                delta_env_snapshots,
                delta_memory_injections,
                delta_replay_events,
                snapshot_bytes_before,
                journal_bytes_before,
                journal_bytes_after,
            ));
        }
        let mut fields = vec![
            ("phase", "save_done".to_string()),
            ("session_id", self.id.clone()),
            ("path", path.display().to_string()),
            ("status", format!("{:?}", self.status)),
            ("result", if result_ok { "ok" } else { "error" }.to_string()),
            ("save_mode", save_mode.to_string()),
            ("metadata_snapshot", metadata_needs_snapshot.to_string()),
            ("vectors_snapshot", vectors_need_snapshot.to_string()),
            ("messages", self.messages.len().to_string()),
            ("delta_messages", delta_messages.to_string()),
            ("delta_env_snapshots", delta_env_snapshots.to_string()),
            (
                "delta_memory_injections",
                delta_memory_injections.to_string(),
            ),
            ("delta_replay_events", delta_replay_events.to_string()),
            ("snapshot_bytes_before", snapshot_bytes_before.to_string()),
            ("snapshot_bytes_after", snapshot_bytes_after.to_string()),
            ("journal_bytes_before", journal_bytes_before.to_string()),
            ("journal_bytes_after", journal_bytes_after.to_string()),
            ("entry_build_ms", entry_build_ms.to_string()),
            ("append_ms", append_ms.to_string()),
            ("journal_stat_ms", journal_stat_ms.to_string()),
            ("checkpoint_ms", checkpoint_ms.to_string()),
            ("elapsed_ms", elapsed.as_millis().to_string()),
        ];
        if let Err(error) = &result {
            fields.push(("error", crate::util::format_error_chain(error)));
            crate::logging::event_warn("SESSION_PERSISTENCE", fields);
        } else {
            crate::logging::event_info("SESSION_PERSISTENCE", fields);
        }
        result
    }
}
