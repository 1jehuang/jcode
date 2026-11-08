//! Event-driven detection of a newer client binary.
//!
//! Every client (local, attached, and `--ssh`) must follow hot reloads, not
//! only the server. Instead of stat-polling, an inotify thread watches the
//! directories whose entries are swapped when a build is published (the
//! `current` channel symlink, the launcher, and the repo self-dev output).
//! Any change raises `PENDING` and wakes the UI loop through the bus; the loop
//! then confirms with `has_newer_binary()` and re-execs once idle.

use std::sync::atomic::{AtomicBool, Ordering};

static PENDING: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);

/// True once since the last call if a watched binary location changed.
pub(super) fn take_pending() -> bool {
    PENDING.swap(false, Ordering::AcqRel)
}

/// Re-arm after a change that could not be acted on yet (busy or typing).
pub(super) fn rearm() {
    PENDING.store(true, Ordering::Release);
}

pub(super) fn ensure_started() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(all(target_os = "linux", not(test)))]
    {
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(path) = crate::build::current_binary_path()
            && let Some(dir) = path.parent()
        {
            dirs.push(dir.to_path_buf());
        }
        if let Ok(path) = crate::build::launcher_binary_path()
            && let Some(dir) = path.parent()
        {
            dirs.push(dir.to_path_buf());
        }
        if let Some(repo) = crate::build::get_repo_dir()
            && let Some(dir) = crate::build::selfdev_binary_path(&repo).parent()
        {
            dirs.push(dir.to_path_buf());
        }
        dirs.sort();
        dirs.dedup();
        let _ = std::thread::Builder::new()
            .name("jcode-bin-watch".to_string())
            .spawn(move || imp::run(dirs));
    }
}

#[cfg(all(target_os = "linux", not(test)))]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    /// Builds write several files and swap symlinks; wait for quiet.
    const DEBOUNCE_MS: i32 = 750;

    pub(super) fn run(dirs: Vec<PathBuf>) {
        // SAFETY: plain syscalls on an fd we own and close on every exit path.
        unsafe {
            let fd = libc::inotify_init1(libc::IN_CLOEXEC);
            if fd < 0 {
                return;
            }
            let mask = libc::IN_CREATE
                | libc::IN_MOVED_TO
                | libc::IN_CLOSE_WRITE
                | libc::IN_ATTRIB
                | libc::IN_DELETE_SELF
                | libc::IN_MOVE_SELF;
            let mut watched = 0;
            for dir in &dirs {
                if let Ok(c) = CString::new(dir.as_os_str().as_bytes())
                    && libc::inotify_add_watch(fd, c.as_ptr(), mask) >= 0
                {
                    watched += 1;
                }
            }
            if watched == 0 {
                libc::close(fd);
                return;
            }
            let mut buf = [0u8; 4096];
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                // Block indefinitely: zero wakeups while nothing is published.
                if libc::poll(&mut pfd, 1, -1) < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) <= 0 {
                    break;
                }
                // Drain the burst until the publisher goes quiet.
                while libc::poll(&mut pfd, 1, DEBOUNCE_MS) > 0 {
                    if libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) <= 0 {
                        break;
                    }
                }
                super::PENDING.store(true, Ordering::Release);
                crate::bus::Bus::global().publish(crate::bus::BusEvent::ClientBinaryChanged);
            }
            libc::close(fd);
        }
    }
}
