//! Process-wide cache of decoded string values, never JSON keys or syntax.
use super::super::preview_result;
use super::source::{Sink, Source, Walk};
use serde::de::DeserializeSeed;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::SystemTime;

// Long sessions are exactly the ones a streaming rescan hurts most: a 10 MB
// transcript scanned per keystroke and cancelled by the next one never
// finishes. Decode it once, off the query path, and keep it resident.
const MAX_FILE: usize = 16 * 1024 * 1024;
const MAX_TEXT: usize = 48 * 1024 * 1024;
const MAX_TOTAL: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
impl Fingerprint {
    fn read(path: &Path) -> Option<Self> {
        let metadata = preview_result(std::fs::metadata(path), "transcript metadata")?;
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            len: metadata.len(),
            modified: preview_result(metadata.modified(), "transcript timestamp"),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        })
    }
}
struct Entry {
    fingerprint: Fingerprint,
    text: Arc<Segments>,
    bytes: usize,
    used: u64,
    /// Bytes of whole JSONL records reflected in `text`. A torn trailing
    /// record failed to parse, so the next append decode restarts there.
    records_end: Boundary,
}
/// End of the decoded JSONL prefix plus the bytes just before it. A rewrite
/// that keeps the inode and grows looks like an append by metadata alone, so
/// these bytes are re-read and compared before the prefix is reused.
#[derive(Clone, Default)]
pub(super) struct Boundary {
    end: u64,
    check: Vec<u8>,
}
const BOUNDARY_CHECK: usize = 256;
impl Fingerprint {
    /// Same file, only grown: an append-only journal gained records.
    fn is_append_of(&self, older: &Fingerprint) -> bool {
        #[cfg(unix)]
        let same_file = self.identity.0 == older.identity.0 && self.identity.1 == older.identity.1;
        #[cfg(not(unix))]
        let same_file = true;
        same_file && self.len > older.len
    }
}
#[derive(Default)]
struct Cache {
    entries: HashMap<PathBuf, Entry>,
    bytes: usize,
    clock: u64,
}
impl Cache {
    fn remove(&mut self, path: &Path) {
        if let Some(entry) = self.entries.remove(path) {
            self.bytes -= entry.bytes;
        }
    }
    /// A stale entry for the same journal that has since only grown, plus
    /// the byte length it covers. Only meaningful for append-only JSONL.
    fn stale_prefix(
        &mut self,
        path: &Path,
        fingerprint: &Fingerprint,
    ) -> Option<(Arc<Segments>, Boundary)> {
        let entry = self.entries.get(path)?;
        fingerprint
            .is_append_of(&entry.fingerprint)
            .then(|| (entry.text.clone(), entry.records_end.clone()))
    }
    fn get(&mut self, path: &Path, fingerprint: Option<&Fingerprint>) -> Option<Arc<Segments>> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(path)?;
        // Keep a stale entry: a grown journal reuses it as a decoded prefix.
        // insert() replaces it and LRU eviction bounds it otherwise.
        if Some(&entry.fingerprint) != fingerprint {
            return None;
        }
        entry.used = self.clock;
        Some(entry.text.clone())
    }
    fn insert(
        &mut self,
        path: &Path,
        fingerprint: Fingerprint,
        text: Arc<Segments>,
        bytes: usize,
        records_end: Boundary,
    ) {
        self.remove(path);
        while self.bytes + bytes > MAX_TOTAL || self.entries.len() >= MAX_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            self.remove(&oldest);
        }
        self.clock = self.clock.wrapping_add(1);
        self.bytes += bytes;
        self.entries.insert(
            path.to_owned(),
            Entry {
                fingerprint,
                text,
                bytes,
                used: self.clock,
                records_end,
            },
        );
    }
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

/// Scheduling hint only. Never trust it as evidence about file contents.
/// This performs no filesystem work on the input/render thread.
pub(super) fn is_cached_hint(path: &Path) -> bool {
    cache()
        .try_lock()
        .is_ok_and(|cache| cache.entries.contains_key(path))
}

/// Validate a resident entry on the search worker without opening a transcript.
pub(super) fn cached_segments(path: &Path, cancel: &AtomicBool) -> Option<Arc<Segments>> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let fingerprint = Fingerprint::read(path);
    preview_result(cache().lock(), "text cache lock")?.get(path, fingerprint.as_ref())
}

