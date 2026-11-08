//! Small query-independent preparation queue. A keystroke can obsolete a query,
//! not the decoded transcript. Finish each bounded file and reuse it next time.
use super::text_cache;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, atomic::AtomicBool, mpsc};

const QUEUE_CAPACITY: usize = 8;
const MAX_HINTS: usize = 32;

struct Queue {
    sender: mpsc::SyncSender<PathBuf>,
    pending: Arc<Mutex<HashSet<PathBuf>>>,
}
impl Queue {
    fn start() -> Option<Self> {
        let (sender, receiver) = mpsc::sync_channel::<PathBuf>(QUEUE_CAPACITY);
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let worker_pending = pending.clone();
        let worker = std::thread::Builder::new()
            .name("jcode-search-prepare".into())
            .spawn(move || {
                let cancel = AtomicBool::new(false);
                while let Ok(path) = receiver.recv() {
                    // segments bounds each source read and normalized allocation.
                    // Query cancellation intentionally does not cancel this work.
                    if text_cache::segments(&path, &cancel).is_none() {
                        // Uncacheable inputs retain the normal streaming search path.
                        crate::logging::debug("Transcript warmup deferred to streaming search");
                    }
                    if let Ok(mut pending) = worker_pending.lock() {
                        pending.remove(&path);
                    }
                }
            });
        super::super::preview_result(worker, "warmup worker startup")?;
        Some(Self { sender, pending })
    }

    fn schedule<'a>(&self, paths: impl Iterator<Item = &'a PathBuf>) {
        for path in paths.take(MAX_HINTS) {
            if text_cache::is_cached_hint(path) {
                continue;
            }
            let Ok(mut pending) = self.pending.try_lock() else {
                return;
            };
            if pending.contains(path) {
                continue;
            }
            pending.insert(path.clone());
            if self.sender.try_send(path.clone()).is_err() {
                pending.remove(path);
                break;
            }
        }
    }
}

pub(super) fn schedule<'a>(paths: impl Iterator<Item = &'a PathBuf>) {
    static QUEUE: OnceLock<Option<Queue>> = OnceLock::new();
    if let Some(queue) = QUEUE.get_or_init(Queue::start) {
        queue.schedule(paths);
    }
}

/// Decode the given transcripts on one background thread, in order, so the
/// first search after opening the picker finds them already cached. Each call
/// supersedes the previous one. Query workers share decodes via the claim in
/// `text_cache`, so a file being prefetched is never decoded twice.
pub(super) fn prefetch(paths: Vec<PathBuf>) {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    use std::sync::atomic::Ordering;
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let spawned = std::thread::Builder::new()
        .name("jcode-search-prefetch".into())
        .spawn(move || {
            let cancel = AtomicBool::new(false);
            for path in paths {
                if GENERATION.load(Ordering::Relaxed) != generation {
                    return;
                }
                if !text_cache::is_cached_hint(&path) {
                    let _ = text_cache::segments(&path, &cancel);
                }
            }
        });
    let _ = super::super::preview_result(spawned, "search prefetch startup");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[test]
    fn preparation_survives_query_cancellation_and_is_reusable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("typing.jsonl");
        std::fs::write(&path, "{\"text\":\"prepared while typing\"}\n").unwrap();
        let queue = Queue::start().unwrap();
        let query_cancel = AtomicBool::new(false);
        queue.schedule(std::iter::once(&path));
        query_cancel.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !text_cache::is_cached_hint(&path) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let text = text_cache::cached_segments(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(&text.text, &["prepared while typing"]);
        assert!(text_cache::cached_segments(&path, &query_cancel).is_none());
    }

    #[test]
    fn scheduling_is_bounded_and_deduplicated() {
        let (sender, _receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let queue = Queue {
            sender,
            pending: pending.clone(),
        };
        let paths: Vec<_> = (0..100)
            .map(|i| PathBuf::from(format!("never-created-{i}")))
            .collect();
        queue.schedule(paths.iter());
        assert_eq!(pending.lock().unwrap().len(), QUEUE_CAPACITY);
        queue.schedule(paths.iter());
        assert_eq!(pending.lock().unwrap().len(), QUEUE_CAPACITY);
    }
}
