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
        if let Some(pid) = self.pid {
            let _ = crate::platform::signal_detached_process_group(pid, libc::SIGKILL);
        }
    }
}