/// None requests the existing streaming scanner. No cache lock spans IO,
/// decoding, normalization, or query matching. Active readers own their Arc.
///
/// The query's `cancel` only abandons *waiting*. Once a decode starts it runs
/// to completion and is published, so typing never discards decode work: the
/// next keystroke's worker waits for that same decode instead of restarting.
pub(super) fn segments(path: &Path, cancel: &AtomicBool) -> Option<Arc<Segments>> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let fingerprint = Fingerprint::read(path);
    if let Some(text) =
        preview_result(cache().lock(), "text cache lock")?.get(path, fingerprint.as_ref())
    {
        return Some(text);
    }
    let _claim = loop {
        if let Some(claim) = DecodeClaim::try_acquire(path) {
            break claim;
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        // Another worker is decoding this file. Its result lands in the cache.
        let fingerprint = Fingerprint::read(path);
        if let Some(text) =
            preview_result(cache().lock(), "text cache lock")?.get(path, fingerprint.as_ref())
        {
            return Some(text);
        }
    };
    // Recheck: the previous claimant may have just published this generation.
    let fingerprint = Fingerprint::read(path);
    if let Some(text) =
        preview_result(cache().lock(), "text cache lock")?.get(path, fingerprint.as_ref())
    {
        return Some(text);
    }
    let text = decode_and_publish(path, fingerprint?, &NEVER_CANCEL)?;
    (!cancel.load(Ordering::Relaxed)).then_some(text)
}

static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);

/// One decode per path at a time, across query workers and warmup.
struct DecodeClaim(PathBuf);
fn in_flight() -> &'static (
    Mutex<std::collections::HashSet<PathBuf>>,
    std::sync::Condvar,
) {
    static IN_FLIGHT: OnceLock<(
        Mutex<std::collections::HashSet<PathBuf>>,
        std::sync::Condvar,
    )> = OnceLock::new();
    IN_FLIGHT.get_or_init(Default::default)
}
impl DecodeClaim {
    /// Claims the path, or waits briefly for the current claimant and
    /// returns None so the caller can recheck the cache and its cancel flag.
    fn try_acquire(path: &Path) -> Option<Self> {
        let (lock, wake) = in_flight();
        let mut claimed = preview_result(lock.lock(), "decode claim lock")?;
        if claimed.insert(path.to_owned()) {
            return Some(Self(path.to_owned()));
        }
        let _ = wake.wait_timeout(claimed, std::time::Duration::from_millis(10));
        None
    }
}
impl Drop for DecodeClaim {
    fn drop(&mut self) {
        let (lock, wake) = in_flight();
        if let Ok(mut claimed) = lock.lock() {
            claimed.remove(&self.0);
        }
        wake.notify_all();
    }
}

/// Whether the file still holds the bytes that ended the cached prefix.
fn boundary_matches(file: &mut std::fs::File, boundary: &Boundary) -> bool {
    use std::io::Seek;
    let Some(from) = boundary.end.checked_sub(boundary.check.len() as u64) else {
        return false;
    };
    let mut bytes = vec![0; boundary.check.len()];
    file.seek(std::io::SeekFrom::Start(from)).is_ok()
        && file.read_exact(&mut bytes).is_ok()
        && bytes == boundary.check
}

fn decode_and_publish(
    path: &Path,
    fingerprint: Fingerprint,
    cancel: &AtomicBool,
) -> Option<Arc<Segments>> {
    if fingerprint.len > MAX_FILE as u64 {
        return None;
    }
    let jsonl = path.extension().is_some_and(|ext| ext == "jsonl");
    // A live journal only appends records. Decode just the new tail and share
    // the already decoded prefix instead of re-reading the whole session.
    let prior = if jsonl {
        preview_result(cache().lock(), "text cache lock")?.stale_prefix(path, &fingerprint)
    } else {
        None
    };
    let mut file = preview_result(std::fs::File::open(path), "transcript open")?;
    let prior = prior.filter(|(_, boundary)| boundary_matches(&mut file, boundary));
    let start = prior.as_ref().map_or(0, |(_, boundary)| boundary.end);
    {
        use std::io::Seek;
        preview_result(
            file.seek(std::io::SeekFrom::Start(start)),
            "transcript seek",
        )?;
    }
    let mut input = Vec::new();
    // Bounded even when a live journal grows after stat.
    let mut reader = super::CancellableReader {
        reader: file,
        cancel,
    };
    preview_result(
        (&mut reader)
            .take(fingerprint.len - start)
            .read_to_end(&mut input),
        "transcript read",
    )?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let records_end = start
        + if jsonl {
            let whole = input
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |end| end + 1);
            // A final record without a newline is complete when it parses,
            // e.g. a finished external transcript. A torn live write is not.
            let tail = &input[whole..];
            if tail.trim_ascii().is_empty()
                || serde_json::from_slice::<serde::de::IgnoredAny>(tail).is_ok()
            {
                input.len() as u64
            } else {
                whole as u64
            }
        } else {
            input.len() as u64
        };
    // Decode whole records only. A record still being written is picked up,
    // exactly once, by the next append decode.
    let input = &input[..(records_end - start) as usize];
    let boundary = Boundary {
        end: records_end,
        check: input[input.len().saturating_sub(BOUNDARY_CHECK)..].to_vec(),
    };
    let (text, bytes) = match prior {
        Some((prefix, _)) => {
            // Only whole records were cached, so the tail starts at a record.
            let (tail, _) = decode(input, true, cancel)?;
            let mut text = Segments {
                text: Vec::with_capacity(prefix.text.len() + tail.text.len()),
                sources: Vec::with_capacity(prefix.sources.len() + tail.sources.len()),
            };
            text.text.extend(prefix.text.iter().cloned());
            text.text.extend(tail.text);
            text.sources.extend(prefix.sources.iter().copied());
            text.sources.extend(tail.sources);
            let bytes = segments_bytes(&text);
            if bytes > MAX_TEXT {
                return None;
            }
            (text, bytes)
        }
        None => decode(input, jsonl, cancel)?,
    };
    let current = Fingerprint::read(path);
    let unchanged = current.as_ref() == Some(&fingerprint);
    // A live journal that grew while we decoded still yields a valid prefix:
    // publish it so the next decode reads only the new records.
    let grown = jsonl
        && current
            .as_ref()
            .is_some_and(|current| current.is_append_of(&fingerprint));
    if !unchanged && !grown {
        return None;
    }
    let text = Arc::new(text);
    preview_result(cache().lock(), "text cache publication")?.insert(
        path,
        fingerprint,
        text.clone(),
        bytes,
        boundary,
    );
    if !unchanged || cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some(text)
}

