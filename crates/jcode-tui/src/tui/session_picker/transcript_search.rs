//! Cancellable full-transcript search. Never reads transcript files on the UI thread.
use super::loading::search_query_tokens;
use super::{ResumeTarget, SessionInfo};
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(150);

struct Candidate {
    id: String,
    paths: Vec<PathBuf>,
    missing_tokens: Vec<String>,
}

#[derive(Default)]
pub(super) struct TranscriptSearch {
    query: String,
    candidates: Vec<Candidate>,
    started_at: Option<Instant>,
    receiver: Option<mpsc::Receiver<String>>,
    cancel: Arc<AtomicBool>,
    matches: HashSet<String>,
}

impl Drop for TranscriptSearch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl TranscriptSearch {
    pub(super) fn reset<'a>(
        &mut self,
        query: &str,
        sessions: impl Iterator<Item = &'a SessionInfo>,
    ) {
        if self.query == query {
            return;
        }
        *self = Self::default();
        self.query = query.to_string();
        if query.is_empty() {
            return;
        }
        let tokens = search_query_tokens(query);
        for session in sessions {
            let missing_tokens: Vec<_> = tokens
                .iter()
                .filter(|token| !session.search_index.contains(token.as_str()))
                .cloned()
                .collect();
            if missing_tokens.is_empty() {
                continue;
            }
            let paths = transcript_paths(session);
            if !paths.is_empty() {
                self.candidates.push(Candidate {
                    id: session.id.clone(),
                    paths,
                    missing_tokens,
                });
            }
        }
        if !self.candidates.is_empty() {
            self.started_at = Some(Instant::now());
        }
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.matches.contains(id)
    }

    pub(super) fn pending(&self) -> bool {
        self.started_at.is_some() || self.receiver.is_some()
    }

    /// Start only after typing pauses, then merge a bounded batch per frame.
    pub(super) fn poll(&mut self) -> bool {
        if self
            .started_at
            .is_some_and(|start| start.elapsed() >= DEBOUNCE)
        {
            self.started_at = None;
            let candidates = std::mem::take(&mut self.candidates);
            let cancel = self.cancel.clone();
            let (sender, receiver) = mpsc::channel();
            match std::thread::Builder::new()
                .name("jcode-transcript-search".into())
                .spawn(move || {
                    for candidate in candidates {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        if paths_match(&candidate.paths, &candidate.missing_tokens, &cancel)
                            && sender.send(candidate.id).is_err()
                        {
                            break;
                        }
                    }
                }) {
                Ok(_) => self.receiver = Some(receiver),
                Err(error) => {
                    crate::logging::debug(&format!("Cannot start transcript search: {error}"))
                }
            }
        }
        let mut changed = false;
        if let Some(receiver) = &self.receiver {
            for _ in 0..64 {
                match receiver.try_recv() {
                    Ok(id) => changed |= self.matches.insert(id),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.receiver = None;
                        break;
                    }
                }
            }
        }
        changed
    }
}

fn transcript_paths(session: &SessionInfo) -> Vec<PathBuf> {
    match &session.resume_target {
        ResumeTarget::JcodeSession { session_id } => {
            // IDs must remain filename stems, not arbitrary paths.
            if session_id.contains(['/', '\\', '\0']) {
                return Vec::new();
            }
            let home = match crate::storage::jcode_dir() {
                Ok(home) => home,
                Err(error) => {
                    crate::logging::debug(&format!(
                        "Cannot locate transcripts for session {session_id}: {error}"
                    ));
                    return Vec::new();
                }
            };
            let dir = home.join("sessions");
            vec![
                dir.join(format!("{session_id}.json")),
                dir.join(format!("{session_id}.journal.jsonl")),
            ]
        }
        ResumeTarget::ClaudeCodeSession { session_path, .. }
        | ResumeTarget::CodexSession { session_path, .. }
        | ResumeTarget::PiSession { session_path }
        | ResumeTarget::OpenCodeSession { session_path, .. }
        | ResumeTarget::CursorSession { session_path, .. } => vec![PathBuf::from(session_path)],
    }
}

