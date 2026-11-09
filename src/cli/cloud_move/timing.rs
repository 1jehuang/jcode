use anyhow::Result;

pub(super) fn timed_move_stage<T>(stage: &str, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = std::time::Instant::now();
    let result = operation();
    crate::logging::info(&format!(
        "[CLOUD_TIMING] stage=handoff operation={} outcome={} duration_ms={:.3}",
        stage.replace(' ', "_"),
        if result.is_ok() { "ok" } else { "error" },
        started.elapsed().as_secs_f64() * 1000.0
    ));
    result
}
