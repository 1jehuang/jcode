/// Run synchronous, filesystem-heavy work (session loads, agent setup) without
/// stalling the async worker it is called from. A reload reconnects dozens of
/// clients at once, and doing this work inline starved the runtime so even
/// tiny sessions waited seconds for their first History frame. Falls back to
/// inline execution on a current-thread runtime, where `block_in_place` panics.
pub(crate) fn run_blocking_io<R>(f: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// CPU time consumed by the calling thread, in milliseconds. Comparing it with
/// wall time separates real work from waiting (scheduler, disk, locks).
pub(crate) fn thread_cpu_ms() -> f64 {
    #[cfg(unix)]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: valid pointer to a timespec; CLOCK_THREAD_CPUTIME_ID is
        // supported on Linux and macOS.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } == 0 {
            return ts.tv_sec as f64 * 1000.0 + ts.tv_nsec as f64 / 1_000_000.0;
        }
    }
    0.0
}
