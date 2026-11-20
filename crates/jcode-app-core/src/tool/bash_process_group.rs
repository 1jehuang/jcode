pub(super) struct ProcessGroupKillGuard {
    pid: Option<u32>,
}

impl ProcessGroupKillGuard {
    pub(super) fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    pub(super) fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for ProcessGroupKillGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid
            && let Err(error) = crate::platform::signal_detached_process_group(pid, libc::SIGKILL)
            && error.raw_os_error() != Some(libc::ESRCH)
        {
            crate::logging::warn(&format!(
                "[bash] failed to kill timed-out process group {pid}: {error}"
            ));
        }
    }
}