/// Decoded, lowercased strings with the transcript source of each one.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Segments {
    pub(crate) text: Vec<String>,
    pub(crate) sources: Vec<Source>,
}
impl Segments {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, Source)> {
        self.text
            .iter()
            .map(String::as_str)
            .zip(self.sources.iter().copied())
    }
}
#[cfg(test)]
impl From<Vec<String>> for Segments {
    fn from(text: Vec<String>) -> Self {
        let sources = vec![Source::Other; text.len()];
        Self { text, sources }
    }
}

struct Collector<'a> {
    text: Segments,
    bytes: usize,
    cancel: &'a AtomicBool,
    overflow: bool,
}
impl Sink for Collector<'_> {
    fn text(&mut self, value: &str, source: Source) -> Result<(), &'static str> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("cancelled");
        }
        let lower = value.to_lowercase();
        self.bytes += lower.capacity() + std::mem::size_of::<String>() + 1;
        if self.bytes > MAX_TEXT {
            self.overflow = true;
            return Err("cache text limit");
        }
        self.text.text.push(lower);
        self.text.sources.push(source);
        Ok(())
    }
}
fn decode(input: &[u8], jsonl: bool, cancel: &AtomicBool) -> Option<(Segments, usize)> {
    let mut collector = Collector {
        text: Segments::default(),
        bytes: 0,
        cancel,
        overflow: false,
    };
    if jsonl {
        // Use std's optimized newline scanner, not an unoptimized per-byte
        // split predicate in selfdev/debug builds. Reuse the record buffer.
        let mut remaining = input;
        let mut line = Vec::new();
        while !remaining.is_empty() {
            line.clear();
            preview_result(
                std::io::BufRead::read_until(&mut remaining, b'\n', &mut line),
                "transcript record read",
            )?;
            if let Err(error) = Walk::new(&mut collector)
                .deserialize(&mut serde_json::Deserializer::from_slice(&line))
            {
                // A partial/corrupt journal record must not prevent later records from matching.
                if !cancel.load(Ordering::Relaxed) && !collector.overflow {
                    crate::logging::debug(&format!(
                        "Skipping malformed transcript cache record: {error}"
                    ));
                }
            }
            if cancel.load(Ordering::Relaxed) || collector.overflow {
                return None;
            }
        }
    } else {
        let mut de = serde_json::Deserializer::from_slice(input);
        while Walk::new(&mut collector).deserialize(&mut de).is_ok() {}
    }
    if cancel.load(Ordering::Relaxed) || collector.overflow {
        return None;
    }
    collector.text.text.shrink_to_fit();
    collector.text.sources.shrink_to_fit();
    let bytes = segments_bytes(&collector.text);
    Some((collector.text, bytes))
}

