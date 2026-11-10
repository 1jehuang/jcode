use std::time::Instant;

/// Content-free timing even when an SSH future is cancelled or returns early.
pub(super) struct StartupTiming {
    started: Instant,
    outcome: &'static str,
}
impl StartupTiming {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            outcome: "error_or_cancelled",
        }
    }
    pub(super) fn success(&mut self) {
        self.outcome = "ok";
    }
}
impl Drop for StartupTiming {
    fn drop(&mut self) {
        crate::logging::info(&format!(
            "[CLOUD_TIMING] stage=ssh_startup outcome={} duration_ms={:.3}",
            self.outcome,
            self.started.elapsed().as_secs_f64() * 1000.0
        ));
    }
}
