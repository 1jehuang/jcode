//! Preserve untracked line counts across git-widget refreshes. The status TTL
//! must not make an idle client reread every untracked file every five seconds.
use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl Fingerprint {
    fn from_metadata(meta: &Metadata) -> Option<Self> {
        // Never follow symlinks or open special files during an idle probe.
        if !meta.is_file() {
            return None;
        }
        let modified = match meta.modified() {
            Ok(modified) => modified,
            Err(error) => {
                crate::logging::debug(&format!(
                    "Git line-count probe has no modification time: {error}"
                ));
                return None;
            }
        };
        Some(Self {
            len: meta.len(),
            modified,
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
            },
        })
    }
}

struct Entry {
    fingerprint: Fingerprint,
    lines: Option<usize>,
    seen: bool,
}

#[derive(Default)]
pub(super) struct GitLineCache {
    entries: HashMap<PathBuf, Entry>,
}

impl GitLineCache {
    pub(super) fn begin_scan(&mut self) {
        for entry in self.entries.values_mut() {
            entry.seen = false;
        }
    }

    pub(super) fn count(&mut self, path: &Path, meta: &Metadata) -> Option<usize> {
        self.count_with(path, meta, count_text_lines)
    }

    fn count_with(
        &mut self,
        path: &Path,
        meta: &Metadata,
        read: impl FnOnce(&Path) -> Option<usize>,
    ) -> Option<usize> {
        let fingerprint = Fingerprint::from_metadata(meta)?;
        if let Some(entry) = self.entries.get_mut(path)
            && entry.fingerprint == fingerprint
        {
            entry.seen = true;
            return entry.lines;
        }
        // Cache binary/oversized results too, otherwise each refresh rereads
        // binaries just to discover the same NUL again.
        let lines = if meta.len() > 2 * 1024 * 1024 {
            None
        } else {
            read(path)
        };
        self.entries.insert(
            path.to_path_buf(),
            Entry {
                fingerprint,
                lines,
                seen: true,
            },
        );
        lines
    }

    pub(super) fn finish_scan(&mut self) {
        // Deleted, newly tracked, and previous-repository paths must not grow
        // this process-wide cache indefinitely.
        self.entries.retain(|_, entry| entry.seen);
    }
}