fn segments_bytes(text: &Segments) -> usize {
    text.text.capacity() * std::mem::size_of::<String>()
        + text.sources.capacity()
        + text.text.iter().map(String::capacity).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn values_only_order_corrupt_records_and_snapshot_streams() {
        let cancel = AtomicBool::new(false);
        let input =
            b"{\"KEY\": [\"FIRST\", 42, {\"ignored\":\"SECOND\"}]}\n{broken\n{\"x\":\"THIRD\"}";
        assert_eq!(
            decode(input, true, &cancel).unwrap().0.text,
            ["first", "second", "third"]
        );
        assert_eq!(
            decode(br#"{"KEY":"ONE"} ["TWO", "\u0041"]"#, false, &cancel)
                .unwrap()
                .0
                .text,
            ["one", "two", "a"]
        );
    }
    #[test]
    fn invalidation_reuse_deletion_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let cancel = AtomicBool::new(false);
        std::fs::write(&path, r#"{"key":"FIRST"}"#).unwrap();
        let first = segments(&path, &cancel).unwrap();
        assert!(Arc::ptr_eq(&first, &segments(&path, &cancel).unwrap()));
        std::fs::write(&path, r#"{"key":"REPLACEMENT"}"#).unwrap();
        assert_eq!(&segments(&path, &cancel).unwrap().text, &["replacement"]);
        std::fs::remove_file(&path).unwrap();
        assert!(segments(&path, &cancel).is_none());
        cancel.store(true, Ordering::Relaxed);
        assert!(segments(&path, &cancel).is_none());
    }
    #[test]
    fn journal_appends_decode_only_new_whole_records() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.journal.jsonl");
        let cancel = AtomicBool::new(false);
        std::fs::write(&path, "{\"t\":\"ONE\"}\n").unwrap();
        let first = segments(&path, &cancel).unwrap();
        assert_eq!(&first.text, &["one"]);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        // A torn record is left for the next decode.
        file.write_all(b"{\"t\":\"TWO\"}\n{\"t\":\"TH").unwrap();
        file.flush().unwrap();
        assert_eq!(&segments(&path, &cancel).unwrap().text, &["one", "two"]);
        file.write_all(b"REE\"}\n").unwrap();
        file.flush().unwrap();
        let last = segments(&path, &cancel).unwrap();
        assert_eq!(&last.text, &["one", "two", "three"]);
        // Unchanged file reuses the published entry.
        assert!(Arc::ptr_eq(&last, &segments(&path, &cancel).unwrap()));
        // A rewrite (not an append) decodes from scratch.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(
            &path,
            "{\"t\":\"FRESH\"}\n{\"t\":\"FILE\"}\n{\"t\":\"HERE\"}\n",
        )
        .unwrap();
        assert_eq!(
            &segments(&path, &cancel).unwrap().text,
            &["fresh", "file", "here"]
        );
    }
    #[test]
    fn complete_unterminated_final_record_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external.jsonl");
        std::fs::write(&path, "{\"t\":\"A\"}\n{\"t\":\"B\"}").unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(&segments(&path, &cancel).unwrap().text, &["a", "b"]);
    }
    #[test]
    fn large_files_request_streaming_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE as u64 + 1).unwrap();
        assert!(segments(&path, &AtomicBool::new(false)).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn inode_replacement_invalidates_even_with_same_length_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, "\"FIRST\"").unwrap();
        let cancel = AtomicBool::new(false);
        let first = segments(&path, &cancel).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, "\"OTHER\"").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(&first.text, &["first"]);
        assert_eq!(&segments(&path, &cancel).unwrap().text, &["other"]);
    }

    #[test]
    fn lru_and_limits() {
        let mut cache = Cache::default();
        let fp = Fingerprint {
            len: 0,
            modified: None,
            #[cfg(unix)]
            identity: (0, 0, 0, 0),
        };
        for index in 0..MAX_ENTRIES {
            cache.insert(
                Path::new(&index.to_string()),
                fp.clone(),
                Arc::new(Segments::default()),
                1,
                Boundary::default(),
            );
        }
        cache.get(Path::new("0"), Some(&fp)).unwrap();
        cache.insert(
            Path::new("extra"),
            fp.clone(),
            Arc::new(Segments::default()),
            1,
            Boundary::default(),
        );
        assert!(cache.entries.contains_key(Path::new("0")));
        assert!(!cache.entries.contains_key(Path::new("1")));
        cache.insert(
            Path::new("new"),
            fp.clone(),
            Arc::new(Segments::default()),
            MAX_TOTAL,
            Boundary::default(),
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, MAX_TOTAL);
        let cancel = AtomicBool::new(false);
        let input = format!(
            "[{}]",
            "\"\",".repeat(MAX_TEXT / std::mem::size_of::<String>() + 2)
        );
        assert!(decode(input.as_bytes(), false, &cancel).is_none());
    }
}
