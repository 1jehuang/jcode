//! Process-wide cache of decoded string values, never JSON keys or syntax.
use super::super::preview_result;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::SystemTime;

const MAX_FILE: usize = 2 * 1024 * 1024;
const MAX_TEXT: usize = 4 * 1024 * 1024;
const MAX_TOTAL: usize = 32 * 1024 * 1024;
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
    text: Arc<Vec<String>>,
    bytes: usize,
    used: u64,
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
    fn get(&mut self, path: &Path, fingerprint: Option<&Fingerprint>) -> Option<Arc<Vec<String>>> {
        self.clock = self.clock.wrapping_add(1);
        if self
            .entries
            .get(path)
            .is_some_and(|entry| Some(&entry.fingerprint) != fingerprint)
        {
            self.remove(path);
        }
        let entry = self.entries.get_mut(path)?;
        entry.used = self.clock;
        Some(entry.text.clone())
    }
    fn insert(
        &mut self,
        path: &Path,
        fingerprint: Fingerprint,
        text: Arc<Vec<String>>,
        bytes: usize,
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
pub(super) fn cached_segments(path: &Path, cancel: &AtomicBool) -> Option<Arc<Vec<String>>> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let fingerprint = Fingerprint::read(path);
    preview_result(cache().lock(), "text cache lock")?.get(path, fingerprint.as_ref())
}

/// None requests the existing streaming scanner. No cache lock spans IO,
/// decoding, normalization, or query matching. Active readers own their Arc.
pub(super) fn segments(path: &Path, cancel: &AtomicBool) -> Option<Arc<Vec<String>>> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let fingerprint = Fingerprint::read(path);
    if let Some(text) =
        preview_result(cache().lock(), "text cache lock")?.get(path, fingerprint.as_ref())
    {
        return Some(text);
    }
    let fingerprint = fingerprint?;
    if fingerprint.len > MAX_FILE as u64 {
        return None;
    }
    let file = preview_result(std::fs::File::open(path), "transcript open")?;
    let mut input = Vec::new();
    // Bounded even when a live journal grows after stat.
    let mut reader = super::CancellableReader {
        reader: file,
        cancel,
    };
    preview_result(
        (&mut reader)
            .take(MAX_FILE as u64 + 1)
            .read_to_end(&mut input),
        "transcript read",
    )?;
    if input.len() > MAX_FILE || cancel.load(Ordering::Relaxed) {
        return None;
    }
    let (text, bytes) = decode(
        &input,
        path.extension().is_some_and(|ext| ext == "jsonl"),
        cancel,
    )?;
    if Fingerprint::read(path).as_ref() != Some(&fingerprint) || cancel.load(Ordering::Relaxed) {
        return None;
    }
    let text = Arc::new(text);
    preview_result(cache().lock(), "text cache publication")?.insert(
        path,
        fingerprint,
        text.clone(),
        bytes,
    );
    Some(text)
}

struct Collector<'a> {
    text: Vec<String>,
    bytes: usize,
    cancel: &'a AtomicBool,
    overflow: bool,
}
impl<'de> DeserializeSeed<'de> for &mut Collector<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<(), D::Error> {
        if self.cancel.load(Ordering::Relaxed) || self.overflow {
            return Err(serde::de::Error::custom("collection stopped"));
        }
        de.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for &mut Collector<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("transcript JSON")
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(E::custom("cancelled"));
        }
        let lower = value.to_lowercase();
        self.bytes += lower.capacity() + std::mem::size_of::<String>();
        if self.bytes > MAX_TEXT {
            self.overflow = true;
            return Err(E::custom("cache text limit"));
        }
        self.text.push(lower);
        Ok(())
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<(), E> {
        self.visit_str(&value)
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
fn decode(input: &[u8], jsonl: bool, cancel: &AtomicBool) -> Option<(Vec<String>, usize)> {
    let mut collector = Collector {
        text: Vec::new(),
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
            if let Err(error) =
                (&mut collector).deserialize(&mut serde_json::Deserializer::from_slice(&line))
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
        while (&mut collector).deserialize(&mut de).is_ok() {}
    }
    if cancel.load(Ordering::Relaxed) || collector.overflow {
        return None;
    }
    collector.text.shrink_to_fit();
    let bytes = collector.text.capacity() * std::mem::size_of::<String>()
        + collector.text.iter().map(String::capacity).sum::<usize>();
    Some((collector.text, bytes))
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
            decode(input, true, &cancel).unwrap().0,
            ["first", "second", "third"]
        );
        assert_eq!(
            decode(br#"{"KEY":"ONE"} ["TWO", "\u0041"]"#, false, &cancel)
                .unwrap()
                .0,
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
        assert_eq!(&*segments(&path, &cancel).unwrap(), &["replacement"]);
        std::fs::remove_file(&path).unwrap();
        assert!(segments(&path, &cancel).is_none());
        cancel.store(true, Ordering::Relaxed);
        assert!(segments(&path, &cancel).is_none());
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
        assert_eq!(&*first, &["first"]);
        assert_eq!(&*segments(&path, &cancel).unwrap(), &["other"]);
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
                Arc::new(vec![]),
                1,
            );
        }
        cache.get(Path::new("0"), Some(&fp)).unwrap();
        cache.insert(Path::new("extra"), fp.clone(), Arc::new(vec![]), 1);
        assert!(cache.entries.contains_key(Path::new("0")));
        assert!(!cache.entries.contains_key(Path::new("1")));
        cache.insert(Path::new("new"), fp.clone(), Arc::new(vec![]), MAX_TOTAL);
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