fn count_text_lines(path: &Path) -> Option<usize> {
    use std::io::Read;
    // Bound the read too: a file can grow after the metadata probe.
    let mut bytes = Vec::new();
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            crate::logging::debug(&format!(
                "Could not open git line-count probe {}: {error}",
                path.display()
            ));
            return None;
        }
    };
    if let Err(error) = file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes) {
        crate::logging::debug(&format!(
            "Could not read git line-count probe {}: {error}",
            path.display()
        ));
        return None;
    }
    if bytes.len() > 2 * 1024 * 1024 || bytes.contains(&0) {
        return None;
    }
    let lines = bytes.iter().filter(|&&b| b == b'\n').count();
    Some(if bytes.last().is_some_and(|&b| b != b'\n') {
        lines + 1
    } else {
        lines
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_probe_preserves_counts_across_refresh_and_edit() {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        let path = dir.path().join("untracked.txt");
        std::fs::write(&path, "one\ntwo").unwrap();
        for _ in 0..2 {
            let info = super::super::gather_git_info_in(Some(dir.path())).unwrap();
            assert_eq!(
                (info.untracked, info.dirty_total, info.added_total),
                (1, 1, 2)
            );
            assert_eq!(info.dirty_files[0].added, Some(2));
        }
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let info = super::super::gather_git_info_in(Some(dir.path())).unwrap();
        assert_eq!(info.added_total, 3);
        assert_eq!(info.dirty_files[0].added, Some(3));
        std::fs::remove_file(&path).unwrap();
        let info = super::super::gather_git_info_in(Some(dir.path())).unwrap();
        assert_eq!(
            (info.untracked, info.dirty_total, info.added_total),
            (0, 0, 0)
        );
    }

    #[test]
    fn unchanged_untracked_files_are_not_reread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("untracked.txt");
        std::fs::write(&path, "one\ntwo").unwrap();
        let mut cache = GitLineCache::default();
        cache.begin_scan();
        assert_eq!(
            cache.count(&path, &std::fs::symlink_metadata(&path).unwrap()),
            Some(2)
        );
        cache.finish_scan();
        cache.begin_scan();
        assert_eq!(
            cache.count_with(
                &path,
                &std::fs::symlink_metadata(&path).unwrap(),
                |_| panic!("idle refresh reread unchanged content")
            ),
            Some(2)
        );
        cache.finish_scan();
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn changed_untracked_files_refresh_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("untracked.txt");
        std::fs::write(&path, "one").unwrap();
        let mut cache = GitLineCache::default();
        assert_eq!(
            cache.count(&path, &std::fs::symlink_metadata(&path).unwrap()),
            Some(1)
        );
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(
            cache.count(&path, &std::fs::symlink_metadata(&path).unwrap()),
            Some(3)
        );
    }

    #[test]
    fn binary_and_large_files_are_cached_without_repeated_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary");
        std::fs::write(&path, b"one\0two").unwrap();
        let mut cache = GitLineCache::default();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(cache.count(&path, &meta), None);
        assert_eq!(
            cache.count_with(&path, &meta, |_| panic!("reread binary")),
            None
        );
        std::fs::File::create(&path)
            .unwrap()
            .set_len(2 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(
            cache.count_with(
                &path,
                &std::fs::symlink_metadata(&path).unwrap(),
                |_| panic!("read oversized file")
            ),
            None
        );
    }

    #[test]
    fn text_line_counts_preserve_empty_and_unterminated_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        for (content, expected) in [("", 0), ("one", 1), ("one\n", 1), ("one\ntwo", 2)] {
            std::fs::write(&path, content).unwrap();
            assert_eq!(count_text_lines(&path), Some(expected));
        }
    }

    #[cfg(unix)]
    #[test]
    fn same_size_replacement_with_restored_mtime_refreshes_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("untracked.txt");
        let replacement = dir.path().join("replacement.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let original = std::fs::symlink_metadata(&path).unwrap();
        let mut cache = GitLineCache::default();
        assert_eq!(cache.count(&path, &original), Some(1));
        std::fs::write(&replacement, "a\nb\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original.modified().unwrap()))
            .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(
            cache.count(&path, &std::fs::symlink_metadata(&path).unwrap()),
            Some(2)
        );
    }

    #[test]
    fn finished_scan_prunes_no_longer_untracked_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("untracked.txt");
        std::fs::write(&path, "one").unwrap();
        let mut cache = GitLineCache::default();
        cache.count(&path, &std::fs::symlink_metadata(&path).unwrap());
        cache.begin_scan();
        cache.finish_scan();
        assert!(cache.entries.is_empty());
    }

    // Reproducible workload from #1832, excluded from the normal fast suite.
    // Run: justrust test -p jcode-tui --lib git_line_cache::tests::benchmark_14000_untracked_files -- --ignored --nocapture
    #[test]
    #[ignore = "creates 14000 files / 89.6MB for an idle-refresh benchmark"]
    fn benchmark_14000_untracked_files() {
        let dir = tempfile::tempdir().unwrap();
        let payload = format!("{}\n", "x".repeat(63)).repeat(100);
        let paths: Vec<_> = (0..14000)
            .map(|i| {
                let path = dir.path().join(format!("file-{i}.txt"));
                std::fs::write(&path, &payload).unwrap();
                path
            })
            .collect();
        let before = std::time::Instant::now();
        let uncached_total: usize = paths
            .iter()
            .map(|path| {
                // The old path statted once for the size limit and again for
                // display mtime, then reread every file on each refresh.
                let meta = std::fs::metadata(path).unwrap();
                assert!(meta.is_file() && meta.len() <= 2 * 1024 * 1024);
                let bytes = std::fs::read(path).unwrap();
                assert!(!bytes.contains(&0));
                let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
                let lines = if bytes.last().is_some_and(|&b| b != b'\n') {
                    newlines + 1
                } else {
                    newlines
                };
                std::fs::symlink_metadata(path).unwrap();
                lines
            })
            .sum();
        let uncached_time = before.elapsed();
        let mut cache = GitLineCache::default();
        cache.begin_scan();
        for path in &paths {
            cache.count(path, &std::fs::symlink_metadata(path).unwrap());
        }
        cache.finish_scan();
        let before = std::time::Instant::now();
        let mut reads = 0;
        cache.begin_scan();
        let cached_total: usize = paths
            .iter()
            .map(|path| {
                cache
                    .count_with(path, &std::fs::symlink_metadata(path).unwrap(), |path| {
                        reads += 1;
                        count_text_lines(path)
                    })
                    .unwrap()
            })
            .sum();
        cache.finish_scan();
        let cached_time = before.elapsed();
        assert_eq!(cached_total, uncached_total);
        assert_eq!(cached_total, 1_400_000);
        assert_eq!(reads, 0, "unchanged refresh must perform no content reads");
        eprintln!(
            "14000 files, 89.6MB: uncached={uncached_time:?}, cached={cached_time:?}, content reads=14000 -> {reads}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn same_size_edit_with_restored_mtime_refreshes_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("untracked.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let original = std::fs::symlink_metadata(&path).unwrap();
        let mut cache = GitLineCache::default();
        assert_eq!(cache.count(&path, &original), Some(1));
        // Give ctime a distinguishable value even on coarse filesystems.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "a\nb\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original.modified().unwrap()))
            .unwrap();
        assert_eq!(
            cache.count(&path, &std::fs::symlink_metadata(&path).unwrap()),
            Some(2)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link");
        std::os::unix::fs::symlink("missing-target", &path).unwrap();
        let mut cache = GitLineCache::default();
        assert_eq!(
            cache.count_with(
                &path,
                &std::fs::symlink_metadata(&path).unwrap(),
                |_| panic!("followed symlink")
            ),
            None
        );
    }
}
