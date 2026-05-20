use anyhow::Result;
use chrono::Utc;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Instant;

use super::journal::{PersistVectorMode, SessionJournalEntry, metadata_requires_snapshot};
use super::storage_paths::{file_len_or_zero, session_journal_path_from_snapshot, session_path};
use super::{MAX_SESSION_JOURNAL_BYTES, RemoteStartupSessionSnapshot, Session, SessionStartupStub};
use crate::storage;

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

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let load_start = Instant::now();
        let snapshot_bytes = file_len_or_zero(path);
        let snapshot_start = Instant::now();
        let mut session: Session = storage::read_json(path)?;
        let snapshot_ms = snapshot_start.elapsed().as_millis();
        let journal_path = session_journal_path_from_snapshot(path);
        let journal_bytes = file_len_or_zero(&journal_path);
        let journal_start = Instant::now();
        let mut journal_entries = 0usize;
        if journal_path.exists() {
            let file = std::fs::File::open(&journal_path)?;
            let reader = BufReader::new(file);
            for (line_idx, line) in reader.lines().enumerate() {
                let line = line?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<SessionJournalEntry>(trimmed) {
                    Ok(entry) => {
                        journal_entries += 1;
                        session.apply_journal_entry(entry)
                    }
                    Err(err) => {
                        crate::logging::warn(&format!(
                            "Session journal parse failed at {} line {}: {}",
                            journal_path.display(),
                            line_idx + 1,
                            err
                        ));
                        break;
                    }
                }
            }
        }
        let journal_ms = journal_start.elapsed().as_millis();
        let finalize_start = Instant::now();
        session.reset_persist_state(path.exists());
        session.reset_provider_messages_cache();
        session.mark_memory_profile_dirty();
        let finalize_ms = finalize_start.elapsed().as_millis();
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
            load_start.elapsed().as_millis(),
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
                ("elapsed_ms", load_start.elapsed().as_millis().to_string()),
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
        if journal_path.exists() {
            let file = std::fs::File::open(&journal_path)?;
            let reader = BufReader::new(file);
            for (line_idx, line) in reader.lines().enumerate() {
                let line = line?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<SessionJournalEntry>(trimmed) {
                    Ok(entry) => {
                        journal_entries += 1;
                        session.apply_journal_meta(entry.meta);
                        session.messages.extend(entry.append_messages);
                        session.replay_events.extend(entry.append_replay_events);
                    }
                    Err(err) => {
                        crate::logging::warn(&format!(
                            "Remote startup journal parse failed at {} line {}: {}",
                            journal_path.display(),
                            line_idx + 1,
                            err
                        ));
                        break;
                    }
                }
            }
        }
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
