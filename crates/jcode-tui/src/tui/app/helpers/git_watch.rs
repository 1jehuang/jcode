//! Event-driven git status refresh (Linux inotify).
//!
//! Watches the repository's `.git` metadata (HEAD, index, refs) plus every
//! non-ignored work-tree directory. Any relevant change schedules one debounced
//! probe. While the watcher is healthy the git widget never polls, so an idle
//! client spawns zero processes and wakes the CPU zero times.
//!
//! Falls back to the stamp-gated polling in `helpers.rs` when inotify is
//! unavailable, the kernel watch limit is hit, or the event queue overflows.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// True while a watcher is running and has not lost events.
static HEALTHY: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);

pub(super) fn is_healthy() -> bool {
    HEALTHY.load(Ordering::Acquire)
}

/// Start the watcher for `repo_root` once per process. `on_change` runs on the
/// watcher thread after a quiet period following any relevant event.
pub(super) fn ensure_started(repo_root: &Path, on_change: fn()) {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        let root = repo_root.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("jcode-git-watch".to_string())
            .spawn(move || imp::run(&root, on_change));
        if spawned.is_err() {
            HEALTHY.store(false, Ordering::Release);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (repo_root, on_change);
}

/// Whether an event on `name` inside the git dir can change the widget.
/// Lock files and temp objects churn during every git command.
pub(super) fn git_dir_event_relevant(name: &str) -> bool {
    !(name.ends_with(".lock") || name.starts_with("tmp_") || name == "ORIG_HEAD.lock")
}

/// Directories never worth watching even if not gitignored.
pub(super) fn skip_dir(name: &str) -> bool {
    name == ".git"
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{HEALTHY, git_dir_event_relevant, skip_dir};
    use std::collections::{BTreeSet, HashMap};
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// Coalesce bursts (a checkout touches thousands of files) into one probe.
    const DEBOUNCE: Duration = Duration::from_millis(250);

    const WORKTREE_MASK: u32 = libc::IN_CLOSE_WRITE
        | libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_DELETE_SELF
        | libc::IN_ONLYDIR;
    const GIT_MASK: u32 = WORKTREE_MASK;

    struct Watcher {
        fd: i32,
        dirs: HashMap<i32, (PathBuf, bool)>, // wd -> (path, is_git_metadata)
        root: PathBuf,
    }

    impl Watcher {
        fn add(&mut self, path: &Path, git_meta: bool) -> bool {
            let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
                return true;
            };
            let mask = if git_meta { GIT_MASK } else { WORKTREE_MASK };
            // SAFETY: valid fd and NUL-terminated path.
            let wd = unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), mask) };
            if wd < 0 {
                let err = std::io::Error::last_os_error();
                // ENOSPC: out of watches. Anything else (ENOENT race) is benign.
                return err.raw_os_error() != Some(libc::ENOSPC);
            }
            self.dirs.insert(wd, (path.to_path_buf(), git_meta));
            true
        }

        fn add_tree(&mut self, dirs: impl IntoIterator<Item = PathBuf>) -> bool {
            dirs.into_iter().all(|dir| self.add(&dir, false))
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            // SAFETY: closing our own fd.
            unsafe { libc::close(self.fd) };
        }
    }

    fn git(root: &Path, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Directories holding tracked or untracked-but-not-ignored files, plus
    /// all their ancestors. One git call instead of walking ignored trees
    /// such as `target/` or `node_modules/`.
    fn worktree_dirs(root: &Path) -> Option<BTreeSet<PathBuf>> {
        let listing = git(
            root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
        )?;
        let mut dirs = BTreeSet::from([root.to_path_buf()]);
        for file in listing.split('\0').filter(|f| !f.is_empty()) {
            let mut dir = Path::new(file).parent();
            while let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
                if !dirs.insert(root.join(d)) {
                    break;
                }
                dir = d.parent();
            }
        }
        Some(dirs)
    }

    fn git_meta_dirs(git_dir: &Path) -> Vec<PathBuf> {
        let mut out = vec![git_dir.to_path_buf()];
        let mut stack = vec![git_dir.join("refs")];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push(entry.path());
                    }
                }
                out.push(dir);
            }
        }
        out
    }

    fn is_ignored(root: &Path, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(root) else {
            return true;
        };
        std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["check-ignore", "-q", "--"])
            .arg(rel)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    pub(super) fn run(root: &Path, on_change: fn()) {
        let git_dir = root.join(".git");
        if !git_dir.is_dir() {
            // Worktrees/submodules use a .git file; keep polling there.
            return;
        }
        // SAFETY: plain syscall.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return;
        }
        let mut w = Watcher {
            fd,
            dirs: HashMap::new(),
            root: root.to_path_buf(),
        };
        let ok = git_meta_dirs(&git_dir).iter().all(|d| w.add(d, true))
            && worktree_dirs(root).is_some_and(|dirs| w.add_tree(dirs));
        if !ok {
            crate::logging::info("git_watch: inotify watch limit reached; using polling");
            return;
        }
        crate::logging::info(&format!(
            "git_watch: watching {} directories under {}",
            w.dirs.len(),
            root.display()
        ));
        HEALTHY.store(true, Ordering::Release);

        let mut buf = vec![0u8; 64 * 1024];
        let mut pending = false;
        loop {
            // Block indefinitely when idle; once dirty, wait out the debounce.
            let timeout = if pending {
                DEBOUNCE.as_millis() as i32
            } else {
                -1
            };
            let mut pfd = libc::pollfd {
                fd: w.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd.
            let ready = unsafe { libc::poll(&mut pfd, 1, timeout) };
            if ready < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if ready == 0 {
                pending = false;
                on_change();
                continue;
            }
            // SAFETY: reading into our buffer.
            let n = unsafe { libc::read(w.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            let mut off = 0usize;
            let header = std::mem::size_of::<libc::inotify_event>();
            let mut new_dirs = Vec::new();
            while off + header <= n as usize {
                // SAFETY: kernel guarantees a full event at this offset.
                let ev: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off).cast()) };
                let name_bytes = &buf[off + header..off + header + ev.len as usize];
                let name_end = name_bytes
                    .iter()
                    .position(|b| *b == 0)
                    .unwrap_or(name_bytes.len());
                let name = String::from_utf8_lossy(&name_bytes[..name_end]).into_owned();
                off += header + ev.len as usize;

                if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                    // Lost events: refresh now and stay healthy; a probe
                    // reads full state anyway.
                    pending = true;
                    continue;
                }
                if ev.mask & libc::IN_IGNORED != 0 {
                    w.dirs.remove(&ev.wd);
                    continue;
                }
                let Some((dir, git_meta)) = w.dirs.get(&ev.wd).cloned() else {
                    continue;
                };
                if git_meta {
                    if git_dir_event_relevant(&name) {
                        pending = true;
                    }
                    if ev.mask & libc::IN_ISDIR != 0
                        && ev.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
                    {
                        new_dirs.push((dir.join(&name), true));
                    }
                    continue;
                }
                if skip_dir(&name) {
                    continue;
                }
                if ev.mask & libc::IN_ISDIR != 0
                    && ev.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
                {
                    new_dirs.push((dir.join(&name), false));
                }
                pending = true;
            }
            for (dir, git_meta) in new_dirs {
                if !git_meta && is_ignored(&w.root, &dir) {
                    continue;
                }
                if !w.add(&dir, git_meta) {
                    crate::logging::info("git_watch: watch limit reached; falling back to polling");
                    HEALTHY.store(false, Ordering::Release);
                    return;
                }
                // Files may have landed before the watch existed.
                if !git_meta && let Some(sub) = worktree_dirs(&dir) {
                    let root = w.root.clone();
                    let extra: Vec<_> = sub
                        .into_iter()
                        .filter(|d| d.starts_with(&root) && d != &dir)
                        .collect();
                    if !w.add_tree(extra) {
                        HEALTHY.store(false, Ordering::Release);
                        return;
                    }
                }
            }
        }
        HEALTHY.store(false, Ordering::Release);
    }

    #[cfg(test)]
    mod tests {
        use std::process::Command;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        static HITS: AtomicUsize = AtomicUsize::new(0);
        fn bump() {
            HITS.fetch_add(1, Ordering::SeqCst);
        }

        fn wait_for(target: usize) -> bool {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if HITS.load(Ordering::SeqCst) >= target {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }

        #[test]
        fn fires_on_worktree_and_git_changes_but_not_ignored_dirs() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().to_path_buf();
            let git = |args: &[&str]| {
                assert!(
                    Command::new("git")
                        .current_dir(&root)
                        .args(args)
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            };
            git(&["init", "-q"]);
            std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::create_dir_all(root.join("target")).unwrap();
            std::fs::write(root.join("src/a.txt"), "a").unwrap();

            let r = root.clone();
            std::thread::spawn(move || super::run(&r, bump));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !super::HEALTHY.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(super::HEALTHY.load(Ordering::Acquire));

            // Ignored dir: no event.
            std::fs::write(root.join("target/junk"), "x").unwrap();
            std::thread::sleep(Duration::from_millis(600));
            assert_eq!(HITS.load(Ordering::SeqCst), 0);

            // Unstaged edit in a nested dir.
            std::fs::write(root.join("src/a.txt"), "b").unwrap();
            assert!(wait_for(1));

            // New directory created after start is picked up.
            std::fs::create_dir_all(root.join("src/new")).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            let before = HITS.load(Ordering::SeqCst);
            std::fs::write(root.join("src/new/b.txt"), "b").unwrap();
            assert!(wait_for(before + 1));

            // Git metadata change (staging).
            let before = HITS.load(Ordering::SeqCst);
            git(&["add", "-A"]);
            assert!(wait_for(before + 1));
        }
    }
}