struct Scan<'a> {
    tokens: &'a [String],
    found: Vec<bool>,
    cancel: &'a AtomicBool,
}

impl Scan<'_> {
    fn complete(&self) -> bool {
        self.found.iter().all(|found| *found)
    }

    fn text<E: serde::de::Error>(&mut self, text: &str) -> Result<(), E> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(E::custom("search cancelled"));
        }
        let lower = text.to_lowercase();
        for (token, found) in self.tokens.iter().zip(&mut self.found) {
            *found |= lower.contains(token);
        }
        if self.complete() {
            return Err(E::custom("search complete"));
        }
        Ok(())
    }
}

// Decode one JSON string at a time, including escaped Unicode and newlines.
// Do not materialize a many-MB session or mistake JSON field names for prose.
impl<'de> DeserializeSeed<'de> for &mut Scan<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(serde::de::Error::custom("search cancelled"));
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for &mut Scan<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("transcript JSON")
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        self.text(value)
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<(), E> {
        self.text(&value)
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(&mut *self)?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key::<IgnoredAny>()?.is_some() {
            map.next_value_seed(&mut *self)?;
        }
        Ok(())
    }
}

struct CancellableReader<'a> {
    reader: std::fs::File,
    cancel: &'a AtomicBool,
}
impl Read for CancellableReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("search cancelled"));
        }
        self.reader.read(buf)
    }
}

fn paths_match(paths: &[PathBuf], tokens: &[String], cancel: &AtomicBool) -> bool {
    let mut scan = Scan {
        tokens,
        found: vec![false; tokens.len()],
        cancel,
    };
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        scan_file(path, &mut scan);
        if scan.complete() {
            return true;
        }
    }
    false
}

fn scan_file(path: &Path, scan: &mut Scan<'_>) {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // Snapshot and journal paths are alternatives and may not both exist.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            crate::logging::debug(&format!(
                "Cannot open transcript {}: {error}",
                path.display()
            ));
            return;
        }
    };
    let mut reader = BufReader::with_capacity(
        64 * 1024,
        CancellableReader {
            reader: file,
            cancel: scan.cancel,
        },
    );
    if path.extension().is_some_and(|ext| ext == "jsonl") {
        // Parse records independently: a partial or corrupt record must not
        // hide later valid messages. from_slice is also much faster here than
        // serde's byte-at-a-time reader adapter.
        let mut line = Vec::new();
        let mut record = 0;
        while !scan.cancel.load(Ordering::Relaxed) {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Err(error) => {
                    if !scan.cancel.load(Ordering::Relaxed) {
                        crate::logging::debug(&format!(
                            "Cannot read transcript {}: {error}",
                            path.display()
                        ));
                    }
                    break;
                }
                Ok(_) => {
                    record += 1;
                    if let Err(error) =
                        (&mut *scan).deserialize(&mut serde_json::Deserializer::from_slice(&line))
                    {
                        log_scan_error(path, Some(record), scan, &error);
                    }
                    if scan.complete() {
                        break;
                    }
                }
            }
        }
        return;
    }
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    // The stream can be one snapshot or multiple JSONL records.
    loop {
        match (&mut *scan).deserialize(&mut deserializer) {
            Ok(()) => {}
            Err(error) => {
                // EOF is normal between values or in a live, partially written record.
                if !error.is_eof() {
                    log_scan_error(path, None, scan, &error);
                }
                break;
            }
        }
    }
}

fn log_scan_error(path: &Path, record: Option<usize>, scan: &Scan<'_>, error: &serde_json::Error) {
    // The visitor uses errors for successful early exit and cancellation.
    if scan.complete() || scan.cancel.load(Ordering::Relaxed) {
        return;
    }
    crate::logging::debug(&format!(
        "Cannot decode transcript {} (record {record:?}): {error}",
        path.display()
    ));
}

#[cfg(test)]
#[path = "transcript_search_tests.rs"]
mod tests;
